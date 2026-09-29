//! Offline HRIR rate conversion with the backend delay removed.

// Rust guideline compliant 2026-02-21
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use sotf_host::sofa::SofaFile;

// Bound both FFT grids before Rubato allocates its plans and scratch. Each FFT
// transform has twice this many samples. This capability limit also covers
// nearly coprime rates whose minimum grid dwarfs the source IR.
const MAX_FFT_GRID_FRAMES: usize = 262_144;

/// Resamples SOFA impulse responses while preserving their physical time origin.
///
/// Keeps exactly `ceil(N * target/source)` output frames, removing the FFT
/// backend's delay and flushing zeros as needed. This finite window can crop
/// sinc response outside the original duration. Sample amplitudes retain the
/// resampler's convention; no additional HRIR gain normalization is applied.
/// Valid matching-rate data remains unchanged. Source rates retain the existing
/// nearest-integer-Hz interpretation. All replacements are staged before commit.
///
/// # Errors
///
/// Returns an error for invalid rates or IR dimensions, size overflow, failed
/// staging allocation, backend failure, or an FFT grid exceeding 262144 frames.
pub fn resample_sofa(sofa: &mut SofaFile, target_rate: u32) -> Result<(), String> {
    let rounded_rate = f64::from(sofa.sample_rate.round());
    if !rounded_rate.is_finite()
        || rounded_rate < 1.0
        || rounded_rate > f64::from(u32::MAX)
        || target_rate == 0
    {
        return Err("HRTF resampling requires positive representable sample rates".into());
    }
    let source_rate = rounded_rate as u32;
    let source_samples = stereo_dataset_len(sofa.num_measurements, sofa.ir_length)?;
    if sofa.impulse_responses.len() != source_samples {
        return Err("HRTF resampling requires exactly M x 2 x N impulse samples".into());
    }
    if source_rate == target_rate {
        return Ok(());
    }

    let new_ir_length = sofa
        .ir_length
        .checked_mul(target_rate as usize)
        .ok_or("HRTF resampling duration overflow")?
        .div_ceil(source_rate as usize);
    let new_samples = stereo_dataset_len(sofa.num_measurements, new_ir_length)?;
    let mut new_impulse_responses;
    if source_samples == 0 {
        new_impulse_responses = Vec::new();
    } else {
        let (input_frames, output_frames) =
            fft_geometry(sofa.ir_length, source_rate as usize, target_rate as usize)?;
        let delay = output_frames / 2;
        let crop_end = delay
            .checked_add(new_ir_length)
            .ok_or("HRTF resampling crop overflow")?;
        let calls = crop_end.div_ceil(output_frames);
        calls
            .checked_mul(output_frames)
            .ok_or("HRTF resampling output timeline overflow")?;

        // new_custom preserves the historical single-sub-chunk geometry and
        // BlackmanHarris2 window; Fft::new would auto-select sub-chunks and
        // change delay and block sizes.
        let mut resampler = Fft::<f32>::new_custom(
            source_rate as usize,
            target_rate as usize,
            input_frames,
            1,
            2,
            WindowFunction::BlackmanHarris2,
            FixedSync::Both,
        )
        .map_err(|e| format!("Failed to create HRTF resampler: {e}"))?;
        // Rubato derives dimensions using integer div_ceil. Check its actual
        // geometry rather than assuming it matches our integer preflight.
        if resampler.input_frames_next() != input_frames
            || resampler.input_frames_max() != input_frames
            || resampler.output_frames_next() != output_frames
            || resampler.output_frames_max() != output_frames
            || resampler.output_delay() != delay
        {
            return Err("Unsupported HRTF resampler geometry or delay".into());
        }
        let mut input = [zero_samples(input_frames)?, zero_samples(input_frames)?];
        let mut output = [zero_samples(output_frames)?, zero_samples(output_frames)?];
        new_impulse_responses = zero_samples(new_samples)?;
        for (source, destination) in sofa
            .impulse_responses
            .chunks_exact(2 * sofa.ir_length)
            .zip(new_impulse_responses.chunks_exact_mut(2 * new_ir_length))
        {
            resampler.reset();
            let (left, right) = source.split_at(sofa.ir_length);
            let (out_left, out_right) = destination.split_at_mut(new_ir_length);
            let mut consumed = 0;
            let mut produced = 0;
            for _ in 0..calls {
                let available = input_frames.min(sofa.ir_length - consumed);
                for (buffer, ear) in input.iter_mut().zip([left, right]) {
                    buffer.fill(0.0);
                    buffer[..available].copy_from_slice(&ear[consumed..consumed + available]);
                }
                let input_adapter = SequentialSliceOfVecs::new(&input, 2, input_frames)
                    .map_err(|e| format!("HRTF input adapter error: {e}"))?;
                let mut output_adapter =
                    SequentialSliceOfVecs::new_mut(&mut output, 2, output_frames)
                        .map_err(|e| format!("HRTF output adapter error: {e}"))?;
                let (read, written) = resampler
                    .process_into_buffer(&input_adapter, &mut output_adapter, None)
                    .map_err(|e| format!("HRTF resampling error: {e}"))?;
                if read != input_frames || written != output_frames {
                    return Err("HRTF resampler returned an incomplete FFT quantum".into());
                }
                let next = produced + written;
                let start = produced.max(delay);
                let end = next.min(crop_end);
                if start < end {
                    let from = start - produced..end - produced;
                    let to = start - delay..end - delay;
                    out_left[to.clone()].copy_from_slice(&output[0][from.clone()]);
                    out_right[to].copy_from_slice(&output[1][from]);
                }
                consumed += available;
                produced = next;
            }
        }
    }

    sofa.impulse_responses = new_impulse_responses;
    sofa.ir_length = new_ir_length;
    sofa.sample_rate = target_rate as f32;
    sofa.data_sample_rate = Some(target_rate as f32);
    log::info!(
        "[BinauralDecoder] Resampled {} HRTF measurements from {} to {} Hz, {} samples each",
        sofa.num_measurements,
        source_rate,
        target_rate,
        new_ir_length
    );
    Ok(())
}

fn stereo_dataset_len(measurements: usize, frames: usize) -> Result<usize, String> {
    let samples = measurements
        .checked_mul(2)
        .and_then(|ears| ears.checked_mul(frames))
        .ok_or("HRTF resampling dataset size overflow")?;
    samples
        .checked_mul(size_of::<f32>())
        .filter(|&bytes| bytes <= isize::MAX as usize)
        .ok_or("HRTF resampling dataset byte size overflow")?;
    Ok(samples)
}

fn zero_samples(frames: usize) -> Result<Vec<f32>, String> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(frames)
        .map_err(|e| format!("Cannot allocate HRTF resampling storage: {e}"))?;
    samples.resize(frames, 0.0);
    Ok(samples)
}

fn fft_geometry(
    source_length: usize,
    source_rate: usize,
    target_rate: usize,
) -> Result<(usize, usize), String> {
    let (mut divisor, mut remainder) = (source_rate, target_rate);
    while remainder != 0 {
        (divisor, remainder) = (remainder, divisor % remainder);
    }
    let reduced_input = source_rate / divisor;
    let reduced_output = target_rate / divisor;
    let desired = source_length
        .checked_next_power_of_two()
        .ok_or("HRTF resampling chunk size overflow")?
        .max(64)
        / 2;
    // The sinc center is floor(input_grid / 2); Rubato reports floor(output_grid
    // / 2) delay. Both grids must be even for those to describe the same time.
    // Round the old reduced-grid multiplier up to the next even value.
    let multiplier = desired
        .div_ceil(reduced_input)
        .checked_add(1)
        .map(|value| value & !1)
        .ok_or("HRTF resampling grid multiplier overflow")?;
    let input = multiplier
        .checked_mul(reduced_input)
        .ok_or("HRTF resampling input grid overflow")?;
    let output = multiplier
        .checked_mul(reduced_output)
        .ok_or("HRTF resampling output grid overflow")?;
    if input > MAX_FFT_GRID_FRAMES || output > MAX_FFT_GRID_FRAMES {
        return Err(format!(
            "Unsupported HRTF resampling FFT grid {input}/{output}; maximum is {MAX_FFT_GRID_FRAMES} frames"
        ));
    }
    // Rubato multiplies by the unreduced rates before dividing by their GCD.
    multiplier
        .checked_mul(source_rate)
        .and_then(|_| multiplier.checked_mul(target_rate))
        .ok_or("Unsupported HRTF resampler rate arithmetic")?;
    // The bounded grids cover doubled real transforms, complex bins, channel
    // scratch and overlap. FFT planner storage remains owned by the backend.
    for grid in [input, output] {
        grid.checked_mul(2 * size_of::<f32>())
            .ok_or("HRTF resampling FFT storage overflow")?;
    }
    Ok((input, output))
}
