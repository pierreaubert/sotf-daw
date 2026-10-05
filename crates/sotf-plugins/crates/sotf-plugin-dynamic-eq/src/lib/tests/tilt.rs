use super::super::dyn_eq_band::{DynEqBand, design_tilt_coefficients};
use super::super::dyn_eq_band_params::DynEqBandParams;
use super::super::dynamic_eq_plugin::DynamicEqPlugin;
use super::super::dynamic_eq_plugin_params::DynamicEqPluginParams;
use super::super::params::DynEqShape;
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;

#[derive(Clone, Copy, Debug)]
struct Complex {
    re: f64,
    im: f64,
}

impl Complex {
    const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    fn add(self, other: Self) -> Self {
        Self::new(self.re + other.re, self.im + other.im)
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.re - other.re, self.im - other.im)
    }

    fn mul(self, other: Self) -> Self {
        Self::new(
            self.re * other.re - self.im * other.im,
            self.re * other.im + self.im * other.re,
        )
    }

    fn scale(self, scale: f64) -> Self {
        Self::new(self.re * scale, self.im * scale)
    }

    fn div(self, other: Self) -> Self {
        let denominator = other.re * other.re + other.im * other.im;
        Self::new(
            (self.re * other.re + self.im * other.im) / denominator,
            (self.im * other.re - self.re * other.im) / denominator,
        )
    }

    fn magnitude(self) -> f64 {
        self.re.hypot(self.im)
    }
}

fn digital_response(
    coefficients: &math_audio_iir_fir::BiquadCoefficients<f64>,
    frequency: f64,
    sample_rate: f64,
) -> Complex {
    let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate;
    let z1 = Complex::new(omega.cos(), -omega.sin());
    let z2 = z1.mul(z1);
    let numerator = Complex::new(coefficients.b0, 0.0)
        .add(z1.scale(coefficients.b1))
        .add(z2.scale(coefficients.b2));
    let denominator = Complex::new(1.0, 0.0)
        .add(z1.scale(coefficients.a1))
        .add(z2.scale(coefficients.a2));
    numerator.div(denominator)
}

/// Independent analog tilt prototype evaluated through the exact bilinear
/// frequency mapping. This never builds digital coefficients, so it does
/// not reuse the production coefficient algebra.
fn analog_tilt_response(gain_db: f64, pivot: f64, frequency: f64, sample_rate: f64) -> Complex {
    let gain = 10.0_f64.powf(gain_db / 20.0);
    let warped_pivot = 2.0 * sample_rate * (std::f64::consts::PI * pivot / sample_rate).tan();
    let warped_signal = 2.0 * sample_rate * (std::f64::consts::PI * frequency / sample_rate).tan();
    // H(s) = (s + g*w0) / (g*s + w0) at s = j*w.
    let numerator = Complex::new(gain * warped_pivot, warped_signal);
    let denominator = Complex::new(warped_pivot, gain * warped_signal);
    numerator.div(denominator)
}

/// Independently derived tilt coefficients through a dimensionless
/// tangent substitution (`t = tan(pi*pivot/fs)`), a different algebraic
/// path from the production `2*fs` formulation.
fn independent_tilt_coefficients(
    pivot: f64,
    sample_rate: f64,
    gain_db: f64,
) -> math_audio_iir_fir::BiquadCoefficients<f64> {
    let gain = 10.0_f64.powf(gain_db / 20.0);
    let tangent = (std::f64::consts::PI * pivot / sample_rate).tan();
    let a0 = gain + tangent;
    math_audio_iir_fir::BiquadCoefficients {
        b0: (1.0 + gain * tangent) / a0,
        b1: (gain * tangent - 1.0) / a0,
        b2: 0.0,
        a1: (tangent - gain) / a0,
        a2: 0.0,
    }
}

#[derive(Clone, Copy, Default)]
struct DirectFormState {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl DirectFormState {
    fn process(
        &mut self,
        coefficients: &math_audio_iir_fir::BiquadCoefficients<f64>,
        input: f64,
    ) -> f64 {
        let output =
            coefficients.b0 * input + coefficients.b1 * self.x1 + coefficients.b2 * self.x2
                - coefficients.a1 * self.y1
                - coefficients.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = input;
        self.y2 = self.y1;
        self.y1 = output;
        output
    }
}

fn reference_compression_gain_reduction(
    input_db: f64,
    threshold_db: f64,
    ratio: f64,
    knee_db: f64,
) -> f64 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee_db < 0.1 {
        if input_db <= threshold_db {
            0.0
        } else {
            (input_db - threshold_db) * slope
        }
    } else if input_db < threshold_db - knee_db * 0.5 {
        0.0
    } else if input_db > threshold_db + knee_db * 0.5 {
        (input_db - threshold_db) * slope
    } else {
        let position = (input_db - threshold_db + knee_db * 0.5) / knee_db;
        position * position * (knee_db * 0.5) * slope
    }
}

/// Independent full-band tilt dynamics reference: raw absolute level
/// detection, one-pole attack/release envelopes, and the amplitude blend.
#[allow(clippy::too_many_arguments)]
fn independent_tilt_reference(
    input: &[f32],
    channels: usize,
    sample_rate: u32,
    pivot: f64,
    target_gain_db: f64,
    threshold_db: f64,
    ratio: f64,
    knee_db: f64,
    attack_ms: f64,
    release_ms: f64,
    linked: bool,
) -> (Vec<f32>, Vec<f64>) {
    let coefficients = independent_tilt_coefficients(pivot, sample_rate as f64, target_gain_db);
    let mut eq = vec![DirectFormState::default(); channels];
    let core_count = if linked { 1 } else { channels };
    let mut envelope = vec![0.0_f64; core_count];
    let attack_coefficient = (-1.0 / (attack_ms * 0.001 * sample_rate as f64)).exp();
    let release_coefficient = (-1.0 / (release_ms * 0.001 * sample_rate as f64)).exp();
    let mut output = vec![0.0_f32; input.len()];
    let mut proportions = Vec::with_capacity(input.len() / channels);
    let mut per_channel = vec![0.0_f64; channels];

    for frame in 0..input.len() / channels {
        let mut frame_proportion = 0.0_f64;
        if linked {
            let mut level = 0.0_f64;
            for channel in 0..channels {
                level = level.max(f64::from(input[frame * channels + channel]).abs());
            }
            let level_db = 20.0 * level.max(1.0e-10).log10();
            let target =
                reference_compression_gain_reduction(level_db, threshold_db, ratio, knee_db);
            let coefficient = if target > envelope[0] {
                attack_coefficient
            } else {
                release_coefficient
            };
            envelope[0] = target + coefficient * (envelope[0] - target);
            let applied = envelope[0].clamp(0.0, target_gain_db.abs()) * target_gain_db.signum();
            let proportion = if target_gain_db.abs() < 0.01 {
                0.0
            } else {
                let full = 10.0_f64.powf(target_gain_db / 20.0);
                let desired = 10.0_f64.powf(applied / 20.0);
                ((desired - 1.0) / (full - 1.0)).clamp(0.0, 1.0)
            };
            frame_proportion = frame_proportion.max(proportion);
            per_channel.fill(proportion);
        } else {
            for channel in 0..channels {
                let sample = f64::from(input[frame * channels + channel]).abs();
                let level_db = 20.0 * sample.max(1.0e-10).log10();
                let target =
                    reference_compression_gain_reduction(level_db, threshold_db, ratio, knee_db);
                let coefficient = if target > envelope[channel] {
                    attack_coefficient
                } else {
                    release_coefficient
                };
                envelope[channel] = target + coefficient * (envelope[channel] - target);
                let applied =
                    envelope[channel].clamp(0.0, target_gain_db.abs()) * target_gain_db.signum();
                let proportion = if target_gain_db.abs() < 0.01 {
                    0.0
                } else {
                    let full = 10.0_f64.powf(target_gain_db / 20.0);
                    let desired = 10.0_f64.powf(applied / 20.0);
                    ((desired - 1.0) / (full - 1.0)).clamp(0.0, 1.0)
                };
                frame_proportion = frame_proportion.max(proportion);
                per_channel[channel] = proportion;
            }
        }

        for channel in 0..channels {
            let index = frame * channels + channel;
            let dry = input[index];
            let equalized = eq[channel].process(&coefficients, f64::from(dry)) as f32;
            output[index] = dry + (equalized - dry) * per_channel[channel] as f32;
        }
        proportions.push(frame_proportion);
    }

    (output, proportions)
}

#[test]
fn tilt_coefficients_match_independent_analog_prototype() {
    let sample_rates: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
    let gains = [-24.0, -12.0, -6.0, 0.0, 6.0, 12.0, 24.0];

    for sample_rate in sample_rates {
        let maximum_pivot = (sample_rate * 0.475).min(20_000.0);
        for pivot in [20.0, 997.0_f64.min(maximum_pivot), maximum_pivot] {
            for gain_db in gains {
                let coefficients = design_tilt_coefficients(pivot, sample_rate, gain_db)
                    .unwrap_or_else(|| {
                        panic!("tilt design rejected Fs={sample_rate}, pivot={pivot}, G={gain_db}")
                    });
                assert_eq!(coefficients.b2, 0.0);
                assert_eq!(coefficients.a2, 0.0);
                assert!(coefficients.a1.abs() < 1.0);

                for index in 1..=256 {
                    let frequency = 0.1 * ((sample_rate * 0.499 / 0.1).powf(index as f64 / 256.0));
                    let actual = digital_response(&coefficients, frequency, sample_rate);
                    let expected = analog_tilt_response(gain_db, pivot, frequency, sample_rate);
                    assert!(actual.re.is_finite() && actual.im.is_finite());
                    assert!(expected.re.is_finite() && expected.im.is_finite());
                    let error = actual.sub(expected).magnitude();
                    let relative = error / expected.magnitude().max(0.05);
                    assert!(
                        relative <= 1.0e-9,
                        "Fs={sample_rate}, pivot={pivot}, G={gain_db}, f={frequency}: relative complex error {relative:e}"
                    );
                }

                // DC, pivot, and Nyquist laws are exact by construction.
                let dc_db = 20.0
                    * digital_response(&coefficients, 0.0, sample_rate)
                        .magnitude()
                        .log10();
                let pivot_db = 20.0
                    * digital_response(&coefficients, pivot, sample_rate)
                        .magnitude()
                        .log10();
                let nyquist_db = 20.0
                    * digital_response(&coefficients, sample_rate * 0.5, sample_rate)
                        .magnitude()
                        .log10();
                assert!(
                    (dc_db - gain_db).abs() < 1.0e-9,
                    "Fs={sample_rate}, pivot={pivot}, G={gain_db}: DC {dc_db} dB"
                );
                assert!(
                    pivot_db.abs() < 1.0e-9,
                    "Fs={sample_rate}, pivot={pivot}, G={gain_db}: pivot {pivot_db} dB"
                );
                assert!(
                    (nyquist_db + gain_db).abs() < 1.0e-9,
                    "Fs={sample_rate}, pivot={pivot}, G={gain_db}: Nyquist {nyquist_db} dB"
                );

                if gain_db != 0.0 {
                    let mut previous_db = 20.0
                        * digital_response(&coefficients, 0.1, sample_rate)
                            .magnitude()
                            .log10();
                    // Positive tilt falls from DC to Nyquist; negative rises.
                    let direction = -gain_db.signum();
                    for index in 1..=512 {
                        let frequency =
                            0.1 * ((sample_rate * 0.499 / 0.1).powf(index as f64 / 512.0));
                        let current_db = 20.0
                            * digital_response(&coefficients, frequency, sample_rate)
                                .magnitude()
                                .log10();
                        let directed_step = (current_db - previous_db) * direction;
                        assert!(
                            directed_step >= -1.0e-6,
                            "non-monotonic tilt at Fs={sample_rate}, pivot={pivot}, G={gain_db}, f={frequency}: step={directed_step} dB"
                        );
                        previous_db = current_db;
                    }
                }
            }
        }
    }

    // Invalid designs are rejected, not clamped into instability.
    assert!(design_tilt_coefficients(0.0, 48_000.0, 6.0).is_none());
    assert!(design_tilt_coefficients(24_000.0, 48_000.0, 6.0).is_none());
    assert!(design_tilt_coefficients(f64::NAN, 48_000.0, 6.0).is_none());
    assert!(design_tilt_coefficients(1_000.0, 0.0, 6.0).is_none());
    assert!(design_tilt_coefficients(1_000.0, 48_000.0, 30.0).is_none());
    assert!(design_tilt_coefficients(1_000.0, 48_000.0, f64::INFINITY).is_none());
}

#[test]
fn held_tilt_proportions_match_independent_recurrence() {
    let sample_rate = 48_000_u32;
    let channels = 2;
    let frames = 4_096;
    let pivot = 1_200.0_f64;
    let proportions = [0.0_f64, 0.25, 0.5, 0.75, 1.0];
    let mut input = vec![0.0_f32; frames * channels];
    for frame in 0..frames {
        for channel in 0..channels {
            let phase = frame as f64 / sample_rate as f64;
            let low = (2.0 * std::f64::consts::PI * (180.0 + channel as f64 * 37.0) * phase).sin();
            let high =
                (2.0 * std::f64::consts::PI * (6_200.0 + channel as f64 * 350.0) * phase).sin();
            input[frame * channels + channel] = (0.23 * low + 0.11 * high) as f32;
        }
    }

    for target_gain_db in [-12.0_f64, 12.0] {
        let coefficients = independent_tilt_coefficients(pivot, sample_rate as f64, target_gain_db);
        for proportion in proportions {
            let mut band =
                DynEqBand::new(channels, sample_rate, pivot as f32, 0.707, 0.0, 3.0, 80.0);
            band.shape = DynEqShape::Tilt;
            band.target_gain_db = target_gain_db as f32;
            band.rebuild_eq_filters(sample_rate);

            let mut reference_states = vec![DirectFormState::default(); channels];
            let mut actual = vec![0.0_f32; input.len()];
            let mut expected = vec![0.0_f32; input.len()];
            for frame in 0..frames {
                for (channel, reference_state) in reference_states.iter_mut().enumerate() {
                    let index = frame * channels + channel;
                    let dry = input[index];
                    let equalized = band.process_eq(channel, f64::from(dry)) as f32;
                    actual[index] = dry + (equalized - dry) * proportion as f32;

                    let independent_eq = reference_state.process(&coefficients, f64::from(dry));
                    expected[index] =
                        (f64::from(dry) + (independent_eq - f64::from(dry)) * proportion) as f32;
                }
            }

            assert_eq!(actual.len(), input.len());
            assert_eq!(expected.len(), input.len());
            assert!(actual.iter().all(|sample| sample.is_finite()));
            assert!(expected.iter().all(|sample| sample.is_finite()));
            let peak_error = actual
                .iter()
                .zip(&expected)
                .map(|(actual, expected)| f64::from((*actual - *expected).abs()))
                .fold(0.0_f64, f64::max);
            let rms_error = (actual
                .iter()
                .zip(&expected)
                .map(|(actual, expected)| (f64::from(*actual) - f64::from(*expected)).powi(2))
                .sum::<f64>()
                / input.len() as f64)
                .sqrt();
            assert!(
                peak_error <= 1.0e-5,
                "tilt G={target_gain_db} p={proportion}: peak error {peak_error}"
            );
            assert!(
                rms_error <= 1.0e-6,
                "tilt G={target_gain_db} p={proportion}: RMS error {rms_error}"
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn make_tilt_plugin(
    channels: usize,
    linked: bool,
    target_gain_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f64,
    attack_ms: f32,
    release_ms: f32,
    sample_rate: u32,
) -> DynamicEqPlugin {
    DynamicEqPlugin::try_from_params_at_sample_rate(
        channels,
        DynamicEqPluginParams {
            num_bands: 1,
            threshold: threshold_db,
            ratio,
            attack_ms,
            release_ms,
            knee: knee_db as f32,
            link_channels: linked,
            mix: 1.0,
            bands: vec![DynEqBandParams {
                shape: DynEqShape::Tilt,
                frequency: 1_000.0,
                gain: target_gain_db,
                band_threshold: threshold_db,
                band_ratio: ratio,
                ..DynEqBandParams::default()
            }],
            stereo_pairs: None,
        },
        sample_rate,
    )
    .unwrap()
}

#[test]
fn public_tilt_process_matches_independent_full_band_reference() {
    let sample_rate = 48_000_u32;
    let channels = 2;
    let frames = 16_000;
    let mut input = vec![0.0_f32; frames * channels];
    for frame in 0..frames {
        let base_amplitude = if frame < 4_000 {
            0.04
        } else if frame < 9_000 {
            0.8
        } else {
            0.08
        };
        let (left_amplitude, right_amplitude) = if (frame / 2_000) % 2 == 0 {
            (base_amplitude, base_amplitude * 0.08)
        } else {
            (base_amplitude * 0.08, base_amplitude)
        };
        let phase = 2.0 * std::f64::consts::PI * 1_000.0 * frame as f64 / sample_rate as f64;
        input[frame * channels] = (left_amplitude * phase.sin()) as f32;
        input[frame * channels + 1] = (right_amplitude * phase.sin()) as f32;
    }
    assert!(input.iter().all(|sample| sample.is_finite()));

    for target_gain_db in [-12.0_f64, 12.0] {
        for linked in [true, false] {
            let (expected, proportions) = independent_tilt_reference(
                &input,
                channels,
                sample_rate,
                1_000.0,
                target_gain_db,
                -30.0,
                4.0,
                6.0,
                3.0,
                80.0,
                linked,
            );
            assert_eq!(expected.len(), input.len());
            assert_eq!(proportions.len(), frames);
            assert!(expected.iter().all(|sample| sample.is_finite()));
            assert!(proportions.iter().all(|proportion| proportion.is_finite()));
            assert!(proportions.iter().any(|&p| (0.05..0.95).contains(&p)));
            assert!(proportions.iter().any(|&p| p > 0.99));

            let mut actual = input.clone();
            let mut plugin = make_tilt_plugin(
                channels,
                linked,
                target_gain_db as f32,
                -30.0,
                4.0,
                6.0,
                3.0,
                80.0,
                sample_rate,
            );
            plugin
                .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
                .unwrap();

            assert_eq!(actual.len(), input.len());
            assert!(actual.iter().all(|sample| sample.is_finite()));
            let input_rms = (input
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>()
                / input.len() as f64)
                .sqrt();
            let residual_rms = (actual
                .iter()
                .zip(expected.iter())
                .map(|(actual, expected)| (f64::from(*actual) - f64::from(*expected)).powi(2))
                .sum::<f64>()
                / actual.len() as f64)
                .sqrt();
            assert!(
                residual_rms / input_rms <= 0.002,
                "tilt gain={target_gain_db}, linked={linked}: input-normalized residual {} exceeds 0.002",
                residual_rms / input_rms
            );

            // Irregular callback partitioning must not change the audio.
            let mut partitioned = input.clone();
            let mut partitioned_plugin = make_tilt_plugin(
                channels,
                linked,
                target_gain_db as f32,
                -30.0,
                4.0,
                6.0,
                3.0,
                80.0,
                sample_rate,
            );
            let partitions = [1, 31, 509, 7, 1_021, 64, 3];
            let mut frame_offset = 0;
            let mut partition_index = 0;
            while frame_offset < frames {
                let block_frames =
                    partitions[partition_index % partitions.len()].min(frames - frame_offset);
                let start = frame_offset * channels;
                let end = start + block_frames * channels;
                partitioned_plugin
                    .process_in_place(
                        &mut partitioned[start..end],
                        &ProcessContext::new(sample_rate, block_frames),
                    )
                    .unwrap();
                frame_offset += block_frames;
                partition_index += 1;
            }
            assert!(partitioned.iter().all(|sample| sample.is_finite()));
            let partition_error = (actual
                .iter()
                .zip(partitioned.iter())
                .map(|(left, right)| (f64::from(*left) - f64::from(*right)).powi(2))
                .sum::<f64>()
                / actual.len() as f64)
                .sqrt();
            assert!(
                partition_error / input_rms <= 1.0e-7,
                "tilt gain={target_gain_db}, linked={linked}: partition error {} exceeds 1e-7",
                partition_error / input_rms
            );
        }
    }
}

/// Absolute DC sweep for the full-band tilt detector.
///
/// A constant input has no detector ripple, so the settled output must match
/// the static gain-reduction law mapped through the amplitude blend within a
/// tight bound. This anchors the absolute threshold in dBFS, not telemetry.
#[test]
fn tilt_absolute_threshold_sweep_matches_static_law() {
    let sample_rate = 48_000_u32;
    let threshold_db = -20.0_f64;
    let ratio = 20.0_f64;
    let target_gain_db = 12.0_f64;

    for dc_gain_sign in [1.0_f64, -1.0] {
        let signed_target = dc_gain_sign * target_gain_db;
        for amplitude_db in [-60.0, -30.0, -21.0, -20.0, -19.0, -12.0, -6.0] {
            let amplitude = 10.0_f64.powf(amplitude_db / 20.0);
            let mut plugin = make_tilt_plugin(
                1,
                false,
                signed_target as f32,
                -20.0,
                20.0,
                0.0,
                0.5,
                20.0,
                sample_rate,
            );
            // Settle the one-pole envelope and the first-order tilt filter.
            let settle_frames = sample_rate as usize / 2;
            let mut settle = vec![amplitude as f32; settle_frames];
            plugin
                .process_in_place(
                    &mut settle,
                    &ProcessContext::new(sample_rate, settle_frames),
                )
                .unwrap();
            let measure_frames = 4_096;
            let mut measure = vec![amplitude as f32; measure_frames];
            plugin
                .process_in_place(
                    &mut measure,
                    &ProcessContext::new(sample_rate, measure_frames),
                )
                .unwrap();
            assert!(measure.iter().all(|sample| sample.is_finite()));

            let static_gr =
                reference_compression_gain_reduction(amplitude_db, threshold_db, ratio, 0.0);
            let applied = static_gr.clamp(0.0, target_gain_db) * signed_target.signum();
            let full = 10.0_f64.powf(signed_target / 20.0);
            let desired = 10.0_f64.powf(applied / 20.0);
            let proportion = ((desired - 1.0) / (full - 1.0)).clamp(0.0, 1.0);
            // At DC the tilt section gain is exactly the signed target gain.
            let expected = amplitude * (1.0 + proportion * (full - 1.0));
            let measured: f64 = measure.iter().map(|sample| f64::from(*sample)).sum::<f64>()
                / measure_frames as f64;
            let relative_error = ((measured - expected) / expected.max(1.0e-9)).abs();
            assert!(
                relative_error <= 1.0e-3,
                "tilt G={signed_target}, input={amplitude_db} dBFS: measured {measured}, static {expected}"
            );
            if amplitude_db < threshold_db {
                assert!(
                    (measured / amplitude - 1.0).abs() < 1.0e-3,
                    "below-threshold DC must stay dry: {measured} vs {amplitude}"
                );
            }
        }
    }
}

/// Tilt ignores Q and shelf slope by contract; stored values are preserved
/// but must not change the emitted audio.
#[test]
fn tilt_ignores_q_and_shelf_slope() {
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let input: Vec<f32> = (0..frames)
        .map(|frame| {
            (0.4_f64
                * (2.0 * std::f64::consts::PI * 440.0 * frame as f64 / sample_rate as f64).sin())
                as f32
        })
        .collect();

    let render = |q: f32, slope: f32| {
        let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(
            1,
            DynamicEqPluginParams {
                num_bands: 1,
                threshold: -30.0,
                ratio: 4.0,
                attack_ms: 1.0,
                release_ms: 40.0,
                knee: 0.0,
                link_channels: false,
                mix: 1.0,
                bands: vec![DynEqBandParams {
                    shape: DynEqShape::Tilt,
                    frequency: 1_000.0,
                    q,
                    gain: 9.0,
                    shelf_slope: slope,
                    band_threshold: -30.0,
                    band_ratio: 4.0,
                    ..DynEqBandParams::default()
                }],
                stereo_pairs: None,
            },
            sample_rate,
        )
        .unwrap();
        let mut audio = input.clone();
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        audio
    };

    let baseline = render(0.707, 1.0);
    assert!(baseline.iter().all(|sample| sample.is_finite()));
    assert_ne!(baseline, input);
    for (q, slope) in [(0.1, 0.1), (10.0, 1.0), (2.5, 0.55)] {
        assert_eq!(render(q, slope), baseline, "q={q}, slope={slope}");
    }
}

/// DC step timing: attack and release 63% times must track the configured
/// milliseconds, and the trajectory must be monotonic toward the static law.
#[test]
fn tilt_dc_step_attack_release_timing() {
    let sample_rate = 48_000_u32;
    let attack_ms = 10.0_f32;
    let release_ms = 100.0_f32;
    let mut plugin = make_tilt_plugin(
        1,
        false,
        12.0,
        -20.0,
        20.0,
        0.0,
        attack_ms,
        release_ms,
        sample_rate,
    );

    // Attack: silence, then a settled loud DC step recorded per frame.
    let mut silence = vec![0.0_f32; 4_096];
    plugin
        .process_in_place(&mut silence, &ProcessContext::new(sample_rate, 4_096))
        .unwrap();
    let step_level = 0.5_f32;
    let static_gr =
        reference_compression_gain_reduction(20.0 * (step_level as f64).log10(), -20.0, 20.0, 0.0)
            as f32;
    let mut attack_trajectory = Vec::new();
    for _ in 0..(sample_rate as usize / 4) {
        let mut frame = vec![step_level];
        plugin
            .process_in_place(&mut frame, &ProcessContext::new(sample_rate, 1))
            .unwrap();
        attack_trajectory.push(plugin.monitoring_gr[0]);
    }
    assert!(attack_trajectory.iter().all(|value| value.is_finite()));
    let settled_attack = *attack_trajectory.last().unwrap();
    assert!(
        (settled_attack - static_gr).abs() / static_gr.max(1.0e-6) < 0.02,
        "attack settled {settled_attack} dB, static {static_gr} dB"
    );
    for pair in attack_trajectory.windows(2) {
        assert!(
            pair[1] + 1.0e-6 >= pair[0],
            "attack trajectory must rise monotonically: {:?}",
            &attack_trajectory[..16.min(attack_trajectory.len())]
        );
    }
    let attack_63 = attack_trajectory
        .iter()
        .position(|value| *value >= settled_attack * 0.632)
        .unwrap() as f32
        / sample_rate as f32
        * 1000.0;
    assert!(
        (attack_63 - attack_ms).abs() / attack_ms < 0.25,
        "attack 63% at {attack_63} ms, configured {attack_ms} ms"
    );

    // Release: back to silence, trajectory must fall monotonically to zero.
    let mut release_trajectory = Vec::new();
    for _ in 0..(sample_rate as usize) {
        let mut frame = vec![0.0_f32];
        plugin
            .process_in_place(&mut frame, &ProcessContext::new(sample_rate, 1))
            .unwrap();
        release_trajectory.push(plugin.monitoring_gr[0]);
    }
    assert!(release_trajectory.iter().all(|value| value.is_finite()));
    for pair in release_trajectory.windows(2) {
        assert!(
            pair[1] <= pair[0] + 1.0e-6,
            "release trajectory must fall monotonically"
        );
    }
    assert!(
        release_trajectory.last().unwrap().abs() < 0.05,
        "release must settle near zero: {}",
        release_trajectory.last().unwrap()
    );
    let release_start = release_trajectory[0].max(1.0e-6);
    let release_37 = release_trajectory
        .iter()
        .position(|value| *value <= release_start * 0.368)
        .unwrap() as f32
        / sample_rate as f32
        * 1000.0;
    assert!(
        (release_37 - release_ms).abs() / release_ms < 0.25,
        "release 37% at {release_37} ms, configured {release_ms} ms"
    );
}

/// Emitted-audio confirmation of the attack/release headline numbers. The
/// meter test above proves envelope timing; this test proves the audible
/// blend follows it, measuring only emitted samples (never `monitoring_gr`).
#[test]
fn tilt_dc_step_attack_release_timing_confirmed_on_emitted_audio() {
    let sample_rate = 48_000_u32;
    let attack_ms = 10.0_f32;
    let release_ms = 100.0_f32;
    // Static target T = (20*log10(0.5) + 24) * (1 - 1/8) = 15.73 dB: above
    // G = 12 dB so attack settles at full blend (p = 1), and the 63%/37%
    // audio crossings land near the configured ms (predicted 8.8/89 ms,
    // both inside the 25% window) through the documented blend law.
    let mut plugin = make_tilt_plugin(
        1,
        false,
        12.0,
        -24.0,
        8.0,
        0.0,
        attack_ms,
        release_ms,
        sample_rate,
    );

    let mut silence = vec![0.0_f32; 4_096];
    plugin
        .process_in_place(&mut silence, &ProcessContext::new(sample_rate, 4_096))
        .unwrap();

    // Attack: loud DC step. Deviation from dry must rise monotonically to
    // the full-blend DC law and cross 63% near the configured attack time.
    let step_level = 0.5_f32;
    let static_target =
        reference_compression_gain_reduction(20.0 * f64::from(step_level).log10(), -24.0, 8.0, 0.0);
    assert!(
        static_target > 12.0,
        "test config must saturate the blend: {static_target}"
    );
    let full_amplitude = 10.0_f64.powf(12.0 / 20.0);
    let expected_settled = f64::from(step_level) * (full_amplitude - 1.0);
    let mut attack_deviation = Vec::new();
    for _ in 0..(sample_rate as usize / 4) {
        let mut frame = vec![step_level];
        plugin
            .process_in_place(&mut frame, &ProcessContext::new(sample_rate, 1))
            .unwrap();
        attack_deviation.push(f64::from(frame[0]) - f64::from(step_level));
    }
    assert!(attack_deviation.iter().all(|value| value.is_finite()));
    let settled = *attack_deviation.last().unwrap();
    assert!(
        (settled - expected_settled).abs() / expected_settled < 0.02,
        "attack settled deviation {settled}, predicted {expected_settled}"
    );
    // The tilt section starts below unity (b0 ~= 0.31 at +12 dB, 1 kHz
    // pivot, 48 kHz), so eq-dry is negative while the blend proportion
    // grows: their product dips to about -7e-4 before rising. Envelope
    // timing is therefore read past a 5 ms filter-settling window (~7.7
    // pole taus of 0.65 ms; residual under 0.05%), mirroring the release
    // side's 10 ms skip. The skipped prefix is bounded, not ignored: it
    // must stay above -0.01 and below the 63% line, proving the crossing
    // genuinely lands after the window.
    let attack_search_start = sample_rate as usize / 200;
    let prefix_min = attack_deviation[..attack_search_start]
        .iter()
        .fold(f64::INFINITY, |min, value| min.min(*value));
    let prefix_max = attack_deviation[..attack_search_start]
        .iter()
        .fold(f64::NEG_INFINITY, |max, value| max.max(*value));
    assert!(
        prefix_min > -0.01,
        "attack prefix dipped below the filter-step bound: {prefix_min}"
    );
    assert!(
        prefix_max < settled * 0.632,
        "attack 63% crossing must land after the settling window"
    );
    for pair in attack_deviation[attack_search_start..].windows(2) {
        assert!(
            pair[1] + 1.0e-5 >= pair[0],
            "attack audio deviation must rise monotonically past the settling window"
        );
    }
    let attack_63 = (attack_search_start
        + attack_deviation[attack_search_start..]
            .iter()
            .position(|value| *value >= settled * 0.632)
            .unwrap()) as f32
        / sample_rate as f32
        * 1000.0;
    assert!(
        (attack_63 - attack_ms).abs() / attack_ms < 0.25,
        "attack 63% audio at {attack_63} ms, configured {attack_ms} ms"
    );

    // Release: step down to a quiet-but-nonzero level below threshold. A
    // return to full silence would measure the EQ ring-down (~1 ms), not
    // the envelope, so the trajectory is read from 10 ms in: the EQ has
    // re-settled (~5 ms) while the blend is still saturated (the envelope
    // desaturates at ~27 ms), making the reference the full-blend constant.
    let quiet_level = 0.05_f32;
    assert!(20.0 * f64::from(quiet_level).log10() < -24.0);
    let mut release_deviation = Vec::new();
    for _ in 0..(sample_rate as usize) {
        let mut frame = vec![quiet_level];
        plugin
            .process_in_place(&mut frame, &ProcessContext::new(sample_rate, 1))
            .unwrap();
        release_deviation.push(f64::from(frame[0]) - f64::from(quiet_level));
    }
    assert!(release_deviation.iter().all(|value| value.is_finite()));
    let search_start = sample_rate as usize / 100;
    let reference = release_deviation[search_start];
    let expected_reference = f64::from(quiet_level) * (full_amplitude - 1.0);
    assert!(
        (reference - expected_reference).abs() / expected_reference < 0.05,
        "release reference {reference}, predicted {expected_reference}"
    );
    for pair in release_deviation[search_start..].windows(2) {
        assert!(
            pair[1] <= pair[0] + 1.0e-5,
            "release audio deviation must fall monotonically"
        );
    }
    assert!(
        release_deviation.last().unwrap().abs() < 0.01,
        "release must settle near zero: {}",
        release_deviation.last().unwrap()
    );
    let release_offset = release_deviation[search_start..]
        .iter()
        .position(|value| *value <= reference * 0.368)
        .unwrap();
    let release_37_ms = (search_start + release_offset) as f32 / sample_rate as f32 * 1000.0;
    assert!(
        (release_37_ms - release_ms).abs() / release_ms < 0.25,
        "release 37% audio at {release_37_ms} ms, configured {release_ms} ms"
    );
}
