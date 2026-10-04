//! Verify the unity shortcut against a deliberately prepared internal history.
// Rust guideline compliant 2026-02-21
use crate::{ABComparePlugin, ABComparePluginParams};
use sotf_host::{AutoGain, AutoGainParams, Plugin, ProcessContext};

fn prepared_gain() -> AutoGain {
    // Prepare a decaying 20 ms linear history just above unity, making the
    // previous approximate eligibility predicate observable without a stall.
    let rate = 8000;
    let mut gain = AutoGain::new(
        2,
        rate,
        AutoGainParams {
            enabled: true,
            smoothing_ms: 0.0,
            max_gain_db: 12.0,
            ..Default::default()
        },
    )
    .unwrap();
    let input: Vec<_> = (0..rate)
        .flat_map(|n| {
            let sample =
                (std::f64::consts::TAU * 997.0 * f64::from(n) / f64::from(rate)).sin() as f32 * 0.1;
            [sample, sample]
        })
        .collect();
    let output: Vec<_> = input.iter().map(|x| x * 0.5).collect();
    gain.measure_input(&input).unwrap();
    gain.measure_output(&output).unwrap();
    for _ in 0..rate {
        gain.next_gain_linear();
    }
    // The setter resets the zero-time dB stage, preserving the linear history.
    gain.set_enabled(false);
    gain.set_enabled(true);
    for _ in 0..2000 {
        gain.next_gain_linear();
    }
    let linear = gain.next_gain_linear();
    assert!(
        linear > 1.0 && linear - 1.0 < 1e-5,
        "prepared gain={linear}"
    );
    gain
}

#[test]
fn prepared_near_unity_history_is_applied_instead_of_skipped() {
    // Public structural path replacement rebuilds AutoGain. This private state
    // probes the shortcut's eligibility contract, not an unsupported live edit.
    let mut plugin = ABComparePlugin::from_params(
        2,
        ABComparePluginParams {
            mix: 1.0,
            auto_gain_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(8000.0).unwrap();
    assert!(plugin.can_use_empty_path_fast_path());
    plugin.auto_gain = prepared_gain();
    let mut reference = prepared_gain();
    assert!(!plugin.can_use_empty_path_fast_path());
    let input: Vec<_> = (0..17).flat_map(|_| [0.125, -0.5]).collect();
    let mut expected = input.clone();
    for frame in expected.as_chunks_mut::<2>().0 {
        let gain = reference.next_gain_linear();
        frame[0] *= gain;
        frame[1] *= gain;
    }
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(&input, &mut output, &ProcessContext::new(8000, 17))
            .unwrap(),
        17
    );
    assert_eq!(output, expected);
    assert_ne!(
        output, input,
        "the old unity shortcut omitted this retained gain"
    );
    plugin.reset();
    assert!(plugin.can_use_empty_path_fast_path());
}
