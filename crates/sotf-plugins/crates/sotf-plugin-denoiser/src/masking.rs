// ============================================================================
// Psychoacoustic Masking
// ============================================================================
//
// Implements frequency masking based on the Bark scale spreading function.
// Bins where the noise is perceptually masked by nearby signal content
// are skipped during denoising (gain set to 1.0), preserving signal quality.
//
// Reference: Zwicker & Fastl, "Psychoacoustics: Facts and Models", 1999

// Rust guideline compliant 2026-02-21
use super::DenoiserPlugin;
use math_audio_dsp::fast_math::fast_log10;

/// Small constant to prevent log(0)
const EPSILON: f32 = 1e-10;

/// Maximum spreading range in Bark
const MAX_SPREAD_BARK: f32 = 4.0;

/// Spreading slope below masker (dB/Bark, negative direction)
const LOWER_SLOPE_DB_PER_BARK: f32 = -10.0;

/// Spreading slope above masker (dB/Bark, positive direction)
const UPPER_SLOPE_DB_PER_BARK: f32 = -25.0;

/// Masking offset: how much below the masker level noise becomes inaudible (dB)
const MASKING_OFFSET_DB: f32 = -15.0;

impl DenoiserPlugin {
    /// Convert frequency in Hz to Bark scale (Zwicker formula)
    #[inline]
    fn freq_to_bark(freq_hz: f32) -> f32 {
        // Traunmüller (1990) approximation
        let f = freq_hz / 1000.0;
        13.0 * (0.76 * f).atan() + 3.5 * (f / 7.5).powi(2).atan()
    }

    /// Precompute Bark mapping and per-bin spreading ranges for all FFT bins.
    /// Called during initialize() when sample_rate is known.
    pub(super) fn precompute_bark_mapping(&mut self) {
        let bin_hz = self.config.sample_rate as f32 / self.config.fft_size as f32;
        for k in 0..self.config.spectrum_size {
            let freq = k as f32 * bin_hz;
            self.masking.bark_map[k] = Self::freq_to_bark(freq);
        }

        // Precompute the (lo, hi) bin range within MAX_SPREAD_BARK for each bin.
        // bark_map is monotonically non-decreasing, so we can use partition_point.
        for j in 0..self.config.spectrum_size {
            let bark_j = self.masking.bark_map[j];
            let lo = self.masking.bark_map[..self.config.spectrum_size]
                .partition_point(|&b| b < bark_j - MAX_SPREAD_BARK);
            let hi = self.masking.bark_map[..self.config.spectrum_size]
                .partition_point(|&b| b <= bark_j + MAX_SPREAD_BARK);
            self.masking.bark_bin_range[j] = (lo, hi);
        }
    }

    /// Compute masking thresholds for a given channel.
    /// Uses signal power and the Bark-scale spreading function.
    /// Prunes dominated maskers while preserving rounded threshold comparisons.
    ///
    /// Thresholds are stored in dB domain to avoid expensive powf() conversion.
    /// Comparison in `is_noise_masked` also works in dB.
    pub(super) fn compute_masking_thresholds(&mut self, channel: usize) {
        let n = self.config.spectrum_size;

        // Compute signal power in dB using fast approximation (1 fast_log10 per bin)
        for k in 0..n {
            let power = self.get_power_at_bin(channel, k).max(EPSILON);
            self.masking.masking_signal_power[k] = 10.0 * fast_log10(power);
        }

        spread_masking(
            &self.masking.masking_signal_power[..n],
            &self.masking.bark_map[..n],
            &self.masking.bark_bin_range[..n],
            &mut self.masking.masking_threshold[..n],
            &mut self.masking.candidates[..n],
        );
    }

    /// Check if noise at a given bin is perceptually masked.
    /// Compares noise power (converted to dB) against the dB-domain threshold.
    #[inline]
    pub(super) fn is_noise_masked(&self, channel: usize, bin: usize) -> bool {
        let noise_power = self.get_noise_power(channel, bin);
        let noise_db = 10.0 * fast_log10(noise_power);
        noise_db < self.masking.masking_threshold[bin]
    }
}

fn spread_masking_reference(
    power: &[f32],
    bark: &[f32],
    ranges: &[(usize, usize)],
    out: &mut [f32],
) {
    out.fill(f32::NEG_INFINITY);
    for (j, &(lo, hi)) in ranges.iter().enumerate() {
        let masker = power[j] + MASKING_OFFSET_DB;
        for k in lo..hi {
            let diff = bark[k] - bark[j];
            let spread = if diff < 0.0 {
                LOWER_SLOPE_DB_PER_BARK * diff.abs()
            } else {
                UPPER_SLOPE_DB_PER_BARK * diff
            };
            let value = masker + spread;
            if value > out[k] {
                out[k] = value;
            }
        }
    }
}

fn spread_masking(
    power: &[f32],
    bark: &[f32],
    ranges: &[(usize, usize)],
    out: &mut [f32],
    queue: &mut [usize],
) {
    // Preserve the original comparison behavior for infinities and NaNs.
    if power.iter().any(|p| !p.is_finite()) {
        spread_masking_reference(power, bark, ranges, out);
        return;
    }
    let n = power.len();
    let max_power = power
        .iter()
        .map(|p| f64::from((*p + MASKING_OFFSET_DB).abs()))
        .fold(0.0, f64::max);
    let max_bark = bark.iter().map(|b| f64::from(b.abs())).fold(0.0, f64::max);
    // Bound both candidates' f32 subtraction, multiplication and addition errors.
    // This is a pruning guard, never a tolerance applied to the returned thresholds.
    let margin = 16.0 * f64::from(f32::EPSILON) * (max_power + 25.0 * max_bark + 1.0);
    out.fill(f32::NEG_INFINITY);
    // Later maskers expire no earlier in each direction. Remove an older one
    // only when it is strictly dominated despite rounding. Near ties stay in
    // the queue and use the original operation order; worst-case work is still
    // quadratic, but dense ordinary spectra need far fewer contributions.
    for reverse in [false, true] {
        let slope = if reverse {
            f64::from(LOWER_SLOPE_DB_PER_BARK)
        } else {
            -f64::from(UPPER_SLOPE_DB_PER_BARK)
        };
        let score = |j: usize| f64::from(power[j] + MASKING_OFFSET_DB) + slope * f64::from(bark[j]);
        let (mut head, mut tail) = (0, 0);
        for step in 0..n {
            let k = if reverse { n - 1 - step } else { step };
            while head < tail
                && if reverse {
                    ranges[queue[head]].0 > k
                } else {
                    ranges[queue[head]].1 <= k
                }
            {
                head += 1;
            }
            while head < tail && score(queue[tail - 1]) + margin < score(k) {
                tail -= 1;
            }
            queue[tail] = k;
            tail += 1;
            for &j in &queue[head..tail] {
                let diff = bark[k] - bark[j];
                let spread = if diff < 0.0 {
                    LOWER_SLOPE_DB_PER_BARK * diff.abs()
                } else {
                    UPPER_SLOPE_DB_PER_BARK * diff
                };
                let value = (power[j] + MASKING_OFFSET_DB) + spread;
                if value > out[k] {
                    out[k] = value;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{spread_masking, spread_masking_reference};
    #[test]
    fn matches_original_bits() {
        let mut rng = 1947_u64;
        for (rate, n) in [
            (44100.0_f32, 257),
            (88200.0, 1025),
            (96000.0, 1025),
            (192000.0, 1025),
        ] {
            let bark: Vec<f32> = (0..n)
                .map(|k| {
                    let f = k as f32 * (rate / (2 * (n - 1)) as f32) / 1000.0;
                    13.0 * (0.76 * f).atan() + 3.5 * (f / 7.5).powi(2).atan()
                })
                .collect();
            let ranges: Vec<_> = bark
                .iter()
                .map(|&b| {
                    (
                        bark.partition_point(|&x| x < b - 4.0),
                        bark.partition_point(|&x| x <= b + 4.0),
                    )
                })
                .collect();
            for pattern in 0..10 {
                let power: Vec<_> = (0..n)
                    .map(|k| {
                        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let unit = (rng >> 40) as f32 / (1_u32 << 24) as f32;
                        match pattern {
                            0 => -60.0,
                            1 => -115.0 + unit * 145.0,
                            2 => -110.0 + 140.0 * k as f32 / n as f32,
                            3 => {
                                if k == n / 2 {
                                    10.0
                                } else {
                                    -115.0
                                }
                            }
                            4 => -25.0 * bark[k] + (unit - 0.5) * 0.00002,
                            5 => 10.0 * bark[k] + (unit - 0.5) * 0.00002,
                            6 => {
                                if k == n / 2 {
                                    f32::INFINITY
                                } else {
                                    -60.0
                                }
                            }
                            7 => {
                                if k == n / 2 {
                                    f32::NAN
                                } else {
                                    -60.0
                                }
                            }
                            8 => {
                                if k == n / 2 {
                                    f32::NEG_INFINITY
                                } else {
                                    -60.0
                                }
                            }
                            _ => -f32::MAX + unit * f32::MAX,
                        }
                    })
                    .collect();
                let mut expected = vec![0.0; n];
                let mut actual = vec![0.0; n];
                let mut queue = vec![0; n];
                spread_masking_reference(&power, &bark, &ranges, &mut expected);
                spread_masking(&power, &bark, &ranges, &mut actual, &mut queue);
                for k in 0..n {
                    assert_eq!(
                        actual[k].to_bits(),
                        expected[k].to_bits(),
                        "rate {rate}, pattern {pattern}, bin {k}"
                    );
                }
            }
        }
    }
}
