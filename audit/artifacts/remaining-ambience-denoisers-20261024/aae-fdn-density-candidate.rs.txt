//! 8-line Feedback Delay Network with time-variant modulation.
//!
//! Core of the LARES-inspired reverb. Implements:
//! - 8 delay lines with mutually-prime lengths
//! - Hadamard mixing matrix (energy-preserving, O(N log N))
//! - Per-line tone correction filters (frequency-dependent RT60)
//! - Time-variant delay modulation (Griesinger's key innovation)
//! - Safety limiter in the feedback path

#![allow(
    clippy::needless_range_loop,
    reason = "fixed-size DSP loops over FDN_SIZE; the index is also used to access multiple parallel arrays"
)]

use crate::delay_line::DelayLine;
use crate::hadamard::hadamard8;
use crate::tone_filter::{ToneCoefficients, ToneFilter};

#[path = "fdn_lfo.rs"]
mod lfo;
use lfo::Modulator;

/// Number of delay lines in the FDN.
pub const FDN_SIZE: usize = 8;

/// Base delay lengths in samples at 48 kHz, room_size=1.0.
/// All prime to maximize echo density and minimize repetition patterns.
const BASE_DELAYS_48K: [usize; FDN_SIZE] = [1553, 1907, 2311, 2719, 3187, 3557, 4001, 4507];
const MAX_ROOM_SIZE: f32 = 3.0;

// A late field needs repeated circulation before its fastest endpoint decays.
// Require one longest-loop circulation per FDN line within that endpoint T60:
// eight traversals per T60, hence four traversals across a T30 fit interval.
// This density policy is independent of the early-reflection room geometry.
const MIN_TRAVERSALS_PER_DECAY: f64 = FDN_SIZE as f64;

fn select_late_delays(
    sample_rate: f32,
    room_size: f32,
    decay: (f32, f32, f32),
) -> [usize; FDN_SIZE] {
    let (rt60, bass, treble) = decay;
    let shortest_decay = f64::from(rt60) * f64::from(bass.min(treble));
    // read(m) traverses m+1 clocks. Reserve that clock before scaling taps.
    let maximum_tap = (shortest_decay * f64::from(sample_rate) / MIN_TRAVERSALS_PER_DECAY)
        .floor()
        .max(2.0)
        - 1.0;
    let room_scale =
        f64::from(room_size.clamp(0.2, MAX_ROOM_SIZE)) * f64::from(sample_rate) / 48_000.0;
    let scale = room_scale.min(maximum_tap / BASE_DELAYS_48K[FDN_SIZE - 1] as f64);
    BASE_DELAYS_48K.map(|base| (base as f64 * scale).round().max(1.0) as usize)
}

/// Per-line modulation frequencies in Hz (all different to avoid beating).
const MOD_FREQUENCIES: [f64; FDN_SIZE] = [0.07, 0.11, 0.13, 0.17, 0.19, 0.23, 0.29, 0.31];

/// 8-line Feedback Delay Network.
pub struct Fdn {
    delay_lines: [DelayLine; FDN_SIZE],
    tone_filters: [ToneFilter; FDN_SIZE],
    /// Current delay lengths in samples (scaled by room_size and sample_rate)
    delay_lengths: [usize; FDN_SIZE],
    previous_delay_lengths: [usize; FDN_SIZE],
    /// Latest room target waits for the active two-head fade to finish.
    pending_delay_lengths: Option<[usize; FDN_SIZE]>,
    pending_tone_coefficients: [ToneCoefficients; FDN_SIZE],
    decay_params: (f32, f32, f32),
    /// Latest requested acoustic room; late-loop density may select a shorter grid.
    room_size: f32,
    /// Allpass interpolation states for modulated reads
    interp_states: [f32; FDN_SIZE],
    previous_interp_states: [f32; FDN_SIZE],
    delay_transition_remaining: usize,
    delay_transition_samples: usize,
    /// Prepared quadrature LFOs; no per-sample trigonometry or f32 phase drift.
    modulators: [Modulator; FDN_SIZE],
    /// Modulation depth in samples
    mod_depth_samples: f32,
    /// Safety limiter threshold (linear)
    limiter_threshold: f32,
    /// Input gain distribution (equal for all lines)
    input_gain: f32,
    sample_rate: f32,
}

impl Fdn {
    /// Create a new FDN.
    ///
    /// - `sample_rate`: audio sample rate
    /// - `room_size`: scales delay lengths (0.2–3.0, default 1.0)
    /// - `rt60`: mid-frequency reverberation time in seconds
    /// - `bass_ratio`: RT60_bass / RT60_mid (e.g., 1.2)
    /// - `treble_ratio`: RT60_treble / RT60_mid (e.g., 0.5)
    /// - `mod_depth`: modulation depth 0.0–1.0 (maps to 0–8 samples)
    /// - `safety_limit_db`: limiter threshold below 0 dBFS
    pub fn new(
        sample_rate: impl Into<f64>,
        room_size: f32,
        rt60: f32,
        bass_ratio: f32,
        treble_ratio: f32,
        mod_depth: f32,
        safety_limit_db: f32,
    ) -> Self {
        let sr = sample_rate.into() as f32;
        let room_size = room_size.clamp(0.2, MAX_ROOM_SIZE);
        let delay_lengths = select_late_delays(sr, room_size, (rt60, bass_ratio, treble_ratio));

        // Allocate for the maximum supported room size so room_size changes can
        // update delay lengths without reallocating on the audio/control path.
        let max_scale = MAX_ROOM_SIZE * sr / 48000.0;
        let max_delay = BASE_DELAYS_48K
            .iter()
            .map(|delay| (*delay as f32 * max_scale).ceil() as usize)
            .max()
            .unwrap_or(1);
        let alloc_size = max_delay + 16;
        let delay_lines = std::array::from_fn(|_| DelayLine::new(alloc_size));

        // Compute per-line tone correction filters
        let rt60_bass = rt60 * bass_ratio;
        let rt60_treble = rt60 * treble_ratio;
        let tone_filters = std::array::from_fn(|i| {
            // read(m) is the (m+1)th previous push. Reads precede this
            // sample's push, so attenuation must cover all m+1 loop clocks.
            let m = delay_lengths[i] as f32 + 1.0;
            let g_dc = 10.0_f32.powf(-3.0 * m / (rt60_bass * sr));
            let g_ny = 10.0_f32.powf(-3.0 * m / (rt60_treble * sr));
            ToneFilter::new_for_loop(g_dc, g_ny, m)
        });

        let modulators = std::array::from_fn(|i| {
            Modulator::new(
                MOD_FREQUENCIES[i],
                f64::from(sr),
                i as f64 / FDN_SIZE as f64,
            )
        });

        let mod_depth_samples = mod_depth * 8.0;
        let limiter_threshold = 10.0_f32.powf(safety_limit_db / 20.0);
        let input_gain = 1.0 / (FDN_SIZE as f32).sqrt();

        Self {
            delay_lines,
            tone_filters,
            delay_lengths,
            previous_delay_lengths: delay_lengths,
            pending_delay_lengths: None,
            pending_tone_coefficients: [ToneCoefficients::SILENCE; FDN_SIZE],
            decay_params: (rt60, bass_ratio, treble_ratio),
            room_size,
            interp_states: [0.0; FDN_SIZE],
            previous_interp_states: [0.0; FDN_SIZE],
            delay_transition_remaining: 0,
            delay_transition_samples: (sr * 0.01).round().max(1.0) as usize,
            modulators,
            mod_depth_samples,
            limiter_threshold,
            input_gain,
            sample_rate: sr,
        }
    }

    /// Process one input sample, producing 8 decorrelated output taps.
    ///
    /// Returns an array of 8 output samples, one per delay line.
    /// These are distributed across speakers by the plugin's routing stage.
    #[inline]
    pub fn process(&mut self, input: f32) -> [f32; FDN_SIZE] {
        let scaled_input = input * self.input_gain;

        // Constant-sum interpolation keeps correlated taps from gaining energy
        // inside the feedback loop. Equal-power weights sum to sqrt(2) at the
        // midpoint and can amplify even a change that quantizes to equal delays.
        let progress =
            1.0 - self.delay_transition_remaining as f32 / self.delay_transition_samples as f32;
        let delay_blend = progress * progress * (3.0 - 2.0 * progress);

        // Read outputs from all delay lines
        let mut outputs = [0.0_f32; FDN_SIZE];
        for i in 0..FDN_SIZE {
            let base_delay = self.delay_lengths[i] as f32;

            if self.mod_depth_samples > 0.01 {
                // Time-variant: modulated delay with allpass interpolation
                let mod_offset = self.modulators[i].next() * self.mod_depth_samples;
                let effective_delay = (base_delay + mod_offset).max(1.0);
                let current =
                    self.delay_lines[i].read_allpass(effective_delay, &mut self.interp_states[i]);
                outputs[i] = if self.delay_transition_remaining > 0 {
                    let previous_delay = self.previous_delay_lengths[i] as f32 + mod_offset;
                    let previous = self.delay_lines[i]
                        .read_allpass(previous_delay.max(1.0), &mut self.previous_interp_states[i]);
                    previous + delay_blend * (current - previous)
                } else {
                    current
                };
            } else {
                // Static delay — integer read
                let current = self.delay_lines[i].read(self.delay_lengths[i]);
                outputs[i] = if self.delay_transition_remaining > 0 {
                    let previous = self.delay_lines[i].read(self.previous_delay_lengths[i]);
                    previous + delay_blend * (current - previous)
                } else {
                    current
                };
            }
        }
        // Apply tone correction (frequency-dependent decay)
        for i in 0..FDN_SIZE {
            outputs[i] = self.tone_filters[i].process(outputs[i]);
        }

        // Hadamard mixing (energy-preserving feedback matrix)
        let mut feedback = outputs;
        hadamard8(&mut feedback);

        // Safety limiter + write back into delay lines
        for i in 0..FDN_SIZE {
            let fb = feedback[i] + scaled_input;
            let limited = soft_clip(fb, self.limiter_threshold);
            self.delay_lines[i].push(limited);
        }

        self.delay_transition_remaining = self.delay_transition_remaining.saturating_sub(1);
        if self.delay_transition_remaining == 0
            && let Some(target) = self.pending_delay_lengths.take()
        {
            self.start_delay_transition(target);
            // Coefficients were prepared by setters; admission needs no powers,
            // allocation, or additional delay-line read heads.
            for i in 0..FDN_SIZE {
                let coefficients = self.pending_tone_coefficients[i];
                self.tone_filters[i].set_coefficients_smooth(
                    coefficients,
                    (self.sample_rate * 0.005).round().max(1.0) as usize,
                );
            }
        }
        outputs
    }

    /// Update decay and its bounded late-field density geometry without allocation.
    pub fn set_rt60(&mut self, rt60: f32, bass_ratio: f32, treble_ratio: f32) {
        self.request_geometry_and_decay((rt60, bass_ratio, treble_ratio), false);
    }

    fn request_geometry_and_decay(&mut self, decay: (f32, f32, f32), room_request: bool) {
        let decay_changed = self.decay_params != decay;
        self.decay_params = decay;
        let target = select_late_delays(self.sample_rate, self.room_size, decay);
        let mut geometry_started = false;
        if self.delay_transition_remaining > 0 {
            // Geometry and damping share the same latest requested endpoint.
            // Never interrupt the active pair of read heads.
            self.pending_delay_lengths = (target != self.delay_lengths).then_some(target);
        } else if room_request || target != self.delay_lengths {
            self.start_delay_transition(target);
            geometry_started = true;
        }
        if decay_changed || geometry_started {
            let coefficients = self.decay_coefficients(self.delay_lengths);
            for (filter, coefficients) in self.tone_filters.iter_mut().zip(coefficients) {
                filter.set_coefficients_smooth(
                    coefficients,
                    (self.sample_rate * 0.005).round().max(1.0) as usize,
                );
            }
        }
        if let Some(target) = self.pending_delay_lengths {
            self.pending_tone_coefficients = self.decay_coefficients(target);
        }
    }

    fn decay_coefficients(&self, delays: [usize; FDN_SIZE]) -> [ToneCoefficients; FDN_SIZE] {
        let (rt60, bass, treble) = self.decay_params;
        let bass_rt60 = rt60 * bass;
        let treble_rt60 = rt60 * treble;
        std::array::from_fn(|i| {
            // Include the read-before-write clock, as in construction/reset.
            let delay = delays[i] as f32 + 1.0;
            let dc = 10.0_f32.powf(-3.0 * delay / (bass_rt60 * self.sample_rate));
            let ny = 10.0_f32.powf(-3.0 * delay / (treble_rt60 * self.sample_rate));
            ToneFilter::coefficients_for_loop(dc, ny, delay)
        })
    }

    /// Update acoustic room and density-limited late delays without allocation.
    ///
    /// The early-reflection room model is independent. Late-loop traversal is
    /// bounded by the shortest endpoint decay, so short tails do not consist
    /// of only a few widely separated echoes. Room and RT60 requests coalesce
    /// in one pending endpoint; an active two-head fade always finishes first.
    pub fn set_room_size(&mut self, room_size: f32, rt60: f32, bass_ratio: f32, treble_ratio: f32) {
        self.room_size = room_size.clamp(0.2, MAX_ROOM_SIZE);
        self.request_geometry_and_decay((rt60, bass_ratio, treble_ratio), true);
    }

    fn start_delay_transition(&mut self, target: [usize; FDN_SIZE]) {
        self.previous_delay_lengths = self.delay_lengths;
        self.previous_interp_states = self.interp_states;
        self.delay_lengths = target;
        self.delay_transition_remaining = self.delay_transition_samples;
    }

    pub fn delay_transition_active(&self) -> bool {
        self.delay_transition_remaining > 0
    }

    /// Update modulation depth (0.0–1.0).
    pub fn set_mod_depth(&mut self, depth: f32) {
        self.mod_depth_samples = depth * 8.0;
    }

    /// Update safety limiter threshold.
    pub fn set_safety_limit_db(&mut self, db: f32) {
        self.limiter_threshold = 10.0_f32.powf(db / 20.0);
    }

    /// Reset all internal state (delay lines, filters, LFO phases).
    pub fn reset(&mut self) {
        // Reset installs the requested endpoint, including a coalesced room,
        // rather than leaving old room geometry or coefficient ramps behind.
        if let Some(target) = self.pending_delay_lengths.take() {
            self.delay_lengths = target;
        }
        let (rt60, bass, treble) = self.decay_params;
        for i in 0..FDN_SIZE {
            let delay = self.delay_lengths[i] as f32 + 1.0;
            let dc = 10.0_f32.powf(-3.0 * delay / (rt60 * bass * self.sample_rate));
            let ny = 10.0_f32.powf(-3.0 * delay / (rt60 * treble * self.sample_rate));
            self.tone_filters[i] = ToneFilter::new_for_loop(dc, ny, delay);
        }
        for dl in &mut self.delay_lines {
            dl.reset();
        }
        for tf in &mut self.tone_filters {
            tf.reset();
        }
        self.interp_states = [0.0; FDN_SIZE];
        self.previous_interp_states = [0.0; FDN_SIZE];
        self.previous_delay_lengths = self.delay_lengths;
        self.delay_transition_remaining = 0;
        for (i, oscillator) in self.modulators.iter_mut().enumerate() {
            oscillator.reset(i as f64 / FDN_SIZE as f64);
        }
    }
}

/// Limit feedback continuously while preserving low-level unity gain.
///
/// The knee begins at half the ceiling (about 6 dB below it). Its value and
/// slope match the identity there, and its asymptote is the existing ceiling.
/// Switching directly from identity to tanh at the ceiling drops the output
/// by about 24%, injecting a discontinuity into the recursive network.
#[inline]
fn soft_clip(x: f32, threshold: f32) -> f32 {
    let knee = threshold * 0.5;
    if x.abs() <= knee {
        x
    } else {
        (knee + knee * ((x.abs() - knee) / knee).tanh()).copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fdn_impulse_decays() {
        let mut fdn = Fdn::new(48000, 1.0, 1.0, 1.0, 1.0, 0.0, 6.0);

        // Feed a single impulse
        let _ = fdn.process(1.0);

        // Process enough samples for ~1.5× RT60 (1.5 seconds at 48kHz)
        let mut max_energy = 0.0_f32;
        for _ in 0..72000 {
            let outputs = fdn.process(0.0);
            let energy: f32 = outputs.iter().map(|v| v * v).sum();
            if energy > max_energy {
                max_energy = energy;
            }
        }

        // After 1.5 RT60, signal should have decayed significantly
        let mut late_energy = 0.0_f32;
        for _ in 0..1000 {
            let outputs = fdn.process(0.0);
            late_energy += outputs.iter().map(|v| v * v).sum::<f32>();
        }
        late_energy /= 1000.0;

        assert!(
            late_energy < max_energy * 0.01,
            "Signal should decay: max_energy={max_energy}, late_avg={late_energy}"
        );
    }

    #[test]
    fn test_fdn_no_blowup() {
        let mut fdn = Fdn::new(48000, 1.0, 2.0, 1.5, 0.5, 0.5, 6.0);

        // Process 5 seconds of white-ish noise
        let mut max_sample = 0.0_f32;
        for i in 0..240000 {
            let input = ((i as f32 * 0.1234).sin() * 0.5).sin() * 0.3;
            let outputs = fdn.process(input);
            for &v in &outputs {
                if v.abs() > max_sample {
                    max_sample = v.abs();
                }
            }
        }

        assert!(
            max_sample < 2.0,
            "FDN should not blow up, max_sample={max_sample}"
        );
        assert!(max_sample.is_finite(), "Outputs must be finite");
    }

    #[test]
    fn test_fdn_decorrelation() {
        let mut fdn = Fdn::new(48000, 1.0, 1.5, 1.2, 0.5, 0.3, 6.0);

        // Feed impulse
        let _ = fdn.process(1.0);

        // Measure normalized cross-correlation of two output lines after the
        // first echo arrivals, rather than comparing only their sums.
        let mut cross = 0.0_f64;
        let mut energy_a = 0.0_f64;
        let mut energy_b = 0.0_f64;
        for _ in 0..48_000 {
            let outputs = fdn.process(0.0);
            let a = f64::from(outputs[0]);
            let b = f64::from(outputs[7]);
            cross += a * b;
            energy_a += a * a;
            energy_b += b * b;
        }
        let correlation = cross / (energy_a * energy_b).sqrt().max(1e-30);
        assert!(
            correlation.abs() < 0.5,
            "FDN lines should be decorrelated, correlation={correlation}"
        );
    }

    #[test]
    fn test_fdn_reset() {
        let mut fdn = Fdn::new(48000, 1.0, 1.5, 1.2, 0.5, 0.3, 6.0);
        let _ = fdn.process(1.0);
        for _ in 0..1000 {
            fdn.process(0.0);
        }

        fdn.reset();

        // After reset, processing silence should produce silence
        for _ in 0..100 {
            let outputs = fdn.process(0.0);
            for &v in &outputs {
                assert!(v.abs() < 1e-10, "After reset, output should be silent");
            }
        }
    }

    #[test]
    fn test_set_room_size_preserves_state() {
        let mut fdn = Fdn::new(48000, 1.0, 1.5, 1.2, 0.8, 0.4, 6.0);

        // Build up some feedback energy.
        for i in 0..20000 {
            let input = ((i as f32 * 0.123).sin() * 0.3).sin() * 0.5;
            fdn.process(input);
        }

        let before: Vec<f32> = (0..2000).flat_map(|_| fdn.process(0.0)).collect();
        let before_energy: f32 = before.iter().map(|v| v * v).sum();
        assert!(before_energy > 1e-8);

        fdn.set_room_size(2.5, 1.5, 1.2, 0.8);

        let after: Vec<f32> = (0..2000).flat_map(|_| fdn.process(0.0)).collect();
        let after_energy: f32 = after.iter().map(|v| v * v).sum();

        // Room-size updates should not truncate feedback energy to silence.
        assert!(
            after_energy > before_energy * 0.1,
            "FDN state should persist across room-size changes: before={before_energy}, after={after_energy}"
        );
    }

    #[test]
    fn limiter_is_continuous_and_monotone_through_its_ceiling() {
        for threshold in [0.25_f32, 1.0, 2.0, 16.0] {
            let below = soft_clip(threshold * (1.0 - 1e-5), threshold);
            let above = soft_clip(threshold * (1.0 + 1e-5), threshold);
            println!(
                "limiter threshold={threshold} threshold_step={}",
                (above - below) / threshold
            );
            assert!(
                above >= below,
                "increasing excitation must not reduce feedback"
            );
            assert!((above - below).abs() < threshold * 3e-5);
            let mut previous = -threshold;
            for index in -16_384..=16_384 {
                let input = index as f32 * threshold / 1024.0;
                let output = soft_clip(input, threshold);
                assert!(output.is_finite() && output.abs() <= threshold);
                assert!(output >= previous);
                assert_eq!(output, -soft_clip(-input, threshold));
                if input.abs() <= threshold * 0.5 {
                    assert_eq!(output, input);
                }
                previous = output;
            }
        }
    }

    #[test]
    fn limiter_knee_has_matching_unit_slope() {
        for threshold in [0.25_f32, 1.0, 2.0, 16.0] {
            let knee = threshold * 0.5;
            let step = threshold * 0.0005;
            let center = soft_clip(knee, threshold);
            let left = (center - soft_clip(knee - step, threshold)) / step;
            let right = (soft_clip(knee + step, threshold) - center) / step;
            assert!((left - 1.0).abs() < 0.001);
            assert!((right - 1.0).abs() < 0.001);
            let saturation_slope = (soft_clip(16.0 * threshold, threshold)
                - soft_clip(15.0 * threshold, threshold))
                / threshold;
            assert!(saturation_slope.abs() < 1e-5);
        }
    }

    #[test]
    fn test_soft_clip() {
        assert!((soft_clip(0.5, 1.0) - 0.5).abs() < 1e-6);
        assert!((soft_clip(-0.5, 1.0) - (-0.5)).abs() < 1e-6);
        // Above threshold, should be compressed
        let clipped = soft_clip(2.0, 1.0);
        assert!(clipped < 2.0 && clipped > 0.9);
        // Symmetric
        assert!((soft_clip(2.0, 1.0) + soft_clip(-2.0, 1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_safety_limit_is_headroom_above_nominal() {
        let fdn = Fdn::new(48_000, 1.0, 1.8, 1.2, 0.5, 0.0, 6.0);
        let expected = 10.0_f32.powf(6.0 / 20.0);
        assert!((fdn.limiter_threshold - expected).abs() < 1e-6);
        assert_eq!(soft_clip(1.0, fdn.limiter_threshold), 1.0);
        assert!(soft_clip(4.0, fdn.limiter_threshold).abs() < 4.0);
    }
}

#[cfg(test)]
mod density_tests {
    use super::{FDN_SIZE, Fdn};

    #[test]
    fn every_prepared_loop_obeys_the_declared_endpoint_density() {
        let mut cases = 0;
        for rate in [44_100_u32, 48_000, 96_000, 192_000] {
            for room in [0.2_f32, 0.7, 1.0, 2.3, 3.0] {
                for rt60 in [0.3_f32, 1.2, 6.0] {
                    for (bass, treble) in [(1.0_f32, 1.0_f32), (0.8, 0.2), (2.0, 0.5)] {
                        let fdn = Fdn::new(rate, room, rt60, bass, treble, 0.0, 0.0);
                        let limit =
                            (f64::from(rt60) * f64::from(bass.min(treble)) * f64::from(rate)
                                / FDN_SIZE as f64)
                                .floor() as usize;
                        assert!(fdn.delay_lengths.iter().all(|tap| *tap < limit));
                        assert!(fdn.delay_lengths.windows(2).all(|pair| pair[0] < pair[1]));
                        assert!(
                            fdn.delay_lengths
                                .iter()
                                .zip(&fdn.delay_lines)
                                .all(|(tap, line)| *tap <= line.max_delay_samples())
                        );
                        cases += 1;
                    }
                }
            }
        }
        println!("fdn_density_geometry cases={cases} endpoint_traversals=8");
    }
}
