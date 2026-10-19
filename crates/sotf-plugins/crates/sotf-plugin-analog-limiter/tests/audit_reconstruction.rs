//! Characterize the native post-color sample guard using independent reconstruction.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::MODEL_NAMES;
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};
use std::f64::consts::TAU;
#[path = "../../sotf-plugin-limiter/tests/support/reconstruction.rs"]
mod reconstruction;
use reconstruction::{db, reconstructed_peak, reconstructed_peak_at_factor, sample_peak};

fn interpolation_norm(factor: usize) -> f64 {
    let radius = 24 / factor;
    (1..factor)
        .map(|phase| {
            (-(radius as isize)..=radius as isize + 1)
                .map(|offset| {
                    let t = phase as f64 / factor as f64 - offset as f64;
                    if t.abs() > radius as f64 {
                        0.0
                    } else {
                        ((std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t)
                            * 0.5
                            * (1.0 + (std::f64::consts::PI * t / radius as f64).cos()))
                        .abs()
                    }
                })
                .sum::<f64>()
        })
        .fold(1.0, f64::max)
}

#[test]
fn near_nyquist_color_changes_preserve_samples_but_do_not_certify_true_peak() {
    let mut random = 0x51fc_0618_u32;
    let mut maximum = -120.0_f64;
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in MODEL_NAMES {
            for frequency in [0.227, 0.371, 0.437, 0.473, 0.491] {
                for _ in 0..3 {
                    random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                    let phase = TAU * f64::from(random) / f64::from(u32::MAX);
                    let mut plugin = AnalogLimiterPlugin::from_params(
                        2,
                        AnalogLimiterPluginParams {
                            threshold: -12.0,
                            lookahead: 0.37,
                            true_peak: true,
                            analog_model: (*model).into(),
                            analog_drive: 24.0,
                            analog_color: 1.0,
                            analog_character: 0.7,
                            analog_trim: 6.0,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    plugin.initialize(f64::from(rate)).unwrap();
                    let input: Vec<f32> = (0..4099)
                        .flat_map(|i| {
                            let x = (1.1 * (TAU * frequency * i as f64 + phase).sin()) as f32;
                            [
                                x,
                                (0.7 * (TAU * (frequency - 0.017) * i as f64 - phase).cos()) as f32,
                            ]
                        })
                        .collect();
                    let mut output = input.clone();
                    let mut pos = 0;
                    let mut event = 0;
                    while pos < input.len() / 2 {
                        if pos % 257 == 0 {
                            let value = [1.0, 0.2, 0.8][event % 3];
                            plugin
                                .set_parameter(
                                    ParameterId::from("analog_color"),
                                    ParameterValue::Float(value),
                                )
                                .unwrap();
                            event += 1;
                        }
                        let n = 257.min(input.len() / 2 - pos);
                        plugin
                            .process_in_place(
                                &mut output[pos * 2..(pos + n) * 2],
                                &ProcessContext::new(rate, n),
                            )
                            .unwrap();
                        pos += n;
                    }
                    // This explicit zero continuation is a chosen render duration,
                    // not an assertion that recursive coloration has finite support.
                    let mut padding = vec![0.0; 2048];
                    plugin
                        .process_in_place(&mut padding, &ProcessContext::new(rate, 1024))
                        .unwrap();
                    output.extend(padding);
                    for ch in 0..2 {
                        let sample = db(sample_peak(&output, 2, ch));
                        let dense = db(reconstructed_peak_at_factor(&output, 2, ch, 4));
                        let criterion = db(reconstructed_peak(&output, 2, ch, rate));
                        maximum = maximum.max(dense);
                        assert!(
                            sample <= -12.0 + 1e-5,
                            "{rate} {model} f={frequency}: sample guard {sample}"
                        );
                        assert!(
                            dense <= -12.0 + db(interpolation_norm(4)) + 1e-5,
                            "bounded samples exceeded the independently derived interpolation L1 bound: {dense}"
                        );
                        assert!(
                            criterion
                                <= -12.0
                                    + db(interpolation_norm(if rate < 96_000 {
                                        4
                                    } else if rate < 192_000 {
                                        2
                                    } else {
                                        1
                                    }))
                                    + 1e-5,
                            "rate-appropriate reconstruction exceeded its interpolation L1 bound"
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    assert!(
        maximum > -11.8,
        "the fixture must expose that sample clamping is not a true-peak guarantee"
    );
    println!(
        "{cases} color/phase cases, maximum dense 4x reconstructed level={maximum} dBTP for -12 dBFS samples"
    );
}
