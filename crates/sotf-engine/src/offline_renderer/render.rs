use super::offline_render_config::OfflineRenderConfig;
use super::render_progress::RenderProgress;
use super::timeline_render_state_guard::TimelineRenderStateGuard;
use super::types::OutputFormat;
use crate::decoder::core::DecodedAudio;
use crate::decoder::source::AudioSource;
use crate::engine::build_plugin_host;
use crate::engine::output_dither::TpdfDither;
use hound::{SampleFormat, WavSpec, WavWriter};
use sotf_plugins::{DawHost, Plugin, ProcessContext, ResamplerPlugin};
use std::io::{Seek, Write};
use std::path::Path;
use std::time::Duration;

const OFFLINE_DITHER_SEED: u64 = 0x6f66_666c_696e_655f;

struct HostOutputState {
    latency_remaining: usize,
    frames_written: usize,
}

impl HostOutputState {
    fn new(latency_samples: usize) -> Self {
        Self {
            latency_remaining: latency_samples,
            frames_written: 0,
        }
    }
}

/// Render audio offline at maximum CPU speed.
///
/// The progress callback runs synchronously on the rendering thread. If it
/// panics, the panic propagates to the caller and the output file may contain a
/// valid but incomplete render. Callers that cannot tolerate callback panics
/// should isolate them before passing the callback here.
pub fn render_offline(
    config: &OfflineRenderConfig,
    on_progress: Option<&mut dyn FnMut(&RenderProgress)>,
) -> Result<(), String> {
    render_offline_with_tail(config, Duration::ZERO, on_progress)
}

/// Render a source with an explicitly timed extra effect tail.
///
/// After the program ends, feed silence through the unchanged plugin chain for
/// `tail_duration`, rounded up to whole frames at the export sample rate.
/// Algorithmic signal delay is compensated as in [`render_offline`]. Recursive
/// effects continue naturally until this requested endpoint; no silence
/// threshold or finite-response claim is implied. [`Duration::ZERO`] preserves
/// the original export duration. Existing configuration literals remain valid.
///
/// # Errors
/// Returns an error for unrepresentable durations, invalid configuration, or
/// decoder, plugin, and output-file failures.
///
/// # Panics
/// A panic in the synchronous progress callback propagates to the caller and
/// can leave an incomplete output file, as with [`render_offline`].
pub fn render_offline_with_tail(
    config: &OfflineRenderConfig,
    tail_duration: Duration,
    mut on_progress: Option<&mut dyn FnMut(&RenderProgress)>,
) -> Result<(), String> {
    if config.frame_size == 0 {
        return Err("Offline render frame_size must be greater than zero".to_string());
    }

    let mut decoder = crate::decoder::core::create_decoder_from_source(&config.source)
        .map_err(|e| format!("Failed to open source: {e}"))?;
    let source_spec = decoder.spec().clone();
    let source_rate = source_spec.sample_rate;
    let output_rate = config.output_sample_rate.unwrap_or(source_rate);
    let input_channels = source_spec.channels as usize;
    if output_rate == 0 {
        return Err("Offline render output sample rate must be greater than zero".to_string());
    }
    let tail_frames = tail_duration_frames(tail_duration, output_rate)?;
    // Validate known total duration before creating or truncating the output.
    let progress_total = source_spec
        .total_frames
        .map(|frames| {
            u64::try_from(
                (u128::from(frames) * u128::from(output_rate)).div_ceil(u128::from(source_rate))
                    + tail_frames as u128,
            )
        })
        .transpose()
        .map_err(|_| "Offline progress duration exceeds the frame counter")?;

    let (mut host, _warnings) = build_plugin_host(&config.plugins, output_rate, input_channels)
        .map_err(|diagnostic| diagnostic.message)?;
    // The file rate is an export contract, even when the configured chain
    // changes its internal clock. Normalize the terminal stream before
    // measuring latency, trimming duration, or constructing the WAV header.
    let terminal_rate = host.output_sample_rate(output_rate)?;
    if terminal_rate != f64::from(output_rate) {
        host.add_plugin(Box::new(ResamplerPlugin::new(
            host.output_channels(),
            terminal_rate,
            output_rate,
            config.frame_size,
        )?))?;
    }
    host.build()?;
    let output_channels = host.output_channels();

    let wav_spec = wav_spec(&config.format, output_channels, output_rate)?;
    let mut writer = WavWriter::create(&config.output_path, wav_spec)
        .map_err(|e| format!("Failed to create output file: {e}"))?;
    let mut dither = TpdfDither::new(OFFLINE_DITHER_SEED);

    let mut resampler = if source_rate == output_rate {
        None
    } else {
        Some(ResamplerPlugin::new(
            input_channels,
            source_rate,
            output_rate,
            config.frame_size,
        )?)
    };
    let resampler_delay = resampler.as_ref().map_or(0.0, Plugin::signal_delay_samples);
    // Keep the source converter's leading output in the stream. Combining
    // its delay with the chain avoids rounding and trimming at every stage.
    let mut host_output = HostOutputState::new(round_signal_delay(
        resampler_delay + serial_signal_delay(&host, output_rate)?,
    )?);

    let mut decode_buf = DecodedAudio::new(source_spec.clone());
    let mut resample_output = Vec::<f32>::new();
    let mut process_output = Vec::<f32>::new();
    let mut source_frames_decoded = 0usize;

    loop {
        decode_buf.clear();
        let decoded_frames = decoder
            .decode_into(&mut decode_buf)
            .map_err(|e| format!("Decode error: {e}"))?;
        if decoded_frames == 0 {
            break;
        }
        source_frames_decoded = source_frames_decoded.saturating_add(decoded_frames);

        if let Some(resampler) = resampler.as_mut() {
            let max_frames = resampler.output_frames_for_input(decoded_frames);
            resample_output.resize(max_frames.saturating_mul(input_channels), 0.0);
            let produced = resampler.process(
                &decode_buf.samples,
                &mut resample_output,
                &ProcessContext::new(source_rate, decoded_frames),
            )?;
            process_and_write(
                &resample_output[..produced * input_channels],
                produced,
                input_channels,
                &mut host,
                config.frame_size,
                &mut process_output,
                &mut writer,
                wav_spec,
                &mut dither,
                &mut host_output,
                usize::MAX,
            )?;
        } else {
            process_and_write(
                &decode_buf.samples,
                decoded_frames,
                input_channels,
                &mut host,
                config.frame_size,
                &mut process_output,
                &mut writer,
                wav_spec,
                &mut dither,
                &mut host_output,
                usize::MAX,
            )?;
        }

        if let Some(ref mut callback) = on_progress {
            callback(&RenderProgress {
                frames_processed: host_output.frames_written as u64,
                total_frames: progress_total,
            });
        }
    }

    let program_frames = usize::try_from(
        (source_frames_decoded as u128 * u128::from(output_rate)).div_ceil(u128::from(source_rate)),
    )
    .map_err(|_| "Offline program duration exceeds addressable frames")?;
    let target_frames = program_frames
        .checked_add(tail_frames)
        .ok_or("Offline total duration exceeds addressable frames")?;
    if let Some(resampler) = resampler.as_mut() {
        // Keep the converter in the path until the compensated export ends.
        // Downstream filters can still need its response after the source's
        // own delay has elapsed. Scheduling latency bounds work only; it must
        // not be trimmed from the concatenated signal.
        let maximum_blocks = continuation_blocks(
            &host_output,
            target_frames,
            host.total_latency_samples() as u128 + resampler.latency_samples() as u128,
            source_rate,
            output_rate,
            config.frame_size,
        )?;
        let silence = vec![0.0f32; config.frame_size * input_channels];
        for _ in 0..maximum_blocks {
            if host_output.frames_written >= target_frames {
                break;
            }
            let max_frames = resampler.output_frames_for_input(config.frame_size);
            resample_output.resize(max_frames.saturating_mul(input_channels), 0.0);
            let produced = resampler.process(
                &silence,
                &mut resample_output,
                &ProcessContext::new(source_rate, config.frame_size),
            )?;
            if produced == 0 {
                continue;
            }
            process_and_write(
                &resample_output[..produced * input_channels],
                produced,
                input_channels,
                &mut host,
                config.frame_size,
                &mut process_output,
                &mut writer,
                wav_spec,
                &mut dither,
                &mut host_output,
                target_frames,
            )?;
        }
        if host_output.frames_written < target_frames {
            return Err(format!(
                "Source conversion and plugin host did not reach the requested {target_frames} frames (wrote {})",
                host_output.frames_written
            ));
        }
    } else {
        drain_host_to_duration(
            &mut host,
            input_channels,
            config.frame_size,
            &mut process_output,
            &mut writer,
            wav_spec,
            &mut dither,
            &mut host_output,
            target_frames,
        )?;
    }

    if let Some(ref mut callback) = on_progress {
        callback(&RenderProgress {
            frames_processed: host_output.frames_written as u64,
            total_frames: progress_total,
        });
    }

    writer
        .finalize()
        .map_err(|e| format!("Failed to finalize output: {e}"))
}

fn tail_duration_frames(duration: Duration, sample_rate: u32) -> Result<usize, String> {
    let frames = u128::from(duration.as_secs()) * u128::from(sample_rate)
        + (u128::from(duration.subsec_nanos()) * u128::from(sample_rate)).div_ceil(1_000_000_000);
    usize::try_from(frames).map_err(|_| "Offline tail duration exceeds addressable frames".into())
}

/// Bound continuation work using both signal delay and chunk waiting. Rates
/// convert the complete budget once, rounding upwards in the supplied-input
/// clock. This budget does not determine which audio samples are exported.
fn continuation_blocks(
    state: &HostOutputState,
    target_frames: usize,
    scheduling_frames: u128,
    input_rate: u32,
    output_rate: u32,
    frame_size: usize,
) -> Result<usize, String> {
    let output_frames = target_frames.saturating_sub(state.frames_written) as u128
        + state.latency_remaining as u128
        + scheduling_frames;
    let input_frames = (output_frames * u128::from(input_rate)).div_ceil(u128::from(output_rate));
    usize::try_from(input_frames.div_ceil(frame_size as u128))
        .ok()
        .and_then(|blocks| blocks.checked_add(8))
        .ok_or_else(|| "Offline continuation exceeds addressable blocks".to_owned())
}

/// Sum fractional group delays in the terminal clock without per-stage rounding.
pub(super) fn serial_signal_delay(host: &DawHost, input_rate: u32) -> Result<f64, String> {
    let mut rate = f64::from(input_rate);
    let mut seconds = 0.0;
    for index in 0..host.plugin_count() {
        let plugin = host
            .get_plugin(index)
            .ok_or("Offline plugin chain is incomplete")?;
        rate = plugin.output_sample_rate(rate);
        if !rate.is_finite() || rate <= 0.0 {
            return Err("Offline plugin output sample rate must be finite and positive".to_owned());
        }
        let delay = plugin.signal_delay_samples();
        if !delay.is_finite() || delay < 0.0 {
            return Err(format!(
                "Offline plugin {index} declares invalid signal delay {delay}"
            ));
        }
        seconds += delay / rate;
    }
    let output_frames = seconds * rate;
    if !output_frames.is_finite() || output_frames >= usize::MAX as f64 {
        return Err("Offline signal delay exceeds addressable frames".to_owned());
    }
    Ok(output_frames)
}

pub(super) fn round_signal_delay(delay: f64) -> Result<usize, String> {
    if !delay.is_finite() || delay < 0.0 || delay >= usize::MAX as f64 {
        return Err("Offline signal delay must be finite, nonnegative, and addressable".to_owned());
    }
    // Nearest-frame trimming leaves at most half a frame of estimated delay.
    // This is sample-grid alignment, not a fractional-delay filter.
    Ok(delay.round() as usize)
}

#[allow(clippy::too_many_arguments)]
fn process_and_write<W: Write + Seek>(
    input: &[f32],
    input_frames: usize,
    input_channels: usize,
    host: &mut DawHost,
    frame_size: usize,
    process_output: &mut Vec<f32>,
    writer: &mut WavWriter<W>,
    wav_spec: WavSpec,
    dither: &mut TpdfDither,
    state: &mut HostOutputState,
    maximum_total_frames: usize,
) -> Result<usize, String> {
    let output_channels = host.output_channels();
    let mut offset = 0usize;
    let mut total_written = 0usize;
    while offset < input_frames {
        let chunk_frames = (input_frames - offset).min(frame_size);
        let input_start = offset * input_channels;
        let input_end = input_start + chunk_frames * input_channels;
        let output_capacity = host
            .output_frames_for_input(chunk_frames)
            .max(chunk_frames)
            .saturating_mul(output_channels);
        process_output.resize(output_capacity, 0.0);
        let actual_frames = host.process(
            &input[input_start..input_end],
            process_output.as_mut_slice(),
        )?;
        let skipped = actual_frames.min(state.latency_remaining);
        state.latency_remaining -= skipped;
        let available = actual_frames
            .saturating_sub(skipped)
            .min(maximum_total_frames.saturating_sub(state.frames_written));
        if available > 0 {
            let start = skipped * output_channels;
            let end = start + available * output_channels;
            write_wav_samples(writer, wav_spec, &process_output[start..end], dither)?;
            state.frames_written = state.frames_written.saturating_add(available);
            total_written = total_written.saturating_add(available);
        }
        offset += chunk_frames;
    }
    Ok(total_written)
}

#[allow(clippy::too_many_arguments)]
fn drain_host_to_duration<W: Write + Seek>(
    host: &mut DawHost,
    input_channels: usize,
    frame_size: usize,
    process_output: &mut Vec<f32>,
    writer: &mut WavWriter<W>,
    wav_spec: WavSpec,
    dither: &mut TpdfDither,
    state: &mut HostOutputState,
    target_frames: usize,
) -> Result<(), String> {
    if state.frames_written >= target_frames {
        return Ok(());
    }

    let silence = vec![0.0f32; frame_size * input_channels];
    let maximum_blocks = continuation_blocks(
        state,
        target_frames,
        host.total_latency_samples() as u128,
        wav_spec.sample_rate,
        wav_spec.sample_rate,
        frame_size,
    )?;
    for _ in 0..maximum_blocks {
        process_and_write(
            &silence,
            frame_size,
            input_channels,
            host,
            frame_size,
            process_output,
            writer,
            wav_spec,
            dither,
            state,
            target_frames,
        )?;
        if state.frames_written >= target_frames {
            return Ok(());
        }
    }

    Err(format!(
        "Plugin host did not drain to the requested {target_frames} frames (wrote {})",
        state.frames_written
    ))
}

fn wav_spec(format: &OutputFormat, channels: usize, sample_rate: u32) -> Result<WavSpec, String> {
    let channels = u16::try_from(channels)
        .map_err(|_| format!("Offline output channel count {channels} exceeds WAV limits"))?;
    match *format {
        OutputFormat::Wav { bits_per_sample } => match bits_per_sample {
            16 | 24 => Ok(WavSpec {
                channels,
                sample_rate,
                bits_per_sample,
                sample_format: SampleFormat::Int,
            }),
            32 => Ok(WavSpec {
                channels,
                sample_rate,
                bits_per_sample,
                sample_format: SampleFormat::Float,
            }),
            other => Err(format!(
                "Unsupported WAV depth {other}; expected 16-bit, 24-bit, or 32-bit float"
            )),
        },
    }
}

fn write_wav_samples<W: Write + Seek>(
    writer: &mut WavWriter<W>,
    wav_spec: WavSpec,
    samples: &[f32],
    dither: &mut TpdfDither,
) -> Result<(), String> {
    match (wav_spec.sample_format, wav_spec.bits_per_sample) {
        (SampleFormat::Float, 32) => {
            for &sample in samples {
                writer
                    .write_sample(sample)
                    .map_err(|e| format!("Write error: {e}"))?;
            }
        }
        (SampleFormat::Int, 16) => {
            for &sample in samples {
                writer
                    .write_sample(dither.quantize_signed(sample, 16) as i16)
                    .map_err(|e| format!("Write error: {e}"))?;
            }
        }
        (SampleFormat::Int, 24) => {
            for &sample in samples {
                writer
                    .write_sample(dither.quantize_signed(sample, 24))
                    .map_err(|e| format!("Write error: {e}"))?;
            }
        }
        _ => return Err("Invalid offline WAV sample format".to_string()),
    }
    Ok(())
}

/// Convenience: render a file with no plugins (passthrough / format conversion).
pub fn render_passthrough(
    input_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    bits_per_sample: u16,
) -> Result<(), String> {
    let config = OfflineRenderConfig {
        source: AudioSource::File(input_path.as_ref().to_path_buf()),
        output_path: output_path.as_ref().to_path_buf(),
        format: OutputFormat::Wav { bits_per_sample },
        plugins: Vec::new(),
        frame_size: 1024,
        output_sample_rate: None,
    };
    render_offline(&config, None)
}

/// Render a timeline to a WAV file at maximum CPU speed.
pub fn render_timeline(
    timeline: &mut crate::timeline::Timeline,
    output_path: impl AsRef<Path>,
    format: &OutputFormat,
    mut on_progress: Option<&mut dyn FnMut(&RenderProgress)>,
) -> Result<(), String> {
    let sample_rate = timeline.transport.sample_rate;
    let channels = timeline.output_channels;
    let frame_size = timeline.frame_size;
    let duration = timeline.duration_samples();
    let mut latency_remaining = timeline.output_latency_samples();
    let wav_spec = wav_spec(format, channels, sample_rate)?;
    let mut writer = WavWriter::create(output_path.as_ref(), wav_spec)
        .map_err(|e| format!("Failed to create output: {e}"))?;
    let mut dither = TpdfDither::new(OFFLINE_DITHER_SEED);
    let render_state = TimelineRenderStateGuard::new(timeline);
    let mut output = vec![0.0f32; frame_size * channels];
    let mut frames_processed = 0u64;

    while frames_processed < duration {
        let produced = render_state.timeline.process(&mut output)?;
        if produced == 0 {
            return Err("Timeline renderer made no progress".to_string());
        }
        let skipped = produced.min(latency_remaining);
        latency_remaining -= skipped;
        let remaining = usize::try_from(duration - frames_processed).unwrap_or(usize::MAX);
        let written = produced.saturating_sub(skipped).min(remaining);
        let start = skipped * channels;
        let end = start + written * channels;
        write_wav_samples(&mut writer, wav_spec, &output[start..end], &mut dither)?;
        frames_processed += written as u64;
        if let Some(ref mut callback) = on_progress {
            callback(&RenderProgress {
                frames_processed,
                total_frames: Some(duration),
            });
        }
    }

    writer
        .finalize()
        .map_err(|e| format!("Finalize error: {e}"))
}
