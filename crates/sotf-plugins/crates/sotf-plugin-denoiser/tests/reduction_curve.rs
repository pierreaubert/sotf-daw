//! Editable frequency-dependent reduction curve (DENOISER-R1).
//!
//! Predeclared bounds used below:
//! - Log-midpoint interpolation: 1e-6 absolute (f64 log arithmetic).
//! - Curve-zero bypass versus the delayed-input oracle: 2e-6 absolute,
//!   the accepted finite-stream bound from audit/denoiser-hiss-finite-stream.md.
//! - Released-band preservation: at least +6 dB versus the flat curve;
//!   unaffected bands agree within 3 dB. White-noise reduction at 40 dB
//!   depth exceeds 30 dB, so both margins are wide by construction.
//! - Configured twins and save/reload comparisons are bit-exact.

// Rust guideline compliant 2026-02-21
use serde_json::json;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{
    DENOISER_CURVE_ANCHOR_HZ, DenoiserPlugin, DenoiserPluginParams, ReductionCurve,
};

const RATE: u32 = 48_000;

/// Deterministic uniform noise in [-1, 1); no thread-local randomness.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / 2_147_483_648.0) * 2.0 - 1.0
    }
}

fn white_noise(frames: usize, channels: usize, seed: u64, amplitude: f32) -> Vec<f32> {
    let mut rng = Lcg(seed);
    (0..frames * channels)
        .map(|_| rng.next() * amplitude)
        .collect()
}

fn process_all(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
    rate: u32,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = blocks[call % blocks.len()].min(input.len() / channels - pos);
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[pos * channels..(pos + n) * channels],
                    &ProcessContext::new(rate, n)
                )
                .unwrap(),
            n
        );
        pos += n;
        call += 1;
    }
    output
}

/// Asserts sample-exact equality with a one-line report.
///
/// Same pass/fail semantics as `assert_eq!` on slices (IEEE `==`: `-0.0`
/// equals `0.0`, NaN mismatches), but a failure prints only the first
/// mismatch and the maximum difference instead of both full arrays.
fn assert_bit_exact(left: &[f32], right: &[f32], context: &str) {
    assert_eq!(
        left.len(),
        right.len(),
        "{context}: lengths {} vs {}",
        left.len(),
        right.len()
    );
    let mut first: Option<(usize, f32, f32)> = None;
    let mut max_diff = 0.0f32;
    for (i, (&a, &b)) in left.iter().zip(right.iter()).enumerate() {
        max_diff = max_diff.max((a - b).abs());
        if a != b && first.is_none() {
            first = Some((i, a, b));
        }
    }
    if let Some((i, a, b)) = first {
        panic!("{context}: first mismatch at {i} ({a} vs {b}), max diff {max_diff:e}");
    }
}

/// Hann-windowed generalized Goertzel magnitude squared at `freq_hz`.
fn goertzel_mag2(samples: &[f32], freq_hz: f64, sample_rate: f64) -> f64 {
    use std::f64::consts::PI;
    let n = samples.len();
    let omega = 2.0 * PI * freq_hz / sample_rate;
    let coeff = 2.0 * omega.cos();
    let mut s1 = 0.0;
    let mut s2 = 0.0;
    for (i, &x) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        let s0 = window * f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(1e-30)
}

#[test]
fn curve_unit_contract_anchors_interpolation_and_canonicalize() {
    assert_eq!(DENOISER_CURVE_ANCHOR_HZ, [125.0, 1000.0, 8000.0]);
    let curve = ReductionCurve {
        low: 0.0,
        mid: 0.5,
        high: 1.0,
    };
    // Anchors reproduce exactly; coordinates clamp outside them.
    assert_eq!(curve.gain_at(125.0), 0.0);
    assert_eq!(curve.gain_at(1000.0), 0.5);
    assert_eq!(curve.gain_at(8000.0), 1.0);
    assert_eq!(curve.gain_at(20.0), 0.0);
    assert_eq!(curve.gain_at(24_000.0), 1.0);
    assert_eq!(curve.gain_at(0.0), 0.0);
    assert_eq!(curve.gain_at(f32::NAN), 0.0);
    // Non-finite coordinates return the low knot (Hiss-mirrored contract:
    // unreachable in practice since bin frequencies are finite and >= 0).
    assert_eq!(curve.gain_at(f32::INFINITY), 0.0);
    assert_eq!(curve.gain_at(f32::NEG_INFINITY), 0.0);
    // Log-linear interpolation: the geometric mean maps to the midpoint.
    let low_mid = (125.0_f64 * 1000.0).sqrt() as f32;
    let mid_high = (1000.0_f64 * 8000.0).sqrt() as f32;
    assert!((curve.gain_at(low_mid) - 0.25).abs() < 1e-6);
    assert!((curve.gain_at(mid_high) - 0.75).abs() < 1e-6);
    // Canonicalization repairs non-finite knots to full reduction.
    assert_eq!(ReductionCurve::canonicalize(0.3), 0.3);
    assert_eq!(ReductionCurve::canonicalize(-0.5), 0.0);
    assert_eq!(ReductionCurve::canonicalize(1.5), 1.0);
    assert_eq!(ReductionCurve::canonicalize(f32::NAN), 1.0);
    assert_eq!(ReductionCurve::canonicalize(f32::INFINITY), 1.0);
    assert!(ReductionCurve::default().is_flat());
    assert!(!curve.is_flat());
    // Shaping stays bounded: scale 0 disables, scale 1 requests the gain.
    assert_eq!(ReductionCurve::shape_gain(0.0, 0.07), 1.0);
    for gain in [0.0, 0.07, 0.5, 1.0] {
        let shaped = ReductionCurve::shape_gain(0.4, gain);
        assert!((gain..=1.0).contains(&shaped), "{gain} -> {shaped}");
    }
}

#[test]
fn flat_curve_matches_default_bit_exact() {
    let input = white_noise(8193, 2, 0xC0FFEE, 0.1);
    for low_latency in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                let base = DenoiserPluginParams {
                    low_latency,
                    multi_resolution: multi,
                    polyphonic_detection: pnd,
                    ..Default::default()
                };
                let mut reference = DenoiserPlugin::from_params(2, base.clone());
                let mut configured = DenoiserPlugin::from_params(
                    2,
                    DenoiserPluginParams {
                        curve_low: 1.0,
                        curve_mid: 1.0,
                        curve_high: 1.0,
                        ..base
                    },
                );
                reference.initialize(f64::from(RATE)).unwrap();
                configured.initialize(f64::from(RATE)).unwrap();
                let expected = process_all(&mut reference, &input, 2, &[1, 17, 257, 63], RATE);
                let actual = process_all(&mut configured, &input, 2, &[1, 17, 257, 63], RATE);
                assert_bit_exact(
                    &expected,
                    &actual,
                    &format!("flat low={low_latency} multi={multi} pnd={pnd}"),
                );
                // A shaped curve edited back to flat re-engages the skip path.
                for (id, value) in [("curve_low", 0.2), ("curve_mid", 0.7), ("curve_high", 0.4)] {
                    configured
                        .parametric_set_parameter(
                            ParameterId::from(id),
                            ParameterValue::Float(value),
                        )
                        .unwrap();
                }
                for id in ["curve_low", "curve_mid", "curve_high"] {
                    configured
                        .parametric_set_parameter(ParameterId::from(id), ParameterValue::Float(1.0))
                        .unwrap();
                }
                configured.reset();
                reference.reset();
                let expected = process_all(&mut reference, &input, 2, &[1024, 7], RATE);
                let actual = process_all(&mut configured, &input, 2, &[1024, 7], RATE);
                assert_bit_exact(
                    &expected,
                    &actual,
                    &format!("flat-after-edit low={low_latency} multi={multi} pnd={pnd}"),
                );
            }
        }
    }
}

#[test]
fn unity_gain_ignores_shaped_curve() {
    // Transparency blends shaped and unshaped gains toward unity with
    // different f32 rounding, so unity holds within rounding, not bit-exact.
    let input = white_noise(8193, 2, 0x01F7, 0.2);
    for low_latency in [false, true] {
        let mut reference = DenoiserPlugin::from_params(
            2,
            DenoiserPluginParams {
                low_latency,
                transparency: 1.0,
                ..Default::default()
            },
        );
        let mut shaped = DenoiserPlugin::from_params(
            2,
            DenoiserPluginParams {
                low_latency,
                transparency: 1.0,
                curve_low: 0.1,
                curve_mid: 0.9,
                curve_high: 0.3,
                ..Default::default()
            },
        );
        reference.initialize(f64::from(RATE)).unwrap();
        shaped.initialize(f64::from(RATE)).unwrap();
        let latency = shaped.latency_samples();
        let expected = process_all(&mut reference, &input, 2, &[3, 1024, 44], RATE);
        let actual = process_all(&mut shaped, &input, 2, &[3, 1024, 44], RATE);
        let mut worst_pair = 0.0f32;
        let mut worst_oracle = 0.0f32;
        for (o, pair) in actual.as_chunks::<2>().0.iter().enumerate() {
            for ch in 0..2 {
                worst_pair = worst_pair.max((pair[ch] - expected[o * 2 + ch]).abs());
                let oracle = if o < latency {
                    0.0
                } else {
                    input[(o - latency) * 2 + ch]
                };
                worst_oracle = worst_oracle.max((pair[ch] - oracle).abs());
            }
        }
        assert!(
            worst_pair < 1e-6,
            "shaped-vs-flat unity: {worst_pair:e} (low={low_latency})"
        );
        assert!(
            worst_oracle < 2e-6,
            "shaped unity oracle: {worst_oracle:e} (low={low_latency})"
        );
    }
}

fn steady_output(curve: (f32, f32, f32), rate: u32, low_latency: bool) -> Vec<f32> {
    let frames = 3 * rate as usize;
    let input = white_noise(frames, 1, 0x5EED1234, 0.1);
    let mut plugin = DenoiserPlugin::from_params(
        1,
        DenoiserPluginParams {
            low_latency,
            reduction_db: 40.0,
            floor_db: -50.0,
            curve_low: curve.0,
            curve_mid: curve.1,
            curve_high: curve.2,
            ..Default::default()
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    let output = process_all(&mut plugin, &input, 1, &[1024, 63], rate);
    // Converged last second only; MCRA needs about one window (~1 s).
    output[output.len() - rate as usize..].to_vec()
}

#[test]
fn shaped_curve_releases_selected_band() {
    for low_latency in [false, true] {
        let flat = steady_output((1.0, 1.0, 1.0), RATE, low_latency);
        let lows_released = steady_output((0.0, 1.0, 1.0), RATE, low_latency);
        let highs_released = steady_output((1.0, 1.0, 0.0), RATE, low_latency);
        let db = |a: f64, b: f64| 10.0 * (a / b).log10();
        let low_flat = goertzel_mag2(&flat, 100.0, f64::from(RATE));
        let low_open = goertzel_mag2(&lows_released, 100.0, f64::from(RATE));
        let high_flat = goertzel_mag2(&flat, 10_000.0, f64::from(RATE));
        let high_open = goertzel_mag2(&highs_released, 10_000.0, f64::from(RATE));
        assert!(
            db(low_open, low_flat) >= 6.0,
            "low release: {:.2} dB (low={low_latency})",
            db(low_open, low_flat)
        );
        assert!(
            db(high_open, high_flat) >= 6.0,
            "high release: {:.2} dB (low={low_latency})",
            db(high_open, high_flat)
        );
        // Untouched bands agree: the low release must not lift the highs and
        // the high release must not lift the lows.
        let high_with_low_open = goertzel_mag2(&lows_released, 10_000.0, f64::from(RATE));
        let low_with_high_open = goertzel_mag2(&highs_released, 100.0, f64::from(RATE));
        assert!(
            db(high_with_low_open, high_flat).abs() <= 3.0,
            "high drift: {:.2} dB (low={low_latency})",
            db(high_with_low_open, high_flat)
        );
        assert!(
            db(low_with_high_open, low_flat).abs() <= 3.0,
            "low drift: {:.2} dB (low={low_latency})",
            db(low_with_high_open, low_flat)
        );
    }
}

#[test]
fn curve_zero_bypasses_reduction_in_every_mode() {
    // An all-zero curve requests unity gain on every reduction path, so the
    // output must match the delayed-input oracle within the accepted 2e-6.
    // Spatial denoising is excluded by design: its post-gain coherence trim
    // sits outside the reduction curve's scope.
    let input = white_noise(12_289, 2, 0xB1A55, 0.15);
    for low_latency in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                for sub in [false, true] {
                    let mut plugin = DenoiserPlugin::from_params(
                        2,
                        DenoiserPluginParams {
                            low_latency,
                            multi_resolution: multi,
                            polyphonic_detection: pnd,
                            spectral_sub_enabled: sub,
                            harmonic_percussive: true,
                            reduction_db: 40.0,
                            curve_low: 0.0,
                            curve_mid: 0.0,
                            curve_high: 0.0,
                            ..Default::default()
                        },
                    );
                    plugin.initialize(f64::from(RATE)).unwrap();
                    let latency = plugin.latency_samples();
                    let output = process_all(&mut plugin, &input, 2, &[5, 1024, 91], RATE);
                    let mut worst = 0.0f32;
                    for (o, frame) in output.as_chunks::<2>().0.iter().enumerate() {
                        for ch in 0..2 {
                            let expected = if o < latency {
                                0.0
                            } else {
                                input[(o - latency) * 2 + ch]
                            };
                            worst = worst.max((frame[ch] - expected).abs());
                        }
                    }
                    assert!(
                        worst < 2e-6,
                        "worst {worst:e} low={low_latency} multi={multi} pnd={pnd} sub={sub}"
                    );
                }
            }
        }
    }
}

#[test]
fn curve_coordinates_are_physical_across_rates() {
    // 100 Hz sits below the 125 Hz anchor at every rate even though its bin
    // index moves (bin ~4 at 48 kHz/2048, bin ~1 at 192 kHz/2048).
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for low_latency in [false, true] {
            let flat = steady_output((1.0, 1.0, 1.0), rate, low_latency);
            let released = steady_output((0.0, 1.0, 1.0), rate, low_latency);
            let ratio = goertzel_mag2(&released, 100.0, f64::from(rate))
                / goertzel_mag2(&flat, 100.0, f64::from(rate));
            let lift = 10.0 * ratio.log10();
            assert!(
                lift >= 6.0,
                "rate={rate} low={low_latency}: {lift:.2} dB at 100 Hz"
            );
        }
    }
}

#[test]
fn curve_knots_persist_and_reproduce_audio() {
    let encoded = serde_json::to_value(DenoiserPluginParams {
        curve_low: 0.25,
        curve_mid: 0.75,
        curve_high: 0.5,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(encoded["curve_low"], 0.25);
    assert_eq!(encoded["curve_mid"], 0.75);
    assert_eq!(encoded["curve_high"], 0.5);
    let restored: DenoiserPluginParams = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(restored.curve_low, 0.25);
    assert_eq!(restored.curve_mid, 0.75);
    assert_eq!(restored.curve_high, 0.5);
    assert_eq!(serde_json::to_value(restored.clone()).unwrap(), encoded);
    // Legacy presets without curve keys keep full reduction.
    let legacy: DenoiserPluginParams = serde_json::from_str("{}").unwrap();
    assert_eq!(legacy.curve_low, 1.0);
    assert_eq!(legacy.curve_mid, 1.0);
    assert_eq!(legacy.curve_high, 1.0);

    let input = white_noise(8193, 2, 0xABCD, 0.12);
    let mut reference = DenoiserPlugin::try_from_params(
        2,
        DenoiserPluginParams {
            curve_low: 0.25,
            curve_mid: 0.75,
            curve_high: 0.5,
            ..Default::default()
        },
    )
    .unwrap();
    let mut reloaded = DenoiserPlugin::try_from_params(2, restored).unwrap();
    reference.initialize(f64::from(RATE)).unwrap();
    reloaded.initialize(f64::from(RATE)).unwrap();
    let expected = process_all(&mut reference, &input, 2, &[1, 17, 257, 63, 1024], RATE);
    // Reloaded twin uses different callback partitions: identical audio.
    let actual = process_all(&mut reloaded, &input, 2, &[4096, 7], RATE);
    assert!(!expected.iter().all(|&s| s == 0.0));
    assert_bit_exact(&expected, &actual, "save/reload twin");
}

#[test]
fn invalid_curve_configs_rejected_transactionally() {
    for (field, mut make) in [
        ("curve_low", DenoiserPluginParams::default()),
        ("curve_mid", DenoiserPluginParams::default()),
        ("curve_high", DenoiserPluginParams::default()),
    ] {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5, 1.5] {
            match field {
                "curve_low" => make.curve_low = bad,
                "curve_mid" => make.curve_mid = bad,
                _ => make.curve_high = bad,
            }
            let error = DenoiserPlugin::try_from_params(2, make.clone())
                .err()
                .expect("invalid knot must fail construction");
            assert!(error.contains(field), "{error}");
        }
    }
    for bad in [
        json!({"curve_low": true}),
        json!({"curve_mid": "0.5"}),
        json!({"curve_high": null}),
    ] {
        assert!(serde_json::from_value::<DenoiserPluginParams>(bad).is_err());
    }
    // Live setters clamp finite input and reject NaN without side effects.
    let mut plugin = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    plugin.initialize(f64::from(RATE)).unwrap();
    let before = plugin.current_values();
    plugin
        .parametric_set_parameter(
            ParameterId::from("curve_mid"),
            ParameterValue::Float(f32::NAN),
        )
        .unwrap_err();
    assert_eq!(plugin.current_values(), before);
    plugin
        .parametric_set_parameter(ParameterId::from("curve_mid"), ParameterValue::Float(1.5))
        .unwrap();
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("curve_mid")),
        Some(ParameterValue::Float(1.0))
    );
    // Profile triggers still do not leak into persistent config.
    let encoded = serde_json::to_value(DenoiserPluginParams::default()).unwrap();
    assert!(encoded.get("learn_noise").is_none());
    assert!(encoded.get("clear_profile").is_none());
}
