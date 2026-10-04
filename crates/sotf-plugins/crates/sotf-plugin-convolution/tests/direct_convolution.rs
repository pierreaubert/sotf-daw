//! Compare the public streaming plugin against direct time-domain convolution.

// Rust guideline compliant 2026-02-21

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};
use rustfft::FftPlanner;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{ParametricInPlacePlugin, ProcessContext, TailLength};
use sotf_plugin_convolution::{ConvolutionLoadStatus, ConvolutionPlugin, ConvolutionPluginParams};
use std::alloc::{GlobalAlloc, Layout};
use std::path::PathBuf;

const NORMAL_LATENCY: usize = 1_024;

thread_local! {
    static AUD134_HEAP_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static AUD134_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static AUD134_DEALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static AUD134_ALLOCATED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

struct Aud134CountingAlloc;

// Count callback-side frees as well as allocations while retaining the host's
// existing allocation counter used by the older integration tests.
unsafe impl GlobalAlloc for Aud134CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = AUD134_HEAP_GUARD.try_with(|enabled| {
            if enabled.get() {
                let _ = AUD134_ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
                let _ = AUD134_ALLOCATED_BYTES
                    .try_with(|bytes| bytes.set(bytes.get().saturating_add(layout.size())));
            }
        });
        unsafe { GlobalAlloc::alloc(&sotf_host::CountingAlloc, layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = AUD134_HEAP_GUARD.try_with(|enabled| {
            if enabled.get() {
                let _ = AUD134_DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { GlobalAlloc::dealloc(&sotf_host::CountingAlloc, ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Aud134CountingAlloc = Aud134CountingAlloc;

fn assert_no_heap_activity(label: &str, f: impl FnOnce()) {
    AUD134_ALLOCATIONS.with(|count| count.set(0));
    AUD134_DEALLOCATIONS.with(|count| count.set(0));
    AUD134_HEAP_GUARD.with(|enabled| enabled.set(true));
    f();
    AUD134_HEAP_GUARD.with(|enabled| enabled.set(false));
    let allocations = AUD134_ALLOCATIONS.with(std::cell::Cell::get);
    let deallocations = AUD134_DEALLOCATIONS.with(std::cell::Cell::get);
    assert_eq!(allocations, 0, "{label}: {allocations} allocations");
    assert_eq!(deallocations, 0, "{label}: {deallocations} deallocations");
}

fn allocated_bytes_while(f: impl FnOnce()) -> usize {
    AUD134_ALLOCATED_BYTES.with(|bytes| bytes.set(0));
    AUD134_HEAP_GUARD.with(|enabled| enabled.set(true));
    f();
    AUD134_HEAP_GUARD.with(|enabled| enabled.set(false));
    AUD134_ALLOCATED_BYTES.with(std::cell::Cell::get)
}

struct ImpulseFile(PathBuf);

impl Drop for ImpulseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_impulse(channels: &[Vec<i16>], sample_rate: u32) -> ImpulseFile {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "sotf-convolution-oracle-{}-{unique}.wav",
        std::process::id()
    ));
    let channel_count = channels.len() as u16;
    let frames = channels[0].len();
    let data_bytes = (frames * channels.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channel_count.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(channel_count) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channel_count * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    std::fs::write(&path, bytes).unwrap();
    ImpulseFile(path)
}

fn direct_convolution(input: &[f32], impulse: &[Vec<i16>]) -> Vec<f64> {
    let channels = 3;
    let mut output = vec![0.0; input.len() + (impulse[0].len() - 1) * channels];
    for (frame, samples) in input.as_chunks::<3>().0.iter().enumerate() {
        for (channel, &sample) in samples.iter().enumerate() {
            for (tap, &coefficient) in impulse[channel % impulse.len()].iter().enumerate() {
                output[(frame + tap) * channels + channel] +=
                    f64::from(sample) * f64::from(coefficient) / 32768.0;
            }
        }
    }
    output
}

// Independently exercise Rubato's whole-clip API to produce reference path
// samples. This validates resampling channel order/plumbing with the same
// resampler library; the f64 direct convolution below remains the independent
// DSP oracle.
fn resample_true_stereo_reference(
    impulse: &[Vec<i16>],
    source_rate: u32,
    target_rate: u32,
) -> Vec<Vec<f64>> {
    let input: Vec<Vec<f32>> = impulse
        .iter()
        .map(|channel| {
            channel
                .iter()
                .map(|&sample| f32::from(sample) / 32768.0)
                .collect()
        })
        .collect();
    let source_frames = input[0].len();
    assert!(input.iter().all(|channel| channel.len() == source_frames));
    let mut resampler = Fft::<f32>::new_custom(
        source_rate as usize,
        target_rate as usize,
        1024,
        2,
        input.len(),
        WindowFunction::BlackmanHarris2,
        FixedSync::Input,
    )
    .expect("construct reference resampler");
    let output_capacity = resampler.process_all_needed_output_len(source_frames);
    let mut output = vec![vec![0.0_f32; output_capacity]; input.len()];
    let input_adapter = SequentialSliceOfVecs::new(&input, input.len(), source_frames)
        .expect("adapt reference input");
    let mut output_adapter =
        SequentialSliceOfVecs::new_mut(&mut output, input.len(), output_capacity)
            .expect("adapt reference output");
    let (_, written) = resampler
        .process_all_into_buffer(&input_adapter, &mut output_adapter, source_frames, None)
        .expect("resample reference paths");
    output
        .into_iter()
        .map(|mut channel| {
            channel.truncate(written);
            channel.into_iter().map(f64::from).collect()
        })
        .collect()
}

fn direct_true_stereo_convolution(input: &[f32], impulse: &[Vec<f64>]) -> [Vec<f64>; 2] {
    assert_eq!(impulse.len(), 4);
    assert_eq!(input.len() % 2, 0);
    let input_frames = input.len() / 2;
    let ir_frames = impulse[0].len();
    assert!(impulse.iter().all(|channel| channel.len() == ir_frames));
    let mut output = [
        vec![0.0; input_frames + ir_frames - 1],
        vec![0.0; input_frames + ir_frames - 1],
    ];
    for input_frame in 0..input_frames {
        let left = f64::from(input[input_frame * 2]);
        let right = f64::from(input[input_frame * 2 + 1]);
        for (tap, (((&ll, &lr), &rl), &rr)) in impulse[0]
            .iter()
            .zip(impulse[1].iter())
            .zip(impulse[2].iter())
            .zip(impulse[3].iter())
            .enumerate()
        {
            let frame = input_frame + tap;
            output[0][frame] += left * ll;
            output[0][frame] += right * rl;
            output[1][frame] += left * lr;
            output[1][frame] += right * rr;
        }
    }
    output
}

fn process_callbacks(
    processor: &mut ConvolutionPlugin,
    input: &[f32],
    callback_pattern: &[usize],
) -> Vec<f32> {
    assert_eq!(input.len() % 2, 0);
    assert!(!callback_pattern.is_empty());
    let mut output = Vec::with_capacity(input.len());
    let input_frames = input.len() / 2;
    let mut input_offset = 0;
    let mut pattern_index = 0;
    while input_offset < input_frames {
        let requested_frames = callback_pattern[pattern_index % callback_pattern.len()];
        pattern_index += 1;
        assert!(requested_frames > 0);
        let frames = requested_frames.min(input_frames - input_offset);
        let start = input_offset * 2;
        let end = (input_offset + frames) * 2;
        let mut block = input[start..end].to_vec();
        processor
            .process_in_place(&mut block, &ProcessContext::new(48_000, frames))
            .unwrap();
        output.extend_from_slice(&block);
        input_offset += frames;
    }
    output
}

fn drain_all(processor: &mut ConvolutionPlugin) -> Vec<f32> {
    let mut output = Vec::new();
    let mut drain_buffer = vec![0.0_f32; 193 * 2];
    for _ in 0..100_000 {
        let result = processor
            .drain(&mut drain_buffer, &ProcessContext::new(48_000, 0))
            .unwrap();
        output.extend_from_slice(&drain_buffer[..result.frames * 2]);
        if result.complete {
            return output;
        }
        assert!(result.frames > 0, "EOS drain must make bounded progress");
    }
    panic!("EOS drain did not complete within its bounded iteration limit");
}

fn render_true_stereo(
    processor: &mut ConvolutionPlugin,
    input: &[f32],
    callback_pattern: &[usize],
) -> Vec<f32> {
    let mut output = process_callbacks(processor, input, callback_pattern);
    output.extend(drain_all(processor));
    output
}

fn direct_legacy_four_channel_convolution(input: &[f32], impulse: &[Vec<i16>]) -> [Vec<f64>; 2] {
    assert_eq!(impulse.len(), 4);
    assert_eq!(input.len() % 2, 0);
    let input_frames = input.len() / 2;
    let ir_frames = impulse[0].len();
    let mut output = [
        vec![0.0; input_frames + ir_frames - 1],
        vec![0.0; input_frames + ir_frames - 1],
    ];
    for input_frame in 0..input_frames {
        for channel in 0..2 {
            let sample = f64::from(input[input_frame * 2 + channel]);
            for tap in 0..ir_frames {
                output[channel][input_frame + tap] +=
                    sample * f64::from(impulse[channel][tap]) / 32768.0;
            }
        }
    }
    output
}

#[test]
fn every_backend_matches_direct_convolution_with_full_tail_and_partial_callbacks() {
    let rate = 48_000;
    // An odd input length leaves a partial final UPC/NUPC partition. Distinct
    // channels expose accidental shared history and incorrect cyclic IR mapping.
    let input: Vec<f32> = (0..2051)
        .flat_map(|frame| {
            let t = frame as f32;
            [
                0.2 * (0.13 * t).sin(),
                0.1 * (0.037 * t).cos(),
                ((frame * 17 % 97) as f32 - 48.0) / 256.0,
            ]
        })
        .collect();
    // Dense IRs exercise every overlap sample; explicit signed taps straddle
    // FFT partition boundaries and keep the last response measurable.
    for ir_frames in [97, 2053, 8195] {
        let impulse: Vec<Vec<i16>> = (0..2)
            .map(|channel| {
                let mut ir: Vec<i16> = (0..ir_frames)
                    .map(|tap| ((tap * (31 + channel * 12) % 101) as i16 - 50) * 3)
                    .collect();
                for (tap, value) in [
                    (0, 16000),
                    (127, -9000),
                    (1023, 7000),
                    (1024, -6000),
                    (2047, 5000),
                    (2048, -4000),
                    (ir_frames - 1, 8000),
                ] {
                    if tap < ir.len() {
                        ir[tap] += if channel == 0 { value } else { -value / 2 };
                    }
                }
                ir
            })
            .collect();
        let file = write_impulse(&impulse, rate);
        let reference = direct_convolution(&input, &impulse);
        for (use_nupc, zero_latency_head, head_taps) in [
            (false, false, 128),
            (true, false, 128),
            (true, true, 32),
            (true, true, 128),
            (true, true, 512),
        ] {
            for mix in [1.0_f32, 0.375] {
                let gain_db = -3.0;
                let mut processor = ConvolutionPlugin::from_params(
                    3,
                    rate,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_str().unwrap().into(),
                        mix,
                        gain_db,
                        use_nupc,
                        zero_latency_head,
                        head_taps,
                    },
                )
                .unwrap();
                processor.initialize(f64::from(rate)).unwrap();
                // Start playback after loading; reset also ends the deliberate
                // IR replacement fade, which is not part of the LTI transfer.
                processor.reset();
                let latency = processor.latency_samples();
                let mut output = vec![0.0_f32; reference.len() + (latency + 67) * 3];
                output[..input.len()].copy_from_slice(&input);
                let mut offset = 0;
                for frames in [1, 7, 31, 257, 1023, 2049, 64].into_iter().cycle() {
                    if offset == output.len() {
                        break;
                    }
                    let end = (offset + frames * 3).min(output.len());
                    processor
                        .process_in_place(
                            &mut output[offset..end],
                            &ProcessContext::new(rate, (end - offset) / 3),
                        )
                        .unwrap();
                    offset = end;
                }
                let gain = 10.0_f64.powf(f64::from(gain_db) / 20.0);
                let mut max_error = 0.0_f64;
                let mut squared_error = 0.0;
                for (index, &actual) in output.iter().enumerate() {
                    let expected = index.checked_sub(latency * 3).map_or(0.0, |i| {
                        let wet = reference.get(i).copied().unwrap_or(0.0);
                        let dry = input.get(i).copied().map(f64::from).unwrap_or(0.0);
                        wet * gain * f64::from(mix) + dry * f64::from(1.0 - mix)
                    });
                    let error = f64::from(actual) - expected;
                    max_error = max_error.max(error.abs());
                    squared_error += error * error;
                }
                let rms_error = (squared_error / output.len() as f64).sqrt();
                // This is an f32 convolution implementation. Require an error
                // below -100 dBFS peak and -120 dBFS RMS against the f64 sum.
                assert!(
                    max_error < 1e-5 && rms_error < 1e-6,
                    "ir={ir_frames} nupc={use_nupc} head={zero_latency_head}/{head_taps} mix={mix}: peak={max_error:e}, rms={rms_error:e}"
                );
            }
        }
    }
}

#[test]
fn true_stereo_four_path_matrix_matches_f64_oracle_through_partitioned_eos() {
    const RATE: u32 = 48_000;
    const IR_RATE: u32 = 44_100;
    const INPUT_FRAMES: usize = 2_579;
    const IR_FRAMES: usize = 2_053;
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let t = frame as f32;
            [0.11 * (0.071 * t).sin(), 0.09 * (0.037 * t).cos()]
        })
        .collect();
    let mut impulse: Vec<Vec<i16>> = (0..4)
        .map(|path| {
            (0..IR_FRAMES)
                .map(|tap| ((tap * (23 + path * 14) % 113) as i16 - 56) * 2)
                .collect()
        })
        .collect();
    for (path, tap, value) in [
        (0, 0, 18_000),
        (1, 127, -12_000),
        (2, 1_023, 10_000),
        (3, 1_024, -8_000),
        (0, 2_048, 6_000),
        (1, IR_FRAMES - 1, 9_000),
        (2, IR_FRAMES - 1, -7_000),
        (3, IR_FRAMES - 1, 5_000),
    ] {
        impulse[path][tap] += value;
    }
    let file = write_impulse(&impulse, IR_RATE);
    let diagonal_file = write_impulse(&impulse[..2], IR_RATE);
    let resampled_impulse = resample_true_stereo_reference(&impulse, IR_RATE, RATE);
    let reference = direct_true_stereo_convolution(&input, &resampled_impulse);
    let resampled_ir_frames = resampled_impulse[0].len();
    let callback_patterns: [&[usize]; 3] = [&[1], &[1_024], &[17, 511, 2_048, 3]];

    for (use_nupc, zero_latency_head, head_taps) in [
        (false, false, 128),
        (true, false, 128),
        (true, true, 32),
        (true, true, 128),
        (true, true, 512),
    ] {
        for callback_pattern in callback_patterns {
            let mut processor = ConvolutionPlugin::from_params_with_routing(
                2,
                RATE,
                ConvolutionPluginParams {
                    ir_file: file.0.to_str().unwrap().into(),
                    mix: 1.0,
                    gain_db: -3.0,
                    use_nupc,
                    zero_latency_head,
                    head_taps,
                },
                true,
            )
            .unwrap();
            let replacement_error = processor
                .load_ir(diagonal_file.0.to_str().unwrap())
                .expect_err("true-stereo mode must reject a two-channel replacement");
            assert!(replacement_error.contains("requires exactly 4 IR channels"));
            processor.initialize(f64::from(RATE)).unwrap();
            processor.reset();
            let latency = processor.latency_samples();
            let expected_latency = if use_nupc && zero_latency_head {
                0
            } else {
                NORMAL_LATENCY
            };
            assert_eq!(
                latency, expected_latency,
                "nupc={use_nupc} head={zero_latency_head}/{head_taps}"
            );
            let tail_frames = latency + resampled_ir_frames - 1;
            assert_eq!(
                processor.tail_length(),
                TailLength::Finite(tail_frames as u64),
                "nupc={use_nupc} head={zero_latency_head}/{head_taps}"
            );

            let mut actual = Vec::with_capacity((INPUT_FRAMES + tail_frames) * 2);
            let mut input_offset = 0;
            let mut pattern_index = 0;
            while input_offset < INPUT_FRAMES {
                let requested_frames = callback_pattern[pattern_index % callback_pattern.len()];
                pattern_index += 1;
                let frames = requested_frames.min(INPUT_FRAMES - input_offset);
                let start = input_offset * 2;
                let end = (input_offset + frames) * 2;
                let mut block = input[start..end].to_vec();
                processor
                    .process_in_place(&mut block, &ProcessContext::new(RATE, frames))
                    .unwrap();
                actual.extend_from_slice(&block);
                input_offset += frames;
            }

            let mut drain_buffer = vec![0.0_f32; 137 * 2];
            for _ in 0..100_000 {
                let result = processor
                    .drain(&mut drain_buffer, &ProcessContext::new(RATE, 0))
                    .unwrap();
                actual.extend_from_slice(&drain_buffer[..result.frames * 2]);
                if result.complete {
                    break;
                }
                assert!(result.frames > 0, "EOS must make bounded progress");
            }

            assert_eq!(actual.len() / 2, INPUT_FRAMES + tail_frames);
            let gain = 10.0_f64.powf(-3.0 / 20.0);
            let mut max_error = 0.0_f64;
            let mut squared_error = 0.0;
            for frame in 0..actual.len() / 2 {
                for output_ch in 0..2 {
                    let expected = frame.checked_sub(latency).map_or(0.0, |input_time| {
                        reference[output_ch].get(input_time).copied().unwrap_or(0.0) * gain
                    });
                    let error = f64::from(actual[frame * 2 + output_ch]) - expected;
                    max_error = max_error.max(error.abs());
                    squared_error += error * error;
                }
            }
            let rms_error = (squared_error / actual.len() as f64).sqrt();
            assert!(
                max_error <= 1e-5 && rms_error <= 1e-6,
                "nupc={use_nupc} head={zero_latency_head}/{head_taps} callbacks={callback_pattern:?}: peak={max_error:e}, rms={rms_error:e}"
            );
        }
    }
}

#[test]
fn legacy_four_channel_ir_still_uses_only_the_first_two_output_paths() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 263;
    const IR_FRAMES: usize = 1_127;
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let t = frame as f32;
            [0.15 * (0.073 * t).sin(), 0.12 * (0.041 * t).cos()]
        })
        .collect();
    let mut impulse: Vec<Vec<i16>> = (0..4)
        .map(|path| {
            (0..IR_FRAMES)
                .map(|tap| ((tap * (13 + path * 19) % 97) as i16 - 48) * 2)
                .collect()
        })
        .collect();
    impulse[2].fill(30_000);
    impulse[3].fill(-29_000);
    impulse[0][0] = 16_000;
    impulse[1][IR_FRAMES - 1] = -12_000;
    let file = write_impulse(&impulse, RATE);
    let reference = direct_legacy_four_channel_convolution(&input, &impulse);

    for (use_nupc, zero_latency_head, head_taps) in [
        (false, false, 128),
        (true, false, 128),
        (true, true, 32),
        (true, true, 128),
    ] {
        // The old constructor intentionally preserves the pre-AUD134 route.
        let mut processor = ConvolutionPlugin::from_params(
            2,
            RATE,
            ConvolutionPluginParams {
                ir_file: file.0.to_str().unwrap().into(),
                mix: 1.0,
                gain_db: 0.0,
                use_nupc,
                zero_latency_head,
                head_taps,
            },
        )
        .unwrap();
        processor.initialize(f64::from(RATE)).unwrap();
        processor.reset();
        let latency = processor.latency_samples();
        let mut output = Vec::with_capacity((INPUT_FRAMES + latency + IR_FRAMES) * 2);
        let mut offset = 0;
        for frames in [1, 9, 257, 31, 511].into_iter().cycle() {
            if offset == INPUT_FRAMES {
                break;
            }
            let count = frames.min(INPUT_FRAMES - offset);
            let start = offset * 2;
            let end = (offset + count) * 2;
            let mut block = input[start..end].to_vec();
            processor
                .process_in_place(&mut block, &ProcessContext::new(RATE, count))
                .unwrap();
            output.extend_from_slice(&block);
            offset += count;
        }
        let mut drain = vec![0.0; 193 * 2];
        for _ in 0..100_000 {
            let result = processor
                .drain(&mut drain, &ProcessContext::new(RATE, 0))
                .unwrap();
            output.extend_from_slice(&drain[..result.frames * 2]);
            if result.complete {
                break;
            }
            assert!(result.frames > 0);
        }
        assert_eq!(output.len() / 2, INPUT_FRAMES + latency + IR_FRAMES - 1);

        let mut peak = 0.0_f64;
        for frame in 0..output.len() / 2 {
            for channel in 0..2 {
                let expected = frame.checked_sub(latency).map_or(0.0, |index| {
                    reference[channel].get(index).copied().unwrap_or(0.0)
                });
                peak = peak.max((f64::from(output[frame * 2 + channel]) - expected).abs());
            }
        }
        assert!(
            peak <= 1e-5,
            "legacy nupc={use_nupc} head={zero_latency_head}/{head_taps}: peak={peak:e}"
        );
    }
}

#[test]
fn each_true_stereo_path_isolated_against_the_f64_oracle_through_eos() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 23;
    const IR_FRAMES: usize = 2_053;
    let callback_patterns: [&[usize]; 3] = [&[1], &[1_024], &[17, 511, 3]];

    for (use_nupc, zero_latency_head, head_taps) in
        [(false, false, 128), (true, false, 128), (true, true, 128)]
    {
        for path in 0..4 {
            let mut matrix: Vec<Vec<i16>> = (0..4).map(|_| vec![0; IR_FRAMES]).collect();
            matrix[path][0] = 16_384;
            matrix[path][127] = -4_096;
            matrix[path][1_023] = 2_048;
            matrix[path][1_024] = -1_024;
            matrix[path][IR_FRAMES - 1] = 512;
            let file = write_impulse(&matrix, RATE);
            let source_channel = if path < 2 { 0 } else { 1 };
            let mut input = vec![0.0_f32; INPUT_FRAMES * 2];
            input[source_channel] = 0.5;
            input[14 * 2 + source_channel] = -0.25;
            let reference_ir: Vec<Vec<f64>> = matrix
                .iter()
                .map(|samples| {
                    samples
                        .iter()
                        .map(|&sample| f64::from(sample) / 32768.0)
                        .collect()
                })
                .collect();
            let reference = direct_true_stereo_convolution(&input, &reference_ir);

            for (partition, callback_pattern) in callback_patterns.iter().enumerate() {
                let mut processor = ConvolutionPlugin::from_params_with_routing(
                    2,
                    RATE,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_string_lossy().into_owned(),
                        mix: 1.0,
                        gain_db: 0.0,
                        use_nupc,
                        zero_latency_head,
                        head_taps,
                    },
                    true,
                )
                .unwrap();
                processor.initialize(f64::from(RATE)).unwrap();
                processor.reset();
                let expected_latency = if use_nupc && zero_latency_head {
                    0
                } else {
                    NORMAL_LATENCY
                };
                assert_eq!(processor.latency_samples(), expected_latency);
                let actual = render_true_stereo(&mut processor, &input, callback_pattern);
                let output_frames = actual.len() / 2;
                assert_eq!(
                    output_frames,
                    INPUT_FRAMES + expected_latency + IR_FRAMES - 1
                );
                for frame in 0..output_frames {
                    for output_channel in 0..2 {
                        let expected = frame
                            .checked_sub(expected_latency)
                            .and_then(|index| reference[output_channel].get(index).copied())
                            .unwrap_or(0.0);
                        let error =
                            (f64::from(actual[frame * 2 + output_channel]) - expected).abs();
                        assert!(
                            error <= 1e-5,
                            "path={path} backend={use_nupc}/{zero_latency_head} partition={partition} frame={frame} out={output_channel}: error={error:e}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rejected_true_stereo_ir_replacement_preserves_live_processing_history() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 2_879;
    const IR_FRAMES: usize = 2_053;
    let mut matrix: Vec<Vec<i16>> = (0..4)
        .map(|path| {
            (0..IR_FRAMES)
                .map(|tap| ((tap * (11 + 9 * path) % 67) as i16 - 33) * 4)
                .collect()
        })
        .collect();
    for (path, tap, value) in [
        (0, 0, 12_000),
        (1, 1_024, -9_000),
        (2, 1_025, 7_000),
        (3, 2_052, 5_000),
    ] {
        matrix[path][tap] += value;
    }
    let good_file = write_impulse(&matrix, RATE);
    let invalid_file = write_impulse(&matrix[..2], RATE);
    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let t = frame as f32;
            [0.12 * (0.11 * t).sin(), 0.08 * (0.047 * t).cos()]
        })
        .collect();
    let callback_pattern = [13, 511, 1_027, 7];

    for (use_nupc, zero_latency_head, head_taps) in
        [(false, false, 128), (true, false, 128), (true, true, 128)]
    {
        let params = ConvolutionPluginParams {
            ir_file: good_file.0.to_string_lossy().into_owned(),
            mix: 1.0,
            gain_db: -1.0,
            use_nupc,
            zero_latency_head,
            head_taps,
        };
        let mut subject =
            ConvolutionPlugin::from_params_with_routing(2, RATE, params.clone(), true).unwrap();
        let mut control =
            ConvolutionPlugin::from_params_with_routing(2, RATE, params, true).unwrap();
        subject.initialize(f64::from(RATE)).unwrap();
        control.initialize(f64::from(RATE)).unwrap();
        subject.reset();
        control.reset();

        let split = 1_537 * 2;
        let mut subject_output =
            process_callbacks(&mut subject, &input[..split], &callback_pattern);
        let mut control_output =
            process_callbacks(&mut control, &input[..split], &callback_pattern);
        let error = subject
            .load_ir(invalid_file.0.to_string_lossy().as_ref())
            .expect_err("two-channel IR must be rejected in matrix mode");
        assert!(error.contains("requires exactly 4 IR channels"));

        subject_output.extend(process_callbacks(
            &mut subject,
            &input[split..],
            &callback_pattern,
        ));
        control_output.extend(process_callbacks(
            &mut control,
            &input[split..],
            &callback_pattern,
        ));
        subject_output.extend(drain_all(&mut subject));
        control_output.extend(drain_all(&mut control));
        assert_eq!(subject_output.len(), control_output.len());
        for (index, (actual, expected)) in subject_output.iter().zip(&control_output).enumerate() {
            assert_eq!(actual.to_bits(), expected.to_bits(), "sample {index}");
        }
    }
}

#[test]
fn true_stereo_async_replacement_uses_new_ir_and_reset_matches_fresh_instance() {
    const RATE: u32 = 48_000;
    const IR_FRAMES: usize = 2_053;
    let make_ir = |salt: usize| {
        let mut matrix: Vec<Vec<i16>> = (0..4)
            .map(|path| {
                (0..IR_FRAMES)
                    .map(|tap| ((tap * (13 + path * 7 + salt) % 89) as i16 - 44) * 3)
                    .collect()
            })
            .collect();
        for (path, response) in matrix.iter_mut().enumerate() {
            response[0] += 9_000 + path as i16 * 1_000;
            response[511] -= 4_000 + path as i16 * 200;
            response[1_024] += 2_000 + path as i16 * 100;
            response[IR_FRAMES - 1] -= 1_000 + path as i16 * 50;
        }
        write_impulse(&matrix, RATE)
    };
    let initial_ir = make_ir(0);
    let replacement_ir = make_ir(5);
    let params = |path: &std::path::Path| ConvolutionPluginParams {
        ir_file: path.to_string_lossy().into_owned(),
        mix: 1.0,
        gain_db: -1.0,
        use_nupc: true,
        zero_latency_head: false,
        head_taps: 128,
    };
    let mut subject =
        ConvolutionPlugin::from_params_with_routing(2, RATE, params(&initial_ir.0), true).unwrap();
    subject.initialize(f64::from(RATE)).unwrap();
    subject.reset();

    let old_signal: Vec<f32> = (0..1_537)
        .flat_map(|frame| {
            let time = frame as f32;
            [0.4 * (0.071 * time).sin(), 0.3 * (0.047 * time).cos()]
        })
        .collect();
    let old_output = process_callbacks(&mut subject, &old_signal, &[1, 37, 509, 13]);
    assert!(old_output.iter().any(|sample| sample.abs() > 1e-4));

    let replacement_path = replacement_ir.0.to_string_lossy().into_owned();
    subject
        .set_parameter(
            ParameterId::from("ir_file"),
            ParameterValue::String(replacement_path.clone()),
        )
        .expect("queue a valid four-path IR through the async parameter route");
    assert_eq!(subject.load_status(), ConvolutionLoadStatus::Loading);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let mut no_frames = [];
        subject
            .process_in_place(&mut no_frames, &ProcessContext::new(RATE, 0))
            .unwrap();
        match subject.load_status() {
            ConvolutionLoadStatus::Ready => break,
            ConvolutionLoadStatus::Failed => panic!("valid async four-path IR load failed"),
            ConvolutionLoadStatus::Idle => panic!("async IR load returned to idle unexpectedly"),
            ConvolutionLoadStatus::Loading => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "async four-path IR load did not complete within ten seconds"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    assert_eq!(
        subject.parametric_get_parameter(&ParameterId::from("ir_file")),
        Some(ParameterValue::String(replacement_path))
    );

    let mut fresh =
        ConvolutionPlugin::from_params_with_routing(2, RATE, params(&replacement_ir.0), true)
            .unwrap();
    fresh.initialize(f64::from(RATE)).unwrap();
    fresh.reset();

    let replacement_input: Vec<f32> = (0..1_601)
        .flat_map(|frame| {
            let time = frame as f32;
            [0.2 * (0.059 * time).sin(), -0.17 * (0.037 * time).cos()]
        })
        .collect();
    let replaced_output = render_true_stereo(&mut subject, &replacement_input, &[1, 17, 511, 29]);
    let fresh_output = render_true_stereo(&mut fresh, &replacement_input, &[1, 17, 511, 29]);
    assert_eq!(replaced_output.len(), fresh_output.len());
    for (index, (actual, expected)) in replaced_output
        .iter()
        .zip(&fresh_output)
        .enumerate()
        .skip(128 * 2)
    {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "successful replacement differs from a fresh replacement IR instance at sample {index}"
        );
    }

    // Both plugins have now processed nonzero audio and reached EOS. Resetting
    // the replaced instance must discard its old stream clock and match fresh.
    subject.reset();
    fresh.reset();
    let reset_input: Vec<f32> = (0..1_139)
        .flat_map(|frame| {
            let time = frame as f32;
            [0.16 * (0.083 * time).sin(), 0.11 * (0.029 * time).cos()]
        })
        .collect();
    let reset_output = render_true_stereo(&mut subject, &reset_input, &[7, 257, 1, 31]);
    let fresh_reset_output = render_true_stereo(&mut fresh, &reset_input, &[7, 257, 1, 31]);
    assert_eq!(reset_output.len(), fresh_reset_output.len());
    for (index, (actual, expected)) in reset_output.iter().zip(&fresh_reset_output).enumerate() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "post-EOS reset differs from a fresh instance at sample {index}"
        );
    }
}

#[test]
fn true_stereo_process_and_full_eos_drain_do_not_allocate_or_deallocate() {
    const RATE: u32 = 48_000;
    const IR_FRAMES: usize = 8_197;
    const INPUT_FRAMES: usize = 8_193;
    let mut impulse = vec![vec![0_i16; IR_FRAMES]; 4];
    for (path, tap, value) in [
        (0, 0, 4096),
        (1, 127, -3072),
        (2, 128, 2048),
        (3, 1_024, -1536),
        (0, 2_048, 1024),
        (1, 4_096, -768),
        (2, 8_196, 512),
    ] {
        impulse[path][tap] = value;
    }
    let file = write_impulse(&impulse, RATE);

    for (use_nupc, zero_latency_head, head_taps) in [
        (false, false, 128),
        (true, false, 128),
        (true, true, 128),
        (true, true, 512),
    ] {
        let mut processor = ConvolutionPlugin::from_params_with_routing(
            2,
            RATE,
            ConvolutionPluginParams {
                ir_file: file.0.to_str().unwrap().into(),
                mix: 1.0,
                gain_db: 0.0,
                use_nupc,
                zero_latency_head,
                head_taps,
            },
            true,
        )
        .unwrap();
        processor.initialize(f64::from(RATE)).unwrap();
        processor.reset();
        let mut input: Vec<f32> = (0..INPUT_FRAMES)
            .flat_map(|frame| {
                let t = frame as f32;
                [0.004 * (0.071 * t).sin(), 0.003 * (0.037 * t).cos()]
            })
            .collect();
        let expected_tail = match processor.tail_length() {
            TailLength::Finite(frames) => frames as usize,
            other => panic!("true-stereo IR should have finite support, got {other:?}"),
        };
        let callback_pattern = [1, 511, 1_024, 2_048, 257];
        let mut drain = vec![0.0_f32; 137 * 2];
        let mut drained_frames = 0;
        let mut drain_complete = false;
        assert_no_heap_activity("true-stereo callbacks plus complete EOS", || {
            let mut input_offset = 0;
            let mut callback_index = 0;
            while input_offset < INPUT_FRAMES {
                let requested = callback_pattern[callback_index % callback_pattern.len()];
                callback_index += 1;
                let frames = requested.min(INPUT_FRAMES - input_offset);
                let begin = input_offset * 2;
                let end = (input_offset + frames) * 2;
                processor
                    .process_in_place(&mut input[begin..end], &ProcessContext::new(RATE, frames))
                    .unwrap();
                input_offset += frames;
            }

            for _ in 0..100_000 {
                let result = processor
                    .drain(&mut drain, &ProcessContext::new(RATE, 0))
                    .unwrap();
                drained_frames += result.frames;
                if result.complete {
                    drain_complete = true;
                    break;
                }
                assert!(result.frames > 0, "EOS drain must make bounded progress");
            }
        });
        assert!(drain_complete, "full EOS drain must reach completion");
        assert_eq!(drained_frames, expected_tail);
    }
}

#[test]
fn true_stereo_rustfft_plan_allocations_fit_reserve_for_accepted_head_sizes_and_levels() {
    // Includes power-of-two sizes, accepted head sizes that produce
    // non-power-of-two FFTs (notably 257 -> 514), and larger partition levels.
    for fft_size in [
        64, 128, 514, 576, 1_022, 1_028, 2_056, 4_112, 8_224, 16_448, 65_536, 131_072,
    ] {
        let allocated = allocated_bytes_while(|| {
            let mut planner = FftPlanner::<f32>::new();
            let forward = planner.plan_fft_forward(fft_size);
            let inverse = planner.plan_fft_inverse(fft_size);
            let scratch = forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len());
            assert!(
                scratch <= fft_size * 8,
                "rustfft {fft_size}-point scratch {scratch} exceeds conservative Bluestein reserve"
            );
            std::hint::black_box((forward, inverse));
        });
        let reserved =
            fft_size * 26 * std::mem::size_of::<rustfft::num_complex::Complex<f32>>() + 4 * 1024;
        assert!(
            allocated <= reserved,
            "rustfft {fft_size}-point forward/inverse plans allocated {allocated} bytes, estimate reserves {reserved}"
        );
    }
}

#[test]
fn tail_bound_retains_the_last_direct_convolution_tap_for_every_backend() {
    for rate in [44_100, 48_000, 96_000] {
        for ir_frames in [1, 255, 256, 257, 1023, 1024, 1025, 4097] {
            let mut impulse = vec![vec![0_i16; ir_frames]; 2];
            impulse[0][0] = 8192;
            impulse[1][0] = -4096;
            impulse[0][ir_frames - 1] = 16384;
            impulse[1][ir_frames - 1] = -8192;
            let file = write_impulse(&impulse, rate);
            for (use_nupc, zero_latency_head, head_taps) in [
                (false, false, 128),
                (true, false, 128),
                (true, true, 32),
                (true, true, 128),
                (true, true, 512),
            ] {
                let mut plugin = ConvolutionPlugin::from_params(
                    3,
                    rate,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_str().unwrap().into(),
                        use_nupc,
                        zero_latency_head,
                        head_taps,
                        ..ConvolutionPluginParams::default()
                    },
                )
                .unwrap();
                plugin.reset();
                let latency = if use_nupc && zero_latency_head {
                    0
                } else {
                    1024
                };
                let bound = latency + ir_frames - 1;
                assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
                let mut input = vec![0.0; 259 * 3];
                input[0..3].copy_from_slice(&[0.5, -0.25, 0.75]);
                input[258 * 3..].copy_from_slice(&[-0.75, 0.5, 0.25]);
                let reference = direct_convolution(&input, &impulse);
                let mut actual = vec![0.0; (259 + bound + 2049) * 3];
                actual[..input.len()].copy_from_slice(&input);
                let mut offset = 0;
                for frames in [1, 17, 255, 1031, 7, 509].into_iter().cycle() {
                    if offset == actual.len() {
                        break;
                    }
                    let end = (offset + frames * 3).min(actual.len());
                    plugin
                        .process_in_place(
                            &mut actual[offset..end],
                            &ProcessContext::new(rate, (end - offset) / 3),
                        )
                        .unwrap();
                    offset = end;
                }
                let max_error = actual
                    .iter()
                    .enumerate()
                    .map(|(i, &sample)| {
                        let expected = i
                            .checked_sub(latency * 3)
                            .and_then(|index| reference.get(index))
                            .copied()
                            .unwrap_or(0.0);
                        (f64::from(sample) - expected).abs()
                    })
                    .fold(0.0_f64, f64::max);
                assert!(
                    max_error < 1e-5,
                    "rate={rate}, length={ir_frames}, nupc={use_nupc}, head={head_taps}/{zero_latency_head}: {max_error:e}"
                );
                let final_frame = 258 + bound;
                assert!((actual[final_frame * 3] + 0.375).abs() < 1e-5);
                assert!(
                    actual[(final_frame + 1) * 3..]
                        .iter()
                        .all(|x| x.abs() < 1e-5)
                );
                // A bound describes the configured response, not elapsed silence.
                assert_eq!(plugin.tail_length(), TailLength::Finite(bound as u64));
            }
        }
    }
}

#[test]
fn tail_getter_does_not_allocate_on_a_fresh_thread() {
    let file = write_impulse(&[vec![16384; 4097]], 48_000);
    let plugin = ConvolutionPlugin::from_params(
        1,
        48_000,
        ConvolutionPluginParams {
            ir_file: file.0.to_str().unwrap().into(),
            ..ConvolutionPluginParams::default()
        },
    )
    .unwrap();
    let plugin = std::thread::spawn(move || {
        sotf_host::assert_no_allocs("Convolution cold tail getter", || {
            for _ in 0..32 {
                assert!(matches!(
                    std::hint::black_box(plugin.tail_length()),
                    TailLength::Finite(_)
                ));
            }
        });
        plugin
    })
    .join()
    .unwrap();
    drop(plugin);
}

#[test]
fn scalar_parameter_access_and_automation_do_not_allocate_on_a_fresh_thread() {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    let plugin = ConvolutionPlugin::new(1, 48_000);
    let ids = [
        "mix",
        "gain_db",
        "use_nupc",
        "zero_latency_head",
        "head_taps",
        "missing",
    ]
    .map(ParameterId::from);
    let mix = ParameterId::from("mix");
    let gain = ParameterId::from("gain_db");
    let plugin = std::thread::spawn(move || {
        let mut plugin = plugin;
        sotf_host::assert_no_allocs("Convolution cold scalar getter", || {
            for id in &ids {
                std::hint::black_box(plugin.parametric_get_parameter(id));
            }
        });
        sotf_host::assert_no_allocs("Convolution scalar automation", || {
            plugin
                .parametric_validate_parameter(&mix, &ParameterValue::Float(0.375))
                .unwrap();
            plugin
                .parametric_set_parameter(mix, ParameterValue::Float(0.375))
                .unwrap();
            plugin
                .parametric_set_parameter(gain, ParameterValue::Float(-3.0))
                .unwrap();
        });
        plugin
    })
    .join()
    .unwrap();
    let current = plugin.current_values();
    for (key, expected) in [
        ("mix", ParameterValue::Float(0.375)),
        ("gain_db", ParameterValue::Float(-3.0)),
        ("use_nupc", ParameterValue::Bool(true)),
        ("zero_latency_head", ParameterValue::Bool(false)),
        ("head_taps", ParameterValue::Int(128)),
        ("ir_file", ParameterValue::String(String::new())),
    ] {
        let id = ParameterId::from(key);
        assert_eq!(plugin.parametric_get_parameter(&id), Some(expected.clone()));
        assert_eq!(current.get(&id), Some(&expected));
    }
}

#[test]
#[ignore = "AUD-134 pre-edit audio baseline capture from clean 9302797"]
fn capture_aud134_preedit_diagonal_stereo_outputs() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 2_051;
    const IR_FRAMES: usize = 2_053;
    let capture_dir = std::env::var("SOTF_AUD134_CAPTURE_DIR")
        .expect("set SOTF_AUD134_CAPTURE_DIR for the one-time baseline capture");
    std::fs::create_dir_all(&capture_dir).unwrap();

    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let phase = frame as f32;
            let left = 0.08 * (0.017 * phase).sin()
                + if [0, 511, 1_024, 2_050].contains(&frame) {
                    0.25
                } else {
                    0.0
                };
            let right = 0.06 * (0.031 * phase).cos()
                + if [7, 1_023, 2_049].contains(&frame) {
                    -0.2
                } else {
                    0.0
                };
            [left, right]
        })
        .collect();
    let mut impulse = vec![vec![0_i16; IR_FRAMES]; 2];
    for (channel, response) in impulse.iter_mut().enumerate() {
        for (tap, sample) in response.iter_mut().enumerate() {
            *sample = ((tap * (19 + channel * 12) % 67) as i16 - 33) * 4;
        }
        for (tap, value) in [
            (0, 18_000),
            (127, -7_000),
            (1_023, 5_000),
            (1_024, -4_000),
            (2_048, 3_000),
            (IR_FRAMES - 1, 8_000),
        ] {
            response[tap] += if channel == 0 { value } else { -value / 2 };
        }
    }
    let file = write_impulse(&impulse, RATE);

    for (name, use_nupc, zero_latency_head, head_taps) in [
        ("upc", false, false, 128),
        ("nupc", true, false, 128),
        ("nupc_head", true, true, 128),
    ] {
        let mut plugin = ConvolutionPlugin::from_params(
            2,
            RATE,
            ConvolutionPluginParams {
                ir_file: file.0.to_str().unwrap().into(),
                mix: 1.0,
                gain_db: -3.0,
                use_nupc,
                zero_latency_head,
                head_taps,
            },
        )
        .unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.reset();
        let mut output = Vec::new();
        let mut input_offset = 0;
        for requested_frames in [1, 17, 257, 1_023, 11, 509].into_iter().cycle() {
            if input_offset == INPUT_FRAMES {
                break;
            }
            let frames = requested_frames.min(INPUT_FRAMES - input_offset);
            let mut block = input[input_offset * 2..(input_offset + frames) * 2].to_vec();
            plugin
                .process_in_place(&mut block, &ProcessContext::new(RATE, frames))
                .unwrap();
            output.extend(block);
            input_offset += frames;
        }

        let capacity_frames = 137;
        let mut drain_buffer = vec![0.0; capacity_frames * 2];
        for _ in 0..100_000 {
            let result = plugin
                .drain(&mut drain_buffer, &ProcessContext::new(RATE, 0))
                .unwrap();
            output.extend_from_slice(&drain_buffer[..result.frames * 2]);
            if result.complete {
                break;
            }
            assert!(result.frames > 0);
        }
        let latency = plugin.latency_samples();
        assert_eq!(
            output.len() / 2,
            INPUT_FRAMES + latency + IR_FRAMES - 1,
            "pre-edit {name} finite stream length"
        );
        let bytes: Vec<u8> = output
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let path = std::path::Path::new(&capture_dir).join(format!("{name}.f32le"));
        std::fs::write(&path, &bytes).unwrap();
        let digest = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        });
        eprintln!(
            "AUD134 pre-edit diagonal stereo {name}: rate={RATE} frames={} latency={latency} ir_frames={IR_FRAMES} digest={digest:016x}",
            output.len() / 2,
        );
    }
}

#[test]
#[ignore = "AUD-134 pre-edit four-channel legacy mapping capture from clean 9302797"]
fn capture_aud134_preedit_four_channel_legacy_outputs() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 2_051;
    const IR_FRAMES: usize = 2_053;
    let capture_dir = std::env::var("SOTF_AUD134_LEGACY_CAPTURE_DIR")
        .expect("set SOTF_AUD134_LEGACY_CAPTURE_DIR for the one-time baseline capture");
    std::fs::create_dir_all(&capture_dir).unwrap();

    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let phase = frame as f32;
            [
                0.08 * (0.017 * phase).sin()
                    + if [0, 511, 1_024, 2_050].contains(&frame) {
                        0.25
                    } else {
                        0.0
                    },
                0.06 * (0.031 * phase).cos()
                    + if [7, 1_023, 2_049].contains(&frame) {
                        -0.2
                    } else {
                        0.0
                    },
            ]
        })
        .collect();
    let mut impulse = vec![vec![0_i16; IR_FRAMES]; 4];
    for (channel, response) in impulse.iter_mut().enumerate() {
        for (tap, sample) in response.iter_mut().enumerate() {
            *sample = ((tap * (17 + channel * 8) % 59) as i16 - 29) * 3;
        }
        for (tap, value) in [
            (0, 13_000 + channel as i16 * 1_100),
            (127, -6_000 - channel as i16 * 200),
            (1_023, 4_500 + channel as i16 * 100),
            (1_024, -3_500 - channel as i16 * 100),
            (2_048, 2_500 + channel as i16 * 100),
            (IR_FRAMES - 1, 6_000 + channel as i16 * 200),
        ] {
            response[tap] += value;
        }
    }
    let file = write_impulse(&impulse, RATE);

    for (name, use_nupc, zero_latency_head, head_taps) in [
        ("legacy_4ch_upc", false, false, 128),
        ("legacy_4ch_nupc", true, false, 128),
        ("legacy_4ch_nupc_head", true, true, 128),
    ] {
        let mut plugin = ConvolutionPlugin::from_params(
            2,
            RATE,
            ConvolutionPluginParams {
                ir_file: file.0.to_str().unwrap().into(),
                mix: 1.0,
                gain_db: -3.0,
                use_nupc,
                zero_latency_head,
                head_taps,
            },
        )
        .unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.reset();
        let mut output = Vec::new();
        let mut input_offset = 0;
        for requested_frames in [1, 17, 257, 1_023, 11, 509].into_iter().cycle() {
            if input_offset == INPUT_FRAMES {
                break;
            }
            let frames = requested_frames.min(INPUT_FRAMES - input_offset);
            let mut block = input[input_offset * 2..(input_offset + frames) * 2].to_vec();
            plugin
                .process_in_place(&mut block, &ProcessContext::new(RATE, frames))
                .unwrap();
            output.extend(block);
            input_offset += frames;
        }

        let capacity_frames = 137;
        let mut drain_buffer = vec![0.0; capacity_frames * 2];
        for _ in 0..100_000 {
            let result = plugin
                .drain(&mut drain_buffer, &ProcessContext::new(RATE, 0))
                .unwrap();
            output.extend_from_slice(&drain_buffer[..result.frames * 2]);
            if result.complete {
                break;
            }
            assert!(result.frames > 0);
        }
        let latency = plugin.latency_samples();
        assert_eq!(
            output.len() / 2,
            INPUT_FRAMES + latency + IR_FRAMES - 1,
            "pre-edit {name} finite stream length"
        );
        let bytes: Vec<u8> = output
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let path = std::path::Path::new(&capture_dir).join(format!("{name}.f32le"));
        std::fs::write(&path, &bytes).unwrap();
        let digest = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
        });
        eprintln!(
            "AUD134 pre-edit four-channel legacy {name}: rate={RATE} frames={} latency={latency} ir_frames={IR_FRAMES} digest={digest:016x}",
            output.len() / 2,
        );
    }
}

#[test]
#[ignore = "AUD-134 replay needs the preserved pre-edit rendered arrays under target/audit-artifacts"]
fn aud134_preedit_full_audio_arrays_match_legacy_and_diagonal_routes() {
    const RATE: u32 = 48_000;
    const INPUT_FRAMES: usize = 2_051;
    const IR_FRAMES: usize = 2_053;
    let artifact_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/audit-artifacts/aud134-preedit-9302797");
    let diagonal_dir = artifact_root.join("render-baseline");
    let legacy_dir = artifact_root.join("legacy-four-channel-baseline");
    assert!(
        diagonal_dir.is_dir(),
        "missing preserved diagonal baseline arrays"
    );
    assert!(
        legacy_dir.is_dir(),
        "missing preserved four-channel baseline arrays"
    );

    let input: Vec<f32> = (0..INPUT_FRAMES)
        .flat_map(|frame| {
            let phase = frame as f32;
            let left = 0.08 * (0.017 * phase).sin()
                + if [0, 511, 1_024, 2_050].contains(&frame) {
                    0.25
                } else {
                    0.0
                };
            let right = 0.06 * (0.031 * phase).cos()
                + if [7, 1_023, 2_049].contains(&frame) {
                    -0.2
                } else {
                    0.0
                };
            [left, right]
        })
        .collect();

    let verify = |name: &str,
                  impulse: &[Vec<i16>],
                  baseline_dir: &std::path::Path,
                  use_nupc: bool,
                  zero_latency_head: bool| {
        let file = write_impulse(impulse, RATE);
        let mut plugin = ConvolutionPlugin::from_params(
            2,
            RATE,
            ConvolutionPluginParams {
                ir_file: file.0.to_string_lossy().into_owned(),
                mix: 1.0,
                gain_db: -3.0,
                use_nupc,
                zero_latency_head,
                head_taps: 128,
            },
        )
        .unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.reset();
        let mut actual = process_callbacks(&mut plugin, &input, &[1, 17, 257, 1_023, 11, 509]);
        let mut drain_buffer = vec![0.0_f32; 137 * 2];
        for _ in 0..100_000 {
            let result = plugin
                .drain(&mut drain_buffer, &ProcessContext::new(RATE, 0))
                .unwrap();
            actual.extend_from_slice(&drain_buffer[..result.frames * 2]);
            if result.complete {
                break;
            }
            assert!(result.frames > 0, "baseline replay drain must progress");
        }
        let expected_frames = INPUT_FRAMES + plugin.latency_samples() + IR_FRAMES - 1;
        assert_eq!(actual.len() / 2, expected_frames, "{name} frame count");
        let actual_bytes: Vec<u8> = actual
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let archived = std::fs::read(baseline_dir.join(format!("{name}.f32le")))
            .expect("read the clean-source pre-edit sample capture");
        assert_eq!(
            actual_bytes, archived,
            "pre-edit full audio differs for {name}"
        );
    };

    let mut stereo_ir = vec![vec![0_i16; IR_FRAMES]; 2];
    for (channel, response) in stereo_ir.iter_mut().enumerate() {
        for (tap, sample) in response.iter_mut().enumerate() {
            *sample = ((tap * (19 + channel * 12) % 67) as i16 - 33) * 4;
        }
        for (tap, value) in [
            (0, 18_000),
            (127, -7_000),
            (1_023, 5_000),
            (1_024, -4_000),
            (2_048, 3_000),
            (IR_FRAMES - 1, 8_000),
        ] {
            response[tap] += if channel == 0 { value } else { -value / 2 };
        }
    }
    verify("upc", &stereo_ir, &diagonal_dir, false, false);
    verify("nupc", &stereo_ir, &diagonal_dir, true, false);
    verify("nupc_head", &stereo_ir, &diagonal_dir, true, true);

    let mut four_channel_ir = vec![vec![0_i16; IR_FRAMES]; 4];
    for (channel, response) in four_channel_ir.iter_mut().enumerate() {
        for (tap, sample) in response.iter_mut().enumerate() {
            *sample = ((tap * (17 + channel * 8) % 59) as i16 - 29) * 3;
        }
        for (tap, value) in [
            (0, 13_000 + channel as i16 * 1_100),
            (127, -6_000 - channel as i16 * 200),
            (1_023, 4_500 + channel as i16 * 100),
            (1_024, -3_500 - channel as i16 * 100),
            (2_048, 2_500 + channel as i16 * 100),
            (IR_FRAMES - 1, 6_000 + channel as i16 * 200),
        ] {
            response[tap] += value;
        }
    }
    verify(
        "legacy_4ch_upc",
        &four_channel_ir,
        &legacy_dir,
        false,
        false,
    );
    verify(
        "legacy_4ch_nupc",
        &four_channel_ir,
        &legacy_dir,
        true,
        false,
    );
    verify(
        "legacy_4ch_nupc_head",
        &four_channel_ir,
        &legacy_dir,
        true,
        true,
    );
}
