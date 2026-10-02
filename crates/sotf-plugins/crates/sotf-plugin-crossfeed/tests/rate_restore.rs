//! Sample-rate reconfiguration and save/reload checks (V3).
//!
//! Frozen bounds (fixed before execution, never loosened after output):
//! - a reconfigured plugin renders bit-exact output against a fresh plugin
//!   built with the same configuration (`initialize` rebuilds every filter,
//!   delay line, smoother, and meter from scratch, so the two states are
//!   identical by construction);
//! - a JSON save/reload round-trip preserves the configuration value
//!   (compared as JSON values) and renders bit-exact audio against the
//!   pre-save plugin;
//! - a rejected update leaves the accepted configuration and all
//!   subsequently rendered audio bit-exact against an untouched twin.
//!
//! These tests pin current contracts; they change no behavior.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_crossfeed::{CrossfeedMode, CrossfeedPlugin, CrossfeedPluginParams};
use std::f64::consts::PI;

const MODES: [CrossfeedMode; 4] = [
    CrossfeedMode::Bauer,
    CrossfeedMode::Meier,
    CrossfeedMode::Mb,
    CrossfeedMode::Hrtf,
];

fn lively_params(mode: CrossfeedMode) -> CrossfeedPluginParams {
    CrossfeedPluginParams {
        mode,
        mix: 0.35,
        bauer_fcut_hz: 800.0,
        bauer_feed_db: 9.0,
        meier_level: 70.0,
        mb_low_freq_hz: 200.0,
        mb_mid_high_freq_hz: 4000.0,
        mb_low_feed_db: 2.0,
        mb_mid_feed_db: 8.0,
        mb_high_feed_db: 1.0,
        itd_delay_ms: 0.5,
        head_yaw_deg: 45.0,
        autogain_enabled: true,
        autogain_target_lufs: -20.0,
        autogain_max_gain_db: 12.0,
        autogain_smoothing_ms: 100.0,
        ..CrossfeedPluginParams::default()
    }
}

fn tone_pair(sample_rate: u32, frames: usize) -> Vec<f32> {
    let mut signal = Vec::with_capacity(frames * 2);
    for n in 0..frames {
        let left = (0.3 * (2.0 * PI * 431.0 * n as f64 / f64::from(sample_rate)).cos()) as f32;
        let right = (0.25 * (2.0 * PI * 911.0 * n as f64 / f64::from(sample_rate)).cos()) as f32;
        signal.push(left);
        signal.push(right);
    }
    signal
}

fn render(
    plugin: &mut CrossfeedPlugin,
    input: &[f32],
    sample_rate: u32,
    partition_frames: usize,
) -> Vec<f32> {
    let mut output = input.to_vec();
    for chunk in output.chunks_mut(partition_frames * 2) {
        let frames = chunk.len() / 2;
        plugin
            .process_in_place(chunk, &ProcessContext::new(sample_rate, frames))
            .unwrap();
    }
    output
}

fn max_abs_diff(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn reinitialize_matches_fresh_plugin_bit_exact() {
    for mode in MODES {
        let params = lively_params(mode);
        let mut moved = CrossfeedPlugin::new(params.clone()).unwrap();
        moved.initialize(48_000).unwrap();
        // Half a second of unrelated audio warms every state at 48 kHz.
        let warmup = tone_pair(48_000, 24_000);
        let _ = render(&mut moved, &warmup, 48_000, 137);
        moved.initialize(96_000).unwrap();

        let mut fresh = CrossfeedPlugin::new(params).unwrap();
        fresh.initialize(96_000).unwrap();

        let settle = tone_pair(96_000, 48_000);
        let _ = render(&mut moved, &settle, 96_000, 1000);
        let _ = render(&mut fresh, &settle, 96_000, 1000);
        let capture = tone_pair(96_000, 24_000);
        let moved_out = render(&mut moved, &capture, 96_000, 1000);
        let fresh_out = render(&mut fresh, &capture, 96_000, 1000);
        let diff = max_abs_diff(&moved_out, &fresh_out);
        println!("[reinit] {mode:?}: max |reconfigured - fresh| = {diff:.3e} (expect 0)");
        assert!(
            moved_out == fresh_out,
            "{mode:?}: reconfigured render differs from fresh render (max {diff:.3e})"
        );
    }
}

#[test]
fn json_save_reload_preserves_config_and_audio() {
    for mode in MODES {
        for mix in [0.0, 0.35, 1.0] {
            let mut params = lively_params(mode);
            params.mix = mix;
            let saved = serde_json::to_value(&params).unwrap();
            let restored: CrossfeedPluginParams = serde_json::from_value(saved.clone()).unwrap();
            let resaved = serde_json::to_value(&restored).unwrap();
            assert_eq!(
                saved, resaved,
                "{mode:?} mix={mix}: save/reload must preserve the configuration value"
            );
            // The yaw control must survive the round-trip explicitly: it is
            // the newly registered index-17 parameter.
            assert_eq!(
                saved.get("head_yaw_deg"),
                Some(&serde_json::json!(45.0)),
                "{mode:?} mix={mix}: yaw must be present in saved state"
            );
            let mut before = CrossfeedPlugin::new(params).unwrap();
            before.initialize(48_000).unwrap();
            let mut after = CrossfeedPlugin::new(restored).unwrap();
            after.initialize(48_000).unwrap();
            let input = tone_pair(48_000, 48_000);
            let before_out = render(&mut before, &input, 48_000, 137);
            let after_out = render(&mut after, &input, 48_000, 137);
            let diff = max_abs_diff(&before_out, &after_out);
            println!(
                "[reload] {mode:?} mix={mix}: max |reloaded - direct| = {diff:.3e} (expect 0)"
            );
            assert!(
                before_out == after_out,
                "{mode:?} mix={mix}: reloaded render differs from direct render (max {diff:.3e})"
            );
        }
    }
}

#[test]
fn rejected_update_preserves_config_and_audio() {
    let params = lively_params(CrossfeedMode::Mb);
    let mut edited = CrossfeedPlugin::new(params.clone()).unwrap();
    edited.initialize(48_000).unwrap();
    let mut twin = CrossfeedPlugin::new(params).unwrap();
    twin.initialize(48_000).unwrap();

    let before = edited.current_values();
    let mut hostile = ParameterSet::new();
    hostile.insert(
        ParameterId::from("no_such_parameter"),
        ParameterValue::Float(1.0),
    );
    assert!(
        edited.apply_values(hostile).is_err(),
        "unknown parameter id must be rejected"
    );
    let mut hostile_single = ParameterSet::new();
    hostile_single.insert(
        ParameterId::from("mix"),
        ParameterValue::String("loud".to_string()),
    );
    assert!(
        edited.apply_values(hostile_single).is_err(),
        "mistyped value must be rejected"
    );
    assert_eq!(
        edited.current_values(),
        before,
        "rejected batches must leave the accepted configuration untouched"
    );
    assert_eq!(
        edited.current_values(),
        twin.current_values(),
        "rejected batches must not diverge from the untouched twin"
    );

    let input = tone_pair(48_000, 24_000);
    let edited_out = render(&mut edited, &input, 48_000, 512);
    let twin_out = render(&mut twin, &input, 48_000, 512);
    let diff = max_abs_diff(&edited_out, &twin_out);
    println!("[reject] max |edited - twin| = {diff:.3e} (expect 0)");
    assert!(
        edited_out == twin_out,
        "audio after rejected updates differs from the untouched twin (max {diff:.3e})"
    );
}

#[test]
fn supported_rate_boundaries_follow_nyquist_validation() {
    // Default parameters initialize at every supported rate. The 8 kHz
    // rejection pins the documented Nyquist rule (the 5700 Hz default
    // crossover exceeds the 4 kHz Nyquist there).
    for rate in [44_100u32, 48_000, 96_000, 192_000] {
        let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams::default()).unwrap();
        plugin.initialize(rate).unwrap();
    }
    let mut plugin = CrossfeedPlugin::new(CrossfeedPluginParams::default()).unwrap();
    assert!(
        plugin.initialize(8_000).is_err(),
        "default parameters must be rejected where the Nyquist rule fails"
    );
}
