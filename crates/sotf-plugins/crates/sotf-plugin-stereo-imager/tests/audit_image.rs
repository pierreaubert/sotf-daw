//! Independent M/S image, transient and automation regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_stereo_imager::{StereoImagerPlugin, StereoImagerPluginParams};

fn process(plugin: &mut StereoImagerPlugin, rate: u32, audio: &mut [f32], partitions: &[usize]) {
    let mut frame = 0;
    let mut block = 0;
    while frame < audio.len() / 2 {
        let count = partitions[block % partitions.len()].min(audio.len() / 2 - frame);
        if partitions.len() > 1 {
            plugin
                .process_in_place(&mut [], &ProcessContext::new(rate, 0))
                .unwrap();
        }
        plugin
            .process_in_place(
                &mut audio[frame * 2..(frame + count) * 2],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        frame += count;
        block += 1;
    }
}

fn signal(frames: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let mid = (i as f32 * 0.173).sin() * 0.13;
            let side = (i as f32 * 0.037).cos() * 0.21;
            [mid + side, mid - side]
        })
        .collect()
}

#[test]
fn repeated_dry_wet_and_neutral_transitions_use_the_sample_clock() {
    for rate in [16_000, 32_000, 48_000, 192_000] {
        for control in ["mix", "width"] {
            let mut rendered = Vec::new();
            for partitions in [&[32768][..], &[1][..], &[31, 257, 7][..]] {
                let mut plugin = StereoImagerPlugin::new(
                    2,
                    StereoImagerPluginParams {
                        width: 0.0,
                        low_width: 1.0,
                        mid_width: 1.0,
                        high_width: 1.0,
                        ..Default::default()
                    },
                );
                plugin.initialize(f64::from(rate)).unwrap();
                let mut output = Vec::new();
                for target in [1.0, 0.0, 1.0, 0.37, 0.0, 1.0] {
                    plugin
                        .set_parameter(ParameterId::from(control), ParameterValue::Float(target))
                        .unwrap();
                    // Long enough to exercise the one-pole's exact settled routes.
                    let mut audio = signal(rate as usize / 4);
                    process(&mut plugin, rate, &mut audio, partitions);
                    output.extend(audio);
                }
                rendered.push(output);
            }
            let error = rendered[0]
                .iter()
                .zip(&rendered[1])
                .chain(rendered[0].iter().zip(&rendered[2]))
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                rendered[0] == rendered[1] && rendered[0] == rendered[2],
                "{rate} {control}: callback-dependent error {error}"
            );
        }
    }
}

#[test]
fn small_nonzero_mix_is_not_reclassified_as_dry() {
    let mix = f32::EPSILON * 0.75;
    let mut plugin = StereoImagerPlugin::new(
        2,
        StereoImagerPluginParams {
            width: 2.0,
            low_width: 2.0,
            mid_width: 2.0,
            high_width: 2.0,
            mix,
            ..Default::default()
        },
    );
    let input = [0.25, -0.25];
    let mut audio = input;
    plugin
        .process_in_place(&mut audio, &ProcessContext::new(48_000, 1))
        .unwrap();
    let expected = [
        input[0] + (1.0 - input[0]) * mix,
        input[1] + (-1.0 - input[1]) * mix,
    ];
    assert!(
        expected != input,
        "the fixture must resolve the nonzero mix in f32"
    );
    assert!(
        audio == expected,
        "small mix {mix}: actual {audio:?}, expected {expected:?}"
    );
}

fn configurations() -> [StereoImagerPluginParams; 5] {
    [
        StereoImagerPluginParams::default(),
        StereoImagerPluginParams {
            low_width: 0.0,
            mid_width: 0.0,
            high_width: 0.0,
            ..Default::default()
        },
        StereoImagerPluginParams {
            mono_bass: true,
            ..Default::default()
        },
        StereoImagerPluginParams {
            low_width: 0.2,
            mid_width: 1.0,
            high_width: 1.6,
            ..Default::default()
        },
        StereoImagerPluginParams {
            width: 1.7,
            low_width: 1.4,
            mid_width: 0.3,
            high_width: 1.2,
            mix: 0.37,
            ..Default::default()
        },
    ]
}

// Closed-form impulse response of the bilinear analog one-pole prototype.
// No production filter/coefficient helpers or recursive filter replay are used.
fn lowpass_impulse(cutoff: f32, rate: u32, n: usize) -> f64 {
    let g = (std::f64::consts::PI * f64::from(cutoff) / f64::from(rate)).tan();
    let b = g / (1.0 + g);
    let pole = (1.0 - g) / (1.0 + g);
    if n == 0 {
        b
    } else {
        b * (1.0 + pole) * pole.powi(n as i32 - 1)
    }
}

#[test]
fn first_transient_and_cross_channel_leakage_match_closed_form_impulses() {
    let mut worst = 0.0_f64;
    for rate in [16_000, 48_000, 192_000] {
        for params in configurations() {
            for channel in 0..2 {
                let mut plugin = StereoImagerPlugin::new(2, params.clone());
                plugin.initialize(f64::from(rate)).unwrap();
                let mut audio = vec![0.0; 4096 * 2];
                audio[channel] = 0.25;
                process(&mut plugin, rate, &mut audio, &[1, 31, 257]);
                let low_weight = if params.mono_bass {
                    0.0
                } else {
                    f64::from(params.low_width)
                };
                for (n, frame) in audio.as_chunks::<2>().0.iter().enumerate() {
                    let delta = if n == 0 { 1.0 } else { 0.0 };
                    let wet = f64::from(params.width)
                        * (f64::from(params.high_width) * delta
                            + (low_weight - f64::from(params.mid_width))
                                * lowpass_impulse(params.low_mid_freq, rate, n)
                            + f64::from(params.mid_width - params.high_width)
                                * lowpass_impulse(params.mid_high_freq, rate, n));
                    let side_response =
                        (1.0 - f64::from(params.mix)) * delta + f64::from(params.mix) * wet;
                    let mid = 0.125 * delta;
                    let side = if channel == 0 {
                        0.125 * side_response
                    } else {
                        -0.125 * side_response
                    };
                    for (actual, expected) in frame.iter().zip([mid + side, mid - side]) {
                        let error = (f64::from(*actual) - expected).abs();
                        worst = worst.max(error);
                        assert!(
                            error < 2e-7,
                            "{rate} {params:?} input={channel} frame={n}: error {error}"
                        );
                    }
                    assert!(
                        (f64::from(frame[0]) + f64::from(frame[1]) - 0.25 * delta).abs() < 2e-7
                    );
                }
                assert!(
                    audio[0] != 0.0 || audio[1] != 0.0,
                    "direct first sample must not acquire PDC delay"
                );
            }
        }
    }
    eprintln!("maximum independent impulse error: {worst}");
}

fn side_magnitude_squared(params: &StereoImagerPluginParams, rate: u32, hz: f64) -> f64 {
    let lp = |cutoff: f32| {
        let ratio = (std::f64::consts::PI * hz / f64::from(rate)).tan()
            / (std::f64::consts::PI * f64::from(cutoff) / f64::from(rate)).tan();
        (1.0 / (1.0 + ratio * ratio), -ratio / (1.0 + ratio * ratio))
    };
    let l = lp(params.low_mid_freq);
    let u = lp(params.mid_high_freq);
    let lw = if params.mono_bass {
        0.0
    } else {
        f64::from(params.low_width)
    };
    let mw = f64::from(params.mid_width);
    let hw = f64::from(params.high_width);
    let k = f64::from(params.mix) * f64::from(params.width);
    let real = 1.0 - f64::from(params.mix) + k * (hw + (lw - mw) * l.0 + (mw - hw) * u.0);
    let imag = k * ((lw - mw) * l.1 + (mw - hw) * u.1);
    real * real + imag * imag
}

#[test]
fn measured_phase_correlation_and_mono_fold_follow_mid_side_energy_law() {
    let mut worst = 0.0_f64;
    for rate in [32_000, 48_000, 192_000] {
        for params in configurations() {
            for hz in [20.0, 250.0, 4000.0, 12000.0] {
                let mut audio: Vec<f32> = (0..rate)
                    .flat_map(|n| {
                        let mid = (std::f64::consts::TAU * 1000.0 * f64::from(n) / f64::from(rate))
                            .sin() as f32
                            * 0.1;
                        let side = (std::f64::consts::TAU * hz * f64::from(n) / f64::from(rate))
                            .cos() as f32
                            * 0.1;
                        [mid + side, mid - side]
                    })
                    .collect();
                let original = audio.clone();
                let mut plugin = StereoImagerPlugin::new(2, params.clone());
                plugin.initialize(f64::from(rate)).unwrap();
                process(&mut plugin, rate, &mut audio, &[257, 1, 31]);
                let (mut left, mut right, mut cross, mut mean_l, mut mean_r) =
                    (0.0, 0.0, 0.0, 0.0, 0.0);
                for (actual, input) in audio
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .zip(original.as_chunks::<2>().0.iter())
                    .skip(rate as usize / 2)
                {
                    let l = f64::from(actual[0]);
                    let r = f64::from(actual[1]);
                    left += l * l;
                    right += r * r;
                    cross += l * r;
                    mean_l += l;
                    mean_r += r;
                    assert!(
                        (l + r - f64::from(input[0]) - f64::from(input[1])).abs() < 2e-7,
                        "mono fold changed"
                    );
                }
                let frames = f64::from(rate / 2);
                let rho = (cross - mean_l * mean_r / frames)
                    / ((left - mean_l * mean_l / frames) * (right - mean_r * mean_r / frames))
                        .sqrt();
                let side_energy = side_magnitude_squared(&params, rate, hz);
                let expected = (1.0 - side_energy) / (1.0 + side_energy);
                let error = (rho - expected).abs();
                worst = worst.max(error);
                assert!(
                    error < 2e-6,
                    "{rate} {hz} {params:?}: measured correlation {rho}, expected {expected}"
                );
                if params.mono_bass && hz == 20.0 {
                    assert!(rho > 0.98, "low bass should approach mono");
                    assert!(
                        side_energy < 0.01,
                        "20 Hz side attenuation must exceed 20 dB"
                    );
                }
            }
        }
    }
    eprintln!("maximum independent correlation error: {worst}");
}

#[test]
fn automation_reaches_exact_dry_and_unity() {
    for rate in [16_000, 48_000, 192_000] {
        for control in ["mix", "width"] {
            let mut plugin = StereoImagerPlugin::new(
                2,
                StereoImagerPluginParams {
                    width: if control == "width" { 0.0 } else { 1.7 },
                    ..Default::default()
                },
            );
            plugin.initialize(f64::from(rate)).unwrap();
            plugin
                .set_parameter(
                    ParameterId::from(control),
                    ParameterValue::Float(if control == "mix" { 0.0 } else { 1.0 }),
                )
                .unwrap();
            let mut tail = signal(rate as usize);
            process(&mut plugin, rate, &mut tail, &[257]);
            // Include tiny normal values to ensure exact route preservation is
            // independent of the normal arithmetic path's rounding.
            let input = [0.0_f32, -0.0, 1e-30, -3e-30, 0.1234567, -0.7654321];
            let mut output = input;
            plugin
                .process_in_place(&mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            plugin
                .process_in_place(&mut output, &ProcessContext::new(rate, 3))
                .unwrap();
            assert!(
                output
                    .iter()
                    .zip(input)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{rate} {control}: failed exact settled route {output:?}"
            );
        }
    }
}
