// ============================================================================
// Auto Gain Compensation
// ============================================================================

mod meter;
use meter::GainMeter;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AutoGainLoudnessType {
    #[default]
    Momentary,
    ShortTerm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoGainParams {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub loudness_type: AutoGainLoudnessType,
    #[serde(default = "default_max_gain_db")]
    pub max_gain_db: f32,
    #[serde(default = "default_smoothing_ms")]
    pub smoothing_ms: f32,
}

fn default_enabled() -> bool {
    false
}
fn default_max_gain_db() -> f32 {
    6.0
}
fn default_smoothing_ms() -> f32 {
    100.0
}

impl Default for AutoGainParams {
    fn default() -> Self {
        Self {
            enabled: false,
            loudness_type: AutoGainLoudnessType::Momentary,
            max_gain_db: 6.0,
            smoothing_ms: 100.0,
        }
    }
}

pub struct AutoGain {
    num_channels: usize,
    sample_rate: u32,
    input_monitor: GainMeter,
    output_monitor: GainMeter,
    gain_db: f64,
    target_gain_db: f64,
    smoothing_coeff: f64,
    current_gain_linear: f64,
    last_input_lufs: f64,
    last_output_lufs: f64,
    last_input_peak: f64,
    last_output_peak: f64,
    enabled: bool,
    loudness_type: AutoGainLoudnessType,
    max_gain_db: f32,
    smoothing_ms: f32,
    /// Optional absolute loudness target. When unset, AutoGain matches the
    /// measured output to the measured input as before.
    target_lufs: Option<f32>,
    /// Fast attack coefficient (~20ms) for gain decreases (output too loud)
    attack_coeff: f64,
    /// Slow release coefficient (~300ms) for gain increases (output recovered)
    release_coeff: f64,
    /// Conversion of the current smoothed dB value, reused when it is unchanged.
    cached_gain_db: f64,
    cached_gain_linear: f64,
}

fn smoothing_coefficient(milliseconds: f32, sample_rate: u32) -> f64 {
    if milliseconds <= 0.0 || sample_rate == 0 {
        0.0
    } else {
        (-1.0 / (f64::from(milliseconds) * 0.001 * f64::from(sample_rate))).exp()
    }
}

impl std::fmt::Debug for AutoGain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let current_db = if self.current_gain_linear > 1e-10 {
            20.0 * self.current_gain_linear.log10()
        } else {
            -200.0
        };
        f.debug_struct("AutoGain")
            .field("enabled", &self.enabled)
            .field("gain_db", &current_db)
            .finish_non_exhaustive()
    }
}

impl AutoGain {
    pub fn new(
        num_channels: usize,
        sample_rate: u32,
        params: AutoGainParams,
    ) -> Result<Self, String> {
        Ok(Self {
            num_channels,
            sample_rate,
            input_monitor: GainMeter::new(num_channels as u32, sample_rate)?,
            output_monitor: GainMeter::new(num_channels as u32, sample_rate)?,
            gain_db: 0.0,
            target_gain_db: 0.0,
            smoothing_coeff: smoothing_coefficient(params.smoothing_ms, sample_rate),
            current_gain_linear: 1.0,
            last_input_lufs: f64::NEG_INFINITY,
            last_output_lufs: f64::NEG_INFINITY,
            last_input_peak: 0.0,
            last_output_peak: 0.0,
            enabled: params.enabled,
            loudness_type: params.loudness_type,
            max_gain_db: params.max_gain_db,
            smoothing_ms: params.smoothing_ms,
            target_lufs: None,
            attack_coeff: smoothing_coefficient(20.0, sample_rate),
            release_coeff: smoothing_coefficient(300.0, sample_rate),
            cached_gain_db: 0.0,
            cached_gain_linear: 1.0,
        })
    }

    pub fn new_default(num_channels: usize, sample_rate: u32) -> Result<Self, String> {
        Self::new(num_channels, sample_rate, Default::default())
    }

    pub fn set_sample_rate(&mut self, sr: u32) -> Result<(), String> {
        let input_monitor = GainMeter::new(self.num_channels as u32, sr)?;
        let output_monitor = GainMeter::new(self.num_channels as u32, sr)?;
        self.sample_rate = sr;
        self.input_monitor = input_monitor;
        self.output_monitor = output_monitor;
        self.smoothing_coeff = smoothing_coefficient(self.smoothing_ms, sr);
        self.attack_coeff = smoothing_coefficient(20.0, sr);
        self.release_coeff = smoothing_coefficient(300.0, sr);
        Ok(())
    }

    pub fn reset(&mut self) {
        self.input_monitor.reset();
        self.output_monitor.reset();
        self.gain_db = 0.0;
        self.target_gain_db = 0.0;
        self.current_gain_linear = 1.0;
        self.cached_gain_db = 0.0;
        self.cached_gain_linear = 1.0;
        self.last_input_lufs = f64::NEG_INFINITY;
        self.last_output_lufs = f64::NEG_INFINITY;
        self.last_input_peak = 0.0;
        self.last_output_peak = 0.0;
    }

    pub fn set_enabled(&mut self, e: bool) {
        self.enabled = e;
        if !e {
            self.set_gain_target(0.0);
        }
    }
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
    pub fn set_max_gain_db(&mut self, m: f32) {
        self.max_gain_db = m.abs();
    }
    pub fn set_smoothing_ms(&mut self, s: f32) {
        self.smoothing_ms = s;
        self.smoothing_coeff = smoothing_coefficient(s, self.sample_rate);
    }
    /// Set an optional absolute loudness target. `None` restores the original
    /// input/output matching behavior.
    pub fn set_target_lufs(&mut self, target: Option<f32>) -> Result<(), String> {
        if let Some(value) = target
            && (!value.is_finite() || !(-120.0..=0.0).contains(&value))
        {
            return Err(format!(
                "target LUFS must be finite and in -120..=0, got {value}"
            ));
        }
        self.target_lufs = target;
        Ok(())
    }
    pub fn target_lufs(&self) -> Option<f32> {
        self.target_lufs
    }
    pub fn set_loudness_type(&mut self, t: AutoGainLoudnessType) {
        self.loudness_type = t;
    }
    pub fn loudness_type(&self) -> AutoGainLoudnessType {
        self.loudness_type
    }
    pub fn max_gain_db(&self) -> f32 {
        self.max_gain_db
    }
    pub fn smoothing_ms(&self) -> f32 {
        self.smoothing_ms
    }

    #[inline]
    pub fn is_unity_gain_stable(&self) -> bool {
        !self.enabled
            || (self.current_gain_linear == 1.0
                && self.gain_db == 0.0
                && self.target_gain_db == 0.0)
    }

    pub fn measure_input(&mut self, input: &[f32]) -> Result<(), String> {
        self.ingest_input(input)?;
        self.refresh_input_measurement();
        Ok(())
    }

    /// Advance the persistent input loudness timeline without publishing the
    /// comparatively expensive derived EBU statistics.
    pub fn ingest_input(&mut self, input: &[f32]) -> Result<(), String> {
        self.input_monitor.add_frames(input)
    }

    pub fn refresh_input_measurement(&mut self) {
        (self.last_input_lufs, self.last_input_peak) =
            self.input_monitor.measurement(self.loudness_type);
    }

    pub fn measure_output(&mut self, output: &[f32]) -> Result<(), String> {
        self.ingest_output(output)?;
        self.refresh_output_measurement();
        Ok(())
    }

    /// Advance the persistent output loudness timeline without publishing the
    /// comparatively expensive derived EBU statistics.
    pub fn ingest_output(&mut self, output: &[f32]) -> Result<(), String> {
        self.output_monitor.add_frames(output)
    }

    pub fn refresh_output_measurement(&mut self) {
        (self.last_output_lufs, self.last_output_peak) =
            self.output_monitor.measurement(self.loudness_type);

        if !self.enabled {
            return;
        }

        if self.last_input_lufs.is_finite() && self.last_output_lufs.is_finite() {
            let target = if let Some(target_lufs) = self.target_lufs {
                target_lufs - self.last_output_lufs as f32
            } else {
                self.last_input_lufs as f32 - self.last_output_lufs as f32
            };
            self.set_gain_target(target.clamp(-self.max_gain_db, self.max_gain_db));
        } else {
            // Silence on either side leaves the gain-LUFS difference undefined.
            // Stuck targets here cause unity-gain misbehavior when sound returns
            // at a different level — decay the target back toward 0 dB (unity)
            // so the smoother converges to a safe value during silence.
            self.set_gain_target(0.0);
        }
    }

    fn set_gain_target(&mut self, target: f32) {
        self.target_gain_db = f64::from(target);
        if self.smoothing_coeff == 0.0 {
            self.gain_db = self.target_gain_db;
        }
    }

    /// Advance the configured dB pole and the fixed asymmetric linear pole.
    ///
    /// The flag identifies an exact floating-point fixed point of both states.
    /// It is valid only until the next target or coefficient change.
    #[inline]
    fn advance_gain(&mut self) -> (f32, bool) {
        let previous_db = self.gain_db;
        let previous_gain = self.current_gain_linear;
        // Retain the original strict 1e-5 dB near-target policy. Wider states
        // avoid the sample-rate-dependent stalls of the former f32 recurrence.
        self.gain_db =
            if self.smoothing_coeff == 0.0 || (self.gain_db - self.target_gain_db).abs() < 1e-5 {
                self.target_gain_db
            } else {
                self.target_gain_db + self.smoothing_coeff * (self.gain_db - self.target_gain_db)
            };
        let smoothed_db = self.gain_db;
        if smoothed_db != self.cached_gain_db {
            self.cached_gain_db = smoothed_db;
            self.cached_gain_linear = (smoothed_db * (std::f64::consts::LN_10 / 20.0)).exp();
        }
        let target_linear = self.cached_gain_linear;

        // Asymmetric smoothing in linear domain
        let coeff = if target_linear < self.current_gain_linear {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.current_gain_linear =
            target_linear + coeff * (self.current_gain_linear - target_linear);
        (
            self.current_gain_linear as f32,
            smoothed_db == previous_db && self.current_gain_linear == previous_gain,
        )
    }

    /// Return the gain for the next audio frame.
    ///
    /// Configured smoothing is a dB one-pole time constant, followed by the
    /// existing 20 ms gain-reduction / 300 ms gain-recovery linear pole.
    /// Both states and coefficients use f64; only the returned gain is rounded
    /// to f32. Disabled calls return unity without advancing either state.
    #[inline]
    pub fn next_gain_linear(&mut self) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        self.advance_gain().0
    }

    /// Advance exactly `n` frame gains without applying them to audio.
    ///
    /// This leaves the same state as `n` scalar calls. Zero-length and disabled
    /// calls leave both states unchanged.
    #[inline]
    pub fn next_n(&mut self, n: usize) {
        if !self.enabled {
            return;
        }
        for _ in 0..n {
            if self.advance_gain().1 {
                break;
            }
        }
    }

    pub fn current_gain_db(&self) -> f32 {
        if !self.enabled {
            0.0
        } else if self.current_gain_linear > 1e-10 {
            (20.0 * self.current_gain_linear.log10()) as f32
        } else {
            -200.0
        }
    }
    pub fn last_input_lufs(&self) -> f64 {
        self.last_input_lufs
    }
    pub fn last_output_lufs(&self) -> f64 {
        self.last_output_lufs
    }
    pub fn last_input_peak(&self) -> f64 {
        self.last_input_peak
    }
    pub fn last_output_peak(&self) -> f64 {
        self.last_output_peak
    }

    /// Apply one shared scalar gain per frame to the requested audio prefix.
    ///
    /// This follows exactly the same recurrence as [`Self::next_gain_linear`].
    /// Surplus samples and zero-frame calls are untouched.
    ///
    /// # Panics
    ///
    /// Panics if enabled, nonempty audio does not contain the requested frames.
    pub fn apply_compensation(&mut self, output: &mut [f32], num_frames: usize) {
        if !self.enabled || num_frames == 0 {
            return;
        }
        let samples = num_frames
            .checked_mul(self.num_channels)
            .expect("AutoGain sample count overflow");
        let mut remaining = &mut output[..samples];
        while !remaining.is_empty() {
            let (gain, stationary) = self.advance_gain();
            if stationary {
                // Both states repeated after a real step. With no intervening
                // controls, every remaining frame has this exact same gain.
                crate::simd::scale_add_simd_inplace(remaining, gain);
                return;
            }
            let (frame, rest) = remaining.split_at_mut(self.num_channels);
            for sample in frame {
                *sample *= gain;
            }
            remaining = rest;
        }
    }

    pub fn get_data(&self) -> AutoGainData {
        let current_db = if self.enabled && self.current_gain_linear > 1e-10 {
            20.0 * self.current_gain_linear.log10()
        } else {
            0.0
        };
        AutoGainData {
            enabled: self.enabled,
            gain_db: current_db as f32,
            input_lufs: self.last_input_lufs,
            output_lufs: self.last_output_lufs,
            input_peak: self.last_input_peak,
            output_peak: self.last_output_peak,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoGainData {
    pub enabled: bool,
    pub gain_db: f32,
    pub input_lufs: f64,
    pub output_lufs: f64,
    pub input_peak: f64,
    pub output_peak: f64,
}

impl Default for AutoGainData {
    fn default() -> Self {
        Self {
            enabled: false,
            gain_db: 0.0,
            input_lufs: -120.0,
            output_lufs: -120.0,
            input_peak: 0.0,
            output_peak: 0.0,
        }
    }
}
#[cfg(test)]
mod tests {
    use crate::auto_gain::*;

    #[test]
    fn test_autogain_convergence() {
        let sample_rate = 48000;
        let mut ag = AutoGain::new(
            2,
            sample_rate,
            AutoGainParams {
                enabled: true,
                loudness_type: AutoGainLoudnessType::Momentary,
                max_gain_db: 12.0,
                smoothing_ms: 50.0,
            },
        )
        .unwrap();

        // Target: match input loudness.
        // Input: 0.5 amplitude sine wave.
        // Process: apply a -6dB attenuation (gain = 0.5).
        // AutoGain should compensate with +6dB (gain = 2.0).

        let block_size = 1024;
        let num_blocks = 50; // Enough for convergence at 50ms smoothing

        let current_signal_gain = 0.5_f32; // -6dB attenuation in the "effect"

        for block in 0..num_blocks {
            let mut input = vec![0.0_f32; block_size * 2];
            for i in 0..block_size {
                let phase = 2.0 * std::f32::consts::PI * 1000.0 * (block * block_size + i) as f32
                    / sample_rate as f32;
                input[i * 2] = phase.sin() * 0.5;
                input[i * 2 + 1] = phase.sin() * 0.5;
            }

            ag.measure_input(&input).unwrap();

            // Simulate effect: attenuate by 6dB
            let mut output = input.clone();
            for s in &mut output {
                *s *= current_signal_gain;
            }

            // FEED-FORWARD measurement (on uncompensated output)
            ag.measure_output(&output).unwrap();

            // Apply compensation
            ag.apply_compensation(&mut output, block_size);

            if block == num_blocks - 1 {
                let gain_db = ag.current_gain_db();
                // Should be close to +6.0 dB
                assert!(
                    (gain_db - 6.0).abs() < 0.5,
                    "AutoGain did not converge to +6dB, got {}dB",
                    gain_db
                );
            }
        }
    }

    #[test]
    fn test_autogain_no_oscillation() {
        let sample_rate = 48000;
        let mut ag = AutoGain::new(
            2,
            sample_rate,
            AutoGainParams {
                enabled: true,
                loudness_type: AutoGainLoudnessType::Momentary,
                max_gain_db: 12.0,
                smoothing_ms: 20.0, // Reasonably fast smoothing
            },
        )
        .unwrap();

        let block_size = 1024;
        let num_blocks = 150;
        let mut gains = Vec::new();

        for block in 0..num_blocks {
            let mut input = vec![0.0_f32; block_size * 2];
            for i in 0..block_size {
                let phase = 2.0 * std::f32::consts::PI * 440.0 * (block * block_size + i) as f32
                    / sample_rate as f32;
                input[i * 2] = phase.sin() * 0.5;
                input[i * 2 + 1] = phase.sin() * 0.5;
            }
            ag.measure_input(&input).unwrap();

            // Effect: -3dB attenuation
            let mut output = input.clone();
            for s in &mut output {
                *s *= 0.707;
            }

            ag.measure_output(&output).unwrap();
            ag.apply_compensation(&mut output, block_size);

            gains.push(ag.current_gain_db());
        }

        // Check last 30 blocks for stability (no oscillations > 0.2 dB)
        // EBU R128 momentary loudness has a 400ms window, so it needs time to settle.
        let stable_part = &gains[num_blocks - 30..];
        let min_gain = stable_part.iter().fold(f32::INFINITY, |a, &b| a.min(b));
        let max_gain = stable_part.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));

        assert!(
            max_gain - min_gain < 0.2,
            "AutoGain is oscillating: range {}dB. Last gains: {:?}",
            max_gain - min_gain,
            stable_part
        );
    }

    #[test]
    fn rejected_sample_rate_preserves_gain_and_smoothing_state() {
        let params = AutoGainParams {
            enabled: true,
            loudness_type: AutoGainLoudnessType::Momentary,
            max_gain_db: 12.0,
            smoothing_ms: 40.0,
        };
        let mut actual = AutoGain::new(2, 48_000, params.clone()).unwrap();
        let mut twin = AutoGain::new(2, 48_000, params).unwrap();
        let input = (0..48_000 * 2)
            .map(|index| {
                let frame = index / 2;
                (std::f32::consts::TAU * 997.0 * frame as f32 / 48_000.0).sin() * 0.55
            })
            .collect::<Vec<_>>();
        let mut measured_output = input.iter().map(|sample| sample * 0.4).collect::<Vec<_>>();
        for gain in [&mut actual, &mut twin] {
            gain.measure_input(&input).unwrap();
            gain.measure_output(&measured_output).unwrap();
        }
        actual.apply_compensation(&mut measured_output, input.len() / 2);
        let mut twin_output = input.iter().map(|sample| sample * 0.4).collect::<Vec<_>>();
        twin.apply_compensation(&mut twin_output, input.len() / 2);
        assert!(actual.current_gain_db().abs() > 0.1);
        assert_eq!(actual.current_gain_db(), twin.current_gain_db());

        assert!(actual.set_sample_rate(9).is_err());
        actual.set_smoothing_ms(40.0);
        twin.set_smoothing_ms(40.0);
        let mut continued_actual = vec![0.2; 512 * 2];
        let mut continued_twin = continued_actual.clone();
        actual.apply_compensation(&mut continued_actual, 512);
        twin.apply_compensation(&mut continued_twin, 512);
        assert_eq!(continued_actual, continued_twin);
        assert_eq!(actual.current_gain_db(), twin.current_gain_db());

        let gain_before_valid_change = actual.current_gain_db();
        actual.set_sample_rate(96_000).unwrap();
        twin.set_sample_rate(96_000).unwrap();
        assert_eq!(actual.current_gain_db(), gain_before_valid_change);
        assert_eq!(actual.current_gain_db(), twin.current_gain_db());
    }
}

#[cfg(test)]
mod smoothing_tests;
