//! Constructor defaults and adaptive timing through actual public processing.
// Rust guideline compliant 2026-02-21
use crate::{AecPlugin, AecPluginParams};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};

#[test]
fn constructor_adaptive_timing_matches_independent_seconds_and_block_arithmetic() {
    for rate in [44100, 48000, 96000, 192000] {
        let plugin = AecPlugin::new(rate);
        let expected_alpha = (-256.0 / f64::from(rate) / 0.100).exp();
        assert!(
            (f64::from(plugin.aec.power_alpha()) - expected_alpha).abs() < 6e-8,
            "rate={rate}: actual alpha={}, expected={expected_alpha}",
            plugin.aec.power_alpha()
        );
        let expected_hold = (0.133 * f64::from(rate) / 256.0).ceil() as usize;
        assert_eq!(
            plugin.aec.transfer_threshold(),
            expected_hold,
            "rate={rate}"
        );
    }
}

fn render(plugin: &mut AecPlugin, rate: u32, source: &[f32], pattern: &[usize]) -> Vec<f32> {
    let mut output = vec![987.0; source.len() / 2];
    let mut offset = 0;
    for &size in pattern.iter().cycle() {
        let frames = size.min(output.len() - offset);
        if frames == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &source[offset * 2..(offset + frames) * 2],
                    &mut output[offset..offset + frames],
                    &ProcessContext::new(rate, frames)
                )
                .unwrap(),
            frames
        );
        offset += frames;
    }
    output
}

fn echo_source(rate: u32) -> Vec<f32> {
    let frames = rate as usize * 2;
    let mut state = 0x12345678_u32;
    let reference: Vec<f32> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as i32 as f64 / i32::MAX as f64 * 0.2) as f32
        })
        .collect();
    (0..frames)
        .flat_map(|n| {
            let delayed = |d| n.checked_sub(d).map(|i| reference[i]).unwrap_or(0.0);
            [0.6 * delayed(21) + 0.2 * delayed(113), reference[n]]
        })
        .collect()
}

#[test]
fn zero_history_public_constructors_and_resets_have_identical_canonical_waveforms() {
    for rate in [44100, 48000, 96000] {
        let source = echo_source(rate);
        for pattern in [&[1][..], &[73, 257, 511][..], &[8193][..]] {
            let mut direct = AecPlugin::new(rate);
            let mut params = AecPlugin::from_params(rate, AecPluginParams::default()).unwrap();
            let mut initialized = AecPlugin::new(rate);
            initialized.initialize(f64::from(rate)).unwrap();
            for epoch in 0..2 {
                if epoch > 0 {
                    direct.reset();
                    params.reset();
                    initialized.reset();
                }
                for p in [&direct, &params, &initialized] {
                    assert_eq!(
                        p.get_parameter(&ParameterId::from("step_size")),
                        Some(ParameterValue::Float(0.5))
                    );
                    assert_eq!(
                        p.get_parameter(&ParameterId::from("echo_tail_ms")),
                        Some(ParameterValue::Float(200.0))
                    );
                    assert_eq!(
                        p.get_parameter(&ParameterId::from("post_filter_enabled")),
                        Some(ParameterValue::Bool(true))
                    );
                    assert_eq!(p.post_filter_mix(), 1.0);
                    assert_eq!(p.latency_samples(), 256);
                    assert_eq!(p.tail_length(), params.tail_length());
                }
                let expected = render(&mut params, rate, &source, pattern);
                assert_eq!(render(&mut initialized, rate, &source, pattern), expected);
                let actual = render(&mut direct, rate, &source, pattern);
                let first = actual.iter().zip(&expected).position(|(a, b)| a != b);
                assert!(
                    first.is_none(),
                    "rate={rate},pattern={pattern:?},epoch={epoch},first_difference={first:?}"
                );
                assert!(
                    actual.iter().any(|&s| s != 0.0),
                    "learned waveform fixture must not be silence"
                );
            }
        }
    }
}

#[test]
fn constructor_default_is_distinct_from_explicit_faster_background_learning() {
    let rate = 48000;
    let source = echo_source(rate);
    let mut direct = AecPlugin::new(rate);
    // Reapplying the exposed default is deliberately a no-op, not a repair.
    direct
        .set_parameter(ParameterId::from("step_size"), ParameterValue::Float(0.5))
        .unwrap();
    let mut canonical = AecPlugin::from_params(rate, AecPluginParams::default()).unwrap();
    let mut faster = AecPlugin::from_params(
        rate,
        AecPluginParams {
            step_size: 0.7,
            ..Default::default()
        },
    )
    .unwrap();
    let actual = render(&mut direct, rate, &source, &[257]);
    let expected = render(&mut canonical, rate, &source, &[257]);
    let alternative = render(&mut faster, rate, &source, &[257]);
    assert!(
        actual == expected,
        "constructor must use the canonical default learning rate"
    );
    let max_difference = actual
        .iter()
        .zip(&alternative)
        .map(|(&a, &b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(
        max_difference > 0.05,
        "nondefault learning must be observable: {max_difference}"
    );
}
