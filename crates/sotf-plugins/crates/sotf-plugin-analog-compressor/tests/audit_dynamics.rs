//! Independent physical-time detector and gain references for linked compression.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES};
use sotf_plugin_analog_compressor::{AnalogCompressorPlugin, AnalogCompressorPluginParams};

fn curve(level: f64, p: &AnalogCompressorPluginParams) -> f64 {
    let over = level - f64::from(p.threshold);
    let knee = f64::from(p.knee);
    let slope = 1.0 - 1.0 / f64::from(p.ratio);
    let gr = if knee == 0.0 || over >= knee / 2.0 {
        slope * over.max(0.0)
    } else if over <= -knee / 2.0 {
        0.0
    } else {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    };
    gr.min(f64::from(p.range_db))
}
#[derive(Default)]
struct Reference {
    envelope: f64,
    makeup: f64,
    hold: usize,
}
impl Reference {
    fn frame(&mut self, input: &[f32], rate: u32, p: &AnalogCompressorPluginParams) -> Vec<f64> {
        let peak = input
            .iter()
            .map(|&x| f64::from(x).abs())
            .fold(0.0, f64::max);
        let hold = (f64::from(p.hold_ms) * f64::from(rate) / 1000.0).round() as usize;
        self.hold = self.hold.min(hold);
        if peak >= self.envelope {
            self.hold = hold;
        } else if self.hold > 0 {
            self.hold -= 1;
            return self.output(input, rate, p);
        }
        let time = if peak > self.envelope {
            p.attack
        } else {
            p.release
        };
        self.envelope = peak
            + (self.envelope - peak) * (-1.0 / (f64::from(time) * 0.001 * f64::from(rate))).exp();
        self.output(input, rate, p)
    }
    fn output(&mut self, input: &[f32], rate: u32, p: &AnalogCompressorPluginParams) -> Vec<f64> {
        let gr = curve(20.0 * self.envelope.max(1e-7).log10(), p);
        let auto = if p.auto_makeup {
            let tau = if gr > self.makeup { 0.010 } else { 0.500 };
            self.makeup = gr + (self.makeup - gr) * (-1.0 / (tau * f64::from(rate))).exp();
            self.makeup.min(24.0)
        } else {
            0.0
        };
        let gain = 10.0_f64.powf((f64::from(p.makeup) + auto - gr) / 20.0);
        input
            .iter()
            .map(|&x| f64::from(x) * (1.0 + f64::from(p.mix) * (gain - 1.0)))
            .collect()
    }
}
fn make(channels: usize, rate: u32, p: &AnalogCompressorPluginParams) -> AnalogCompressorPlugin {
    let mut plugin = AnalogCompressorPlugin::from_params(channels, p.clone()).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}
fn process(
    p: &mut AnalogCompressorPlugin,
    input: &[f32],
    channels: usize,
    rate: u32,
    parts: &[usize],
) -> Vec<f32> {
    let mut out = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = parts[call % parts.len()].min(input.len() / channels - pos);
        p.process_in_place(
            &mut out[pos * channels..(pos + n) * channels],
            &ProcessContext::new(rate, n),
        )
        .unwrap();
        pos += n;
        call += 1;
    }
    out
}
#[test]
fn linked_control_sweeps_match_independent_detector_hold_and_makeup_clocks() {
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 8] {
            for auto in [false, true] {
                let mut p = AnalogCompressorPluginParams {
                    threshold: -24.0,
                    ratio: 4.0,
                    attack: 5.0,
                    release: 80.0,
                    knee: 6.0,
                    range_db: 9.0,
                    hold_ms: 3.75,
                    makeup: 2.0,
                    mix: 0.6,
                    auto_makeup: auto,
                    ..Default::default()
                };
                let mut a = make(channels, rate, &p);
                let mut b = make(channels, rate, &p);
                let mut reference = Reference::default();
                let frames = rate as usize / 16;
                let mut time = 0;
                for (key, value) in [
                    ("attack", 0.1),
                    ("ratio", 12.0),
                    ("release", 1000.0),
                    ("attack", 100.0),
                    ("range_db", 120.0),
                    ("hold_ms", 0.0),
                    ("threshold", -18.0),
                    ("mix", 1.0),
                ] {
                    for plugin in [&mut a, &mut b] {
                        plugin
                            .set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                            .unwrap();
                    }
                    match key {
                        "attack" => p.attack = value,
                        "ratio" => p.ratio = value,
                        "release" => p.release = value,
                        "range_db" => p.range_db = value,
                        "hold_ms" => p.hold_ms = value,
                        "threshold" => p.threshold = value,
                        "mix" => p.mix = value,
                        _ => unreachable!(),
                    }
                    let input: Vec<f32> = (time..time + frames)
                        .flat_map(|i| {
                            let amp = if (i / (rate as usize / 20)).is_multiple_of(2) {
                                0.7
                            } else {
                                0.025
                            };
                            let hot = (i / 1531) % channels;
                            (0..channels).map(move |ch| {
                                if ch == hot {
                                    amp
                                } else {
                                    0.01 * (i as f64 * 0.137 + ch as f64).sin() as f32
                                }
                            })
                        })
                        .collect();
                    let actual = process(&mut a, &input, channels, rate, &[8192]);
                    let split = process(&mut b, &input, channels, rate, &[1, 31, 257]);
                    assert!(
                        actual == split,
                        "{rate} {channels} {key}: callback partitions changed control clock"
                    );
                    for (src, dst) in input.chunks(channels).zip(actual.chunks(channels)) {
                        let expected = reference.frame(src, rate, &p);
                        for (&actual, expected) in dst.iter().zip(expected) {
                            let error =
                                (f64::from(actual) - expected).abs() / (1.0 + expected.abs());
                            maximum = maximum.max(error);
                            assert!(
                                error < 3e-4,
                                "{rate} channels={channels} auto={auto} {key}: normalized dynamics error {error:e}"
                            );
                        }
                    }
                    time += frames;
                }
            }
        }
    }
    println!("maximum normalized physical-time dynamics error={maximum:e}");
}
#[test]
fn dc_attack_and_release_match_closed_form_from_sample_zero() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let p = AnalogCompressorPluginParams {
            threshold: -18.0,
            ratio: 4.0,
            attack: 10.0,
            release: 50.0,
            knee: 0.0,
            ..Default::default()
        };
        let n = rate as usize / 50;
        let tail = rate as usize / 10;
        let mut input = vec![0.8; n];
        input.resize(n + tail, 0.03);
        let output = process(&mut make(1, rate, &p), &input, 1, rate, &[1, 31, 257]);
        assert_eq!(
            output[0], input[0],
            "zero-latency compressor attack permits onset overshoot"
        );
        let high = f64::from(input[0]);
        let low = f64::from(input[n]);
        let at_release = high * (1.0 - (-(n as f64) / (0.010 * f64::from(rate))).exp());
        for (i, (&x, &y)) in input.iter().zip(&output).enumerate() {
            let env = if i < n {
                high * (1.0 - (-((i + 1) as f64) / (0.010 * f64::from(rate))).exp())
            } else {
                low + (at_release - low) * (-((i - n + 1) as f64) / (0.050 * f64::from(rate))).exp()
            };
            let expected =
                f64::from(x) * 10.0_f64.powf(-curve(20.0 * env.max(1e-7).log10(), &p) / 20.0);
            assert!(
                (f64::from(y) - expected).abs() < 3e-4,
                "{rate} sample={i}: closed-form trajectory error"
            );
        }
    }
}
#[test]
fn measured_soft_knee_has_continuous_value_and_first_derivative_at_both_edges() {
    let delta = 0.02_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for ratio in [1.0, 2.0, 4.0, 20.0] {
            for knee in [2.0, 6.0, 20.0] {
                let p = AnalogCompressorPluginParams {
                    threshold: -24.0,
                    ratio,
                    knee,
                    attack: 0.1,
                    release: 10.0,
                    ..Default::default()
                };
                for upper in [false, true] {
                    let edge = f64::from(p.threshold)
                        + if upper {
                            f64::from(knee) / 2.0
                        } else {
                            -f64::from(knee) / 2.0
                        };
                    let levels = [edge - delta, edge, edge + delta];
                    let mut measured = [0.0; 3];
                    for (i, level) in levels.into_iter().enumerate() {
                        let x = 10.0_f64.powf(level / 20.0) as f32;
                        let output =
                            process(&mut make(1, rate, &p), &vec![x; 4096], 1, rate, &[257]);
                        measured[i] = -20.0 * (f64::from(output[4095]) / f64::from(x)).log10();
                        assert!(
                            (measured[i] - curve(20.0 * f64::from(x).log10(), &p)).abs() < 1e-4
                        );
                    }
                    let left = (measured[1] - measured[0]) / delta;
                    let right = (measured[2] - measured[1]) / delta;
                    let expected = if upper {
                        1.0 - 1.0 / f64::from(ratio)
                    } else {
                        0.0
                    };
                    assert!(
                        (left - right).abs() < 0.02
                            && (left - expected).abs() < 0.02
                            && (right - expected).abs() < 0.02,
                        "{rate} ratio={ratio} knee={knee} upper={upper}: knee derivatives {left}/{right}"
                    );
                }
            }
        }
    }
}
fn stage(rate: u32, p: &AnalogCompressorPluginParams) -> AnalogColorStage {
    let mut s = AnalogColorStage::new(2);
    s.set_model_id(
        MODEL_NAMES
            .iter()
            .position(|&m| m == p.analog_model)
            .unwrap() as u32,
    )
    .unwrap();
    for prepared in [false, true] {
        if prepared {
            s.prepare(f64::from(rate), 8192).unwrap();
        }
        s.set_drive_db(p.analog_drive).unwrap();
        s.set_color(p.analog_color).unwrap();
        s.set_character(p.analog_character).unwrap();
        s.set_output_trim_db(p.analog_trim).unwrap();
    }
    s.reset();
    s
}
#[test]
fn compression_parallel_mix_precedes_color_even_at_zero_mix() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in MODEL_NAMES {
            for mix in [0.0, 0.5, 1.0] {
                let p = AnalogCompressorPluginParams {
                    threshold: -30.0,
                    ratio: 8.0,
                    attack: 1.0,
                    release: 100.0,
                    makeup: 3.0,
                    mix,
                    analog_model: (*model).into(),
                    analog_drive: 12.0,
                    analog_color: 0.8,
                    analog_character: 0.3,
                    analog_trim: -2.0,
                    ..Default::default()
                };
                let input: Vec<f32> = (0..4099)
                    .flat_map(|i| {
                        let x = (0.3 * (i as f64 * 0.137).sin() + 0.1 * (i as f64 * 0.719).cos())
                            as f32;
                        [x, -x * 0.37]
                    })
                    .collect();
                let actual = process(&mut make(2, rate, &p), &input, 2, rate, &[257]);
                let clean = AnalogCompressorPluginParams {
                    analog_model: "Harmonics".into(),
                    analog_drive: 0.0,
                    analog_color: 0.0,
                    analog_character: 0.5,
                    analog_trim: 0.0,
                    ..p.clone()
                };
                let mut expected = process(&mut make(2, rate, &clean), &input, 2, rate, &[257]);
                let mut color = stage(rate, &p);
                for block in expected.chunks_mut(514) {
                    color.process_interleaved(block, block.len() / 2).unwrap();
                }
                let error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                assert!(
                    error < 2e-6,
                    "{rate} {model} mix={mix}: core/mix-before-color error {error:e}"
                );
                if mix == 0.0 {
                    let difference = actual
                        .iter()
                        .zip(&input)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert!(
                        difference > 0.01,
                        "zero compression mix must still exercise coloration"
                    );
                }
            }
        }
    }
}
