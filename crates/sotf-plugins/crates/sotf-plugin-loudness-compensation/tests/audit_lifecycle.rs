//! Independent response and calibrated-volume lifecycle regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_loudness_compensation::{
    LoudnessCompensationPlugin, LoudnessCompensationPluginParams,
};

fn prepared(rate: u32, params: LoudnessCompensationPluginParams) -> LoudnessCompensationPlugin {
    let mut plugin = LoudnessCompensationPlugin::from_params(2, params).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(
    plugin: &mut LoudnessCompensationPlugin,
    rate: u32,
    input: &[f32],
    parts: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut frame = 0;
    let mut block = 0;
    while frame < input.len() / 2 {
        let n = parts[block % parts.len()].min(input.len() / 2 - frame);
        plugin
            .process_in_place(
                &mut output[frame * 2..(frame + n) * 2],
                &ProcessContext::new(rate, n),
            )
            .unwrap();
        frame += n;
        block += 1;
    }
    output
}

#[test]
fn initializing_auto_uses_calibrated_volume_instead_of_the_iso_mode_level() {
    for rate in [16_000, 48_000, 192_000] {
        for volume in [0.0, -11.0, -35.0] {
            for normalized in [false, true] {
                let base = LoudnessCompensationPluginParams {
                    reference_level_db: 80.0,
                    playback_volume_db: volume,
                    headroom_normalized: normalized,
                    auto_calibrated: true,
                    ..Default::default()
                };
                let mut auto = prepared(
                    rate,
                    LoudnessCompensationPluginParams {
                        mode: 2,
                        ..base.clone()
                    },
                );
                let mut iso = prepared(
                    rate,
                    LoudnessCompensationPluginParams {
                        mode: 1,
                        playback_level_db: 80.0 + volume,
                        ..base
                    },
                );
                auto.reset();
                iso.reset();
                let mut impulse = vec![0.0; 8192];
                impulse[0] = 0.1;
                impulse[3] = -0.07;
                let actual = render(&mut auto, rate, &impulse, &[1, 31, 257]);
                let expected = render(&mut iso, rate, &impulse, &[4096]);
                let error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    actual == expected,
                    "fs={rate} volume={volume} normalized={normalized}: Auto/ISO mismatch {error}"
                );
            }
        }
    }
}

#[test]
fn reinitialization_discards_old_rate_filter_and_transition_history() {
    for mode in [0, 1, 2] {
        let params = LoudnessCompensationPluginParams {
            mode,
            auto_calibrated: true,
            reference_level_db: 80.0,
            playback_level_db: 60.0,
            playback_volume_db: -20.0,
            headroom_normalized: true,
            ..Default::default()
        };
        for position in ["disabled", "pre", "post"] {
            let params = LoudnessCompensationPluginParams {
                auto_gain_enabled: position != "disabled",
                auto_gain_position: position.into(),
                ..params.clone()
            };
            let mut moved = prepared(48_000, params.clone());
            render(&mut moved, 48_000, &vec![0.1; 8192], &[4096]);
            for rate in [16_000, 192_000, 48_000] {
                moved.initialize(f64::from(rate)).unwrap();
                let mut fresh = prepared(rate, params.clone());
                let mut impulse = vec![0.0; 8192];
                impulse[0] = 0.1;
                let actual = render(&mut moved, rate, &impulse, &[257, 31]);
                let expected = render(&mut fresh, rate, &impulse, &[4096]);
                let error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    actual == expected,
                    "mode={mode} {position} fs={rate}: retained old-rate state, error={error}"
                );
            }
        }
    }
}

#[test]
fn volume_changes_during_silence_are_clock_invariant_and_adopt_the_final_contour() {
    for rate in [16_000, 48_000, 192_000] {
        for position in ["disabled", "pre", "post"] {
            let params = LoudnessCompensationPluginParams {
                mode: 2,
                auto_calibrated: true,
                reference_level_db: 80.0,
                playback_volume_db: 0.0,
                auto_gain_enabled: position != "disabled",
                auto_gain_position: position.into(),
                ..Default::default()
            };
            let scenario = |parts: &[usize]| {
                let mut plugin = prepared(rate, params.clone());
                for volume in [-7.0, -11.25, -35.0] {
                    plugin
                        .set_parameter(
                            ParameterId::from("playback_volume_db"),
                            ParameterValue::Float(volume),
                        )
                        .unwrap();
                    let silence =
                        render(&mut plugin, rate, &vec![0.0; rate as usize / 5 * 2], parts);
                    assert!(
                        silence.iter().all(|&x| x == 0.0),
                        "volume controls must not create audio from silence"
                    );
                }
                let mut probe = vec![0.0; 8192];
                probe[0] = 0.1;
                render(&mut plugin, rate, &probe, parts)
            };
            let actual = scenario(&[4096]);
            assert!(
                actual == scenario(&[1, 31, 257]),
                "{position} fs={rate}: silent volume automation depends on callback size"
            );
            let mut reference = prepared(
                rate,
                LoudnessCompensationPluginParams {
                    mode: 1,
                    playback_level_db: 45.0,
                    ..params
                },
            );
            reference.reset();
            let mut probe = vec![0.0; 8192];
            probe[0] = 0.1;
            let expected = render(&mut reference, rate, &probe, &[4096]);
            assert!(
                actual == expected,
                "{position} fs={rate}: first transient after silent updates must use the final contour"
            );
        }
    }
}

fn magnitude(b: [f64; 3], a: [f64; 3], omega: f64) -> f64 {
    let power = |v: [f64; 3]| {
        let re = v[0] + v[1] * omega.cos() + v[2] * (2.0 * omega).cos();
        let im = -v[1] * omega.sin() - v[2] * (2.0 * omega).sin();
        re * re + im * im
    };
    (power(b) / power(a)).sqrt()
}

fn shelf(rate: u32, corner: f64, gain: f64, probe: f64, high: bool) -> f64 {
    let w = std::f64::consts::TAU * corner.min(f64::from(rate) * 0.45) / f64::from(rate);
    let c = w.cos();
    let a = 10.0_f64.powf(gain / 40.0);
    let beta = 2.0 * a.sqrt() * w.sin() / (2.0 * 0.707);
    let (b, d) = if high {
        (
            [
                a * ((a + 1.0) + (a - 1.0) * c + beta),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                a * ((a + 1.0) + (a - 1.0) * c - beta),
            ],
            [
                (a + 1.0) - (a - 1.0) * c + beta,
                2.0 * ((a - 1.0) - (a + 1.0) * c),
                (a + 1.0) - (a - 1.0) * c - beta,
            ],
        )
    } else {
        (
            [
                a * ((a + 1.0) - (a - 1.0) * c + beta),
                2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                a * ((a + 1.0) - (a - 1.0) * c - beta),
            ],
            [
                (a + 1.0) + (a - 1.0) * c + beta,
                -2.0 * ((a - 1.0) + (a + 1.0) * c),
                (a + 1.0) + (a - 1.0) * c - beta,
            ],
        )
    };
    magnitude(b, d, std::f64::consts::TAU * probe / f64::from(rate))
}

#[test]
fn manual_response_curves_match_independent_rbj_equations_at_rate_extremes() {
    const WINDOW: usize = 8192;
    for rate in [16_000, 48_000, 192_000] {
        for (bass, treble) in [(12.0, 6.0), (-12.0, -6.0)] {
            for cycles in [2, 5, 17, 83, 733, 3201] {
                let frequency = cycles as f64 * f64::from(rate) / WINDOW as f64;
                let expected = shelf(rate, 100.0, bass / 2.0, frequency, false).powi(2)
                    * shelf(rate, 8000.0, treble / 2.0, frequency, true).powi(2);
                let mut plugin = prepared(
                    rate,
                    LoudnessCompensationPluginParams {
                        low_freq: 100.0,
                        low_gain: bass as f32,
                        high_freq: 8000.0,
                        high_gain: treble as f32,
                        mid_enabled: false,
                        ..Default::default()
                    },
                );
                let frames = rate as usize / 2 + WINDOW;
                let input: Vec<_> = (0..frames)
                    .flat_map(|i| {
                        let sample = (std::f64::consts::TAU * cycles as f64 * i as f64
                            / WINDOW as f64)
                            .cos() as f32
                            * 0.001;
                        [sample, sample]
                    })
                    .collect();
                let output = render(&mut plugin, rate, &input, &[8192]);
                let mut re = 0.0;
                let mut im = 0.0;
                for frame in frames - WINDOW..frames {
                    let angle =
                        -std::f64::consts::TAU * cycles as f64 * frame as f64 / WINDOW as f64;
                    let sample = f64::from(output[frame * 2]);
                    re += sample * angle.cos();
                    im += sample * angle.sin();
                }
                let actual = re.hypot(im) * 2.0 / WINDOW as f64 / 0.001;
                let error = 20.0 * (actual / expected).log10();
                assert!(
                    error.abs() < 0.002,
                    "fs={rate} f={frequency} bass={bass} treble={treble}: response error={error} dB"
                );
            }
        }
    }
}

#[test]
fn rendered_iso_curves_match_published_contour_difference_checkpoints() {
    // ISO 226:2003 Annex B rounded SPL checkpoints, normalized to 1 kHz.
    // These targets do not call the production contour or fitting functions.
    for rate in [16_000, 48_000, 192_000] {
        for (level, targets) in [
            (40.0, [20.9, 11.9, 0.0, 6.1]),
            (60.0, [10.5, 6.2, 0.0, 3.2]),
        ] {
            for (frequency, target) in [20.0, 100.0, 1000.0, 12500.0].into_iter().zip(targets) {
                if frequency >= f64::from(rate) * 0.45 {
                    continue;
                }
                let mut plugin = prepared(
                    rate,
                    LoudnessCompensationPluginParams {
                        mode: 1,
                        playback_level_db: level,
                        reference_level_db: 80.0,
                        ..Default::default()
                    },
                );
                let frames = rate as usize * 2;
                let input: Vec<_> = (0..frames)
                    .flat_map(|i| {
                        let tone = (std::f64::consts::TAU * frequency * i as f64 / f64::from(rate))
                            .cos() as f32
                            * 0.001;
                        [tone, tone]
                    })
                    .collect();
                let output = render(&mut plugin, rate, &input, &[8192]);
                let mut re = 0.0;
                let mut im = 0.0;
                for frame in rate as usize..frames {
                    let phase = -std::f64::consts::TAU * frequency * frame as f64 / f64::from(rate);
                    let value = f64::from(output[frame * 2]);
                    re += value * phase.cos();
                    im += value * phase.sin();
                }
                let measured = 20.0 * (re.hypot(im) * 2.0 / f64::from(rate) / 0.001).log10();
                let error = (measured - target).abs();
                let tolerance = if frequency == 1000.0 { 1.0 } else { 2.0 };
                assert!(
                    error < tolerance,
                    "fs={rate} {level}->80 phon, f={frequency}: {measured:.4} dB vs {target}, error={error:.4}"
                );
            }
        }
    }
}
