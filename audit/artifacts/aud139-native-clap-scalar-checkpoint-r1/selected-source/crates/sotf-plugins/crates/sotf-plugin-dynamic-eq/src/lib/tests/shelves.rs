use super::super::dyn_eq_band::{design_shelf_coefficients, DynEqBand};
use super::super::dyn_eq_band_params::DynEqBandParams;
use super::super::dynamic_eq_plugin::DynamicEqPlugin;
use super::super::dynamic_eq_plugin_params::DynamicEqPluginParams;
use super::super::params::DynEqShape;
use sotf_host::parameters::{ParameterId, ParameterValue};
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

/// Independent continuous-time shelf prototype followed by frequency
/// prewarping and the bilinear substitution. This does not reuse the direct
/// digital coefficient equations under test.
fn analog_prewarped_response(
    shape: DynEqShape,
    target_gain_db: f64,
    slope: f64,
    corner: f64,
    frequency: f64,
    sample_rate: f64,
) -> Complex {
    let amplitude = 10.0_f64.powf(target_gain_db / 40.0);
    let inverse_q = ((amplitude + amplitude.recip()) * (slope.recip() - 1.0) + 2.0).sqrt();
    let damping = amplitude.sqrt() * inverse_q;
    let prewarped_frequency = (std::f64::consts::PI * frequency / sample_rate).tan()
        / (std::f64::consts::PI * corner / sample_rate).tan();
    let s = Complex::new(0.0, prewarped_frequency);
    let s_squared = s.mul(s);
    let one = Complex::new(1.0, 0.0);
    let constant = Complex::new(amplitude, 0.0);
    let linear = s.scale(damping);

    let (numerator, denominator) = match shape {
        DynEqShape::LowShelf => (
            s_squared.add(linear).add(constant),
            s_squared.scale(amplitude).add(linear).add(one),
        ),
        DynEqShape::HighShelf => (
            s_squared.scale(amplitude).add(linear).add(one),
            s_squared.add(linear).add(constant),
        ),
        DynEqShape::Peak => panic!("Peak has no shelf prototype"),
    };
    numerator.div(denominator).scale(amplitude)
}

fn assert_close(actual: Complex, expected: Complex, context: &str) {
    let error = actual.sub(expected).magnitude();
    let response_floor = 0.05;
    let relative_error = error / expected.magnitude().max(response_floor);
    assert!(
        relative_error <= 1.0e-6,
        "{context}: relative complex error {relative_error:e}; actual={actual:?}, expected={expected:?}"
    );
}

fn stable_poles(coefficients: &math_audio_iir_fir::BiquadCoefficients<f64>) -> bool {
    let discriminant = coefficients.a1 * coefficients.a1 - 4.0 * coefficients.a2;
    if discriminant >= 0.0 {
        let root = discriminant.sqrt();
        let pole_a = (-coefficients.a1 + root) * 0.5;
        let pole_b = (-coefficients.a1 - root) * 0.5;
        pole_a.abs() < 1.0 && pole_b.abs() < 1.0
    } else {
        coefficients.a2 > 0.0 && coefficients.a2.sqrt() < 1.0
    }
}

#[test]
fn shelf_coefficients_match_independent_prewarped_analog_reference() {
    let sample_rates: [f64; 3] = [44_100.0, 48_000.0, 96_000.0];
    let gains = [-24.0, -12.0, -6.0, 0.0, 6.0, 12.0, 24.0];
    let slopes = [0.1, 0.25, 0.5, 1.0];

    for sample_rate in sample_rates {
        let maximum_corner = (sample_rate * 0.475).min(20_000.0);
        let corners = [20.0, 997.0_f64.min(maximum_corner), maximum_corner];
        for corner in corners {
            for target_gain_db in gains {
                for slope in slopes {
                    for shape in [DynEqShape::LowShelf, DynEqShape::HighShelf] {
                        let coefficients = design_shelf_coefficients(
                            shape,
                            corner,
                            sample_rate,
                            target_gain_db,
                            slope,
                        )
                        .unwrap_or_else(|| {
                            panic!(
                                "coefficient design rejected {shape:?}, Fs={sample_rate}, f0={corner}, G={target_gain_db}, S={slope}"
                            )
                        });
                        assert!(stable_poles(&coefficients));

                        for index in 1..=256 {
                            let frequency =
                                0.1 * ((sample_rate * 0.499 / 0.1).powf(index as f64 / 256.0));
                            let actual = digital_response(&coefficients, frequency, sample_rate);
                            let expected = analog_prewarped_response(
                                shape,
                                target_gain_db,
                                slope,
                                corner,
                                frequency,
                                sample_rate,
                            );
                            assert!(actual.re.is_finite() && actual.im.is_finite());
                            assert!(expected.re.is_finite() && expected.im.is_finite());
                            assert_close(
                                actual,
                                expected,
                                &format!(
                                    "{shape:?}, Fs={sample_rate}, f0={corner}, G={target_gain_db}, S={slope}, f={frequency}"
                                ),
                            );
                        }

                        let dc = digital_response(&coefficients, 0.0, sample_rate).magnitude();
                        let nyquist =
                            digital_response(&coefficients, sample_rate * 0.5, sample_rate)
                                .magnitude();
                        let midpoint =
                            digital_response(&coefficients, corner, sample_rate).magnitude();
                        let target_amplitude = 10.0_f64.powf(target_gain_db / 20.0);
                        let midpoint_amplitude = 10.0_f64.powf(target_gain_db / 40.0);
                        let (expected_dc, expected_nyquist) = match shape {
                            DynEqShape::LowShelf => (target_amplitude, 1.0),
                            DynEqShape::HighShelf => (1.0, target_amplitude),
                            DynEqShape::Peak => unreachable!(),
                        };
                        assert!((dc - expected_dc).abs() < 1.0e-7);
                        assert!((nyquist - expected_nyquist).abs() < 1.0e-7);
                        assert!((midpoint - midpoint_amplitude).abs() < 1.0e-7);

                        if target_gain_db != 0.0 {
                            let mut previous_db = 20.0
                                * digital_response(&coefficients, 0.1, sample_rate)
                                    .magnitude()
                                    .log10();
                            let gain_direction = target_gain_db.signum();
                            let side_direction = match shape {
                                DynEqShape::LowShelf => -1.0,
                                DynEqShape::HighShelf => 1.0,
                                DynEqShape::Peak => unreachable!(),
                            };
                            for index in 1..=1024 {
                                let frequency =
                                    0.1 * ((sample_rate * 0.499 / 0.1).powf(index as f64 / 1024.0));
                                let current_db = 20.0
                                    * digital_response(&coefficients, frequency, sample_rate)
                                        .magnitude()
                                        .log10();
                                let directed_step =
                                    (current_db - previous_db) * gain_direction * side_direction;
                                assert!(
                                    directed_step >= -2.0e-4,
                                    "non-monotonic {shape:?} response at Fs={sample_rate}, f0={corner}, G={target_gain_db}, S={slope}, f={frequency}: step={directed_step} dB"
                                );
                                previous_db = current_db;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn partial_shelf_gain_uses_amplitude_blend_not_a_redesigned_shelf() {
    for (shape, full_gain, applied_gain, expected_midpoint_db) in [
        (DynEqShape::LowShelf, 12.0, 6.0, 1.528_974_684_1),
        (DynEqShape::HighShelf, -12.0, -6.0, -4.471_025_315_9),
    ] {
        let full = design_shelf_coefficients(shape, 1_000.0, 48_000.0, full_gain, 1.0).unwrap();
        let desired_amplitude = 10.0_f64.powf(applied_gain / 20.0);
        let full_amplitude = 10.0_f64.powf(full_gain / 20.0);
        let proportion = (desired_amplitude - 1.0) / (full_amplitude - 1.0);
        let full_midpoint = digital_response(&full, 1_000.0, 48_000.0);
        let blended_midpoint =
            Complex::new(1.0, 0.0).add(full_midpoint.sub(Complex::new(1.0, 0.0)).scale(proportion));
        let blended_db = 20.0 * blended_midpoint.magnitude().log10();
        assert!((blended_db - expected_midpoint_db).abs() < 1.0e-8);

        let redesigned =
            design_shelf_coefficients(shape, 1_000.0, 48_000.0, applied_gain, 1.0).unwrap();
        let redesigned_db = 20.0
            * digital_response(&redesigned, 1_000.0, 48_000.0)
                .magnitude()
                .log10();
        assert!((redesigned_db - applied_gain * 0.5).abs() < 1.0e-7);
        assert!((blended_db - redesigned_db).abs() > 1.0);
    }
}

#[test]
fn old_band_state_defaults_to_peak_with_unity_slope() {
    let old_json = r#"{
        "frequency": 1250.0,
        "q": 1.25,
        "gain": -8.0,
        "band_threshold": -31.0,
        "band_ratio": 4.0,
        "active": true,
        "solo": false
    }"#;
    let band: DynEqBandParams = serde_json::from_str(old_json).unwrap();
    assert_eq!(band.shape, DynEqShape::Peak);
    assert_eq!(band.shelf_slope, 1.0);
    assert_eq!(band.frequency, 1_250.0);
    assert_eq!(band.q, 1.25);
    assert_eq!(band.gain, -8.0);
}

fn analog_prewarped_coefficients(
    shape: DynEqShape,
    target_gain_db: f64,
    slope: f64,
    corner: f64,
    sample_rate: f64,
) -> math_audio_iir_fir::BiquadCoefficients<f64> {
    let amplitude = 10.0_f64.powf(target_gain_db / 40.0);
    let inverse_q = ((amplitude + amplitude.recip()) * (slope.recip() - 1.0) + 2.0).sqrt();
    let damping = amplitude.sqrt() * inverse_q;
    let k = 1.0 / (std::f64::consts::PI * corner / sample_rate).tan();
    let (b0, b1, b2, a0, a1, a2) = match shape {
        DynEqShape::LowShelf => (
            amplitude * (k * k + damping * k + amplitude),
            amplitude * (-2.0 * k * k + 2.0 * amplitude),
            amplitude * (k * k - damping * k + amplitude),
            amplitude * k * k + damping * k + 1.0,
            -2.0 * amplitude * k * k + 2.0,
            amplitude * k * k - damping * k + 1.0,
        ),
        DynEqShape::HighShelf => (
            amplitude * (amplitude * k * k + damping * k + 1.0),
            amplitude * (-2.0 * amplitude * k * k + 2.0),
            amplitude * (amplitude * k * k - damping * k + 1.0),
            k * k + damping * k + amplitude,
            -2.0 * k * k + 2.0 * amplitude,
            k * k - damping * k + amplitude,
        ),
        DynEqShape::Peak => panic!("Peak has no shelf prototype"),
    };
    math_audio_iir_fir::BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
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

fn butterworth_detector_coefficients(
    shape: DynEqShape,
    corner: f64,
    sample_rate: f64,
) -> math_audio_iir_fir::BiquadCoefficients<f64> {
    let omega = 2.0 * std::f64::consts::PI * corner / sample_rate;
    let cosine = omega.cos();
    let alpha = omega.sin() * std::f64::consts::FRAC_1_SQRT_2;
    let (b0, b1, b2) = match shape {
        DynEqShape::LowShelf => ((1.0 - cosine) * 0.5, 1.0 - cosine, (1.0 - cosine) * 0.5),
        DynEqShape::HighShelf => ((1.0 + cosine) * 0.5, -(1.0 + cosine), (1.0 + cosine) * 0.5),
        DynEqShape::Peak => panic!("Peak has no single shelf detector"),
    };
    let a0 = 1.0 + alpha;
    math_audio_iir_fir::BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: -2.0 * cosine / a0,
        a2: (1.0 - alpha) / a0,
    }
}

#[derive(Clone, Copy)]
struct ReferenceDynamics {
    shape: DynEqShape,
    corner: f64,
    slope: f64,
    target_gain_db: f64,
    threshold_db: f64,
    ratio: f64,
    knee_db: f64,
    attack_ms: f64,
    release_ms: f64,
    linked: bool,
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

fn independent_dynamic_reference(
    input: &[f32],
    channels: usize,
    sample_rate: u32,
    dynamics: ReferenceDynamics,
) -> (Vec<f32>, Vec<f64>) {
    let detector_coefficients =
        butterworth_detector_coefficients(dynamics.shape, dynamics.corner, sample_rate as f64);
    let eq_coefficients = analog_prewarped_coefficients(
        dynamics.shape,
        dynamics.target_gain_db,
        dynamics.slope,
        dynamics.corner,
        sample_rate as f64,
    );
    let mut detector = vec![DirectFormState::default(); channels];
    let mut eq = vec![DirectFormState::default(); channels];
    let core_count = if dynamics.linked { 1 } else { channels };
    let mut envelope = vec![0.0_f64; core_count];
    let attack_coefficient = (-1.0 / (dynamics.attack_ms * 0.001 * sample_rate as f64)).exp();
    let release_coefficient = (-1.0 / (dynamics.release_ms * 0.001 * sample_rate as f64)).exp();
    let mut output = vec![0.0_f32; input.len()];
    let mut proportions = Vec::with_capacity(input.len() / channels);

    let mut per_channel_proportion = vec![0.0_f64; channels];
    for frame in 0..input.len() / channels {
        let mut detected = vec![0.0_f64; channels];
        for channel in 0..channels {
            detected[channel] = detector[channel]
                .process(
                    &detector_coefficients,
                    input[frame * channels + channel] as f64,
                )
                .abs();
        }

        let mut frame_proportion = 0.0_f64;
        if dynamics.linked {
            let level = detected.iter().copied().fold(0.0_f64, f64::max);
            let level_db = 20.0 * level.max(1.0e-10).log10();
            let target = reference_compression_gain_reduction(
                level_db,
                dynamics.threshold_db,
                dynamics.ratio,
                dynamics.knee_db,
            );
            let coefficient = if target > envelope[0] {
                attack_coefficient
            } else {
                release_coefficient
            };
            envelope[0] = target + coefficient * (envelope[0] - target);
            let applied_gain = envelope[0].clamp(0.0, dynamics.target_gain_db.abs())
                * dynamics.target_gain_db.signum();
            let full_amplitude = 10.0_f64.powf(dynamics.target_gain_db / 20.0);
            let desired_amplitude = 10.0_f64.powf(applied_gain / 20.0);
            let proportion = if dynamics.target_gain_db.abs() < 0.01 {
                0.0
            } else {
                ((desired_amplitude - 1.0) / (full_amplitude - 1.0)).clamp(0.0, 1.0)
            };
            frame_proportion = frame_proportion.max(proportion);
            per_channel_proportion.fill(proportion);
        } else {
            for channel in 0..channels {
                let level_db = 20.0 * detected[channel].max(1.0e-10).log10();
                let target = reference_compression_gain_reduction(
                    level_db,
                    dynamics.threshold_db,
                    dynamics.ratio,
                    dynamics.knee_db,
                );
                let coefficient = if target > envelope[channel] {
                    attack_coefficient
                } else {
                    release_coefficient
                };
                envelope[channel] = target + coefficient * (envelope[channel] - target);
                let applied_gain = envelope[channel].clamp(0.0, dynamics.target_gain_db.abs())
                    * dynamics.target_gain_db.signum();
                let full_amplitude = 10.0_f64.powf(dynamics.target_gain_db / 20.0);
                let desired_amplitude = 10.0_f64.powf(applied_gain / 20.0);
                let proportion = if dynamics.target_gain_db.abs() < 0.01 {
                    0.0
                } else {
                    ((desired_amplitude - 1.0) / (full_amplitude - 1.0)).clamp(0.0, 1.0)
                };
                frame_proportion = frame_proportion.max(proportion);
                per_channel_proportion[channel] = proportion;
            }
        }

        for channel in 0..channels {
            let index = frame * channels + channel;
            let dry = input[index];
            let equalized = eq[channel].process(&eq_coefficients, dry as f64) as f32;
            output[index] = dry + (equalized - dry) * per_channel_proportion[channel] as f32;
        }
        proportions.push(frame_proportion);
    }

    (output, proportions)
}

#[allow(clippy::too_many_arguments)]
fn make_dynamic_shelf(
    channels: usize,
    linked: bool,
    shape: DynEqShape,
    target_gain_db: f32,
    threshold_db: f32,
    ratio: f32,
    knee_db: f32,
    attack_ms: f32,
    release_ms: f32,
) -> DynamicEqPlugin {
    DynamicEqPlugin::try_from_params_at_sample_rate(
        channels,
        DynamicEqPluginParams {
            num_bands: 1,
            threshold: threshold_db,
            ratio,
            attack_ms,
            release_ms,
            knee: knee_db,
            link_channels: linked,
            mix: 1.0,
            bands: vec![DynEqBandParams {
                shape,
                shelf_slope: 0.5,
                frequency: 1_000.0,
                gain: target_gain_db,
                band_threshold: threshold_db,
                band_ratio: ratio,
                ..DynEqBandParams::default()
            }],
        },
        48_000,
    )
    .unwrap()
}

fn make_mixed_shape_plugin(linked: bool) -> DynamicEqPlugin {
    make_mixed_shape_plugin_with_mix(linked, 1.0)
}

fn make_mixed_shape_plugin_with_mix(linked: bool, mix: f32) -> DynamicEqPlugin {
    make_mixed_shape_plugin_with_controls(linked, mix, [true; 3], [false; 3])
}

fn make_mixed_shape_plugin_with_controls(
    linked: bool,
    mix: f32,
    active: [bool; 3],
    solo: [bool; 3],
) -> DynamicEqPlugin {
    let mut plugin = DynamicEqPlugin::try_from_params_at_sample_rate(
        2,
        DynamicEqPluginParams {
            num_bands: 3,
            threshold: -35.0,
            ratio: 4.0,
            attack_ms: 2.0,
            release_ms: 80.0,
            knee: 4.0,
            link_channels: linked,
            mix,
            bands: vec![
                DynEqBandParams {
                    shape: DynEqShape::Peak,
                    frequency: 1_000.0,
                    q: 1.2,
                    gain: 8.0,
                    active: active[0],
                    solo: solo[0],
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    shape: DynEqShape::LowShelf,
                    frequency: 250.0,
                    gain: 6.0,
                    shelf_slope: 0.55,
                    active: active[1],
                    solo: solo[1],
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    shape: DynEqShape::HighShelf,
                    frequency: 3_000.0,
                    gain: -9.0,
                    shelf_slope: 0.8,
                    active: active[2],
                    solo: solo[2],
                    ..DynEqBandParams::default()
                },
            ],
        },
        48_000,
    )
    .unwrap();

    for (band, threshold, ratio) in [(0, -34.0, 3.0), (1, -30.0, 6.0), (2, -32.0, 5.0)] {
        plugin
            .set_parameter(
                ParameterId::from(format!("band_{band}_threshold")),
                ParameterValue::Float(threshold),
            )
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from(format!("band_{band}_ratio")),
                ParameterValue::Float(ratio),
            )
            .unwrap();
        assert!(plugin.bands[band].use_band_threshold);
        assert!(plugin.bands[band].use_band_ratio);
    }
    plugin
}

fn mixed_shape_stereo_input(frames: usize) -> Vec<f32> {
    let sample_rate = 48_000.0_f64;
    let mut input = vec![0.0_f32; frames * 2];
    for frame in 0..frames {
        let stage_amplitude = if frame < 4_000 {
            0.04
        } else if frame < 10_000 {
            0.8
        } else {
            0.25
        };
        let (left_scale, right_scale) = if frame >= 14_000 || (frame / 2_000) % 2 == 0 {
            (1.0, 0.12)
        } else {
            (0.12, 1.0)
        };
        let time = frame as f64 / sample_rate;
        let low = (2.0 * std::f64::consts::PI * 100.0 * time).sin();
        let peak = (2.0 * std::f64::consts::PI * 1_000.0 * time).sin();
        let high = (2.0 * std::f64::consts::PI * 8_000.0 * time).sin();
        let dry = (0.45 * low + 0.25 * peak + 0.3 * high) * stage_amplitude;
        input[frame * 2] = (dry * left_scale) as f32;
        input[frame * 2 + 1] = (dry * right_scale) as f32;
    }
    input
}

#[test]
fn public_process_matches_independent_linked_and_unlinked_shelf_dynamics() {
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
    assert_eq!(input.len(), frames * channels);
    assert!(input.iter().all(|sample| sample.is_finite()));

    for shape in [DynEqShape::LowShelf, DynEqShape::HighShelf] {
        for target_gain_db in [-12.0, 12.0] {
            for linked in [true, false] {
                let dynamics = ReferenceDynamics {
                    shape,
                    corner: 1_000.0,
                    slope: 0.5,
                    target_gain_db,
                    threshold_db: -30.0,
                    ratio: 4.0,
                    knee_db: 6.0,
                    attack_ms: 3.0,
                    release_ms: 80.0,
                    linked,
                };
                let (expected, proportions) =
                    independent_dynamic_reference(&input, channels, sample_rate, dynamics);
                assert_eq!(expected.len(), input.len());
                assert_eq!(proportions.len(), frames);
                assert!(expected.iter().all(|sample| sample.is_finite()));
                assert!(proportions.iter().all(|proportion| proportion.is_finite()));
                assert!(proportions.iter().any(|&p| (0.05..0.95).contains(&p)));
                assert!(proportions.iter().any(|&p| p > 0.99));

                let mut actual = input.clone();
                let mut plugin = make_dynamic_shelf(
                    channels,
                    linked,
                    dynamics.shape,
                    dynamics.target_gain_db as f32,
                    dynamics.threshold_db as f32,
                    dynamics.ratio as f32,
                    dynamics.knee_db as f32,
                    dynamics.attack_ms as f32,
                    dynamics.release_ms as f32,
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
                    "{shape:?}, gain={target_gain_db}, linked={linked}: input-normalized full-vector residual {} exceeds 0.002",
                    residual_rms / input_rms
                );

                let mut partitioned = input.clone();
                let mut partitioned_plugin = make_dynamic_shelf(
                    channels,
                    linked,
                    dynamics.shape,
                    dynamics.target_gain_db as f32,
                    dynamics.threshold_db as f32,
                    dynamics.ratio as f32,
                    dynamics.knee_db as f32,
                    dynamics.attack_ms as f32,
                    dynamics.release_ms as f32,
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
                assert_eq!(partitioned.len(), input.len());
                let partition_error = (actual
                    .iter()
                    .zip(partitioned.iter())
                    .map(|(left, right)| (f64::from(*left) - f64::from(*right)).powi(2))
                    .sum::<f64>()
                    / actual.len() as f64)
                    .sqrt();
                assert!(
                    partition_error / input_rms <= 1.0e-7,
                    "{shape:?}, gain={target_gain_db}, linked={linked}: absolute-time partition error {} exceeds 1e-7",
                    partition_error / input_rms
                );
            }
        }
    }
}

#[test]
fn public_mixed_peak_and_shelf_bands_keep_serial_audio_and_dry_detection() {
    let sample_rate = 48_000_u32;
    let frames = 16_000;
    let input = mixed_shape_stereo_input(frames);
    assert_eq!(input.len(), frames * 2);
    assert!(input.iter().all(|sample| sample.is_finite()));

    for linked in [true, false] {
        let mut plugin = make_mixed_shape_plugin(linked);
        let mut actual = input.clone();
        plugin
            .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert_eq!(actual.len(), input.len());
        assert!(actual.iter().all(|sample| sample.is_finite()));
        assert!(plugin.monitoring_gr[..3]
            .iter()
            .all(|value| value.is_finite()));
        assert!(
            plugin.monitoring_gr[..3].iter().all(|value| *value > 0.1),
            "linked={linked}: mixed Peak/LowShelf/HighShelf overrides did not all engage: {:?}",
            &plugin.monitoring_gr[..3]
        );
        assert!(
            actual
                .iter()
                .zip(&input)
                .any(|(processed, dry)| (processed - dry).abs() > 1.0e-3),
            "linked={linked}: mixed audible chain had no effect"
        );
    }

    let mut solo_chain =
        make_mixed_shape_plugin_with_controls(true, 1.0, [true; 3], [false, true, false]);
    let mut solo_output = input.clone();
    solo_chain
        .process_in_place(&mut solo_output, &ProcessContext::new(sample_rate, frames))
        .unwrap();

    let mut only_solo_band =
        make_mixed_shape_plugin_with_controls(true, 1.0, [false, true, false], [false; 3]);
    let mut solo_reference = input.clone();
    only_solo_band
        .process_in_place(
            &mut solo_reference,
            &ProcessContext::new(sample_rate, frames),
        )
        .unwrap();
    assert!(solo_output.iter().all(|sample| sample.is_finite()));
    assert_eq!(
        solo_output, solo_reference,
        "solo must isolate one audible band"
    );

    // Preserve the legacy Peak rule: an inactive solo flag still causes the
    // active non-solo Peak band to be skipped because any_solo includes it.
    let mut inactive_solo_peak = DynamicEqPlugin::try_from_params_at_sample_rate(
        1,
        DynamicEqPluginParams {
            num_bands: 2,
            threshold: -40.0,
            ratio: 4.0,
            mix: 1.0,
            bands: vec![
                DynEqBandParams {
                    frequency: 1_000.0,
                    gain: 12.0,
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    frequency: 2_000.0,
                    gain: 12.0,
                    active: false,
                    solo: true,
                    ..DynEqBandParams::default()
                },
            ],
            ..DynamicEqPluginParams::default()
        },
        sample_rate,
    )
    .unwrap();
    let peak_frames = 4_096;
    let mut peak_audio = (0..peak_frames)
        .map(|frame| {
            (0.6_f64
                * (2.0 * std::f64::consts::PI * 1_000.0 * frame as f64 / sample_rate as f64).sin())
                as f32
        })
        .collect::<Vec<_>>();
    let peak_dry = peak_audio.clone();
    inactive_solo_peak
        .process_in_place(
            &mut peak_audio,
            &ProcessContext::new(sample_rate, peak_frames),
        )
        .unwrap();
    assert_eq!(peak_audio, peak_dry, "inactive solo Peak behavior changed");

    // Band 0's Peak filter audibly raises a 1 kHz tone above band 1's
    // threshold. Band 1's high-shelf detector must still see the original dry
    // tone, which remains below that threshold.
    let isolation_frames = 8_192;
    let mut isolation = DynamicEqPlugin::try_from_params_at_sample_rate(
        1,
        DynamicEqPluginParams {
            num_bands: 2,
            threshold: -20.0,
            ratio: 4.0,
            attack_ms: 0.1,
            release_ms: 40.0,
            knee: 0.0,
            link_channels: false,
            mix: 1.0,
            bands: vec![
                DynEqBandParams {
                    shape: DynEqShape::Peak,
                    frequency: 1_000.0,
                    q: 1.0,
                    gain: 12.0,
                    ..DynEqBandParams::default()
                },
                DynEqBandParams {
                    shape: DynEqShape::HighShelf,
                    frequency: 500.0,
                    gain: 12.0,
                    shelf_slope: 0.7,
                    ..DynEqBandParams::default()
                },
            ],
        },
        sample_rate,
    )
    .unwrap();
    isolation
        .set_parameter(
            ParameterId::from("band_0_threshold"),
            ParameterValue::Float(-35.0),
        )
        .unwrap();
    isolation
        .set_parameter(
            ParameterId::from("band_0_ratio"),
            ParameterValue::Float(20.0),
        )
        .unwrap();
    isolation
        .set_parameter(
            ParameterId::from("band_1_threshold"),
            ParameterValue::Float(-15.0),
        )
        .unwrap();
    isolation
        .set_parameter(
            ParameterId::from("band_1_ratio"),
            ParameterValue::Float(4.0),
        )
        .unwrap();
    let mut isolation_audio = (0..isolation_frames)
        .map(|frame| {
            (0.08_f64
                * (2.0 * std::f64::consts::PI * 1_000.0 * frame as f64 / sample_rate as f64).sin())
                as f32
        })
        .collect::<Vec<_>>();
    let isolation_input = isolation_audio.clone();
    isolation
        .process_in_place(
            &mut isolation_audio,
            &ProcessContext::new(sample_rate, isolation_frames),
        )
        .unwrap();
    assert_eq!(isolation_audio.len(), isolation_input.len());
    assert!(isolation_audio.iter().all(|sample| sample.is_finite()));
    assert!(isolation.monitoring_gr[0] > 6.0);
    assert!(
        isolation.monitoring_gr[1] < 0.1,
        "later shelf detector was contaminated by earlier Peak audio: {:?}",
        &isolation.monitoring_gr[..2]
    );
    let input_rms = (isolation_input
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / isolation_input.len() as f64)
        .sqrt();
    let output_rms = (isolation_audio
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>()
        / isolation_audio.len() as f64)
        .sqrt();
    assert!(output_rms > input_rms * 1.5);
}

#[test]
fn populated_mixed_shelf_reset_and_dry_to_wet_epochs_match_fresh_state() {
    let sample_rate = 48_000_u32;
    let frames = 8_192;
    let input = mixed_shape_stereo_input(frames);
    for linked in [true, false] {
        let mut populated = make_mixed_shape_plugin(linked);
        let mut warmup = input.clone();
        populated
            .process_in_place(&mut warmup, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(warmup.iter().all(|sample| sample.is_finite()));
        assert!(warmup
            .iter()
            .zip(&input)
            .any(|(wet, dry)| (wet - dry).abs() > 1.0e-3));

        populated.reset();
        let mut after_reset = input.clone();
        populated
            .process_in_place(&mut after_reset, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        let mut fresh = make_mixed_shape_plugin(linked);
        let mut fresh_output = input.clone();
        fresh
            .process_in_place(&mut fresh_output, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert_eq!(after_reset.len(), input.len());
        assert!(after_reset.iter().all(|sample| sample.is_finite()));
        assert_eq!(
            after_reset, fresh_output,
            "linked={linked}: reset differs from fresh"
        );

        let mut epochs = make_mixed_shape_plugin(linked);
        for epoch in 0..2 {
            epochs
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.0))
                .unwrap();
            let dry_block = mixed_shape_stereo_input(4_096);
            let mut settled_dry = dry_block.clone();
            epochs
                .process_in_place(&mut settled_dry, &ProcessContext::new(sample_rate, 4_096))
                .unwrap();
            assert_eq!(settled_dry.len(), dry_block.len());
            assert!(settled_dry.iter().all(|sample| sample.is_finite()));

            let mut dry_fast_path = dry_block.clone();
            epochs
                .process_in_place(&mut dry_fast_path, &ProcessContext::new(sample_rate, 4_096))
                .unwrap();
            assert_eq!(dry_fast_path, dry_block, "linked={linked}, epoch={epoch}");

            epochs
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
                .unwrap();
            let mut wet_after_dry = input.clone();
            epochs
                .process_in_place(
                    &mut wet_after_dry,
                    &ProcessContext::new(sample_rate, frames),
                )
                .unwrap();
            let mut fresh_wet = make_mixed_shape_plugin_with_mix(linked, 0.0);
            fresh_wet
                .set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
                .unwrap();
            let mut fresh_wet_output = input.clone();
            fresh_wet
                .process_in_place(
                    &mut fresh_wet_output,
                    &ProcessContext::new(sample_rate, frames),
                )
                .unwrap();
            assert_eq!(wet_after_dry.len(), input.len());
            assert!(wet_after_dry.iter().all(|sample| sample.is_finite()));
            assert_eq!(
                wet_after_dry, fresh_wet_output,
                "linked={linked}, epoch={epoch}: dry-to-wet reset differs from fresh"
            );
        }
    }
}

#[test]
fn held_shelf_proportions_match_independent_full_waveform_recurrence() {
    let sample_rate = 48_000_u32;
    let channels = 2;
    let frames = 4_096;
    let corner = 1_200.0_f64;
    let slope = 0.5_f64;
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

    for shape in [DynEqShape::LowShelf, DynEqShape::HighShelf] {
        for target_gain_db in [-12.0_f64, 12.0] {
            let coefficients = analog_prewarped_coefficients(
                shape,
                target_gain_db,
                slope,
                corner,
                sample_rate as f64,
            );
            for proportion in proportions {
                let mut band = DynEqBand::new(
                    channels,
                    sample_rate,
                    corner as f32,
                    0.707,
                    target_gain_db as f32,
                    3.0,
                    80.0,
                );
                band.shape = shape;
                band.shelf_slope = slope as f32;
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
                        expected[index] = (f64::from(dry)
                            + (independent_eq - f64::from(dry)) * proportion)
                            as f32;
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
                    "{shape:?} G={target_gain_db} p={proportion}: peak error {peak_error}"
                );
                assert!(
                    rms_error <= 1.0e-6,
                    "{shape:?} G={target_gain_db} p={proportion}: RMS error {rms_error}"
                );
            }
        }
    }
}

#[test]
fn shape_specific_detector_passes_its_side_and_rejects_the_other_side() {
    let sample_rate = 48_000_u32;
    let frames = 12_000;
    for (shape, signal_frequency, should_trigger) in [
        (DynEqShape::LowShelf, 100.0, true),
        (DynEqShape::LowShelf, 8_000.0, false),
        (DynEqShape::HighShelf, 8_000.0, true),
        (DynEqShape::HighShelf, 100.0, false),
    ] {
        let mut plugin = make_dynamic_shelf(1, false, shape, 12.0, -25.0, 20.0, 0.0, 0.5, 40.0);
        let mut audio = (0..frames)
            .map(|frame| {
                (0.8_f64
                    * (2.0 * std::f64::consts::PI * signal_frequency * frame as f64
                        / sample_rate as f64)
                        .sin()) as f32
            })
            .collect::<Vec<_>>();
        plugin
            .process_in_place(&mut audio, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(audio.iter().all(|sample| sample.is_finite()));
        let observed_reduction = plugin.monitoring_gr[0];
        if should_trigger {
            assert!(
                observed_reduction > 6.0,
                "{shape:?} detector did not pass {signal_frequency} Hz: {observed_reduction} dB"
            );
        } else {
            assert!(
                observed_reduction < 0.1,
                "{shape:?} detector leaked {signal_frequency} Hz: {observed_reduction} dB"
            );
        }
    }
}

#[test]
fn shelf_parameters_are_structural_and_zero_gain_boundary_is_preserved() {
    let mut plugin =
        make_dynamic_shelf(1, false, DynEqShape::Peak, 6.0, -30.0, 4.0, 0.0, 1.0, 40.0);
    assert!(plugin
        .set_parameter(ParameterId::from("band_0_shape"), ParameterValue::Int(1),)
        .is_err());
    assert_eq!(plugin.bands[0].shape, DynEqShape::Peak);
    assert_eq!(plugin.bands[0].shelf_slope, 0.5);
    assert!(plugin
        .set_parameter(ParameterId::from("band_0_shape"), ParameterValue::Int(3),)
        .is_err());
    assert!(plugin
        .set_parameter(
            ParameterId::from("band_0_shelf_slope"),
            ParameterValue::Float(0.5),
        )
        .is_err());
    assert_eq!(plugin.bands[0].shape, DynEqShape::Peak);
    assert_eq!(plugin.bands[0].shelf_slope, 0.5);

    let invalid = DynamicEqPluginParams {
        num_bands: 1,
        bands: vec![DynEqBandParams {
            shape: DynEqShape::LowShelf,
            shelf_slope: 0.099,
            ..DynEqBandParams::default()
        }],
        ..DynamicEqPluginParams::default()
    };
    assert!(DynamicEqPlugin::try_from_params_at_sample_rate(1, invalid, 48_000).is_err());

    assert_eq!(
        super::super::dyn_eq_band::DynEqBand::modulation_proportion(0.009, 10.0),
        0.0
    );
    assert_eq!(
        super::super::dyn_eq_band::DynEqBand::modulation_proportion(-0.009, 10.0),
        0.0
    );
    assert!(
        (super::super::dyn_eq_band::DynEqBand::modulation_proportion(0.01, 0.01) - 1.0).abs()
            < 1.0e-4
    );
    assert!(
        (super::super::dyn_eq_band::DynEqBand::modulation_proportion(0.011, 0.011) - 1.0).abs()
            < 1.0e-6
    );
}

#[test]
fn public_shelf_process_preserves_both_sides_of_the_zero_gain_boundary() {
    let sample_rate = 48_000_u32;
    let frames = 4_096;
    for shape in [DynEqShape::LowShelf, DynEqShape::HighShelf] {
        let frequency = match shape {
            DynEqShape::LowShelf => 100.0_f64,
            DynEqShape::HighShelf => 8_000.0_f64,
            DynEqShape::Peak => unreachable!(),
        };
        let input = (0..frames)
            .map(|frame| {
                (0.6_f64
                    * (2.0 * std::f64::consts::PI * frequency * frame as f64 / sample_rate as f64)
                        .sin()) as f32
            })
            .collect::<Vec<_>>();
        for target_gain_db in [-0.011_f32, -0.01, -0.009, 0.0, 0.009, 0.01, 0.011] {
            let mut plugin =
                make_dynamic_shelf(1, false, shape, target_gain_db, -60.0, 20.0, 0.0, 0.1, 40.0);
            let mut actual = input.clone();
            plugin
                .process_in_place(&mut actual, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            assert_eq!(actual.len(), input.len());
            assert!(actual.iter().all(|sample| sample.is_finite()));
            let maximum_difference = actual
                .iter()
                .zip(&input)
                .map(|(actual, input)| (*actual - *input).abs())
                .fold(0.0_f32, f32::max);
            if target_gain_db.abs() < 0.01 {
                assert_eq!(actual, input, "{shape:?} G={target_gain_db}");
            } else {
                assert!(
                    maximum_difference > 1.0e-5,
                    "{shape:?} G={target_gain_db}: public EQ remained dry"
                );
            }
        }
    }
}

fn make_reinitialization_multiband(sample_rate: u32) -> DynamicEqPlugin {
    DynamicEqPlugin::try_from_params_at_sample_rate(
        2,
        DynamicEqPluginParams {
            num_bands: 2,
            threshold: -36.0,
            ratio: 4.0,
            attack_ms: 2.0,
            release_ms: 60.0,
            knee: 4.0,
            link_channels: false,
            mix: 1.0,
            bands: vec![
                DynEqBandParams {
                    shape: DynEqShape::LowShelf,
                    shelf_slope: 0.5,
                    frequency: 10_000.0,
                    q: 1.0,
                    gain: 12.0,
                    band_threshold: -42.0,
                    band_ratio: 4.0,
                    active: true,
                    solo: false,
                },
                DynEqBandParams {
                    shape: DynEqShape::Peak,
                    frequency: 700.0,
                    q: 1.2,
                    gain: -5.0,
                    band_threshold: -36.0,
                    band_ratio: 3.0,
                    active: true,
                    solo: false,
                    ..DynEqBandParams::default()
                },
            ],
        },
        sample_rate,
    )
    .unwrap()
}

fn reinitialization_signal(start_frame: usize, frames: usize, sample_rate: u32) -> Vec<f32> {
    let mut signal = Vec::with_capacity(frames * 2);
    for frame in start_frame..start_frame + frames {
        let low_phase = 2.0 * std::f32::consts::PI * 700.0 * frame as f32 / sample_rate as f32;
        let high_phase = 2.0 * std::f32::consts::PI * 12_000.0 * frame as f32 / sample_rate as f32;
        let low = 0.21 * low_phase.sin();
        let high = 0.31 * high_phase.sin();
        signal.push(low + high);
        signal.push(0.55 * low - 0.35 * high);
    }
    signal
}

#[test]
fn invalid_shelf_reinitialize_preserves_populated_state_and_allows_valid_retry() {
    let original_rate = 48_000;
    let invalid_rate = 8_000;
    let mut candidate = make_reinitialization_multiband(original_rate);
    let mut twin = make_reinitialization_multiband(original_rate);

    let mut candidate_prefix = reinitialization_signal(0, 4_096, original_rate);
    let mut twin_prefix = candidate_prefix.clone();
    let prefix_frames = candidate_prefix.len() / 2;
    candidate
        .process_in_place(
            &mut candidate_prefix,
            &ProcessContext::new(original_rate, prefix_frames),
        )
        .unwrap();
    twin.process_in_place(
        &mut twin_prefix,
        &ProcessContext::new(original_rate, prefix_frames),
    )
    .unwrap();
    assert!(candidate_prefix.iter().all(|sample| sample.is_finite()));
    assert!(candidate_prefix.iter().any(|sample| sample.abs() > 1.0e-5));
    assert_eq!(candidate_prefix, twin_prefix);

    let values_before = candidate.current_values();
    let frequency_before = candidate.bands[0].frequency;
    let rate_before = candidate.sample_rate;
    let result = candidate.initialize(invalid_rate);
    assert!(
        result.is_err(),
        "10 kHz shelf at {original_rate} Hz should reject {invalid_rate} Hz reinitialize; result={result:?}, committed rate={} Hz, shelf cutoff={} Hz",
        candidate.sample_rate,
        candidate.bands[0].frequency
    );
    assert_eq!(candidate.sample_rate, rate_before);
    assert_eq!(candidate.bands[0].frequency, frequency_before);
    assert_eq!(candidate.current_values(), values_before);

    let mut candidate_suffix = reinitialization_signal(4_096, 2_048, original_rate);
    let mut twin_suffix = candidate_suffix.clone();
    let suffix_frames = candidate_suffix.len() / 2;
    candidate
        .process_in_place(
            &mut candidate_suffix,
            &ProcessContext::new(original_rate, suffix_frames),
        )
        .unwrap();
    twin.process_in_place(
        &mut twin_suffix,
        &ProcessContext::new(original_rate, suffix_frames),
    )
    .unwrap();
    assert!(candidate_suffix.iter().all(|sample| sample.is_finite()));
    assert_eq!(candidate_suffix, twin_suffix);

    let retry_rate = 44_100;
    candidate.initialize(retry_rate).unwrap();
    let mut fresh = make_reinitialization_multiband(retry_rate);
    let mut candidate_retry = reinitialization_signal(0, 2_048, retry_rate);
    let mut fresh_retry = candidate_retry.clone();
    let retry_frames = candidate_retry.len() / 2;
    candidate
        .process_in_place(
            &mut candidate_retry,
            &ProcessContext::new(retry_rate, retry_frames),
        )
        .unwrap();
    fresh
        .process_in_place(
            &mut fresh_retry,
            &ProcessContext::new(retry_rate, retry_frames),
        )
        .unwrap();
    assert!(candidate_retry.iter().all(|sample| sample.is_finite()));
    assert_eq!(candidate_retry, fresh_retry);
}
