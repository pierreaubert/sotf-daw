//! Small reproducible setup/process baseline for AUD145 legacy EQ routing.
//!
//! Run with `AUD145_BENCH_OUTPUT=... cargo bench -p sotf-plugin-eq
//! --bench aud145_legacy_timing`. The same source and cases are retained for
//! the post-change comparison; do not edit the fixture between runs.

use sotf_host::{ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};
use std::fs;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Instant;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const BLOCK_FRAMES: usize = 256;
const WARMUP_CALLBACKS: usize = 1_024;
const SETUP_REPETITIONS: usize = 32;
const CALLBACKS_PER_SAMPLE: usize = 512;
const SAMPLES: usize = 9;

struct LegacyCase {
    name: &'static str,
    params: EqPluginParams,
    oversampling: i32,
    global_topology: i32,
}

fn band(filter_type: &str, freq: f64, q: f64, db_gain: f64) -> serde_json::Value {
    serde_json::json!({
        "filter_type": filter_type,
        "freq": freq,
        "q": q,
        "db_gain": db_gain,
        "order": 2,
        "topology": "biquad"
    })
}

fn params(
    filters: serde_json::Value,
    channel_filters: Option<serde_json::Value>,
    auto_gain: serde_json::Value,
) -> EqPluginParams {
    let mut value = serde_json::json!({
        "filters": filters,
        "auto_gain": auto_gain
    });
    if let Some(channel_filters) = channel_filters {
        value["channel_filters"] = channel_filters;
    }
    serde_json::from_value(value).expect("deserialize stable legacy EQ benchmark settings")
}

fn cases() -> Vec<LegacyCase> {
    let mixed = vec![
        band("peak", 630.0, 1.2, 5.0),
        serde_json::json!({
            "filter_type": "lowpass",
            "freq": 4_300.0,
            "q": 0.8,
            "db_gain": -1.0,
            "order": 2,
            "topology": "warped_biquad",
            "lambda": 0.25
        }),
        serde_json::json!({
            "filter_type": "peak",
            "freq": 8_200.0,
            "q": 1.4,
            "db_gain": 0.0,
            "order": 2,
            "topology": "kautz_filter",
            "kautz_sections": [{ "pole_freq": 8_200.0, "q": 1.4, "gain": 0.04 }]
        }),
        band("lowshelf", 120.0, 0.8, -3.0),
    ];
    let channel_filters = vec![
        vec![band("peak", 800.0, 0.9, 4.0)],
        vec![band("peak", 1_700.0, 1.3, -5.0)],
    ];
    let auto_gain = serde_json::json!({
        "enabled": true,
        "max_gain_db": 6.0,
        "smoothing_ms": 80.0
    });

    vec![
        LegacyCase {
            name: "mixed_biquad_warped_kautz",
            params: params(mixed.into(), None, serde_json::json!({ "enabled": false })),
            oversampling: 1,
            global_topology: 0,
        },
        LegacyCase {
            name: "channel_filters",
            params: params(
                serde_json::json!([]),
                Some(channel_filters.into()),
                serde_json::json!({ "enabled": false }),
            ),
            oversampling: 1,
            global_topology: 0,
        },
        LegacyCase {
            name: "shared_4x_biquad",
            params: params(
                serde_json::json!([
                    band("highpass", 42.0, 0.707, 0.0),
                    band("peak", 1_150.0, 2.0, 6.0)
                ]),
                None,
                serde_json::json!({ "enabled": false }),
            ),
            oversampling: 4,
            global_topology: 0,
        },
        LegacyCase {
            name: "svf_peak",
            params: params(
                serde_json::json!([band("peak", 1_000.0, 1.0, 6.0)]),
                None,
                serde_json::json!({ "enabled": false }),
            ),
            oversampling: 1,
            global_topology: 1,
        },
        LegacyCase {
            name: "autogain_enabled",
            params: params(
                serde_json::json!([band("peak", 1_000.0, 1.0, 9.0)]),
                None,
                auto_gain,
            ),
            oversampling: 1,
            global_topology: 0,
        },
    ]
}

fn make_plugin(case: &LegacyCase) -> EqPlugin {
    let mut plugin = EqPlugin::from_params(CHANNELS, SAMPLE_RATE, case.params.clone())
        .expect("construct legacy benchmark EQ");
    plugin
        .parametric_set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(case.oversampling),
        )
        .unwrap();
    plugin
        .parametric_set_parameter(
            ParameterId::from("topology"),
            ParameterValue::Int(case.global_topology),
        )
        .unwrap();
    plugin.plugin_initialize(SAMPLE_RATE).unwrap();
    plugin
}

fn input_block() -> Vec<f32> {
    (0..BLOCK_FRAMES * CHANNELS)
        .map(|index| {
            let frame = index / CHANNELS;
            let channel = index % CHANNELS;
            let time = frame as f64 / f64::from(SAMPLE_RATE);
            let frequency = 113.0 + 71.0 * channel as f64;
            (0.13 * (std::f64::consts::TAU * frequency * time).sin()
                + 0.04 * (std::f64::consts::TAU * 1_100.0 * time).cos()) as f32
        })
        .collect()
}

fn main() {
    let output_path = PathBuf::from(
        std::env::var_os("AUD145_BENCH_OUTPUT")
            .expect("set AUD145_BENCH_OUTPUT to a CSV output path"),
    );
    let input = input_block();
    let mut output = vec![0.0; input.len()];
    let context = ProcessContext::new(SAMPLE_RATE, BLOCK_FRAMES);
    let mut csv = String::from("case,phase,sample,ns_per_operation\n");

    for case in cases() {
        for _ in 0..8 {
            black_box(make_plugin(&case));
        }
        for sample in 0..SAMPLES {
            let start = Instant::now();
            for _ in 0..SETUP_REPETITIONS {
                let plugin = make_plugin(&case);
                black_box(&plugin);
                drop(plugin);
            }
            let elapsed_ns = start.elapsed().as_secs_f64() * 1_000_000_000.0;
            csv.push_str(&format!(
                "{},setup,{sample},{:.3}\n",
                case.name,
                elapsed_ns / SETUP_REPETITIONS as f64
            ));
        }

        let mut plugin = make_plugin(&case);
        for _ in 0..WARMUP_CALLBACKS {
            plugin
                .process(&input, &mut output, &context)
                .expect("warm legacy EQ callback");
        }
        for sample in 0..SAMPLES {
            let start = Instant::now();
            for _ in 0..CALLBACKS_PER_SAMPLE {
                plugin
                    .process(&input, &mut output, &context)
                    .expect("timed legacy EQ callback");
                black_box(&output);
            }
            let elapsed_ns = start.elapsed().as_secs_f64() * 1_000_000_000.0;
            csv.push_str(&format!(
                "{},process,{sample},{:.3}\n",
                case.name,
                elapsed_ns / CALLBACKS_PER_SAMPLE as f64
            ));
        }
    }

    fs::write(output_path, csv).expect("write raw AUD145 timing samples");
}
