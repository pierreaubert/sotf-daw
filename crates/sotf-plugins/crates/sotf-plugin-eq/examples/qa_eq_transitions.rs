//! Opt-in EQ oversampled automation gate; exits nonzero on partition differences.
//! Run with cargo run -p sotf-plugin-eq --example qa_eq_transitions --features qa.

// Rust guideline compliant 2026-02-21
use sotf_host::{AutoGainParams, ParameterId, ParameterValue, Plugin};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
};
use std::f64::consts::TAU;

#[global_allocator]
static ALLOCATOR: sotf_host::CountingAlloc = sotf_host::CountingAlloc;

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

fn main() {
    let mut cases = 0;
    let mut failures = 0;
    let mut maximum = 0.0_f64;
    for rate in [44_100_u32, 48_000, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            let input = input(channels, rate);
            let mut events = events(rate);
            // Exercise automation during FIR priming, including interrupted ramps.
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
                    for ordered in [false, true] {
                        for factor in [2, 4] {
                            for warm in [false, true] {
                                let mut outputs = Vec::new();
                                let mut static_outputs = Vec::new();
                                for pattern in [&[1][..], &[17, 257, 1024][..]] {
                                    let mut p = prepared(rate, channels, order, tdf2, ordered);
                                    p.set_parameter(
                                        ParameterId::from("oversampling"),
                                        ParameterValue::Int(factor),
                                    )
                                    .unwrap();
                                    if warm {
                                        static_outputs.push(render(
                                            p.as_mut(),
                                            &input,
                                            rate,
                                            channels,
                                            &[],
                                            pattern,
                                        ));
                                    }
                                    outputs.push(render(
                                        p.as_mut(),
                                        &input,
                                        rate,
                                        channels,
                                        &events,
                                        pattern,
                                    ));
                                }
                                if warm {
                                    assert_eq!(
                                        static_outputs[0], static_outputs[1],
                                        "static oversampled EQ must be partition invariant"
                                    );
                                }
                                let difference = outputs[0]
                                    .iter()
                                    .zip(&outputs[1])
                                    .map(|(&a, &b)| (f64::from(a) - f64::from(b)).abs())
                                    .fold(0.0_f64, f64::max);
                                let first =
                                    outputs[0].iter().zip(&outputs[1]).position(|(a, b)| a != b);
                                failures += usize::from(first.is_some());
                                maximum = maximum.max(difference);
                                println!(
                                    "{}",
                                    serde_json::json!({"rate":rate,"channels":channels,"order":order,"tdf2":tdf2,"ordered":ordered,"factor":factor,"warm":warm,"first_difference_frame":first.map(|n|n/channels),"maximum_absolute_difference":difference})
                                );
                                cases += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 576);
    println!(
        "{}",
        serde_json::json!({"case":"oversampled_peak_transitions", "cases":cases,"failures":failures,"maximum_absolute_difference":maximum,"static_control_cases":288})
    );
    assert_eq!(
        failures, 0,
        "oversampled automation must be partition invariant"
    );
}
