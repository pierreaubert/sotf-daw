//! AutoGain stays outside the acoustic feedback path and preserves initialization history.
// Rust guideline compliant 2026-02-21
use crate::{AaePlugin, params::AaePluginParams};
use sotf_host::{ParameterValue, Plugin, ProcessContext};

fn plugin(enabled: bool) -> AaePlugin {
    let mut plugin = AaePlugin::from_params(AaePluginParams {
        auto_gain_enabled: enabled,
        auto_gain_max_db: 12.0,
        auto_gain_smoothing_ms: 100.0,
        ..Default::default()
    })
    .unwrap();
    assert!(plugin.auto_gain.is_some());
    plugin.initialize(48_000).unwrap();
    plugin
}

#[test]
fn final_compensation_does_not_enter_acoustic_feedback_history() {
    let mut enabled = plugin(true);
    let mut disabled = plugin(false);
    let frames = 8193;
    let input: Vec<_> = (0..frames)
        .flat_map(|n| {
            let x = (n as f64 * 0.071).sin() as f32 * 0.003;
            [x, x * 0.3]
        })
        .collect();
    let mut a = vec![0.0; frames * enabled.output_channels()];
    let mut b = a.clone();
    let context = ProcessContext::new(48000, frames);
    for _ in 0..12 {
        enabled.process(&input, &mut a, &context).unwrap();
        disabled.process(&input, &mut b, &context).unwrap();
    }
    assert_ne!(a, b);
    assert_eq!(enabled.final_limiter_gain, 1.0);
    assert_eq!(disabled.final_limiter_gain, 1.0);
    enabled
        .set_parameter("auto_gain_enabled".into(), ParameterValue::Bool(false))
        .unwrap();
    // Any feedback change would now remain audible after the output stage is off.
    for _ in 0..6 {
        enabled.process(&input, &mut a, &context).unwrap();
        disabled.process(&input, &mut b, &context).unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn reinitialize_preserves_gain_history_while_reset_clears_it() {
    let mut plugin = plugin(true);
    let frames = 8193;
    let input: Vec<_> = (0..frames)
        .flat_map(|n| {
            let x = (n as f64 * 0.137).sin() as f32 * 0.03;
            [x, x * 0.3]
        })
        .collect();
    let mut output = vec![0.0; frames * plugin.output_channels()];
    for _ in 0..12 {
        plugin
            .process(&input, &mut output, &ProcessContext::new(48000, frames))
            .unwrap();
    }
    let gain = plugin.auto_gain.as_ref().unwrap().data().gain_db;
    assert!(gain.abs() > 0.01);
    plugin.initialize(96_000).unwrap();
    assert_eq!(plugin.auto_gain.as_ref().unwrap().data().gain_db, gain);
    plugin.reset();
    assert_eq!(plugin.auto_gain.as_ref().unwrap().data().gain_db, 0.0);
}
