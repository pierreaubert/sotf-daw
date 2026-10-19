//! Final-output reconstruction and transactional ISP configuration checks.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::f64::consts::TAU;
mod support {
    pub mod reconstruction;
}
use support::reconstruction::{db, reconstructed_peak, sample_peak};

fn make(rate: u32, choice: usize, link: f32) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db":-12.0,"release_ms":10.0,"lookahead_ms":0.37,
        "true_peak":true,"isp_mode":true,"oversampling":choice,"link_amount":link
    }))
    .unwrap();
    let mut p = LimiterPlugin::from_params(2, params);
    p.initialize(f64::from(rate)).unwrap();
    p
}
fn render(p: &mut LimiterPlugin, rate: u32, input: &[f32], parts: &[usize]) -> Vec<f32> {
    render_controls(p, rate, input, parts, false)
}
fn render_controls(
    p: &mut LimiterPlugin,
    rate: u32,
    input: &[f32],
    parts: &[usize],
    automate: bool,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut block = 0;
    while offset < input.len() / 2 {
        if automate && offset % 73 == 0 {
            let event = offset / 73;
            let values = [
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
                ("feed_forward", ParameterValue::Bool(event % 2 == 0)),
            ]
            .into_iter()
            .map(|(key, value)| (ParameterId::from(key), value))
            .collect();
            p.apply_values(values).unwrap();
        }
        let frames = parts[block % parts.len()].min(input.len() / 2 - offset);
        let frames = if automate {
            frames.min(73 - offset % 73)
        } else {
            frames
        };
        p.process_in_place(
            &mut output[offset * 2..(offset + frames) * 2],
            &ProcessContext::new(rate, frames),
        )
        .unwrap();
        offset += frames;
        block += 1;
    }
    p.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
    for call in 0..1000 {
        let frames = [1, 17, 256][call % 3];
        let mut buffer = vec![1234.0; frames * 2];
        let result = p
            .drain(&mut buffer, &ProcessContext::new(rate, frames))
            .unwrap();
        assert!(buffer[result.frames * 2..].iter().all(|&x| x == 1234.0));
        output.extend_from_slice(&buffer[..result.frames * 2]);
        if result.complete {
            return output;
        }
    }
    panic!("limiter drain did not complete")
}
fn source(frequency: f64, phase: f64) -> Vec<f32> {
    (0..2049)
        .flat_map(|i| {
            let edge = if i == 0 || i == 2048 { 0.6 } else { 0.0 };
            let a = (1.1 * (TAU * frequency * i as f64 + phase).sin()) as f32 + edge;
            let b =
                (0.7 * (TAU * (frequency - 0.017) * i as f64 - phase * 0.7).cos()) as f32 - edge;
            [a, b]
        })
        .collect()
}
#[test]
fn randomized_high_frequency_phases_respect_final_emitted_isp_criterion() {
    let mut state = 0x1718_ef56_u32;
    let mut maximum = -120.0_f64;
    let mut cases = 0;
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for choice in 0..3 {
            for link in [0.0, 0.5, 1.0] {
                let mut p = make(rate, choice, link);
                for frequency in [0.227, 0.371, 0.437, 0.473, 0.491] {
                    for _ in 0..4 {
                        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                        let phase = TAU * f64::from(state) / f64::from(u32::MAX);
                        p.reset();
                        let output = render(&mut p, rate, &source(frequency, phase), &[1, 31, 257]);
                        for ch in 0..2 {
                            let sample = db(sample_peak(&output, 2, ch));
                            let reconstructed = db(reconstructed_peak(&output, 2, ch, rate));
                            maximum = maximum.max(reconstructed);
                            assert!(
                                sample <= -12.0 + 1e-5,
                                "sample rate={rate} choice={choice} link={link} f={frequency} phase={phase}: {sample}"
                            );
                            assert!(
                                reconstructed <= -11.9,
                                "reconstruction rate={rate} choice={choice} link={link} f={frequency} phase={phase}: {reconstructed}"
                            );
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    println!("{cases} randomized phase cases; maximum reconstructed level={maximum} dBTP");
}
#[test]
fn rejected_bulk_controls_leave_all_state_and_audio_unchanged() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for choice in 0..3 {
            let input = source(0.473, 0.37);
            for invalid in [
                vec![
                    ("dual_release", ParameterValue::Bool(true)),
                    ("mix", ParameterValue::Float(0.5)),
                ],
                vec![
                    ("release", ParameterValue::Float(50.0)),
                    ("soft", ParameterValue::Bool(true)),
                ],
                vec![
                    ("release", ParameterValue::Float(50.0)),
                    ("threshold", ParameterValue::Float(f32::NAN)),
                ],
                vec![
                    ("dual_release", ParameterValue::Bool(true)),
                    ("lookahead", ParameterValue::Float(1.0)),
                ],
                vec![
                    ("dual_release", ParameterValue::Bool(true)),
                    ("z_unknown", ParameterValue::Float(1.0)),
                ],
            ] {
                let mut used = make(rate, choice, 0.5);
                let mut baseline = make(rate, choice, 0.5);
                for p in [&mut used, &mut baseline] {
                    let mut warm = input[..514].to_vec();
                    p.process_in_place(&mut warm, &ProcessContext::new(rate, 257))
                        .unwrap();
                }
                let before = used.current_values();
                let values = invalid
                    .into_iter()
                    .map(|(key, value)| (ParameterId::from(key), value))
                    .collect();
                assert!(used.apply_values(values).is_err());
                assert_eq!(
                    used.current_values(),
                    before,
                    "rejected batch partially mutated state at {rate}, choice {choice}"
                );
                let a = render(&mut used, rate, &input, &[31]);
                let b = render(&mut baseline, rate, &input, &[257]);
                assert!(
                    a == b,
                    "rejected batch changed populated DSP history at {rate}, choice {choice}"
                );
            }
        }
    }
}

#[test]
fn rapid_valid_batches_keep_the_ceiling_and_one_clock_through_eof() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for choice in 0..3 {
            let input = source(0.491, 2.71);
            let mut a = make(rate, choice, 0.5);
            let mut b = make(rate, choice, 0.5);
            let large = render_controls(&mut a, rate, &input, &[4096], true);
            let split = render_controls(&mut b, rate, &input, &[1, 31, 257], true);
            assert!(
                large == split,
                "rate={rate}, factor choice={choice}: rapid control clocks differ"
            );
            for ch in 0..2 {
                assert!(db(sample_peak(&large, 2, ch)) <= -12.0 + 1e-5);
                let peak = db(reconstructed_peak(&large, 2, ch, rate));
                assert!(
                    peak <= -11.9,
                    "rate={rate} choice={choice} ch={ch}: rapid-control ISP {peak}"
                );
            }
        }
    }
}
#[test]
fn joint_isp_preparation_validates_final_state_instead_of_sorted_key_order() {
    for choice in 0..3 {
        let mut p = LimiterPlugin::new(2, -12.0, 10.0, 0.0, true);
        let values = [
            ("isp_mode", ParameterValue::Bool(true)),
            ("lookahead", ParameterValue::Float(0.37)),
            ("soft", ParameterValue::Bool(false)),
            ("oversampling", ParameterValue::Int(choice)),
        ]
        .into_iter()
        .map(|(key, value)| (ParameterId::from(key), value))
        .collect();
        p.apply_values(values).unwrap();
        p.initialize(48_000.0).unwrap();
        let output = render(&mut p, 48_000, &source(0.437, 0.9), &[257]);
        assert!(db(reconstructed_peak(&output, 2, 0, 48_000)) <= -11.9);
    }
}

#[test]
fn eos_accepts_identical_snapshots_and_rejects_changed_batches_until_reset() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for choice in 0..3 {
            let mut p = make(rate, choice, 0.5);
            let input = source(0.473, 0.37);
            let original = render(&mut p, rate, &input, &[257]);
            let current = p.current_values();
            p.apply_values(current.clone()).unwrap();
            let mut changed = current.clone();
            changed.insert(
                ParameterId::from("dual_release"),
                ParameterValue::Bool(true),
            );
            changed.insert(ParameterId::from("release"), ParameterValue::Float(50.0));
            assert!(p.apply_values(changed).is_err());
            assert_eq!(p.current_values(), current);
            p.reset();
            let restarted = render(&mut p, rate, &input, &[31]);
            assert!(
                original == restarted,
                "rate={rate} factor choice={choice}: EOS controls leaked after reset"
            );
        }
    }
}

#[test]
fn unchanged_eos_snapshot_preserves_legacy_constructor_release_range() {
    for choice in 0..3 {
        let mut p = LimiterPlugin::new(2, -12.0, 5.0, 0.37, false);
        p.set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(choice),
        )
        .unwrap();
        p.initialize(48_000.0).unwrap();
        assert_eq!(
            p.get_parameter(&ParameterId::from("release")),
            Some(ParameterValue::Float(5.0))
        );
        let _ = render(&mut p, 48_000, &source(0.473, 0.37), &[257]);
        let current = p.current_values();
        p.apply_values(current.clone()).unwrap();
        assert_eq!(p.current_values(), current);
    }
}
