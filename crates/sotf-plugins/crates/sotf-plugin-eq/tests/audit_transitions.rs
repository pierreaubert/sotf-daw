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

// The reference shares only the transport FIRs. Control timing, RBJ design and
// recursive histories are calculated independently on the upsampled timeline.
fn oversampled_reference(
    input: &[f32],
    rate: u32,
    channels: usize,
    order: usize,
    tdf2: bool,
    factor: u32,
    events: &[Event],
) -> Vec<f32> {
    let mut os = sotf_host::oversampling::Oversampler::new(factor, channels).unwrap();
    os.reserve_for_max_frames(input.len() / channels).unwrap();
    let delay = os.upsampling_delay_frames();
    let length = ((f64::from(rate) * 0.005) as usize) * factor as usize;
    let mut config = Config {
        freq: 800.0,
        q: 0.7,
        gain: 6.0,
    };
    let mut target = coefficients(config, rate * factor, order);
    let mut old = target.clone();
    let mut states = vec![vec![State::default(); order / 2]; channels];
    let (mut next, mut elapsed, mut position) = (0, length, 0);
    let mut output = input.to_vec();
    os.process(&mut output, input.len() / channels, |planar, frames| {
        for frame in 0..frames {
            while next < events.len() && position == events[next].frame * factor as usize + delay {
                let t = elapsed.min(length) as f64 / length as f64;
                for stage in 0..order / 2 {
                    old[stage] = blend(old[stage], target[stage], t);
                }
                let event = &events[next];
                match event.field {
                    "freq" => config.freq = f64::from(event.value),
                    "q" => config.q = f64::from(event.value),
                    "gain" => config.gain = f64::from(event.value),
                    _ => unreachable!(),
                }
                target = coefficients(config, rate * factor, order);
                elapsed = 0;
                next += 1;
            }
            let t = elapsed.min(length) as f64 / length as f64;
            for (channel, samples) in planar.iter_mut().enumerate() {
                let mut x = f64::from(samples[frame]);
                for stage in 0..order / 2 {
                    x = states[channel][stage].process(
                        x,
                        blend(old[stage], target[stage], t),
                        tdf2,
                    );
                }
                samples[frame] = x as f32;
            }
            elapsed += 1;
            position += 1;
        }
    })
    .unwrap();
    assert_eq!(next, events.len());
    output
}

#[test]
fn oversampled_automation_matches_independent_delayed_control_trajectory() {
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            let input = input(channels, rate);
            let mut events = events(rate);
            events.splice(
                0..0,
                [
                    Event {
                        frame: 0,
                        field: "gain",
                        value: -6.0,
                    },
                    Event {
                        frame: 1,
                        field: "freq",
                        value: 120.0,
                    },
                    Event {
                        frame: 7,
                        field: "q",
                        value: 20.0,
                    },
                    Event {
                        frame: 19,
                        field: "gain",
                        value: 12.0,
                    },
                ],
            );
            for order in [2, 4, 8] {
                for tdf2 in [false, true] {
                    for factor in [2, 4] {
                        let expected = oversampled_reference(
                            &input, rate, channels, order, tdf2, factor, &events,
                        );
                        for ordered in [false, true] {
                            let mut p = prepared(rate, channels, order, tdf2, ordered);
                            p.set_parameter(
                                ParameterId::from("oversampling"),
                                ParameterValue::Int(factor as i32),
                            )
                            .unwrap();
                            let actual = render(
                                p.as_mut(),
                                &input,
                                rate,
                                channels,
                                &events,
                                &[17, 257, 1024],
                            );
                            let error = actual
                                .iter()
                                .zip(&expected)
                                .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                                .fold(0.0_f64, f64::max);
                            maximum = maximum.max(error);
                            assert!(
                                error < 2e-5,
                                "oversampled reference error {error}: {rate}/{channels}/{order}/{tdf2}/{ordered}/{factor}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 288);
    println!(
        "{}",
        serde_json::json!({"case":"oversampled_independent_trajectory","cases":cases,"maximum_reference_error":maximum})
    );
}

#[test]
fn dense_oversampled_edits_wrap_history_and_coalesce_same_frame_targets() {
    let rate = 48_000;
    let mut events = Vec::new();
    for frame in 0..2049 {
        events.push(Event {
            frame,
            field: "freq",
            value: 400.0 + (frame % 37) as f32 * 17.0,
        });
        events.push(Event {
            frame,
            field: "gain",
            value: (frame % 19) as f32 - 9.0,
        });
        events.push(Event {
            frame,
            field: "q",
            value: 0.5 + (frame % 11) as f32 * 0.1,
        });
    }
    let mut maximum = 0.0_f64;
    let mut cases = 0;
    for channels in [1, 6] {
        for order in [2, 8] {
            for tdf2 in [false, true] {
                for factor in [2, 4] {
                    let input = input(channels, rate);
                    let expected =
                        oversampled_reference(&input, rate, channels, order, tdf2, factor, &events);
                    for ordered in [false, true] {
                        let mut p = prepared(rate, channels, order, tdf2, ordered);
                        p.set_parameter(
                            ParameterId::from("oversampling"),
                            ParameterValue::Int(factor as i32),
                        )
                        .unwrap();
                        let actual = render(
                            p.as_mut(),
                            &input,
                            rate,
                            channels,
                            &events,
                            &[257, 17, 1024],
                        );
                        let error = actual
                            .iter()
                            .zip(&expected)
                            .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                            .fold(0.0_f64, f64::max);
                        maximum = maximum.max(error);
                        assert!(
                            error < 2e-5,
                            "dense edit reference error {error}: {channels}/{order}/{tdf2}/{ordered}/{factor}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 32);
    println!(
        "{}",
        serde_json::json!({"case":"oversampled_dense_edits","cases":cases,"edits_per_case":events.len(),"maximum_reference_error":maximum})
    );
}

#[test]
fn reset_discards_buffered_oversampled_controls_without_heap_activity() {
    for factor in [2, 4] {
        for ordered in [false, true] {
            let mut dirty = prepared(48_000, 2, 8, true, ordered);
            let mut fresh = prepared(48_000, 2, 8, true, ordered);
            for p in [&mut dirty, &mut fresh] {
                p.set_parameter(
                    ParameterId::from("oversampling"),
                    ParameterValue::Int(factor),
                )
                .unwrap();
                p.set_parameter(
                    ParameterId::from("band_0_freq"),
                    ParameterValue::Float(1234.0),
                )
                .unwrap();
                p.set_parameter(
                    ParameterId::from("band_0_gain"),
                    ParameterValue::Float(-9.0),
                )
                .unwrap();
            }
            let input = input(2, 48_000);
            render(dirty.as_mut(), &input[..137 * 2], 48_000, 2, &[], &[17]);
            for p in [&mut dirty, &mut fresh] {
                let heap = sotf_host::test_utils::measure_heap_activity(|| p.reset());
                assert_eq!(heap, (0, 0));
            }
            let a = render(dirty.as_mut(), &input, 48_000, 2, &[], &[17, 257, 1024]);
            let b = render(fresh.as_mut(), &input, 48_000, 2, &[], &[1]);
            assert_eq!(
                a, b,
                "reset retained old transport/control state: {factor}/{ordered}"
            );
        }
    }
}

#[test]
fn every_band_can_automate_on_every_source_frame_without_history_overflow_or_heap_work() {
    let mut cases = 0;
    for factor in [2, 4] {
        for ordered in [false, true] {
            let mut p = EqPlugin::from_params(
                6,
                48_000,
                EqPluginParams {
                    filters: (0..20)
                        .map(|band| BiquadFilterConfig {
                            filter_type: "peak".into(),
                            freq: 300.0 + band as f64 * 100.0,
                            q: 0.7,
                            db_gain: 0.0,
                            order: 8,
                            topology: EqFilterTopology::Biquad,
                            placement: ordered.then_some(EqBandPlacement::Stereo),
                            lambda: None,
                            kautz_sections: vec![],
                        })
                        .collect(),
                    channel_filters: None,
                    stereo_pairs: None,
                    auto_gain: AutoGainParams::default(),
                },
            )
            .unwrap()
            .into_boxed_plugin();
            p.set_parameter(
                ParameterId::from("auto_gain_enabled"),
                ParameterValue::Bool(false),
            )
            .unwrap();
            p.set_parameter(
                ParameterId::from("oversampling"),
                ParameterValue::Int(factor),
            )
            .unwrap();
            p.initialize(48_000.0).unwrap();
            let ids: Vec<_> = (0..20)
                .map(|band| ParameterId::from(format!("band_{band}_gain")))
                .collect();
            let source = input(6, 48_000);
            let mut output = [0.0; 6];
            for frame in 0..1025 {
                let heap = sotf_host::test_utils::measure_heap_activity(|| {
                    for (band, id) in ids.iter().enumerate() {
                        let gain = ((frame + band) % 7) as f32 - 3.0;
                        p.set_parameter(id.clone(), ParameterValue::Float(-gain))
                            .unwrap();
                        p.set_parameter(id.clone(), ParameterValue::Float(gain))
                            .unwrap();
                    }
                    assert_eq!(
                        p.process(
                            &source[frame * 6..(frame + 1) * 6],
                            &mut output,
                            &sotf_host::ProcessContext::new(48_000, 1)
                        )
                        .unwrap(),
                        1
                    );
                });
                assert_eq!(heap, (0, 0));
                assert!(output.iter().all(|x| x.is_finite()));
            }
            cases += 1;
        }
    }
    println!(
        "{}",
        serde_json::json!({"case":"all_band_dense_automation","cases":cases,"bands":20,"channels":6,"order":8,"source_frames":1025,"edits_per_source_frame":40})
    );
}
