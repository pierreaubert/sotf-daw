//! Public pause/continue behavior for the coupled Integrated Loudness/LRA meter.
// Rust guideline compliant 2026-09-29
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_host::{
    IntegratedLoudnessMode, LoudnessData, LoudnessMonitor, LoudnessMonitorPlugin,
    LoudnessRangeConfig, LoudnessRangeMode, LoudnessRangeStatus, ParameterId, ParameterValue,
    Plugin, ProcessContext,
};
use std::sync::Arc;

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;

fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn snapshot_without_running_flag(data: &LoudnessData) -> serde_json::Value {
    let mut value = serde_json::to_value(data).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("integrated_measurement_running");
    value
}

fn make_plugin() -> LoudnessMonitorPlugin {
    let mut plugin = LoudnessMonitorPlugin::new(CHANNELS)
        .unwrap()
        .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig {
            mode: LoudnessRangeMode::WholeProgram,
            capacity_windows: 256,
        }))
        .unwrap();
    plugin.initialize(RATE).unwrap();
    plugin
}

fn stereo_tone(frames: usize, amplitude: f32, frequency: f64, opposite_phase: bool) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let phase = std::f64::consts::TAU * frequency * frame as f64 / f64::from(RATE);
            let sample = (amplitude as f64 * phase.sin()) as f32;
            [sample, if opposite_phase { -sample } else { sample }]
        })
        .collect()
}

fn feed(plugin: &mut LoudnessMonitorPlugin, signal: &[f32], callback_frames: usize) {
    feed_at_rate(plugin, signal, callback_frames, RATE);
}

fn feed_at_rate(
    plugin: &mut LoudnessMonitorPlugin,
    signal: &[f32],
    callback_frames: usize,
    sample_rate: u32,
) {
    let channels = plugin.input_channels();
    assert_eq!(signal.len() % channels, 0);
    for input in signal.chunks(callback_frames * channels) {
        let frames = input.len() / channels;
        let mut output = vec![f32::NAN; input.len()];
        let processed = plugin
            .process(
                input,
                &mut output,
                &ProcessContext::new(sample_rate, frames),
            )
            .unwrap();
        assert_eq!(processed, frames);
        assert_eq!(output, input, "analyzer must remain bit-exact pass-through");
    }
}

fn uniform_multichannel_tone(
    sample_rate: u32,
    channels: usize,
    frames: usize,
    amplitude: f32,
    frequency: f64,
) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let phase = std::f64::consts::TAU * frequency * frame as f64 / f64::from(sample_rate);
            let sample = (amplitude as f64 * phase.sin()) as f32;
            std::iter::repeat_n(sample, channels)
        })
        .collect()
}

fn feed_monitor(monitor: &mut LoudnessMonitor, signal: &[f32], callback_frames: usize) {
    for input in signal.chunks(callback_frames * CHANNELS) {
        monitor.add_frames(input).unwrap();
    }
}

fn layout_plugin(config_id: &str, with_lra: bool, sample_rate: u32) -> LoudnessMonitorPlugin {
    let layout =
        ChannelLayout::from_speaker_config(get_speaker_config(config_id).unwrap()).unwrap();
    let mut plugin = LoudnessMonitorPlugin::with_channel_layout(layout)
        .unwrap()
        .with_integrated_mode(IntegratedLoudnessMode::Rolling)
        .unwrap()
        .with_loudness_range(with_lra.then_some(LoudnessRangeConfig {
            mode: LoudnessRangeMode::WholeProgram,
            capacity_windows: 128,
        }))
        .unwrap();
    plugin.initialize(sample_rate).unwrap();
    plugin
}

fn set_integrated_running(plugin: &mut LoudnessMonitorPlugin, running: bool) {
    plugin
        .set_parameter(
            ParameterId::from("integrated_running"),
            ParameterValue::Bool(running),
        )
        .unwrap();
}

#[test]
fn pause_excludes_audio_from_integrated_and_lra_but_keeps_live_meters_running() {
    let programme_a = stereo_tone(RATE as usize * 4, 0.1, 997.0, false);
    let paused_programme = stereo_tone(RATE as usize * 5, 0.8, 307.0, true);
    let programme_c = stereo_tone(RATE as usize * 4, 0.23, 3_713.0, false);

    // Include one irregular small callback, the canonical 100 ms block, a
    // larger irregular block, and a multi-second callback. Separate tests
    // exercise a single-frame transition through a partial sub-block.
    for callback_frames in [137, 480, 8_193, RATE as usize * 4] {
        let mut paused = make_plugin();
        let mut concatenated_control = make_plugin();
        feed(&mut paused, &programme_a, callback_frames);
        feed(&mut concatenated_control, &programme_a, callback_frames);
        let before_pause = snapshot(&paused);
        assert!(before_pause.integrated_valid);
        assert!(before_pause.maximum_momentary_lufs.is_some());
        assert!(before_pause.maximum_shortterm_lufs.is_some());
        assert!(before_pause.maximum_true_peak_dbtp.is_some());
        assert!(before_pause.loudness_range.unwrap().observed_windows > 0);

        set_integrated_running(&mut paused, false);
        assert_eq!(
            paused
                .get_parameter(&ParameterId::from("integrated_running"))
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        feed(&mut paused, &paused_programme, callback_frames);
        let during_pause = snapshot(&paused);

        assert!(!during_pause.integrated_measurement_running);
        assert_eq!(during_pause.integrated_lufs, before_pause.integrated_lufs);
        assert_eq!(during_pause.integrated_valid, before_pause.integrated_valid);
        assert_eq!(during_pause.loudness_range, before_pause.loudness_range);
        assert!(during_pause.momentary_lufs > before_pause.momentary_lufs);
        assert!(
            during_pause.maximum_momentary_lufs.unwrap()
                > before_pause.maximum_momentary_lufs.unwrap()
        );
        assert!(
            during_pause.maximum_shortterm_lufs.unwrap()
                > before_pause.maximum_shortterm_lufs.unwrap()
        );
        assert!(during_pause.peak > before_pause.peak);
        assert!(
            during_pause.maximum_true_peak_dbtp.unwrap()
                > before_pause.maximum_true_peak_dbtp.unwrap()
        );
        assert_ne!(during_pause.correlation_lr, before_pause.correlation_lr);
        let paused_maximum_true_peak = during_pause.maximum_true_peak_dbtp.unwrap();
        drop(before_pause);
        drop(during_pause);

        set_integrated_running(&mut paused, true);
        feed(&mut paused, &programme_c, callback_frames);
        feed(&mut concatenated_control, &programme_c, callback_frames);
        let resumed = snapshot(&paused);
        let control = snapshot(&concatenated_control);

        assert!(resumed.integrated_measurement_running);
        assert!(
            (resumed.integrated_lufs - control.integrated_lufs).abs() < 1.0e-12,
            "callback={callback_frames}, candidate={}, control={}",
            resumed.integrated_lufs,
            control.integrated_lufs
        );
        assert_eq!(resumed.integrated_valid, control.integrated_valid);
        assert_eq!(resumed.loudness_range, control.loudness_range);
        assert!(resumed.maximum_true_peak_dbtp.unwrap() >= paused_maximum_true_peak);
    }
}

#[test]
fn paused_samples_do_not_satisfy_active_lane_windows_or_consume_lra_capacity() {
    let mut plugin = LoudnessMonitorPlugin::new(CHANNELS)
        .unwrap()
        .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig {
            mode: LoudnessRangeMode::WholeProgram,
            capacity_windows: 2,
        }))
        .unwrap();
    plugin.initialize(RATE).unwrap();

    feed(
        &mut plugin,
        &stereo_tone(RATE as usize / 10, 0.1, 997.0, false),
        137,
    );
    let cold = snapshot(&plugin);
    assert!(!cold.integrated_valid);
    assert_eq!(
        cold.loudness_range.unwrap().status,
        LoudnessRangeStatus::WarmingUp
    );
    assert_eq!(cold.loudness_range.unwrap().observed_windows, 0);

    set_integrated_running(&mut plugin, false);
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize * 4, 0.8, 307.0, true),
        137,
    );
    let paused = snapshot(&plugin);

    assert!(!paused.integrated_valid);
    assert_eq!(paused.integrated_lufs, cold.integrated_lufs);
    let range = paused.loudness_range.unwrap();
    assert_eq!(range.status, LoudnessRangeStatus::WarmingUp);
    assert_eq!(range.observed_windows, 0);
    assert_eq!(range.retained_windows, 0);
    assert_eq!(paused.query_error, None);
    assert!(paused.momentary_valid);
    assert!(paused.shortterm_valid);
    assert!(paused.maximum_shortterm_lufs.is_some());
}

#[test]
fn reset_and_snapshot_rebuild_preserve_pause_state_until_explicit_continue() {
    let mut plugin = make_plugin();
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize * 4, 0.1, 997.0, false),
        480,
    );
    let held = snapshot(&plugin);
    assert!(held.integrated_measurement_running);

    set_integrated_running(&mut plugin, false);
    assert_eq!(
        plugin
            .get_parameter(&ParameterId::from("integrated_running"))
            .and_then(|value| value.as_bool()),
        Some(false)
    );
    // The control changes immediately, while a retained immutable snapshot may
    // keep showing the last successfully published state.
    assert!(held.integrated_measurement_running);
    drop(held);
    feed(&mut plugin, &stereo_tone(480, 0.8, 307.0, true), 137);
    assert!(!snapshot(&plugin).integrated_measurement_running);

    plugin.reset();
    let reset = snapshot(&plugin);
    assert!(!reset.integrated_measurement_running);
    assert!(!reset.integrated_valid);
    assert_eq!(reset.maximum_momentary_lufs, None);
    assert_eq!(reset.maximum_shortterm_lufs, None);
    assert_eq!(reset.maximum_true_peak_dbtp, None);
    assert_eq!(
        reset.loudness_range.unwrap().status,
        LoudnessRangeStatus::WarmingUp
    );

    // Spatial cache reconstruction keeps the control state. Changing the
    // integrated-history policy starts a fresh history but does not resume it.
    plugin.set_spatial_enabled(true);
    assert!(!snapshot(&plugin).integrated_measurement_running);
    plugin = plugin
        .with_integrated_mode(IntegratedLoudnessMode::Rolling)
        .unwrap();
    assert!(!snapshot(&plugin).integrated_measurement_running);

    set_integrated_running(&mut plugin, true);
    assert!(snapshot(&plugin).integrated_measurement_running);
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize, 0.1, 997.0, false),
        480,
    );
    let running_value = snapshot(&plugin).integrated_lufs;
    set_integrated_running(&mut plugin, true);
    assert_eq!(snapshot(&plugin).integrated_lufs, running_value);

    set_integrated_running(&mut plugin, false);
    assert!(!snapshot(&plugin).integrated_measurement_running);
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
        .unwrap();
    assert!(snapshot(&plugin).integrated_measurement_running);
    set_integrated_running(&mut plugin, false);
    assert!(!snapshot(&plugin).integrated_measurement_running);
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
        .unwrap();
    assert!(snapshot(&plugin).integrated_measurement_running);
    assert_eq!(
        plugin
            .get_parameter(&ParameterId::from("integrated_running"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );

    set_integrated_running(&mut plugin, false);
    plugin.initialize(RATE).unwrap();
    assert!(plugin.integrated_measurement_running());
    assert_eq!(
        plugin
            .get_parameter(&ParameterId::from("integrated_running"))
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    plugin.pause_integrated_measurement();
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize, 0.1, 997.0, false),
        480,
    );
    plugin.start_integrated_measurement().unwrap();
    let restarted = snapshot(&plugin);
    assert!(restarted.integrated_measurement_running);
    assert!(!restarted.integrated_valid);
    assert_eq!(restarted.maximum_momentary_lufs, None);
    assert_eq!(restarted.maximum_shortterm_lufs, None);
}

#[test]
fn non_divisible_rate_pause_uses_active_frames_for_warmup_and_lra() {
    let rate = 11_025;
    let mut paused = LoudnessMonitor::new_with_integrated_mode(
        CHANNELS as u32,
        rate,
        IntegratedLoudnessMode::WholeProgram,
    )
    .unwrap()
    .with_loudness_range(Some(LoudnessRangeConfig {
        mode: LoudnessRangeMode::WholeProgram,
        capacity_windows: 64,
    }))
    .unwrap();
    let mut concatenated_control = LoudnessMonitor::new_with_integrated_mode(
        CHANNELS as u32,
        rate,
        IntegratedLoudnessMode::WholeProgram,
    )
    .unwrap()
    .with_loudness_range(Some(LoudnessRangeConfig {
        mode: LoudnessRangeMode::WholeProgram,
        capacity_windows: 64,
    }))
    .unwrap();

    let programme_a = uniform_multichannel_tone(rate, CHANNELS, 1_102, 0.1, 997.0);
    let paused_programme = uniform_multichannel_tone(rate, CHANNELS, rate as usize * 4, 0.8, 307.0);
    let programme_c =
        uniform_multichannel_tone(rate, CHANNELS, rate as usize * 3 + 257, 0.23, 3_713.0);
    feed_monitor(&mut paused, &programme_a, 137);
    feed_monitor(&mut concatenated_control, &programme_a, 137);
    let before_pause = paused.get_loudness();
    assert!(!before_pause.integrated_valid);
    assert_eq!(
        before_pause.loudness_range.unwrap().observed_windows,
        0,
        "one accepted backend sub-block is below the four-block I/LRA windows"
    );

    paused.pause_integrated_measurement();
    feed_monitor(&mut paused, &paused_programme, 137);
    let during_pause = paused.get_loudness();
    assert!(!during_pause.integrated_valid);
    assert_eq!(during_pause.integrated_lufs, before_pause.integrated_lufs);
    assert_eq!(during_pause.loudness_range, before_pause.loudness_range);
    assert!(during_pause.shortterm_valid);
    assert!(during_pause.maximum_shortterm_lufs.unwrap() > -20.0);

    paused.continue_integrated_measurement();
    feed_monitor(&mut paused, &programme_c, 137);
    feed_monitor(&mut concatenated_control, &programme_c, 137);
    let resumed = paused.get_loudness();
    let control = concatenated_control.get_loudness();
    assert!(resumed.integrated_valid);
    assert_eq!(resumed.integrated_lufs, control.integrated_lufs);
    assert_eq!(resumed.loudness_range, control.loudness_range);
    assert_eq!(
        resumed.loudness_range.unwrap().observed_windows,
        2,
        "active sub-blocks 30 and 31 each produce one short-term observation"
    );
}

#[test]
fn nondivisible_rate_keeps_four_and_thirty_backend_subblock_admission() {
    let rate = 11_025;
    let channels = CHANNELS as u32;
    let sub_block_frames = rate as usize / 10;
    let mut monitor = LoudnessMonitor::new_with_integrated_mode(
        channels,
        rate,
        IntegratedLoudnessMode::WholeProgram,
    )
    .unwrap()
    .with_loudness_range(Some(LoudnessRangeConfig {
        mode: LoudnessRangeMode::WholeProgram,
        capacity_windows: 64,
    }))
    .unwrap();

    feed_monitor(
        &mut monitor,
        &uniform_multichannel_tone(rate, CHANNELS, sub_block_frames * 3, 0.1, 997.0),
        137,
    );
    let before_fourth = monitor.get_loudness();
    assert!(!before_fourth.integrated_valid);
    assert_eq!(before_fourth.loudness_range.unwrap().observed_windows, 0);

    feed_monitor(
        &mut monitor,
        &uniform_multichannel_tone(rate, CHANNELS, sub_block_frames, 0.1, 997.0),
        137,
    );
    let after_fourth = monitor.get_loudness();
    assert!(after_fourth.integrated_valid);
    assert!(after_fourth.integrated_lufs.is_finite());
    assert_eq!(after_fourth.loudness_range.unwrap().observed_windows, 0);

    feed_monitor(
        &mut monitor,
        &uniform_multichannel_tone(rate, CHANNELS, sub_block_frames * 25, 0.1, 997.0),
        137,
    );
    let before_thirtieth = monitor.get_loudness();
    assert_eq!(before_thirtieth.loudness_range.unwrap().observed_windows, 0);

    feed_monitor(
        &mut monitor,
        &uniform_multichannel_tone(rate, CHANNELS, sub_block_frames, 0.1, 997.0),
        137,
    );
    let after_thirtieth = monitor.get_loudness();
    assert_eq!(
        after_thirtieth.loudness_range.unwrap().observed_windows,
        1,
        "the 30th completed backend sub-block is the first LRA observation"
    );
}

#[test]
fn one_frame_pause_after_a_partial_subblock_matches_concatenated_programme() {
    fn prepared_monitor() -> LoudnessMonitor {
        LoudnessMonitor::new_with_integrated_mode(
            CHANNELS as u32,
            RATE,
            IntegratedLoudnessMode::WholeProgram,
        )
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig {
            mode: LoudnessRangeMode::WholeProgram,
            capacity_windows: 128,
        }))
        .unwrap()
    }

    let mut paused = prepared_monitor();
    let mut control = prepared_monitor();
    let programme_a_frames = RATE as usize * 3 + 4_799;
    let programme_a = uniform_multichannel_tone(RATE, CHANNELS, programme_a_frames, 0.1, 997.0);
    let paused_programme = uniform_multichannel_tone(RATE, CHANNELS, 9_601, 0.8, 307.0);
    let programme_c =
        uniform_multichannel_tone(RATE, CHANNELS, RATE as usize * 3 + 1, 0.23, 3_713.0);
    let programme_a_prefix_samples = (programme_a_frames - 1) * CHANNELS;

    for monitor in [&mut paused, &mut control] {
        feed_monitor(monitor, &programme_a[..programme_a_prefix_samples], 137);
        monitor
            .add_frames(&programme_a[programme_a_prefix_samples..])
            .unwrap();
    }
    let before_pause = paused.get_loudness();
    assert!(before_pause.integrated_valid);
    paused.pause_integrated_measurement();
    feed_monitor(&mut paused, &paused_programme, 137);
    let during_pause = paused.get_loudness();
    assert!(
        during_pause.maximum_shortterm_lufs.unwrap() > before_pause.maximum_shortterm_lufs.unwrap()
    );

    paused.continue_integrated_measurement();
    paused.add_frames(&programme_c[..CHANNELS]).unwrap();
    feed_monitor(&mut paused, &programme_c[CHANNELS..], 137);
    control.add_frames(&programme_c[..CHANNELS]).unwrap();
    feed_monitor(&mut control, &programme_c[CHANNELS..], 137);
    let resumed = paused.get_loudness();
    let concatenated = control.get_loudness();
    assert_eq!(resumed.integrated_lufs, concatenated.integrated_lufs);
    assert_eq!(resumed.loudness_range, concatenated.loudness_range);
}

#[test]
fn explicit_51_and_714_routes_pause_integrated_and_lra_only() {
    let rate = 8_000;
    for (layout_id, with_lra, callback_frames) in [
        ("5.1", false, 137),
        ("5.1", true, 8_193),
        ("7.1.4", false, 8_193),
        ("7.1.4", true, 137),
    ] {
        let mut paused = layout_plugin(layout_id, with_lra, rate);
        let mut control = layout_plugin(layout_id, with_lra, rate);
        let channels = paused.input_channels();
        let programme_a =
            uniform_multichannel_tone(rate, channels, rate as usize * 3 + 200, 0.1, 997.0);
        let paused_programme =
            uniform_multichannel_tone(rate, channels, rate as usize * 3 + 200, 0.8, 307.0);
        let programme_c =
            uniform_multichannel_tone(rate, channels, rate as usize * 3 + 200, 0.23, 3_713.0);

        feed_at_rate(&mut paused, &programme_a, callback_frames, rate);
        feed_at_rate(&mut control, &programme_a, callback_frames, rate);
        let before_pause = snapshot(&paused);
        assert!(before_pause.integrated_valid, "layout={layout_id}");
        if with_lra {
            assert!(before_pause.loudness_range.unwrap().observed_windows > 0);
        } else {
            assert_eq!(before_pause.loudness_range, None);
        }

        paused.pause_integrated_measurement();
        feed_at_rate(&mut paused, &paused_programme, callback_frames, rate);
        let during_pause = snapshot(&paused);
        assert_eq!(during_pause.integrated_lufs, before_pause.integrated_lufs);
        assert_eq!(during_pause.loudness_range, before_pause.loudness_range);
        assert!(during_pause.maximum_shortterm_lufs.unwrap() > -10.0);
        drop(before_pause);
        drop(during_pause);

        paused.continue_integrated_measurement();
        feed_at_rate(&mut paused, &programme_c, callback_frames, rate);
        feed_at_rate(&mut control, &programme_c, callback_frames, rate);
        let resumed = snapshot(&paused);
        let concatenated = snapshot(&control);
        assert_eq!(
            resumed.integrated_lufs, concatenated.integrated_lufs,
            "layout={layout_id}"
        );
        assert_eq!(resumed.integrated_valid, concatenated.integrated_valid);
        assert_eq!(
            resumed.loudness_range, concatenated.loudness_range,
            "layout={layout_id}"
        );
    }
}

#[test]
fn serialized_and_copied_snapshots_keep_running_default_and_paused_state() {
    let mut plugin = make_plugin();
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize * 4, 0.1, 997.0, false),
        480,
    );
    let mut serialized_snapshot = (*snapshot(&plugin)).clone();
    let serialized = serde_json::to_value(&serialized_snapshot).unwrap();
    let mut legacy = serialized.clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("integrated_measurement_running");
    let old_snapshot: LoudnessData = serde_json::from_value(legacy).unwrap();
    assert!(old_snapshot.integrated_measurement_running);

    set_integrated_running(&mut plugin, false);
    feed(&mut plugin, &stereo_tone(480, 0.8, 307.0, true), 137);
    let paused_snapshot = snapshot(&plugin);
    assert!(!paused_snapshot.integrated_measurement_running);
    serialized_snapshot.update_from(&paused_snapshot);
    assert!(!serialized_snapshot.integrated_measurement_running);

    let roundtrip: LoudnessData =
        serde_json::from_value(serde_json::to_value(&*paused_snapshot).unwrap()).unwrap();
    assert!(!roundtrip.integrated_measurement_running);

    let paused_values = snapshot_without_running_flag(&paused_snapshot);
    plugin.continue_integrated_measurement();
    let continued_snapshot = snapshot(&plugin);
    assert!(continued_snapshot.integrated_measurement_running);
    assert_eq!(
        snapshot_without_running_flag(&continued_snapshot),
        paused_values
    );
}

#[test]
fn pause_copies_the_current_snapshot_without_regressing_telemetry_under_weak_pressure() {
    let mut plugin = make_plugin();
    feed(
        &mut plugin,
        &stereo_tone(RATE as usize * 4, 0.1, 997.0, false),
        8_193,
    );

    // Leave a Weak nested owner in a stale cache generation, then advance to a
    // distinct published generation. The update must reject that slot and copy
    // the complete current snapshot into the fallback before changing the bit.
    let first = snapshot(&plugin);
    let stale_weak_peaks = Arc::downgrade(&first.channel_peaks);
    drop(first);
    feed(&mut plugin, &stereo_tone(4_800, 0.2, 2_003.0, false), 8_193);
    let latest = snapshot(&plugin);
    let expected = snapshot_without_running_flag(&latest);

    plugin.pause_integrated_measurement();
    let paused = snapshot(&plugin);
    assert!(!paused.integrated_measurement_running);
    assert_eq!(snapshot_without_running_flag(&paused), expected);
    assert_eq!(
        stale_weak_peaks.strong_count(),
        1,
        "the stale weakly-referenced generation remains cached but was not reused"
    );

    plugin.continue_integrated_measurement();
    // Both writable cache generations are still retained/weak-blocked, so the
    // old published control bit may lag until another complete publication.
    assert!(!snapshot(&plugin).integrated_measurement_running);
    drop((latest, paused));
    feed(&mut plugin, &stereo_tone(137, 0.2, 2_003.0, false), 137);
    assert!(snapshot(&plugin).integrated_measurement_running);
}
