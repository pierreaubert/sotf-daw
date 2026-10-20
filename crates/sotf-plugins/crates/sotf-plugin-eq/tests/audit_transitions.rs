//! Independent 1x standard-EQ coefficient trajectories and realization states.
//!
//! Bounds fixed before execution: absolute output error 2e-5; callback partition
//! outputs must be bit-identical. Coefficients derive from the peaking transfer
//! equation, with explicit DF-I/TDF-II histories and no production math helpers.

// Rust guideline compliant 2026-02-21
use sotf_host::{AutoGainParams, ParameterId, ParameterValue, Plugin};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
};
use std::f64::consts::{PI, TAU};

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

#[derive(Clone, Copy)]
struct Config {
    freq: f64,
    q: f64,
    gain: f64,
}
fn coefficients(c: Config, rate: u32, order: usize) -> Vec<[f64; 5]> {
    let count = order / 2;
    (0..count)
        .map(|stage| {
            let q = if count == 1 {
                c.q
            } else {
                c.q / (2.0 * (PI * (2 * stage + 1) as f64 / (2 * order) as f64).cos())
            };
            let a = 10.0_f64.powf(c.gain / count as f64 / 40.0);
            let w = TAU * c.freq / f64::from(rate);
            let alpha = w.sin() / (2.0 * q);
            let denominator = 1.0 + alpha / a;
            [
                (1.0 + alpha * a) / denominator,
                -2.0 * w.cos() / denominator,
                (1.0 - alpha * a) / denominator,
                -2.0 * w.cos() / denominator,
                (1.0 - alpha / a) / denominator,
            ]
        })
        .collect()
}
fn blend(a: [f64; 5], b: [f64; 5], t: f64) -> [f64; 5] {
    if t == 0.0 {
        return a;
    }
    if t == 1.0 {
        return b;
    }
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}
#[derive(Clone, Copy, Default)]
struct State {
    x: [f64; 2],
    y: [f64; 2],
    z: [f64; 2],
}
impl State {
    fn process(&mut self, x: f64, c: [f64; 5], tdf2: bool) -> f64 {
        if tdf2 {
            let y = c[0] * x + self.z[0];
            self.z[0] = c[1] * x - c[3] * y + self.z[1];
            self.z[1] = c[2] * x - c[4] * y;
            y
        } else {
            let y = c[0] * x + c[1] * self.x[0] + c[2] * self.x[1]
                - c[3] * self.y[0]
                - c[4] * self.y[1];
            self.x = [x, self.x[0]];
            self.y = [y, self.y[0]];
            y
        }
    }
}
struct Event {
    frame: usize,
    field: &'static str,
    value: f32,
}
fn events(rate: u32) -> Vec<Event> {
    vec![
        Event {
            frame: 257,
            field: "freq",
            value: 120.0,
        },
        Event {
            frame: 269,
            field: "gain",
            value: -12.0,
        },
        Event {
            frame: 271,
            field: "q",
            value: 20.0,
        },
        Event {
            frame: 900,
            field: "freq",
            value: 16000.0,
        },
        Event {
            frame: 1234,
            field: "gain",
            value: 18.0,
        },
        Event {
            frame: 1301,
            field: "q",
            value: 0.1,
        },
        Event {
            frame: 1500,
            field: "freq",
            value: 20.0,
        },
        Event {
            frame: 1733,
            field: "freq",
            value: (0.45 * f64::from(rate)).min(20000.0) as f32,
        },
        Event {
            frame: 2000,
            field: "q",
            value: 20.0,
        },
        Event {
            frame: 2071,
            field: "gain",
            value: -18.0,
        },
        Event {
            frame: 2089,
            field: "freq",
            value: 997.0,
        },
    ]
}
fn input(channels: usize, rate: u32) -> Vec<f32> {
    (0..6000 * channels)
        .map(|i| {
            let n = i / channels;
            let ch = i % channels;
            let phase = TAU * n as f64 / f64::from(rate);
            ((0.01 * (phase * 73.0).sin()
                + 0.007 * (phase * 1733.0).cos()
                + if n == 17 { 0.03 } else { 0.0 })
                / (ch + 1) as f64) as f32
        })
        .collect()
}
fn reference(
    input: &[f32],
    rate: u32,
    channels: usize,
    order: usize,
    tdf2: bool,
    events: &[Event],
) -> Vec<f32> {
    let mut config = Config {
        freq: 800.0,
        q: 0.7,
        gain: 6.0,
    };
    let mut old = coefficients(config, rate, order);
    let mut target = old.clone();
    let mut states = vec![vec![State::default(); order / 2]; channels];
    let length = (f64::from(rate) * 0.005) as usize;
    let mut elapsed = length;
    let mut next = 0;
    let mut output = vec![0.0; input.len()];
    for n in 0..input.len() / channels {
        if next < events.len() && events[next].frame == n {
            let e = &events[next];
            old = old
                .iter()
                .zip(&target)
                .map(|(&a, &b)| blend(a, b, elapsed.min(length) as f64 / length as f64))
                .collect();
            match e.field {
                "freq" => config.freq = f64::from(e.value),
                "q" => config.q = f64::from(e.value),
                "gain" => config.gain = f64::from(e.value),
                _ => unreachable!(),
            }
            target = coefficients(config, rate, order);
            elapsed = 0;
            next += 1;
        }
        let t = elapsed.min(length) as f64 / length as f64;
        for ch in 0..channels {
            let mut x = f64::from(input[n * channels + ch]);
            for stage in 0..order / 2 {
                let c = blend(old[stage], target[stage], t);
                // Jury inequalities for real normalized second-order sections.
                assert!(1.0 + c[3] + c[4] > 0.0 && 1.0 - c[3] + c[4] > 0.0 && 1.0 - c[4] > 0.0);
                x = states[ch][stage].process(x, c, tdf2);
            }
            output[n * channels + ch] = x as f32;
        }
        elapsed = elapsed.saturating_add(1);
    }
    output
}
fn prepared(
    rate: u32,
    channels: usize,
    order: usize,
    tdf2: bool,
    ordered: bool,
) -> Box<dyn Plugin> {
    let eq = EqPlugin::from_params(
        channels,
        rate,
        EqPluginParams {
            filters: vec![BiquadFilterConfig {
                filter_type: "peak".into(),
                freq: 800.0,
                q: 0.7,
                db_gain: 6.0,
                order,
                topology: EqFilterTopology::Biquad,
                placement: ordered.then_some(EqBandPlacement::Stereo),
                lambda: None,
                kautz_sections: vec![],
            }],
            channel_filters: None,
            stereo_pairs: None,
            auto_gain: AutoGainParams::default(),
        },
    )
    .unwrap();
    let mut plugin = eq.into_boxed_plugin();
    plugin
        .set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("tdf2"), ParameterValue::Bool(tdf2))
        .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}
fn render(
    plugin: &mut dyn Plugin,
    input: &[f32],
    rate: u32,
    channels: usize,
    events: &[Event],
    pattern: &[usize],
) -> Vec<f32> {
    let updates: Vec<_> = events
        .iter()
        .map(|e| {
            (
                ParameterId::from(format!("band_0_{}", e.field)),
                ParameterValue::Float(e.value),
            )
        })
        .collect();
    let mut output = vec![f32::NAN; input.len()];
    let (mut n, mut call, mut next) = (0, 0, 0);
    while n < input.len() / channels {
        if next < events.len() && n == events[next].frame {
            let (id, value) = &updates[next];
            let heap = sotf_host::test_utils::measure_heap_activity(|| {
                plugin.set_parameter(id.clone(), value.clone()).unwrap()
            });
            assert_eq!(heap, (0, 0));
            next += 1;
        }
        let limit = events.get(next).map_or(input.len() / channels, |e| e.frame);
        let frames = pattern[call % pattern.len()].min(limit - n);
        let context = sotf_host::ProcessContext::new(rate, frames);
        let heap = sotf_host::test_utils::measure_heap_activity(|| {
            assert_eq!(
                plugin
                    .process(
                        &input[n * channels..(n + frames) * channels],
                        &mut output[n * channels..(n + frames) * channels],
                        &context
                    )
                    .unwrap(),
                frames
            );
        });
        assert_eq!(heap, (0, 0));
        n += frames;
        call += 1;
    }
    assert!(output.iter().all(|x| x.is_finite()));
    output
}

#[test]
fn native_peak_transitions_match_independent_realizations_at_absolute_sample_events() {
    let mut cases = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            let input = input(channels, rate);
            let events = events(rate);
            for order in [2, 4, 8] {
                for tdf2 in [false, true] {
                    for ordered in [false, true] {
                        let expected = reference(&input, rate, channels, order, tdf2, &events);
                        let mut p = prepared(rate, channels, order, tdf2, ordered);
                        let actual = render(p.as_mut(), &input, rate, channels, &events, &[1]);
                        let error = actual
                            .iter()
                            .zip(&expected)
                            .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                            .fold(0.0_f64, f64::max);
                        maximum = maximum.max(error);
                        assert!(
                            error < 2e-5,
                            "reference error {error}: {rate}/{channels}/{order}/{tdf2}/{ordered}"
                        );
                        let mut p = prepared(rate, channels, order, tdf2, ordered);
                        let partitioned = render(
                            p.as_mut(),
                            &input,
                            rate,
                            channels,
                            &events,
                            &[17, 257, 1024],
                        );
                        if actual != partitioned {
                            let at = actual
                                .iter()
                                .zip(&partitioned)
                                .position(|(a, b)| a != b)
                                .unwrap();
                            panic!(
                                "partition difference frame{}: {:?}/{:?}: {rate}/{channels}/{order}/{tdf2}/{ordered}",
                                at / channels,
                                actual[at],
                                partitioned[at]
                            );
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 144);
    println!(
        "{}",
        serde_json::json!({"case":"native_peak_transitions","cases":cases,"max_reference_error":maximum})
    );
}
