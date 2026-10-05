//! Frozen HEAD-baseline identity tests for the shared hiss backends.
//!
//! Compares current default (engaged) renders bit-exactly against a frozen
//! transcription of the pre-change HEAD algorithm. See `mod legacy_head`
//! for provenance.

// Rust guideline compliant 2026-02-21
use plugins_denoiser::hiss::HissReducer;
use plugins_denoiser::spectral_hiss::{SPECTRAL_HISS_FFT_SIZE, SpectralHissReducer};

/// Frozen transcription of the pre-change HEAD denoiser algorithms.
///
/// Provenance: `audit/muse-parallel-2026-10-01/restoration-backend/`
/// `legacy-head-reference/{spectral_hiss.rs.txt,hiss.rs.txt}` at HEAD
/// commit `aa0a3d1f59304cc2b5ee84dd080c840611e5cbf7` (sha256 `d154d297…`
/// spectral, `02b2586b…` time-domain; full hashes in `provenance.json`).
/// Only the DSP bodies are transcribed (legacy unit tests omitted) with
/// the two reducer types renamed to prevent confusion with production
/// types. Method bodies match the retained HEAD text exactly, including
/// the pre-fix spectral `set_params` (no non-finite guards). One
/// read-only test accessor (`high_band_noise`) is added because HEAD
/// stages those levels write-only; it changes no behavior.
///
/// Limitation: this is a HEAD baseline, not a snapshot of the dirty
/// pre-Muse worktree (`provenance.json`). Identity claims cover
/// HEAD-vs-current only; an unknown dirty-tree delta is out of scope.
mod legacy_head {
    use math_audio_dsp::stft::{RealFftProcessor, generate_hann_window};

    pub const LEGACY_FFT_SIZE: usize = 1024;
    const HOP_SIZE: usize = LEGACY_FFT_SIZE / 4;
    const NUM_BINS: usize = LEGACY_FFT_SIZE / 2 + 1;
    const MIN_HISTORY_SLOTS: usize = 8;
    const MIN_HISTORY_SECONDS: f32 = 0.512;
    const GAIN_ATTACK_SECONDS: f32 = 0.015;
    const GAIN_RELEASE_SECONDS: f32 = 0.050;
    const BYPASS_SECONDS: f32 = 0.005;

    /// Higher-latency stationary-hiss reducer using WOLA and a bounded
    /// minimum-statistics noise estimate.
    struct BypassFade {
        mix: f32,
        target: f32,
        step: f32,
    }

    pub struct LegacySpectralHiss {
        channels: usize,
        sample_rate: u32,
        cutoff_hz: f32,
        threshold_linear: f32,
        strength: f32,
        bypass: BypassFade,
        hops_per_slot: usize,
        gain_attack: f32,
        gain_release: f32,
        fft: Vec<RealFftProcessor>,
        window: Vec<f32>,
        input: Vec<Vec<f32>>,
        input_write: usize,
        input_fill: usize,
        output: Vec<f32>,
        output_mask: usize,
        output_read: usize,
        output_write: usize,
        output_fill: usize,
        latency_fill: usize,
        dry_delay: Vec<f32>,
        dry_pos: usize,
        power: Vec<Vec<f32>>,
        smoothed_power: Vec<Vec<f32>>,
        smoothed_gain: Vec<Vec<f32>>,
        high_band_noise: Vec<f32>,
        current_min: Vec<Vec<f32>>,
        minimum_history: Vec<Vec<f32>>,
        history_slot: usize,
        hops_in_slot: usize,
    }

    impl LegacySpectralHiss {
        pub fn new(channels: usize) -> Self {
            let output_frames = (LEGACY_FFT_SIZE * 4).next_power_of_two();
            Self {
                channels,
                sample_rate: 48_000,
                cutoff_hz: 4_000.0,
                threshold_linear: 10.0_f32.powf(-30.0 / 20.0),
                strength: 0.5,
                bypass: BypassFade {
                    mix: 1.0,
                    target: 1.0,
                    step: 1.0,
                },
                hops_per_slot: 12,
                gain_attack: 0.35,
                gain_release: 0.9,
                fft: (0..channels)
                    .map(|_| RealFftProcessor::new_bidirectional(LEGACY_FFT_SIZE))
                    .collect(),
                window: generate_hann_window(LEGACY_FFT_SIZE),
                input: vec![vec![0.0; LEGACY_FFT_SIZE]; channels],
                input_write: 0,
                // Prime the causal analysis window with zero-valued history. The
                // first hop is therefore available after HOP_SIZE input frames.
                input_fill: LEGACY_FFT_SIZE - HOP_SIZE,
                output: vec![0.0; output_frames * channels],
                output_mask: output_frames - 1,
                output_read: 0,
                output_write: 0,
                output_fill: 0,
                latency_fill: 0,
                dry_delay: vec![0.0; LEGACY_FFT_SIZE * channels],
                dry_pos: 0,
                power: vec![vec![0.0; NUM_BINS]; channels],
                smoothed_power: vec![vec![0.0; NUM_BINS]; channels],
                smoothed_gain: vec![vec![1.0; NUM_BINS]; channels],
                high_band_noise: vec![0.0; channels],
                current_min: vec![vec![f32::INFINITY; NUM_BINS]; channels],
                minimum_history: vec![vec![f32::INFINITY; MIN_HISTORY_SLOTS * NUM_BINS]; channels],
                history_slot: 0,
                hops_in_slot: 0,
            }
        }

        pub fn initialize(&mut self, sample_rate: u32) -> Result<(), String> {
            if sample_rate == 0 {
                return Err("sample rate must be nonzero".into());
            }
            self.sample_rate = sample_rate;
            self.hops_per_slot = ((MIN_HISTORY_SECONDS * sample_rate as f32
                / (MIN_HISTORY_SLOTS * HOP_SIZE) as f32)
                .round() as usize)
                .max(1);
            let hop_seconds = HOP_SIZE as f32 / sample_rate as f32;
            self.gain_attack = (-hop_seconds / GAIN_ATTACK_SECONDS).exp();
            self.gain_release = (-hop_seconds / GAIN_RELEASE_SECONDS).exp();
            self.bypass.step = 1.0 / (BYPASS_SECONDS * sample_rate as f32).max(1.0);
            self.reset();
            Ok(())
        }

        pub fn set_params(&mut self, cutoff_hz: f32, threshold_db: f32, strength: f32) {
            self.cutoff_hz = cutoff_hz.clamp(20.0, self.sample_rate as f32 * 0.45);
            self.threshold_linear = 10.0_f32.powf(threshold_db.clamp(-120.0, 0.0) / 20.0);
            self.strength = strength.clamp(0.0, 1.0);
        }

        pub fn set_enabled(&mut self, enabled: bool) {
            self.bypass.target = if enabled { 1.0 } else { 0.0 };
        }

        pub const fn latency_samples(&self) -> usize {
            LEGACY_FFT_SIZE
        }

        /// Reads the staged live high-band levels (test accessor only;
        /// not part of the HEAD API — HEAD stages these levels
        /// write-only and gates on a local instead).
        pub fn high_band_noise(&self) -> &[f32] {
            &self.high_band_noise
        }

        pub fn reset(&mut self) {
            for channel in &mut self.input {
                channel.fill(0.0);
            }
            self.input_write = 0;
            self.input_fill = LEGACY_FFT_SIZE - HOP_SIZE;
            self.output.fill(0.0);
            self.output_read = 0;
            self.output_write = 0;
            self.output_fill = 0;
            self.latency_fill = 0;
            self.dry_delay.fill(0.0);
            self.dry_pos = 0;
            for channel in &mut self.power {
                channel.fill(0.0);
            }
            for channel in &mut self.smoothed_power {
                channel.fill(0.0);
            }
            for channel in &mut self.smoothed_gain {
                channel.fill(1.0);
            }
            for channel in &mut self.current_min {
                channel.fill(f32::INFINITY);
            }
            for channel in &mut self.minimum_history {
                channel.fill(f32::INFINITY);
            }
            self.history_slot = 0;
            self.hops_in_slot = 0;
            self.bypass.mix = self.bypass.target;
            self.high_band_noise.fill(0.0);
        }

        fn process_hop(&mut self) {
            let scale = 1.0 / (LEGACY_FFT_SIZE as f32 * 1.5);
            let cutoff_bin = ((self.cutoff_hz * LEGACY_FFT_SIZE as f32 / self.sample_rate as f32)
                .ceil() as usize)
                .min(NUM_BINS - 1);
            for ch in 0..self.channels {
                for i in 0..LEGACY_FFT_SIZE {
                    let source = (self.input_write + i) & (LEGACY_FFT_SIZE - 1);
                    self.fft[ch].time_buffer[i] = self.input[ch][source] * self.window[i];
                }
                self.fft[ch].forward();
                // First pass updates every bin before tonality is classified, so
                // protection cannot depend on FFT scan order.
                for bin in 0..NUM_BINS {
                    let value = self.fft[ch].freq_buffer[bin];
                    let power = value.re * value.re + value.im * value.im;
                    self.power[ch][bin] = power;
                    let previous = self.smoothed_power[ch][bin];
                    let smoothed = if previous == 0.0 {
                        power
                    } else {
                        0.8 * previous + 0.2 * power
                    };
                    self.smoothed_power[ch][bin] = smoothed;
                    self.current_min[ch][bin] = self.current_min[ch][bin].min(smoothed);
                }

                let mut aggregate_noise = 0.0;
                for bin in cutoff_bin..NUM_BINS {
                    let mut noise = self.current_min[ch][bin];
                    for slot in 0..MIN_HISTORY_SLOTS {
                        noise = noise.min(self.minimum_history[ch][slot * NUM_BINS + bin]);
                    }
                    if noise.is_finite() {
                        aggregate_noise += noise;
                    }
                }
                // Parseval with an unnormalised real FFT: doubled one-sided bin
                // energy is N² * mean(x² w²); periodic Hann mean(w²)=3/8.
                let window_energy = 0.375;
                let noise_rms = (aggregate_noise / window_energy).sqrt() * (2.0_f32).sqrt()
                    / LEGACY_FFT_SIZE as f32;
                self.high_band_noise[ch] = noise_rms;

                // Second pass applies classification and always advances the gain
                // smoother, including threshold/strength release back to unity.
                for bin in cutoff_bin..NUM_BINS {
                    let power = self.power[ch][bin];
                    let mut noise = self.current_min[ch][bin];
                    for slot in 0..MIN_HISTORY_SLOTS {
                        noise = noise.min(self.minimum_history[ch][slot * NUM_BINS + bin]);
                    }

                    let mut target_gain = 1.0;
                    if self.strength > 0.0
                        && noise.is_finite()
                        && noise > 0.0
                        && noise_rms <= self.threshold_linear
                    {
                        let wiener = (1.0 - noise / power.max(1.0e-20)).clamp(0.0, 1.0);
                        let floor = 1.0 - self.strength;
                        // Persistent narrowband peaks are wanted programme,
                        // not broadband hiss. Preserve bins whose local peak
                        // is strongly tonal relative to their neighbours.
                        // Preserve the full three-bin Hann main lobe around a
                        // local tonal peak. A bin-centred sinusoid has each
                        // adjacent-bin power at one quarter of the peak.
                        let mut tonal = false;
                        for candidate in bin.saturating_sub(1)..=(bin + 1).min(NUM_BINS - 1) {
                            if candidate > 0 && candidate + 1 < NUM_BINS {
                                let centre = self.smoothed_power[ch][candidate];
                                let neighbour = 0.5
                                    * (self.smoothed_power[ch][candidate - 1]
                                        + self.smoothed_power[ch][candidate + 1]);
                                tonal |= centre > 3.0 * neighbour.max(1.0e-20);
                            }
                        }
                        target_gain = if tonal {
                            1.0
                        } else {
                            floor + (1.0 - floor) * wiener.sqrt()
                        };
                        // Slow release avoids isolated high-gain time/frequency
                        // holes (the usual source of musical-noise chirps),
                        // while attenuation can engage promptly.
                    }
                    let previous_gain = self.smoothed_gain[ch][bin];
                    let coefficient = if target_gain < previous_gain {
                        self.gain_attack
                    } else {
                        self.gain_release
                    };
                    let gain = coefficient * previous_gain + (1.0 - coefficient) * target_gain;
                    self.smoothed_gain[ch][bin] = gain;
                    self.fft[ch].freq_buffer[bin] *= gain;
                }
                self.fft[ch].inverse();
                for i in 0..LEGACY_FFT_SIZE {
                    let frame = (self.output_write + i) & self.output_mask;
                    self.output[frame * self.channels + ch] +=
                        self.fft[ch].time_buffer[i] * self.window[i] * scale;
                }
            }
            self.output_write = (self.output_write + HOP_SIZE) & self.output_mask;
            self.output_fill += HOP_SIZE;
            self.hops_in_slot += 1;
            if self.hops_in_slot == self.hops_per_slot {
                for ch in 0..self.channels {
                    let start = self.history_slot * NUM_BINS;
                    self.minimum_history[ch][start..start + NUM_BINS]
                        .copy_from_slice(&self.current_min[ch]);
                    self.current_min[ch].fill(f32::INFINITY);
                }
                self.history_slot = (self.history_slot + 1) % MIN_HISTORY_SLOTS;
                self.hops_in_slot = 0;
            }
        }

        pub fn process(&mut self, buffer: &mut [f32]) {
            if self.channels == 0 {
                return;
            }
            let frames = buffer.len() / self.channels;
            debug_assert_eq!(buffer.len(), frames * self.channels);

            for frame in 0..frames {
                let base = frame * self.channels;
                // Capture every input sample before writing any output. This is
                // essential for callbacks larger than the FFT: analysis/control
                // timing must not depend on the host's partitioning.
                for ch in 0..self.channels {
                    let sample = buffer[base + ch];
                    self.input[ch][self.input_write] =
                        if sample.is_finite() { sample } else { 0.0 };
                }
                self.input_write = (self.input_write + 1) & (LEGACY_FFT_SIZE - 1);
                self.input_fill += 1;
                if self.input_fill == LEGACY_FFT_SIZE {
                    self.process_hop();
                    self.input_fill = LEGACY_FFT_SIZE - HOP_SIZE;
                }

                let startup = self.latency_fill < HOP_SIZE;
                debug_assert!(startup || self.output_fill > 0);
                for ch in 0..self.channels {
                    let dry_index = self.dry_pos + ch;
                    let dry = self.dry_delay[dry_index];
                    self.dry_delay[dry_index] = self.input[ch]
                        [(self.input_write + LEGACY_FFT_SIZE - 1) & (LEGACY_FFT_SIZE - 1)];
                    let wet = if startup {
                        0.0
                    } else {
                        self.output[self.output_read * self.channels + ch]
                    };
                    buffer[base + ch] = dry + (wet - dry) * self.bypass.mix;
                    if !startup {
                        self.output[self.output_read * self.channels + ch] = 0.0;
                    }
                }
                self.dry_pos += self.channels;
                if self.dry_pos == self.dry_delay.len() {
                    self.dry_pos = 0;
                }
                if startup {
                    self.latency_fill += 1;
                } else {
                    self.output_read = (self.output_read + 1) & self.output_mask;
                    self.output_fill -= 1;
                }
                if self.bypass.mix < self.bypass.target {
                    self.bypass.mix = (self.bypass.mix + self.bypass.step).min(self.bypass.target);
                } else if self.bypass.mix > self.bypass.target {
                    self.bypass.mix = (self.bypass.mix - self.bypass.step).max(self.bypass.target);
                }
            }
        }
    }

    /// Focused high-frequency stationary noise reducer.
    ///
    /// This is intentionally simpler than the full STFT denoiser: it splits each
    /// channel into a low-passed body and a high-frequency residual, tracks the
    /// residual power at fast and slow time scales, and attenuates only persistent,
    /// low-level residual energy. It is a zero-latency high-band downward expander,
    /// not a spectral noise estimator.
    pub struct LegacyHissExpander {
        channels: usize,
        sample_rate: u32,
        cutoff_hz: f32,
        threshold_db: f32,
        strength: f32,
        lowpass_state: Vec<f32>,
        fast_env: Vec<f32>,
        noise_env: Vec<f32>,
        alpha: f32,
        target_alpha: f32,
        alpha_smoothing_coeff: f32,
        threshold_power: f32,
        fast_coeff: f32,
        slow_coeff: f32,
        gain_attack_coeff: f32,
        gain_release_coeff: f32,
        gain: Vec<f32>,
        reducing: Vec<bool>,
        candidate_samples: Vec<u32>,
        hold_remaining: Vec<u32>,
        persistence_samples: u32,
        hold_samples: u32,
        wet_mix: f32,
        target_wet_mix: f32,
        wet_mix_coeff: f32,
    }

    impl LegacyHissExpander {
        pub fn new(channels: usize) -> Self {
            let mut reducer = Self {
                channels,
                sample_rate: 48000,
                cutoff_hz: 4000.0,
                threshold_db: -30.0,
                strength: 0.5,
                lowpass_state: vec![0.0; channels],
                fast_env: vec![0.0; channels],
                noise_env: vec![0.0; channels],
                alpha: 0.0,
                target_alpha: 0.0,
                alpha_smoothing_coeff: 0.0,
                threshold_power: 0.0,
                fast_coeff: 0.0,
                slow_coeff: 0.0,
                gain_attack_coeff: 0.0,
                gain_release_coeff: 0.0,
                gain: vec![1.0; channels],
                reducing: vec![false; channels],
                candidate_samples: vec![0; channels],
                hold_remaining: vec![0; channels],
                persistence_samples: 0,
                hold_samples: 0,
                wet_mix: 1.0,
                target_wet_mix: 1.0,
                wet_mix_coeff: 0.0,
            };
            reducer.update_coefficients(true);
            reducer
        }

        pub fn initialize(&mut self, sample_rate: u32) -> Result<(), String> {
            if sample_rate == 0 {
                return Err("sample rate must be nonzero".to_string());
            }
            self.sample_rate = sample_rate;
            self.update_coefficients(true);
            Ok(())
        }

        pub fn set_params(&mut self, cutoff_hz: f32, threshold_db: f32, strength: f32) {
            self.cutoff_hz = if cutoff_hz.is_finite() {
                cutoff_hz.max(20.0)
            } else {
                4_000.0
            };
            self.threshold_db = if threshold_db.is_finite() {
                threshold_db
            } else {
                -30.0
            };
            self.strength = if strength.is_finite() {
                strength.clamp(0.0, 1.0)
            } else {
                0.5
            };
            self.update_coefficients(false);
        }

        /// Set the live bypass target. `immediate` is reserved for initialization
        /// and reset; automation uses the smoothed transition.
        pub fn set_enabled(&mut self, enabled: bool, immediate: bool) {
            self.target_wet_mix = if enabled { 1.0 } else { 0.0 };
            if immediate {
                self.wet_mix = self.target_wet_mix;
            }
        }

        pub fn reset(&mut self) {
            self.lowpass_state.fill(0.0);
            self.fast_env.fill(0.0);
            self.noise_env.fill(0.0);
            self.gain.fill(1.0);
            self.reducing.fill(false);
            self.candidate_samples.fill(0);
            self.hold_remaining.fill(0);
            self.wet_mix = self.target_wet_mix;
        }

        /// Algorithmic latency in samples.
        ///
        /// HissReducer is a sample-by-sample first-order IIR lowpass with an
        /// envelope follower. It has no lookahead, FFT buffering, or block
        /// processing, so its latency is zero.
        pub fn latency_samples(&self) -> usize {
            0
        }

        pub fn process(&mut self, buffer: &mut [f32]) {
            if self.channels == 0 {
                return;
            }

            for frame in buffer.chunks_mut(self.channels) {
                self.alpha = self.target_alpha
                    + self.alpha_smoothing_coeff * (self.alpha - self.target_alpha);
                if (self.alpha - self.target_alpha).abs() < 1e-8 {
                    self.alpha = self.target_alpha;
                }
                self.wet_mix =
                    self.target_wet_mix + self.wet_mix_coeff * (self.wet_mix - self.target_wet_mix);
                if (self.wet_mix - self.target_wet_mix).abs() < 1e-8 {
                    self.wet_mix = self.target_wet_mix;
                }
                for (ch, sample) in frame.iter_mut().enumerate() {
                    let dry = if sample.is_finite() { *sample } else { 0.0 };
                    let low = self.alpha * dry + (1.0 - self.alpha) * self.lowpass_state[ch];
                    self.lowpass_state[ch] = low;

                    let high = dry - low;
                    let high_power = high * high;
                    self.fast_env[ch] =
                        self.fast_coeff * self.fast_env[ch] + (1.0 - self.fast_coeff) * high_power;
                    self.noise_env[ch] =
                        self.slow_coeff * self.noise_env[ch] + (1.0 - self.slow_coeff) * high_power;
                    if self.lowpass_state[ch].abs() < 1e-20 {
                        self.lowpass_state[ch] = 0.0;
                    }
                    if self.fast_env[ch].abs() < 1e-20 {
                        self.fast_env[ch] = 0.0;
                    }
                    if self.noise_env[ch].abs() < 1e-20 {
                        self.noise_env[ch] = 0.0;
                    }

                    // Fast/slow power ratio rejects attacks and zero-crossing
                    // modulation. Hysteresis plus a short hold prevents chatter.
                    let noise_power = self.noise_env[ch];
                    let power_ratio = self.fast_env[ch] / noise_power.max(1e-20);
                    let persistent = (0.5..=2.0).contains(&power_ratio);
                    let enter_level = noise_power < self.threshold_power;
                    let exit_level = noise_power < self.threshold_power * 2.0;
                    if self.reducing[ch] {
                        if exit_level && power_ratio <= 4.0 {
                            self.hold_remaining[ch] = self.hold_samples;
                        } else if self.hold_remaining[ch] > 0 {
                            self.hold_remaining[ch] -= 1;
                        } else {
                            self.reducing[ch] = false;
                        }
                    } else if noise_power > 1e-12 && enter_level && persistent {
                        self.candidate_samples[ch] = self.candidate_samples[ch].saturating_add(1);
                        if self.candidate_samples[ch] >= self.persistence_samples {
                            self.reducing[ch] = true;
                            self.hold_remaining[ch] = self.hold_samples;
                            self.candidate_samples[ch] = 0;
                        }
                    } else {
                        self.candidate_samples[ch] = 0;
                    }

                    let level_depth =
                        (1.0 - noise_power / self.threshold_power.max(1e-20)).clamp(0.0, 1.0);
                    let steady_depth = (1.0 - (power_ratio - 1.0).abs()).clamp(0.0, 1.0);
                    let reduction_depth = if self.reducing[ch] {
                        level_depth * steady_depth
                    } else {
                        0.0
                    };
                    let target_gain = 1.0 - self.strength * reduction_depth;
                    let coeff = if target_gain < self.gain[ch] {
                        self.gain_attack_coeff
                    } else {
                        self.gain_release_coeff
                    };
                    self.gain[ch] = target_gain + coeff * (self.gain[ch] - target_gain);
                    if (self.gain[ch] - target_gain).abs() < 1e-8 {
                        self.gain[ch] = target_gain;
                    }
                    let processed = if self.gain[ch] == 1.0 {
                        dry
                    } else {
                        low + high * self.gain[ch]
                    };
                    *sample = dry + self.wet_mix * (processed - dry);
                }
            }
        }

        fn update_coefficients(&mut self, snap_cutoff: bool) {
            let sr = self.sample_rate.max(1) as f32;
            let cutoff = self.cutoff_hz.min(sr * 0.45).max(20.0);
            self.target_alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / sr).exp();
            if snap_cutoff {
                self.alpha = self.target_alpha;
            }
            self.alpha_smoothing_coeff = (-1.0 / (0.005 * sr)).exp();
            self.threshold_power = 10.0_f32.powf(self.threshold_db / 10.0);
            self.fast_coeff = (-1.0 / (0.005 * sr)).exp();
            self.slow_coeff = (-1.0 / (0.100 * sr)).exp();
            self.gain_attack_coeff = (-1.0 / (0.001 * sr)).exp();
            self.gain_release_coeff = (-1.0 / (0.050 * sr)).exp();
            self.persistence_samples = (0.030 * sr).round() as u32;
            self.hold_samples = (0.020 * sr).round() as u32;
            self.wet_mix_coeff = (-1.0 / (0.005 * sr)).exp();
        }
    }
}

use legacy_head::{LegacyHissExpander, LegacySpectralHiss};

const SR: u32 = 48_000;
const RATE: f64 = 48_000.0;
const LATENCY: usize = SPECTRAL_HISS_FFT_SIZE;
/// Exact-bin measurement tone: bin 213 of the reducer FFT (N=1024).
const TONE_HZ: f64 = 213.0 * 48_000.0 / 1024.0;
const PARTITIONS: [usize; 5] = [1, 64, 511, 73, 997];

fn lcg(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// High-passed stationary hiss, same construction family as the
/// profile/curve/link contract tests.
fn spectral_hiss_fixture(frames: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    let mut previous = 0.0f32;
    (0..frames)
        .map(|_| {
            let white = lcg(&mut state);
            let high_pass = 0.035 * (white - previous);
            previous = white;
            high_pass
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (amplitude as f64 * (2.0 * std::f64::consts::PI * freq_hz * i as f64 / RATE).sin())
                as f32
        })
        .collect()
}

fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    assert_eq!(left.len(), right.len());
    let mut out = Vec::with_capacity(left.len() * 2);
    for pair in left.iter().zip(right.iter()) {
        out.push(*pair.0);
        out.push(*pair.1);
    }
    out
}

fn render_process(process: &mut dyn FnMut(&mut [f32]), channels: usize, input: &[f32]) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut part = 0;
    while offset < input.len() {
        let count = (PARTITIONS[part % PARTITIONS.len()] * channels).min(input.len() - offset);
        let mut block = input[offset..offset + count].to_vec();
        process(&mut block);
        output.extend(block);
        offset += count;
        part += 1;
    }
    output
}

fn mean_power(signal: &[f32]) -> f64 {
    signal
        .iter()
        .map(|s| {
            let d = f64::from(*s);
            d * d
        })
        .sum::<f64>()
        / signal.len() as f64
}

fn power_db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// Independent f64 one-pole high-band power oracle.
fn one_pole_high_power(signal: &[f32], cutoff_hz: f64, rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in signal {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    sum / signal.len() as f64
}

#[test]
fn spectral_engaged_defaults_match_head_baseline_bit_exactly() {
    assert_eq!(SPECTRAL_HISS_FFT_SIZE, 1024);
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let tone = sine_tone(frames, 0.06, TONE_HZ);
    let mixed: Vec<f32> = hiss.iter().zip(tone.iter()).map(|(h, t)| h + t).collect();

    for channels in [1, 2] {
        let input = if channels == 1 {
            mixed.clone()
        } else {
            let right_hiss = spectral_hiss_fixture(frames, 0x51ab_0002);
            let right: Vec<f32> = right_hiss
                .iter()
                .zip(tone.iter())
                .map(|(h, t)| h + t)
                .collect();
            interleave(&mixed, &right)
        };
        let mut current = SpectralHissReducer::new(channels);
        current.initialize(SR).unwrap();
        current.set_params(4_000.0, -30.0, 0.65);
        current.set_enabled(true);
        let mut legacy = LegacySpectralHiss::new(channels);
        legacy.initialize(SR).unwrap();
        legacy.set_params(4_000.0, -30.0, 0.65);
        legacy.set_enabled(true);
        assert_eq!(current.latency_samples(), legacy.latency_samples());
        assert_eq!(current.latency_samples(), LATENCY);
        // Identical warmup-then-reset on both sides also exercises the
        // reset path inside the comparison.
        let mut warm = spectral_hiss_fixture(4800 * channels, 0x51ab_0003);
        current.process(&mut warm);
        legacy.process(&mut warm);
        current.reset();
        legacy.reset();
        let current_out = render_process(&mut |b| current.process(b), channels, &input);
        let legacy_out = render_process(&mut |b| legacy.process(b), channels, &input);
        assert_eq!(
            current_out, legacy_out,
            "engaged spectral defaults diverged from HEAD with {channels} channel(s)"
        );
        assert!(
            legacy
                .high_band_noise()
                .iter()
                .all(|level| level.is_finite() && *level >= 0.0),
            "legacy estimator levels must stay finite and non-negative"
        );
    }

    // The comparison above is meaningful only while the reducer is
    // actually engaged: hiss-only suppression must clear the proven
    // 2 dB live bound at backend level.
    let mut engaged = SpectralHissReducer::new(1);
    engaged.initialize(SR).unwrap();
    engaged.set_params(4_000.0, -30.0, 0.65);
    let output = render_process(&mut |b| engaged.process(b), 1, &hiss);
    let start = SR as usize / 2 + LATENCY;
    let suppression = power_db(mean_power(&output[start..]) / mean_power(&hiss[start - LATENCY..]));
    assert!(
        suppression < -2.0,
        "baseline fixture did not engage: {suppression:.2} dB"
    );
}

#[test]
fn time_domain_engaged_defaults_match_head_baseline_bit_exactly() {
    let frames = SR as usize * 2;
    let mut state = 0x7e5f_0001u32;
    let hiss: Vec<f32> = (0..frames).map(|_| 0.02 * lcg(&mut state)).collect();

    for channels in [1, 2] {
        let input = if channels == 1 {
            hiss.clone()
        } else {
            interleave(&hiss, &sine_tone(frames, 0.5, 750.0))
        };
        let mut current = HissReducer::new(channels);
        current.initialize(SR).unwrap();
        current.set_params(4_000.0, -30.0, 0.8);
        current.set_enabled(true, true);
        let mut legacy = LegacyHissExpander::new(channels);
        legacy.initialize(SR).unwrap();
        legacy.set_params(4_000.0, -30.0, 0.8);
        legacy.set_enabled(true, true);
        assert_eq!(current.latency_samples(), legacy.latency_samples());
        assert_eq!(current.latency_samples(), 0);
        let mut warm: Vec<f32> = (0..4800 * channels)
            .map(|i| if i % 2 == 0 { 0.05 } else { -0.05 })
            .collect();
        current.process(&mut warm);
        legacy.process(&mut warm);
        current.reset();
        legacy.reset();
        let current_out = render_process(&mut |b| current.process(b), channels, &input);
        let legacy_out = render_process(&mut |b| legacy.process(b), channels, &input);
        assert_eq!(
            current_out, legacy_out,
            "engaged time-domain defaults diverged from HEAD with {channels} channel(s)"
        );
    }

    // Engagement proof for the compared fixture: last-second high-band
    // suppression must clear 2 dB.
    let mut engaged = HissReducer::new(1);
    engaged.initialize(SR).unwrap();
    engaged.set_params(4_000.0, -30.0, 0.8);
    let output = render_process(&mut |b| engaged.process(b), 1, &hiss);
    let steady = SR as usize;
    let suppression = power_db(
        one_pole_high_power(&output[steady..], 4_000.0, RATE)
            / one_pole_high_power(&hiss[steady..], 4_000.0, RATE),
    );
    assert!(
        suppression < -2.0,
        "baseline fixture did not engage: {suppression:.2} dB"
    );
}

#[test]
fn spectral_transient_defaults_match_head_baseline_bit_exactly() {
    // Guard-off transient behavior is frozen legacy audio: the impulse
    // fixture renders bit-identically against HEAD. Known limitation (r6
    // measured -4.81 dB live peak loss on the sibling contract fixture):
    // engaged defaults co-attenuate transients riding on quiet hiss. That
    // loss is NOT blessed here — identity only proves no silent change;
    // the enhanced opt-in path with the hard < 3 dB bound lives in
    // profile_curve_link::spectral_profile_preserves_engaged_transient_peaks.
    let frames = SR as usize * 2;
    let hiss = spectral_hiss_fixture(frames, 0x51ab_0001);
    let mut impulses = vec![0.0f32; frames];
    let mut k = SR as usize;
    while k < frames {
        impulses[k] = 1.0;
        k += 4800;
    }
    let input: Vec<f32> = hiss
        .iter()
        .zip(impulses.iter())
        .map(|(h, t)| h + t)
        .collect();

    let mut current = SpectralHissReducer::new(1);
    current.initialize(SR).unwrap();
    current.set_params(4_000.0, -30.0, 0.85);
    current.set_enabled(true);
    let mut legacy = LegacySpectralHiss::new(1);
    legacy.initialize(SR).unwrap();
    legacy.set_params(4_000.0, -30.0, 0.85);
    legacy.set_enabled(true);
    let current_out = render_process(&mut |b| current.process(b), 1, &input);
    let legacy_out = render_process(&mut |b| legacy.process(b), 1, &input);
    assert_eq!(
        current_out, legacy_out,
        "guard-off transient defaults diverged from HEAD"
    );
}
