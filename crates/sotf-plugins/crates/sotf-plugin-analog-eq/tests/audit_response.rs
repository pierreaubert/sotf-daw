//! Independent bilinear-prototype response and time-varying Direct Form I references.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES};
use sotf_plugin_analog_eq::{AnalogEqPlugin, AnalogEqPluginParams};
use std::f64::consts::{PI, TAU};
use std::ops::{Add, Div, Mul};

#[derive(Clone, Copy, Default)]
struct C(f64, f64);
impl Add for C {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self(self.0 + b.0, self.1 + b.1)
    }
}
impl Mul for C {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self(self.0 * b.0 - self.1 * b.1, self.0 * b.1 + self.1 * b.0)
    }
}
impl Div for C {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        let q = b.0 * b.0 + b.1 * b.1;
        Self(
            (self.0 * b.0 + self.1 * b.1) / q,
            (self.1 * b.0 - self.0 * b.1) / q,
        )
    }
}
fn polynom(p: [f64; 3], s: C) -> C {
    C(p[0], 0.0) + s * C(p[1], 0.0) + s * s * C(p[2], 0.0)
}
fn prototypes(kind: usize, gain: f64, q: f64) -> ([f64; 3], [f64; 3]) {
    let a = 10.0_f64.powf(gain / 40.0);
    match kind {
        0 => ([a * a, a * (2.0 * a).sqrt(), a], [1.0, (2.0 * a).sqrt(), a]),
        3 => ([a, a * (2.0 * a).sqrt(), a * a], [a, (2.0 * a).sqrt(), 1.0]),
        _ => ([1.0, a / q, 1.0], [1.0, 1.0 / (a * q), 1.0]),
    }
}
fn bands(p: &AnalogEqPluginParams) -> [(f64, f64, f64); 4] {
    [
        (p.low_freq, p.low_gain, 0.71),
        (p.mid1_freq, p.mid1_gain, p.mid1_q),
        (p.mid2_freq, p.mid2_gain, p.mid2_q),
        (p.high_freq, p.high_gain, 0.71),
    ]
}
fn response(p: &AnalogEqPluginParams, rate: u32, f: f64) -> C {
    bands(p)
        .into_iter()
        .enumerate()
        .fold(C(1.0, 0.0), |h, (kind, (fc, gain, q))| {
            let (b, a) = prototypes(kind, gain, q);
            let s = C(
                0.0,
                (PI * f / f64::from(rate)).tan() / (PI * fc / f64::from(rate)).tan(),
            );
            h * polynom(b, s) / polynom(a, s)
        })
}
#[derive(Clone, Copy, Default)]
struct History {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}
fn step(x: f64, state: &mut History, kind: usize, band: (f64, f64, f64), rate: u32) -> f64 {
    let (fc, gain, q) = band;
    let (b, a) = prototypes(kind, gain, q);
    let t = (PI * fc / f64::from(rate)).tan();
    // Substitute s=(1-z^-1)/(t*(1+z^-1)) into the analog polynomials.
    let bilinear = |p: [f64; 3]| {
        [
            p[0] * t * t + p[1] * t + p[2],
            2.0 * (p[0] * t * t - p[2]),
            p[0] * t * t - p[1] * t + p[2],
        ]
    };
    let b = bilinear(b);
    let a = bilinear(a);
    let y =
        (b[0] * x + b[1] * state.x1 + b[2] * state.x2 - a[1] * state.y1 - a[2] * state.y2) / a[0];
    state.x2 = state.x1;
    state.x1 = x;
    state.y2 = state.y1;
    state.y1 = y;
    y
}
fn make(channels: usize, rate: u32, p: &AnalogEqPluginParams) -> AnalogEqPlugin {
    let mut plugin = AnalogEqPlugin::from_params(channels, p.clone()).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}
fn process(
    p: &mut AnalogEqPlugin,
    rate: u32,
    channels: usize,
    input: &[f32],
    parts: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut call = 0;
    while offset < input.len() / channels {
        let frames = parts[call % parts.len()].min(input.len() / channels - offset);
        p.process_in_place(
            &mut output[offset * channels..(offset + frames) * channels],
            &ProcessContext::new(rate, frames),
        )
        .unwrap();
        offset += frames;
        call += 1;
    }
    output
}
fn profile(extreme: bool) -> AnalogEqPluginParams {
    if extreme {
        AnalogEqPluginParams {
            low_freq: 20.0,
            low_gain: 24.0,
            mid1_freq: 100.0,
            mid1_gain: 24.0,
            mid1_q: 10.0,
            mid2_freq: 12000.0,
            mid2_gain: -24.0,
            mid2_q: 0.1,
            high_freq: 20000.0,
            high_gain: 24.0,
            ..Default::default()
        }
    } else {
        AnalogEqPluginParams {
            low_freq: 80.0,
            low_gain: 12.0,
            mid1_freq: 400.0,
            mid1_gain: -12.0,
            mid1_q: 0.7,
            mid2_freq: 7000.0,
            mid2_gain: 18.0,
            mid2_q: 8.0,
            high_freq: 13000.0,
            high_gain: -18.0,
            ..Default::default()
        }
    }
}
#[test]
fn clean_eq_complex_response_matches_independent_analog_prototypes() {
    let mut maximum = 0.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for extreme in [false, true] {
            let p = profile(extreme);
            let frames = (rate as usize * 2).next_power_of_two();
            let mut input = vec![0.0; frames];
            input[0] = 1.0;
            let impulse = process(&mut make(1, rate, &p), rate, 1, &input, &[1, 257, 8192]);
            let mut probes: Vec<f64> = (0..41)
                .map(|i| 20.0 * (f64::from(rate) * 0.49 / 20.0).powf(i as f64 / 40.0))
                .collect();
            probes.extend(bands(&p).map(|b| b.0));
            for f in probes {
                let angle = -TAU * f / f64::from(rate);
                let z = C(angle.cos(), angle.sin());
                let mut phase = C(1.0, 0.0);
                let mut actual = C::default();
                for &x in &impulse {
                    actual = actual + phase * C(f64::from(x), 0.0);
                    phase = phase * z;
                }
                let expected = response(&p, rate, f);
                let difference = (actual.0 - expected.0).hypot(actual.1 - expected.1)
                    / (1.0 + expected.0.hypot(expected.1));
                maximum = maximum.max(difference);
                assert!(
                    difference < 2e-5,
                    "{rate} extreme={extreme} f={f}: complex relative error {difference:e}"
                );
            }
        }
    }
    println!("maximum normalized complex-response error={maximum:e}");
}
#[test]
fn coefficient_steps_preserve_dfi_history_and_match_an_independent_live_reference() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let mut p = AnalogEqPluginParams::default();
        let mut plugin = make(1, rate, &p);
        let mut state = [History::default(); 4];
        let mut time = 0;
        for (frames, key, value) in [
            (513, "mid1_gain", 12.0),
            (538, "mid1_freq", 1600.0),
            (486, "mid1_q", 4.0),
            (1023, "mid1_gain", -12.0),
            (1025, "low_gain", 9.0),
        ] {
            plugin
                .set_parameter(ParameterId::from(key), ParameterValue::Float(value))
                .unwrap();
            match key {
                "mid1_gain" => p.mid1_gain = f64::from(value),
                "mid1_freq" => p.mid1_freq = f64::from(value),
                "mid1_q" => p.mid1_q = f64::from(value),
                "low_gain" => p.low_gain = f64::from(value),
                _ => unreachable!(),
            }
            let input: Vec<f32> = (time..time + frames)
                .map(|i| (0.17 * (TAU * 997.0 * i as f64 / f64::from(rate)).sin() + 0.01) as f32)
                .collect();
            let actual = process(&mut plugin, rate, 1, &input, &[1, 31, 257]);
            let mut peak = 0.0_f32;
            for (&x, &y) in input.iter().zip(&actual) {
                let mut expected = f64::from(x);
                for (kind, band) in bands(&p).into_iter().enumerate() {
                    expected = step(expected, &mut state[kind], kind, band, rate);
                }
                assert!(
                    (f64::from(y) - expected).abs() < 3e-6,
                    "{rate} control={key}: live DFI error {}",
                    f64::from(y) - expected
                );
                peak = peak.max(y.abs());
            }
            assert!(
                peak < 2.0,
                "bounded fixture transient exceeded 2.0: {rate} {key} {peak}"
            );
            time += frames;
        }
    }
}
fn color_stage(rate: u32, channels: usize, p: &AnalogEqPluginParams) -> AnalogColorStage {
    let mut stage = AnalogColorStage::new(channels);
    stage
        .set_model_id(
            MODEL_NAMES
                .iter()
                .position(|&m| m == p.analog_model)
                .unwrap() as u32,
        )
        .unwrap();
    for prepared in [false, true] {
        if prepared {
            stage.prepare(f64::from(rate), 8192).unwrap();
        }
        stage.set_drive_db(p.analog_drive as f32).unwrap();
        stage.set_color(p.analog_color as f32).unwrap();
        stage.set_character(p.analog_character as f32).unwrap();
        stage.set_output_trim_db(p.analog_trim as f32).unwrap();
    }
    stage
}
#[test]
fn eq_precedes_color_and_flat_zero_color_nulls_for_every_model() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in MODEL_NAMES {
            let input: Vec<f32> = (0..8197)
                .flat_map(|i| {
                    let x =
                        (0.31 * (i as f64 * 0.071).sin() + 0.07 * (i as f64 * 0.731).cos()) as f32;
                    [x, -x * 0.37]
                })
                .collect();
            let p = AnalogEqPluginParams {
                analog_model: (*model).into(),
                analog_drive: 18.0,
                analog_color: 0.8,
                analog_character: 0.3,
                analog_trim: -2.0,
                ..profile(false)
            };
            let clean = AnalogEqPluginParams {
                analog_model: "Harmonics".into(),
                analog_drive: 0.0,
                analog_color: 0.0,
                analog_character: 0.5,
                analog_trim: 0.0,
                ..p.clone()
            };
            let actual = process(&mut make(2, rate, &p), rate, 2, &input, &[257]);
            let mut expected = process(&mut make(2, rate, &clean), rate, 2, &input, &[257]);
            let mut stage = color_stage(rate, 2, &p);
            for block in expected.chunks_mut(514) {
                stage.process_interleaved(block, block.len() / 2).unwrap();
            }
            let error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                error < 2e-6,
                "{rate} {model}: EQ-before-color error {error:e}"
            );
            let mut reversed = input.clone();
            let mut stage = color_stage(rate, 2, &p);
            for block in reversed.chunks_mut(514) {
                stage.process_interleaved(block, block.len() / 2).unwrap();
            }
            let reversed = process(&mut make(2, rate, &clean), rate, 2, &reversed, &[257]);
            let separation = actual
                .iter()
                .zip(reversed)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                separation > 1e-4,
                "{rate} {model}: order fixture must distinguish the reversed chain"
            );
            let flat = AnalogEqPluginParams {
                analog_model: (*model).into(),
                analog_drive: 24.0,
                analog_character: 1.0,
                analog_color: 0.0,
                ..Default::default()
            };
            let null = process(&mut make(2, rate, &flat), rate, 2, &input, &[1, 31, 8192]);
            let error = null
                .iter()
                .zip(&input)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                error < 2e-7,
                "{rate} {model}: zero-color flat null {error:e}"
            );
        }
    }
}
