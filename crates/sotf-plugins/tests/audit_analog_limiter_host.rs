//! Analog limiter safety after native/downsampled coloration and through host EOF.

// Rust guideline compliant 2026-02-21
use sotf_host::oversampling::AutoOversampledPlugin;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParameterValue, Plugin, TailLength};
use sotf_plugins::{DawHost, create_plugin};
use std::f64::consts::TAU;
#[path = "../crates/sotf-plugin-limiter/tests/support/reconstruction.rs"]
mod reconstruction;
use reconstruction::{db, reconstructed_peak, reconstructed_peak_at_factor, sample_peak};
#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn host(rate: u32, model: &str, factor: u32, color: f32, protect: bool) -> DawHost {
    let preset = serde_json::json!({"threshold":-12.0,"release":10.0,"lookahead":0.37,
        "true_peak":true,"analog_model":model,"analog_drive":24.0,"analog_color":color,
        "analog_character":0.7,"analog_trim":6.0});
    let saved = serde_json::to_string(&preset).unwrap();
    let reloaded = serde_json::from_str(&saved).unwrap();
    let colored = create_plugin("analog_limiter", &reloaded, 2, rate).unwrap();
    let colored: Box<dyn Plugin> = if factor == 1 {
        colored
    } else {
        Box::new(AutoOversampledPlugin::new_with_max_frames(colored, factor, 4096).unwrap())
    };
    let mut h = DawHost::new(2, rate);
    h.set_max_block_frames(4096).unwrap();
    h.add_plugin(colored).unwrap();
    if protect {
        let final_stage = serde_json::json!({"threshold_db":-12.0,"release_ms":10.0,"lookahead_ms":0.37,
            "isp_mode":true,"true_peak":true,"soft":false,"mix":1.0,"oversampling":0});
        h.add_plugin(create_plugin("limiter", &final_stage, 2, rate).unwrap())
            .unwrap();
    }
    h.build().unwrap();
    h
}
fn drain(h: &mut DawHost) -> Vec<f32> {
    let mut output = Vec::new();
    for _ in 0..10000 {
        let mut block = vec![1234.0; h.drain_output_frames_max() * 2];
        let result = h.drain(&mut block).unwrap();
        output.extend_from_slice(&block[..result.frames * 2]);
        if result.complete {
            return output;
        }
    }
    panic!("host EOF did not complete")
}
fn render(h: &mut DawHost, rate: u32, phase: f64, parts: &[usize], automate: bool) -> Vec<f32> {
    // 1024 trailing zero frames are an explicit render budget for recursive
    // coloration. drain completion below does not prove mathematical support.
    let input: Vec<f32> = (0..5123)
        .flat_map(|i| {
            if i >= 4099 {
                [0.0, 0.0]
            } else {
                [
                    (1.1 * (TAU * 0.473 * i as f64 + phase).sin()) as f32,
                    (0.7 * (TAU * 0.491 * i as f64 - phase).cos()) as f32,
                ]
            }
        })
        .collect();
    let mut output = vec![0.0; input.len()];
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / 2 {
        if automate && pos % 257 == 0 {
            let event = pos / 257;
            h.set_plugin_parameter(
                0,
                "analog_color",
                ParameterValue::Float([0.2, 0.8, 1.0][event % 3]),
            )
            .unwrap();
            h.set_plugin_parameter(
                0,
                "analog_drive",
                ParameterValue::Float([12.0, 24.0, 36.0][event % 3]),
            )
            .unwrap();
        }
        let n = parts[call % parts.len()].min(input.len() / 2 - pos);
        let n = if automate { n.min(257 - pos % 257) } else { n };
        h.process(
            &input[pos * 2..(pos + n) * 2],
            &mut output[pos * 2..(pos + n) * 2],
        )
        .unwrap();
        pos += n;
        call += 1;
    }
    assert_eq!(
        h.tail_length(),
        TailLength::Infinite,
        "active color is not a finite tail proof at {rate}"
    );
    output.extend(drain(h));
    output
}
#[test]
fn post_downsampling_isp_stage_bounds_the_final_host_chain_during_color_changes() {
    let mut random = 0x51fc_0618_u32;
    let mut unprotected = -120.0_f64;
    let mut protected = -120.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in sotf_plugin_analog_common::MODEL_NAMES {
            for factor in [1, 2, 4] {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                let phase = TAU * f64::from(random) / f64::from(u32::MAX);
                let mut a = host(rate, model, factor, 1.0, true);
                let mut b = host(rate, model, factor, 1.0, true);
                let large = render(&mut a, rate, phase, &[4096], true);
                let split = render(&mut b, rate, phase, &[1, 31, 257], true);
                assert!(
                    large == split,
                    "{rate} {model} {factor}x: queued color/OS/EOF output depends on callback size"
                );
                for ch in 0..2 {
                    let sample = db(sample_peak(&large, 2, ch));
                    let peak = db(reconstructed_peak(&large, 2, ch, rate));
                    protected = protected.max(peak);
                    assert!(
                        sample <= -12.0 + 1e-5,
                        "final sample peak {rate} {model} {factor}x: {sample}"
                    );
                    assert!(
                        peak <= -11.9,
                        "final protected peak {rate} {model} {factor}x: {peak}"
                    );
                    assert!(peak > -36.0, "silence must not pass protection");
                }
                let mut raw = host(rate, model, factor, 1.0, false);
                let raw = render(&mut raw, rate, phase, &[257], true);
                unprotected = unprotected.max(db(reconstructed_peak_at_factor(&raw, 2, 0, 4)));
            }
        }
    }
    assert!(
        unprotected > -11.8,
        "the characterization must expose unprotected reconstructed overshoot"
    );
    println!(
        "maximum unprotected 4x reconstruction={unprotected} dBTP; final protected rate-appropriate peak={protected} dBTP"
    );
}
#[test]
fn zero_color_eof_is_exact_while_color_transitions_keep_infinite_metadata_until_reset() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for model in sotf_plugin_analog_common::MODEL_NAMES {
            for factor in [1, 2, 4] {
                let mut h = host(rate, model, factor, 0.0, false);
                assert!(matches!(h.tail_length(), TailLength::Finite(_)));
                let latency = h.total_latency_samples();
                let mut input = vec![0.0; 514];
                input[0] = 0.01;
                input[3] = -0.0037;
                let mut output = vec![0.0; input.len()];
                h.process(&input, &mut output).unwrap();
                output.extend(drain(&mut h));
                assert!(output.iter().any(|&x| x != 0.0));
                for ch in 0..2 {
                    let peak = (0..output.len() / 2)
                        .max_by(|&a, &b| {
                            output[a * 2 + ch]
                                .abs()
                                .total_cmp(&output[b * 2 + ch].abs())
                        })
                        .unwrap();
                    // Inner lookahead can project to a fractional native frame
                    // after resampling; integer metadata then rounds that delay.
                    assert!(peak.abs_diff(latency + ch) <= usize::from(factor > 1));
                }
                // Independent EOF reference: ordinary zero continuation of a
                // fresh host, with no calls to the drain scheduler.
                let mut reference_host = host(rate, model, factor, 0.0, false);
                let mut padded = input.clone();
                padded.resize(output.len(), 0.0);
                let mut reference = vec![0.0; padded.len()];
                for (src, dst) in padded.chunks(62).zip(reference.chunks_mut(62)) {
                    reference_host.process(src, dst).unwrap();
                }
                assert!(
                    output == reference,
                    "{rate} {model} {factor}x: finite host drain differs from ordinary zero continuation"
                );
                h.reset();
                for value in [0.7, 0.0] {
                    h.set_plugin_parameter_immediate(
                        0,
                        "analog_color",
                        ParameterValue::Float(value),
                    )
                    .unwrap();
                    assert_eq!(h.tail_length(), TailLength::Infinite);
                }
                h.reset();
                assert!(matches!(h.tail_length(), TailLength::Finite(_)));
                let mut actual = vec![0.0; input.len()];
                h.process(&input, &mut actual).unwrap();
                actual.extend(drain(&mut h));
                assert!(
                    actual == output,
                    "{rate} {model} {factor}x: finite zero-color EOS/reset changed retained output"
                );
            }
        }
    }
}
#[test]
fn queued_analog_limiter_controls_apply_without_callback_heap_activity() {
    for model in sotf_plugin_analog_common::MODEL_NAMES {
        let mut h = host(48_000, model, 1, 0.0, false);
        for (key, value) in [
            ("threshold", -18.0),
            ("release", 80.0),
            ("mix", 0.7),
            ("analog_drive", 12.0),
            ("analog_color", 0.7),
            ("analog_character", 0.3),
            ("analog_trim", -3.0),
        ] {
            h.set_plugin_parameter(0, key, ParameterValue::Float(value))
                .unwrap();
        }
        h.set_plugin_parameter(0, "soft", ParameterValue::Bool(true))
            .unwrap();
        h.set_plugin_parameter(0, "true_peak", ParameterValue::Bool(true))
            .unwrap();
        let input = [0.3; 514];
        let mut output = [0.0; 514];
        assert_no_allocs_or_deallocs("Analog Limiter actual queued host callback", || {
            h.process(&input, &mut output).unwrap();
        });
    }
}
