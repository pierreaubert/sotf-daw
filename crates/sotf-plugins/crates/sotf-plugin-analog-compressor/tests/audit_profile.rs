//! Local diagnostic timings; these samples are not realtime deadline guarantees.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::MODEL_NAMES;
use sotf_plugin_analog_compressor::{AnalogCompressorPlugin, AnalogCompressorPluginParams};
use std::time::Instant;

#[test]
fn measure_prepared_high_channel_count_cost_without_a_timing_gate() {
    for rate in [48_000, 192_000] {
        for channels in [2, 8, 32] {
            for model in MODEL_NAMES {
                for color in [0.0, 0.7] {
                    let mut plugin = AnalogCompressorPlugin::from_params(
                        channels,
                        AnalogCompressorPluginParams {
                            threshold: -24.0,
                            ratio: 8.0,
                            attack: 1.0,
                            release: 80.0,
                            auto_makeup: true,
                            analog_model: (*model).into(),
                            analog_drive: 12.0,
                            analog_color: color,
                            ..Default::default()
                        },
                    )
                    .unwrap();
                    plugin.initialize(f64::from(rate)).unwrap();
                    let input: Vec<f32> = (0..512 * channels)
                        .map(|i| 0.3 * (i as f64 * 0.137).sin() as f32)
                        .collect();
                    let mut output = input.clone();
                    let context = ProcessContext::new(rate, 512);
                    for _ in 0..8 {
                        output.copy_from_slice(&input);
                        plugin.process_in_place(&mut output, &context).unwrap();
                    }
                    let mut timings = Vec::with_capacity(64);
                    for _ in 0..64 {
                        output.copy_from_slice(&input);
                        let start = Instant::now();
                        plugin
                            .process_in_place(std::hint::black_box(&mut output), &context)
                            .unwrap();
                        timings.push(start.elapsed().as_secs_f64() * 1e6);
                        assert!(output.iter().all(|x| x.is_finite()));
                    }
                    timings.sort_by(f64::total_cmp);
                    println!(
                        "PROFILE {}",
                        serde_json::json!({"rate":rate,"channels":channels,"model":model,"color":color,
                    "block_frames":512,"samples":64,"median_us":timings[32],"p95_us":timings[60],"max_us":timings[63],
                    "block_period_us":512.0/f64::from(rate)*1e6,"os":std::env::consts::OS,"arch":std::env::consts::ARCH})
                    );
                }
            }
        }
    }
}
