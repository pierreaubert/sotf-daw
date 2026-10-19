//! Final-chain limiter safety through the prepared factory host and finite drain.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, ParameterValue, create_plugin};
use std::f64::consts::TAU;
#[path = "../crates/sotf-plugin-limiter/tests/support/reconstruction.rs"]
mod reconstruction;
use reconstruction::{db, reconstructed_peak, sample_peak};

fn render(
    rate: u32,
    choice: usize,
    link: f32,
    phase: f64,
    parts: &[usize],
    automate: bool,
) -> Vec<f32> {
    let preset = serde_json::json!({"threshold_db":-12.0,"release_ms":10.0,"lookahead_ms":0.37,
        "true_peak":true,"isp_mode":true,"oversampling":choice,"link_amount":link});
    let saved = serde_json::to_string(&preset).unwrap();
    let reloaded = serde_json::from_str(&saved).unwrap();
    let mut host = DawHost::new(2, rate);
    host.set_max_block_frames(4096).unwrap();
    host.add_plugin(create_plugin("gain", &serde_json::json!({"gain_db":12.0}), 2, rate).unwrap())
        .unwrap();
    host.add_plugin(create_plugin("limiter", &reloaded, 2, rate).unwrap())
        .unwrap();
    host.build().unwrap();
    let input: Vec<f32> = (0..4099)
        .flat_map(|i| {
            let a = (0.43 * (TAU * 0.491 * i as f64 + phase).sin()) as f32;
            let b = (0.27 * (TAU * 0.473 * i as f64 - phase).cos()) as f32;
            [a, b]
        })
        .collect();
    let mut output = vec![0.0; input.len()];
    let mut offset = 0;
    let mut block = 0;
    while offset < input.len() / 2 {
        if automate && offset % 73 == 0 {
            let event = offset / 73;
            for (key, value) in [
                (
                    "threshold",
                    ParameterValue::Float([-12.0, -18.0, -15.0][event % 3]),
                ),
                (
                    "release",
                    ParameterValue::Float([10.0, 50.0, 200.0][event % 3]),
                ),
                (
                    "link_amount",
                    ParameterValue::Float([0.0, 0.5, 1.0][event % 3]),
                ),
                ("dual_release", ParameterValue::Bool(event % 2 == 0)),
                ("true_peak", ParameterValue::Bool(event % 2 == 1)),
            ] {
                host.set_plugin_parameter(1, key, value).unwrap();
            }
            // An incompatible immediate edit must not compromise the next
            // callback, including after older valid controls have been queued.
            assert!(
                host.set_plugin_parameter_immediate(1, "mix", ParameterValue::Float(0.5))
                    .is_err()
            );
            assert!(
                host.set_plugin_parameter_immediate(1, "soft", ParameterValue::Bool(true))
                    .is_err()
            );
        }
        let frames = parts[block % parts.len()].min(input.len() / 2 - offset);
        let frames = if automate {
            frames.min(73 - offset % 73)
        } else {
            frames
        };
        host.process(
            &input[offset * 2..(offset + frames) * 2],
            &mut output[offset * 2..(offset + frames) * 2],
        )
        .unwrap();
        offset += frames;
        block += 1;
    }
    for _ in 0..1000 {
        let mut tail = vec![1234.0; host.drain_output_frames_max() * 2];
        let result = host.drain(&mut tail).unwrap();
        output.extend_from_slice(&tail[..result.frames * 2]);
        if result.complete {
            return output;
        }
    }
    panic!("final host chain drain did not complete")
}
#[test]
fn final_gain_limiter_chain_bounds_random_phase_audio_and_rapid_controls() {
    let mut state = 0xb7f2_0918_u32;
    let mut maximum = -120.0_f64;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for choice in 0..3 {
            for (link, automate) in [(0.0, false), (0.5, false), (1.0, false), (0.5, true)] {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                let phase = TAU * f64::from(state) / f64::from(u32::MAX);
                let large = render(rate, choice, link, phase, &[4096], automate);
                let split = render(rate, choice, link, phase, &[1, 31, 257], automate);
                assert!(
                    large == split,
                    "{rate} choice={choice} link={link} automation={automate}: host callbacks changed output"
                );
                for ch in 0..2 {
                    let sample = db(sample_peak(&large, 2, ch));
                    let peak = db(reconstructed_peak(&large, 2, ch, rate));
                    maximum = maximum.max(peak);
                    assert!(
                        sample <= -12.0 + 1e-5,
                        "final host sample ceiling: {sample}"
                    );
                    assert!(
                        peak <= -11.9,
                        "final host ISP {rate} choice={choice} link={link} phase={phase}: {peak}"
                    );
                    assert!(peak > -30.0, "silence must not pass final-chain safety");
                }
            }
        }
    }
    println!("maximum full-host reconstructed level={maximum} dBTP");
}
