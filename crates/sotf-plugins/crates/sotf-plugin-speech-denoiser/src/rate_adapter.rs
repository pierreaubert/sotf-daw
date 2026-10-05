//! Prepared streaming rate conversion around the fixed 48 kHz RNNoise model.

use plugins_denoiser::rnnoise::RnnoiseBackend;
use sotf_host::fractional_delay::fractional_delay_coefficients;
use sotf_host::plugin::{Plugin, PluginResult, ProcessContext};
use sotf_plugin_resampler::ResamplerPlugin;

const MODEL_RATE: f64 = 48_000.0;
const HOST_CHUNK: usize = 64;
const FRACTIONAL_TAPS: usize = 65;
// ResamplerPlugin::new() selects Medium's 128-tap sinc on both stages.
const RESAMPLER_SINC_SUPPORT: usize = 128;
// A host may request any finite positive clock, but preparation must not let
// an extreme ratio ask rubato for unbounded memory before our fallible scratch
// reservations run. This is a resource limit on prepared buffers, not a rate
// quantization rule; all supported validator clocks fit well below it.
const MAX_PREPARED_AUDIO_BYTES: f64 = (256 * 1024 * 1024) as f64;
// Each Medium sinc table is 128 taps × 256 phases × f32. The cutoff bank has
// at most 19 tables per stage, so both stages use under 5 MiB of coefficients.
const PREPARED_SINC_TABLE_BYTES: f64 = (8 * 1024 * 1024) as f64;

fn prepared_samples(count: usize) -> PluginResult<Vec<f32>> {
    let mut samples = Vec::new();
    samples
        .try_reserve_exact(count)
        .map_err(|_| "speech adapter cannot reserve prepared audio buffers")?;
    samples.resize(count, 0.0);
    Ok(samples)
}

struct FrameRing {
    samples: Vec<f32>,
    channels: usize,
    read: usize,
    len: usize,
}

impl FrameRing {
    fn new(capacity: usize, channels: usize, initial_zeros: usize) -> PluginResult<Self> {
        let count = capacity
            .checked_mul(channels)
            .filter(|&count| count <= isize::MAX as usize / size_of::<f32>())
            .ok_or("speech adapter ring capacity overflow")?;
        if initial_zeros > capacity || capacity == 0 {
            return Err("speech adapter ring has invalid capacity".into());
        }
        Ok(Self {
            samples: prepared_samples(count)?,
            channels,
            read: 0,
            len: initial_zeros,
        })
    }

    fn capacity(&self) -> usize {
        self.samples.len() / self.channels
    }

    fn push(&mut self, frame: &[f32]) -> PluginResult<()> {
        if self.len == self.capacity() {
            return Err("speech adapter output ring overflow".into());
        }
        let index = (self.read + self.len) % self.capacity();
        self.samples[index * self.channels..(index + 1) * self.channels].copy_from_slice(frame);
        self.len += 1;
        Ok(())
    }

    fn pop(&mut self, output: &mut [f32]) -> PluginResult<()> {
        if self.len == 0 {
            return Err("speech adapter output ring underflow".into());
        }
        output.copy_from_slice(
            &self.samples[self.read * self.channels..(self.read + 1) * self.channels],
        );
        self.read = (self.read + 1) % self.capacity();
        self.len -= 1;
        Ok(())
    }

    fn reset(&mut self, initial_zeros: usize) {
        self.samples.fill(0.0);
        self.read = 0;
        self.len = initial_zeros;
    }
}

struct WetFractionalDelay {
    coefficients: [f32; FRACTIONAL_TAPS],
    history: Vec<f32>,
    channels: usize,
    write: usize,
}

impl WetFractionalDelay {
    fn new(fraction: f64, channels: usize) -> PluginResult<Self> {
        let count = FRACTIONAL_TAPS
            .checked_mul(channels)
            .ok_or("speech fractional-delay capacity overflow")?;
        Ok(Self {
            coefficients: fractional_delay_coefficients(fraction),
            history: prepared_samples(count)?,
            channels,
            write: 0,
        })
    }

    fn process(&mut self, input: &[f32], output: &mut [f32]) {
        for channel in 0..self.channels {
            self.history[self.write * self.channels + channel] = input[channel];
            let mut sum = 0.0;
            for (tap, coefficient) in self.coefficients.iter().enumerate() {
                let slot = (self.write + FRACTIONAL_TAPS - tap) % FRACTIONAL_TAPS;
                sum += self.history[slot * self.channels + channel] * coefficient;
            }
            output[channel] = sum;
        }
        self.write = (self.write + 1) % FRACTIONAL_TAPS;
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.write = 0;
    }
}

/// Host-clock adapter; all buffers and filter coefficients are allocated at setup.
pub(super) struct RateAdapter {
    host_rate: f64,
    channels: usize,
    input: ResamplerPlugin,
    output: ResamplerPlugin,
    model: Vec<f32>,
    converted: Vec<f32>,
    wet_frame: [f32; 2],
    fractional: WetFractionalDelay,
    wet: FrameRing,
    dry: FrameRing,
    prefill: usize,
    latency: usize,
    drain_frames: usize,
}

impl RateAdapter {
    pub(super) fn new(host_rate: f64, channels: usize) -> PluginResult<Self> {
        if !host_rate.is_finite() || host_rate <= 0.0 || !(1..=2).contains(&channels) {
            return Err("speech adapter needs a finite positive rate and mono/stereo".into());
        }
        // Rubato prepares a maximum relative ratio of 2. Reject only ratios
        // whose single prepared chunk cannot fit in an addressable buffer.
        // This check precedes backend construction and its internal allocation.
        let model_ratio = MODEL_RATE / host_rate;
        let host_ratio = host_rate / MODEL_RATE;
        let maximum_chunk_frames = model_ratio.max(host_ratio) * (HOST_CHUNK as f64 * 2.0);
        let addressable_frames = isize::MAX as f64 / (channels * size_of::<f32>()) as f64;
        if !maximum_chunk_frames.is_finite() || maximum_chunk_frames >= addressable_frames {
            return Err("speech adapter rate needs an unaddressable prepared chunk".into());
        }
        // Count several internal backend blocks, both channel buffers, the
        // delayed host dry path, and the wet FIFO before constructing rubato.
        // Final capacities are still checked and fallibly reserved below.
        let prepared_bytes = PREPARED_SINC_TABLE_BYTES
            + (maximum_chunk_frames * 8.0 + host_ratio * 1_200.0 * 4.0 + 4_096.0)
                * (channels * size_of::<f32>()) as f64;
        if !prepared_bytes.is_finite() || prepared_bytes > MAX_PREPARED_AUDIO_BYTES {
            return Err("speech adapter prepared audio exceeds memory budget".into());
        }
        let mut input = ResamplerPlugin::new(channels, host_rate, MODEL_RATE, HOST_CHUNK)?;
        let mut output = ResamplerPlugin::new(channels, MODEL_RATE, host_rate, HOST_CHUNK)?;
        input.initialize(host_rate)?;
        output.initialize(MODEL_RATE)?;
        let model_frames = input
            .output_frames_envelope(HOST_CHUNK)
            .ok_or("speech adapter input envelope unavailable")?;
        let converted_frames = output
            .output_frames_envelope(model_frames)
            .ok_or("speech adapter output envelope unavailable")?;
        let ratio = host_rate / MODEL_RATE;
        let delay = (input.signal_delay_samples() + 960.0) * ratio + output.signal_delay_samples();
        if !delay.is_finite() || delay < 0.0 {
            return Err("speech adapter has invalid signal delay".into());
        }
        let whole_delay = delay.ceil();
        let chunk_wait = (HOST_CHUNK as f64 * ratio).ceil();
        if whole_delay >= usize::MAX as f64 || chunk_wait >= usize::MAX as f64 {
            return Err("speech adapter latency exceeds addressable frames".into());
        }
        let prefill = HOST_CHUNK
            .checked_add(chunk_wait as usize)
            .and_then(|frames| frames.checked_add(HOST_CHUNK))
            .ok_or("speech adapter prefill overflow")?;
        let latency = prefill
            .checked_add(whole_delay as usize)
            .and_then(|frames| frames.checked_add(32))
            .ok_or("speech adapter latency overflow")?;
        // Continue silence past the declared signal latency by the complete
        // finite support of both sinc stages and the host-side delay filter.
        // RNNoise's recursive tail is still reported as Unknown by the plugin.
        let model_tail = (RESAMPLER_SINC_SUPPORT as f64 * ratio).ceil();
        if !model_tail.is_finite() || model_tail >= usize::MAX as f64 {
            return Err("speech adapter filter tail overflow".into());
        }
        let drain_frames = latency
            .checked_add(RESAMPLER_SINC_SUPPORT)
            .and_then(|frames| frames.checked_add(model_tail as usize))
            .and_then(|frames| frames.checked_add(FRACTIONAL_TAPS))
            .and_then(|frames| frames.checked_add(HOST_CHUNK))
            .ok_or("speech adapter drain horizon overflow")?;
        let wet_capacity = prefill
            .checked_add(converted_frames)
            .and_then(|frames| frames.checked_add(HOST_CHUNK))
            .ok_or("speech adapter output capacity overflow")?;
        let model_len = model_frames
            .checked_mul(channels)
            .ok_or("speech adapter model capacity overflow")?;
        let converted_len = converted_frames
            .checked_mul(channels)
            .ok_or("speech adapter converted capacity overflow")?;
        if model_len > isize::MAX as usize / size_of::<f32>()
            || converted_len > isize::MAX as usize / size_of::<f32>()
        {
            return Err("speech adapter scratch exceeds addressable memory".into());
        }
        Ok(Self {
            host_rate,
            channels,
            input,
            output,
            model: prepared_samples(model_len)?,
            converted: prepared_samples(converted_len)?,
            wet_frame: [0.0; 2],
            fractional: WetFractionalDelay::new(whole_delay - delay, channels)?,
            wet: FrameRing::new(wet_capacity, channels, prefill)?,
            dry: FrameRing::new(latency, channels, latency)?,
            prefill,
            latency,
            drain_frames,
        })
    }

    pub(super) fn latency(&self) -> usize {
        self.latency
    }

    pub(super) fn drain_frames(&self) -> usize {
        self.drain_frames
    }

    pub(super) fn reset(&mut self) {
        self.input.reset();
        self.output.reset();
        self.fractional.reset();
        self.wet.reset(self.prefill);
        self.dry.reset(self.latency);
        self.model.fill(0.0);
        self.converted.fill(0.0);
    }

    /// Convert at most 64 host frames and deliver exactly as many host frames.
    pub(super) fn process_chunk(
        &mut self,
        buffer: &mut [f32],
        dry_output: &mut [f32],
        backend: &mut RnnoiseBackend,
        bypass: bool,
    ) -> PluginResult<()> {
        let frames = buffer.len() / self.channels;
        if frames > HOST_CHUNK
            || frames * self.channels != buffer.len()
            || dry_output.len() != buffer.len()
        {
            return Err("speech adapter chunk shape is invalid".into());
        }
        let mut sanitized = [0.0_f32; HOST_CHUNK * 2];
        for (index, sample) in buffer.iter().enumerate() {
            sanitized[index] = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        for frame in 0..frames {
            let range = frame * self.channels..(frame + 1) * self.channels;
            self.dry.pop(&mut dry_output[range.clone()])?;
            self.dry.push(&sanitized[range])?;
        }
        let model_frames = self.input.process(
            &sanitized[..buffer.len()],
            &mut self.model,
            &ProcessContext::new(self.host_rate, frames),
        )?;
        if model_frames > 0 {
            if backend.process(
                &mut self.model[..model_frames * self.channels],
                model_frames,
                self.channels,
                bypass,
            ) != model_frames
            {
                return Err("RNNoise did not accept all converted frames".into());
            }
            let converted_frames = self.output.process(
                &self.model[..model_frames * self.channels],
                &mut self.converted,
                &ProcessContext::new(MODEL_RATE, model_frames),
            )?;
            for frame in 0..converted_frames {
                let range = frame * self.channels..(frame + 1) * self.channels;
                self.fractional
                    .process(&self.converted[range], &mut self.wet_frame[..self.channels]);
                self.wet.push(&self.wet_frame[..self.channels])?;
            }
        }
        for frame in 0..frames {
            let range = frame * self.channels..(frame + 1) * self.channels;
            self.wet.pop(&mut buffer[range])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypassed_model_impulse_exposes_aligned_latency_and_complete_finite_filter_tail() {
        for rate in [
            8_000.0,
            22_050.0,
            44_100.0,
            88_200.0,
            96_000.0,
            192_000.0,
            384_000.0,
            768_000.0,
            1_234.567_8,
            12_345.678,
            45_678.901,
            123_456.78,
        ] {
            let mut adapter = RateAdapter::new(rate, 1).unwrap();
            let mut backend = RnnoiseBackend::new();
            backend
                .initialize_with_model(
                    48_000,
                    1,
                    crate::SpeechDenoiserModel::default().backend_id(),
                )
                .unwrap();
            let expected_peak = adapter.latency() + 17;
            let frames = adapter.drain_frames() + 256;
            let mut output = Vec::with_capacity(frames);
            for offset in (0..frames).step_by(HOST_CHUNK) {
                let count = (frames - offset).min(HOST_CHUNK);
                let mut input = [0.0_f32; HOST_CHUNK];
                let mut delayed_dry = [0.0_f32; HOST_CHUNK];
                if (offset..offset + count).contains(&17) {
                    input[17 - offset] = 1.0;
                }
                adapter
                    .process_chunk(
                        &mut input[..count],
                        &mut delayed_dry[..count],
                        &mut backend,
                        true,
                    )
                    .unwrap();
                output.extend_from_slice(&input[..count]);
            }
            let actual_peak = output
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            assert!(
                actual_peak.0.abs_diff(expected_peak) <= 4,
                "{rate} Hz wet impulse peaked at {}, expected near {expected_peak}",
                actual_peak.0
            );
            assert!(
                output[adapter.drain_frames()..]
                    .iter()
                    .all(|sample| sample.abs() < 1.0e-4),
                "{rate} Hz finite converter/filter support outlived the declared drain"
            );
        }
    }

    #[test]
    fn bypassed_wet_low_band_is_phase_aligned_with_full_band_dry() {
        for rate in [44_100.0, 96_000.0] {
            let mut adapter = RateAdapter::new(rate, 1).unwrap();
            let mut backend = RnnoiseBackend::new();
            backend
                .initialize_with_model(
                    48_000,
                    1,
                    crate::SpeechDenoiserModel::default().backend_id(),
                )
                .unwrap();
            let frames = adapter.latency() + 4096;
            let mut wet = Vec::with_capacity(frames);
            let mut dry = Vec::with_capacity(frames);
            for offset in (0..frames).step_by(HOST_CHUNK) {
                let count = (frames - offset).min(HOST_CHUNK);
                let mut input = [0.0_f32; HOST_CHUNK];
                let mut delayed_dry = [0.0_f32; HOST_CHUNK];
                for (index, sample) in input[..count].iter_mut().enumerate() {
                    let phase = (offset + index) as f64 * 250.0 * std::f64::consts::TAU / rate;
                    *sample = (0.5 * phase.sin()) as f32;
                }
                adapter
                    .process_chunk(
                        &mut input[..count],
                        &mut delayed_dry[..count],
                        &mut backend,
                        true,
                    )
                    .unwrap();
                wet.extend_from_slice(&input[..count]);
                dry.extend_from_slice(&delayed_dry[..count]);
            }
            let start = adapter.latency() + 512;
            let end = frames - 4;
            let energy: f64 = dry[start..end]
                .iter()
                .map(|&sample| f64::from(sample).powi(2))
                .sum();
            assert!(energy > 100.0, "{rate} Hz dry oracle is silent");
            let correlation = |lag: isize| -> f64 {
                (start..end)
                    .map(|index| {
                        f64::from(dry[index])
                            * f64::from(wet[index.checked_add_signed(lag).unwrap()])
                    })
                    .sum::<f64>()
            };
            let centered = correlation(0);
            for lag in [-3, -2, -1, 1, 2, 3] {
                assert!(
                    centered >= correlation(lag),
                    "{rate} Hz wet/dry peak shifted by {lag} frames"
                );
            }
        }
    }
}
