//! Correlated, transient lifecycle commands for Loudness Monitor instances.
// Rust guideline compliant 2026-09-29
use sotf_host::ProcessContext;
use sotf_host::{LoudnessData, LoudnessMonitorPlugin, ParameterId, ParameterValue, Plugin};
use std::sync::Arc;

const RATE: u32 = 48_000;

fn make_plugin() -> LoudnessMonitorPlugin {
    let mut plugin = LoudnessMonitorPlugin::new(2).unwrap();
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
}

fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn command(
    plugin: &mut LoudnessMonitorPlugin,
    instance_id: u64,
    request_id: u64,
    operation: &str,
) -> Result<(), String> {
    plugin.set_parameter(
        ParameterId::from("integrated_control_command"),
        ParameterValue::String(format!("{instance_id}:{request_id}:{operation}")),
    )
}

fn feed_tone(plugin: &mut LoudnessMonitorPlugin, frames: usize) {
    let input = (0..frames)
        .flat_map(|frame| {
            let value = (std::f32::consts::TAU * 997.0 * frame as f32 / RATE as f32).sin() * 0.5;
            [value, value]
        })
        .collect::<Vec<_>>();
    for block in input.chunks(480 * 2) {
        let frames = block.len() / 2;
        let mut output = vec![0.0; block.len()];
        plugin
            .process(block, &mut output, &ProcessContext::new(RATE, frames))
            .unwrap();
        assert_eq!(output, block);
    }
}

#[test]
fn commands_echo_receipts_preserve_reset_state_and_deduplicate_exact_requests() {
    let mut plugin = make_plugin();
    let instance_id = plugin.integrated_control_instance_id();
    assert_ne!(instance_id, 0);
    assert_eq!(
        snapshot(&plugin).integrated_control_instance_id,
        instance_id
    );
    assert_eq!(snapshot(&plugin).integrated_control_request_id, 0);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("integrated_control_command")),
        Some(ParameterValue::String(String::new()))
    );

    // The empty metadata default is inert, which keeps generic parameter
    // enumeration/restoration from replaying a transient action.
    plugin
        .set_parameter(
            ParameterId::from("integrated_control_command"),
            ParameterValue::String(String::new()),
        )
        .unwrap();

    command(&mut plugin, instance_id, 1, "pause").unwrap();
    let paused = snapshot(&plugin);
    assert!(!paused.integrated_measurement_running);
    assert_eq!(paused.integrated_control_request_id, 1);
    drop(paused);

    // Reset while paused preserves pause; Continue and Start while already
    // running both receive distinct receipts. Duplicate Start must not clear
    // the live meter a second time.
    command(&mut plugin, instance_id, 2, "reset").unwrap();
    assert!(!snapshot(&plugin).integrated_measurement_running);
    assert_eq!(snapshot(&plugin).integrated_control_request_id, 2);
    command(&mut plugin, instance_id, 3, "continue").unwrap();
    assert!(snapshot(&plugin).integrated_measurement_running);
    command(&mut plugin, instance_id, 4, "start").unwrap();
    assert!(snapshot(&plugin).integrated_measurement_running);
    feed_tone(&mut plugin, RATE as usize);
    let before_duplicate = snapshot(&plugin);
    assert!(before_duplicate.maximum_true_peak_dbtp.is_some());
    let peak_before_duplicate = before_duplicate.maximum_true_peak_dbtp;
    drop(before_duplicate);
    command(&mut plugin, instance_id, 4, "start").unwrap();
    let after_duplicate = snapshot(&plugin);
    assert_eq!(after_duplicate.integrated_control_request_id, 4);
    assert_eq!(
        after_duplicate.maximum_true_peak_dbtp,
        peak_before_duplicate
    );
    drop(after_duplicate);

    command(&mut plugin, instance_id, 5, "reset").unwrap();
    let reset_running = snapshot(&plugin);
    assert!(reset_running.integrated_measurement_running);
    assert_eq!(reset_running.integrated_control_request_id, 5);
    assert_eq!(reset_running.maximum_true_peak_dbtp, None);
    drop(reset_running);

    command(&mut plugin, instance_id, 6, "pause").unwrap();
    command(&mut plugin, instance_id, 7, "pause").unwrap();
    assert!(!snapshot(&plugin).integrated_measurement_running);
    assert_eq!(snapshot(&plugin).integrated_control_request_id, 7);
    command(&mut plugin, instance_id, 8, "continue").unwrap();
    command(&mut plugin, instance_id, 9, "continue").unwrap();
    assert!(snapshot(&plugin).integrated_measurement_running);
    assert_eq!(snapshot(&plugin).integrated_control_request_id, 9);
}

#[test]
fn malformed_reordered_conflicting_and_wrong_instance_commands_are_rejected() {
    let mut plugin = make_plugin();
    let instance_id = plugin.integrated_control_instance_id();
    command(&mut plugin, instance_id, 10, "pause").unwrap();
    let before = snapshot(&plugin);
    let before_running = before.integrated_measurement_running;
    let before_ack = before.integrated_control_request_id;
    drop(before);

    let malformed = [
        "0:1:reset",
        "1:0:reset",
        "01:2:reset",
        "1:02:reset",
        "18446744073709551616:2:reset",
        "1:18446744073709551616:reset",
        "1:11:reset:",
        "1:11:Reset",
        " 1:11:reset",
        "1:11:unknown",
        "1:11:résumé",
    ];
    for value in malformed {
        assert!(
            plugin
                .set_parameter(
                    ParameterId::from("integrated_control_command"),
                    ParameterValue::String(value.to_string()),
                )
                .is_err(),
            "accepted malformed command {value:?}"
        );
    }
    assert!(command(&mut plugin, instance_id, 9, "continue").is_err());
    assert!(command(&mut plugin, instance_id, 10, "continue").is_err());
    assert!(command(&mut plugin, instance_id + 1, 11, "reset").is_err());
    let after = snapshot(&plugin);
    assert_eq!(after.integrated_measurement_running, before_running);
    assert_eq!(after.integrated_control_request_id, before_ack);
}

#[test]
fn reinitialize_preserves_runtime_identity_and_request_high_water() {
    let mut plugin = make_plugin();
    let instance_id = plugin.integrated_control_instance_id();
    command(&mut plugin, instance_id, 5, "pause").unwrap();
    plugin.initialize(f64::from(RATE)).unwrap();

    let after_reinitialize = snapshot(&plugin);
    assert_eq!(
        after_reinitialize.integrated_control_instance_id,
        instance_id
    );
    assert_eq!(after_reinitialize.integrated_control_request_id, 5);
    assert!(after_reinitialize.integrated_measurement_running);
    drop(after_reinitialize);

    // The old Pause is a duplicate and must not execute against the newly
    // initialized measurement epoch.
    command(&mut plugin, instance_id, 5, "pause").unwrap();
    let after_duplicate = snapshot(&plugin);
    assert!(after_duplicate.integrated_measurement_running);
    assert_eq!(after_duplicate.integrated_control_request_id, 5);

    let replacement = make_plugin();
    assert_ne!(replacement.integrated_control_instance_id(), instance_id);
}

#[test]
fn retained_generations_hold_receipt_until_an_exact_retry_can_publish() {
    let mut plugin = make_plugin();
    let instance_id = plugin.integrated_control_instance_id();
    let initial = snapshot(&plugin);
    let nested_weak = Arc::downgrade(&initial.channel_peaks);
    drop(initial);

    command(&mut plugin, instance_id, 1, "pause").unwrap();
    let first = snapshot(&plugin);
    assert_eq!(first.integrated_control_request_id, 1);
    command(&mut plugin, instance_id, 2, "continue").unwrap();
    let second = snapshot(&plugin);
    assert_eq!(second.integrated_control_request_id, 2);

    // The weak nested reader pins the first generation's prepared channel
    // storage; strong readers retain the next two generations. The command is
    // applied internally but no stale snapshot may claim its receipt.
    command(&mut plugin, instance_id, 3, "pause").unwrap();
    assert!(!plugin.integrated_measurement_running());
    let blocked = snapshot(&plugin);
    assert_eq!(blocked.integrated_control_request_id, 2);
    assert!(blocked.integrated_measurement_running);
    drop(blocked);

    drop(nested_weak);
    drop(first);
    command(&mut plugin, instance_id, 3, "pause").unwrap();
    let recovered = snapshot(&plugin);
    assert_eq!(recovered.integrated_control_request_id, 3);
    assert!(!recovered.integrated_measurement_running);
    drop(recovered);

    // The high-water mark survives ordinary reset. Reset while paused records
    // request 4 without entering Running.
    drop(second);
    command(&mut plugin, instance_id, 4, "reset").unwrap();
    let reset = snapshot(&plugin);
    assert_eq!(reset.integrated_control_request_id, 4);
    assert!(!reset.integrated_measurement_running);
}
