//! Independent programme signals, public timelines and loudness-range lifecycle.
// Rust guideline compliant 2026-02-21
use sotf_host::speaker_config::{ChannelAssignment, ChannelLayout, ChannelRole};
use sotf_host::{
    IntegratedLoudnessMode, LoudnessData, LoudnessMonitor, LoudnessMonitorPlugin,
    LoudnessRangeConfig, LoudnessRangeData, LoudnessRangeMode, LoudnessRangeStatus, ParameterId,
    ParameterValue, Plugin, ProcessContext,
};
use std::sync::Arc;

fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn config(mode: LoudnessRangeMode, capacity: usize) -> LoudnessRangeConfig {
    LoudnessRangeConfig {
        mode,
        capacity_windows: capacity,
    }
}

fn stereo_tone(rate: u32, frames: usize, levels: &[f64], segment: usize) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let level = levels[(n / segment).min(levels.len() - 1)];
            let sample = (10.0_f64.powf(level / 20.0)
                * (std::f64::consts::TAU * 1_000.0 * n as f64 / f64::from(rate)).sin())
                as f32;
            [sample, sample]
        })
        .collect()
}

fn process_plugin_frames(plugin: &mut LoudnessMonitorPlugin, rate: u32, input: &[f32]) {
    assert_eq!(input.len() % 2, 0);
    let frames = input.len() / 2;
    let mut output = vec![0.0; input.len()];
    plugin
        .process(input, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    assert_eq!(output, input);
}

fn feed_lra_tone(
    monitor: &mut LoudnessMonitor,
    rate: u32,
    channels: usize,
    total_frames: usize,
    callback_frames: &[usize],
    query_each_callback: bool,
) -> LoudnessData {
    assert!(!callback_frames.is_empty());
    assert!(callback_frames.iter().all(|&frames| frames > 0));

    let mut left = rate;
    let mut right = 1_000_u32;
    while right != 0 {
        (left, right) = (right, left % right);
    }
    let period_frames = rate as usize / left as usize;
    let period: Vec<f32> = (0..period_frames)
        .map(|frame| {
            (0.1 * (std::f64::consts::TAU * 1_000.0 * frame as f64 / f64::from(rate)).sin()) as f32
        })
        .collect();
    let block_capacity = callback_frames
        .iter()
        .copied()
        .max()
        .unwrap()
        .min(total_frames);
    let mut input = vec![0.0_f32; block_capacity * channels];
    let mut data = LoudnessData::new(channels);
    let mut frame_offset = 0;
    let mut callback_index = 0;

    while frame_offset < total_frames {
        let block_frames = callback_frames[callback_index % callback_frames.len()]
            .min(total_frames - frame_offset);
        for frame in 0..block_frames {
            let sample = period[(frame_offset + frame) % period_frames];
            input[frame * channels..(frame + 1) * channels].fill(sample);
        }
        monitor
            .add_frames(&input[..block_frames * channels])
            .unwrap();
        frame_offset += block_frames;
        callback_index += 1;
        if query_each_callback {
            monitor.update_loudness_data(&mut data);
        }
    }
    monitor.update_loudness_data(&mut data);
    data
}

#[test]
fn published_ebu_tone_programmes_and_repetition_match_required_ranges() {
    // EBU Tech 3342 (2023), Table 1 cases 1–4. Authentic programmes remain
    // untested; these generated signals do not imply full EBU compliance.
    let rate = 48_000;
    for (levels, expected) in [
        (&[-20.0, -30.0][..], 10.0),
        (&[-20.0, -15.0][..], 5.0),
        (&[-40.0, -20.0][..], 20.0),
        (&[-50.0, -35.0, -20.0, -35.0, -50.0][..], 15.0),
    ] {
        let tone = stereo_tone(rate, rate as usize * 20, &[0.0], usize::MAX);
        for repetitions in [1, 2] {
            let mut monitor = LoudnessMonitor::new(2, rate)
                .unwrap()
                .with_loudness_range(Some(config(LoudnessRangeMode::WholeProgram, 4_000)))
                .unwrap();
            let mut input = vec![0.0; 8_192 * 2];
            for _ in 0..repetitions {
                for &level in levels {
                    let amplitude = 10.0_f64.powf(level / 20.0) as f32;
                    for chunk in tone.chunks(input.len()) {
                        for (out, &source) in input.iter_mut().zip(chunk) {
                            *out = source * amplitude;
                        }
                        monitor.add_frames(&input[..chunk.len()]).unwrap();
                    }
                }
            }
            // The standard's file-measurement recommendation is explicit input.
            monitor.add_frames(&vec![0.0; rate as usize * 3]).unwrap();
            let result = monitor.get_loudness().loudness_range.unwrap();
            assert_eq!(result.status, LoudnessRangeStatus::Valid);
            assert!(
                (result.range_lu.unwrap() - expected).abs() <= 1.0,
                "levels={levels:?}, repetitions={repetitions}: {result:?}"
            );
        }
    }
}

#[test]
fn observations_follow_audio_time_independently_of_callbacks_and_queries() {
    let rate = 8_000;
    let input = stereo_tone(
        rate,
        rate as usize * 9 + 17,
        &[-20.0, -30.0, -15.0],
        rate as usize * 3,
    );
    let mut expected = None;
    for block in [1, 17, 137, 799, 800, 801, 8_193] {
        let mut monitor = LoudnessMonitor::new(2, rate)
            .unwrap()
            .with_loudness_range(Some(config(LoudnessRangeMode::Rolling, 41)))
            .unwrap();
        let mut plain = LoudnessMonitor::new(2, rate).unwrap();
        let mut data = LoudnessData::new(2);
        let mut control = LoudnessData::new(2);
        for (index, chunk) in input.chunks(block * 2).enumerate() {
            monitor.add_frames(chunk).unwrap();
            plain.add_frames(chunk).unwrap();
            if index % 7 == 0 {
                monitor.update_loudness_data(&mut data);
                plain.update_loudness_data(&mut control);
                assert_eq!(data.momentary_lufs, control.momentary_lufs);
                assert_eq!(data.shortterm_lufs, control.shortterm_lufs);
                assert_eq!(data.integrated_lufs, control.integrated_lufs);
                assert_eq!(data.peak, control.peak);
            }
        }
        monitor.update_loudness_data(&mut data);
        let result = data.loudness_range.unwrap();
        assert_eq!(result.observed_windows, 61);
        assert_eq!(result.retained_windows, 41);
        if let Some(expected) = expected {
            assert_eq!(result, expected);
        } else {
            expected = Some(result);
        }
        monitor.add_frames(&[]).unwrap();
        assert_eq!(monitor.get_loudness().loudness_range, Some(result));
        monitor.reset().unwrap();
        assert_eq!(
            monitor.get_loudness().loudness_range.unwrap().status,
            LoudnessRangeStatus::WarmingUp
        );
    }
    for frames in [23_999, 24_000, 24_799, 24_800] {
        let mut monitor = LoudnessMonitor::new(2, rate)
            .unwrap()
            .with_loudness_range(Some(LoudnessRangeConfig::default()))
            .unwrap();
        monitor.add_frames(&input[..frames * 2]).unwrap();
        assert_eq!(
            monitor
                .get_loudness()
                .loudness_range
                .unwrap()
                .observed_windows,
            if frames < 24_000 {
                0
            } else if frames < 24_800 {
                1
            } else {
                2
            }
        );
    }
}

#[test]
fn stability_changes_on_the_exact_accepted_frame_at_standard_and_rounded_rates() {
    for rate in [48_000_u32, 11_025] {
        let mut monitor = LoudnessMonitor::new(2, rate)
            .unwrap()
            .with_loudness_range(Some(LoudnessRangeConfig::default()))
            .unwrap();
        let threshold_frames = rate as usize * 60;
        let callbacks = if rate == 48_000 {
            vec![threshold_frames - 1]
        } else {
            vec![137, 799, 1_301, 8_192]
        };
        let before_boundary = feed_lra_tone(
            &mut monitor,
            rate,
            2,
            threshold_frames - 1,
            &callbacks,
            rate != 48_000,
        );
        let before = before_boundary.loudness_range.unwrap();
        assert_eq!(before.status, LoudnessRangeStatus::Valid);
        assert!(
            before
                .range_lu
                .is_some_and(|value| value.is_finite() && value >= 0.0)
        );
        assert!(!before.is_stable, "rate={rate}: {before:?}");
        assert_eq!(before.timebase_is_exact, rate.is_multiple_of(10));

        // Empty input, a rejected malformed callback, and a repeated TP-only
        // drain cannot add active measurement frames or LRA observations.
        monitor.add_frames(&[]).unwrap();
        assert!(monitor.add_frames(&[0.1]).is_err());
        monitor.finish_true_peak();
        monitor.finish_true_peak();
        let after_empty_and_drain = monitor.get_loudness().loudness_range.unwrap();
        assert!(!after_empty_and_drain.is_stable);
        assert_eq!(
            after_empty_and_drain.observed_windows,
            before.observed_windows
        );

        monitor.add_frames(&[0.0, 0.0]).unwrap();
        let at_boundary = monitor.get_loudness().loudness_range.unwrap();
        assert_eq!(at_boundary.status, LoudnessRangeStatus::Valid);
        assert!(at_boundary.is_stable, "rate={rate}: {at_boundary:?}");
        if rate == 11_025 {
            assert_eq!(
                at_boundary.observed_windows, before.observed_windows,
                "the rounded-rate stability frame changes the bit without dirtying LRA"
            );
        }
    }
}

#[test]
fn stable_true_roundtrips_and_each_epoch_transition_clears_it() {
    let rate = 8_000;
    let frames = rate as usize * 60;
    let input = stereo_tone(rate, frames, &[-20.0], usize::MAX);
    let mut plugin = LoudnessMonitorPlugin::new(2)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig::default()))
        .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();

    for transition in ["reset", "start", "reinitialize", "disable-enable"] {
        process_plugin_frames(&mut plugin, rate, &input);
        let stable_snapshot = snapshot(&plugin);
        let stable_range = stable_snapshot.loudness_range.unwrap();
        assert_eq!(stable_range.status, LoudnessRangeStatus::Valid);
        assert!(
            stable_range.is_stable,
            "before {transition}: {stable_range:?}"
        );

        let roundtrip: LoudnessData =
            serde_json::from_value(serde_json::to_value(&*stable_snapshot).unwrap()).unwrap();
        assert_eq!(roundtrip.loudness_range, Some(stable_range));
        let mut copied = LoudnessData::new(2);
        copied.update_from(&stable_snapshot);
        assert_eq!(copied.loudness_range, Some(stable_range));
        drop((stable_snapshot, copied));

        match transition {
            "reset" => plugin.reset(),
            "start" => {
                let command = format!("{}:1:start", plugin.integrated_control_instance_id());
                plugin
                    .set_parameter(
                        ParameterId::from("integrated_control_command"),
                        ParameterValue::String(command),
                    )
                    .unwrap();
            }
            "reinitialize" => plugin.initialize(f64::from(rate)).unwrap(),
            "disable-enable" => {
                plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                    .unwrap();
                plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let reset_range = snapshot(&plugin).loudness_range.unwrap();
        assert!(
            !reset_range.is_stable,
            "after {transition}: {reset_range:?}"
        );
    }
}

#[test]
fn stable_transition_waits_for_nested_weak_readers_then_recovers() {
    let rate = 8_000;
    let threshold_frames = rate as usize * 60;
    let input = stereo_tone(rate, threshold_frames, &[-20.0], usize::MAX);
    let mut plugin = LoudnessMonitorPlugin::new(2)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig::default()))
        .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();

    // Reach a valid false snapshot first, then use three small callbacks to
    // retain nested Weak owners from all cache generations. The two spare
    // generations are blocked while the published false snapshot is retained.
    let before_block_frames = threshold_frames - 4;
    process_plugin_frames(&mut plugin, rate, &input[..before_block_frames * 2]);
    let valid_false = snapshot(&plugin);
    assert_eq!(
        valid_false.loudness_range.unwrap().status,
        LoudnessRangeStatus::Valid
    );
    assert!(!valid_false.loudness_range.unwrap().is_stable);
    drop(valid_false);

    let mut blocked_peak_generations = Vec::new();
    for frame in before_block_frames..threshold_frames - 1 {
        process_plugin_frames(&mut plugin, rate, &input[frame * 2..frame * 2 + 2]);
        let published = snapshot(&plugin);
        assert_eq!(
            published.loudness_range.unwrap().status,
            LoudnessRangeStatus::Valid
        );
        assert!(!published.loudness_range.unwrap().is_stable);
        blocked_peak_generations.push(Arc::downgrade(&published.channel_peaks));
    }

    let old_false = snapshot(&plugin);
    let old_range = old_false.loudness_range.unwrap();
    assert_eq!(old_range.status, LoudnessRangeStatus::Valid);
    assert!(!old_range.is_stable);

    process_plugin_frames(
        &mut plugin,
        rate,
        &input[(threshold_frames - 1) * 2..threshold_frames * 2],
    );
    let blocked = snapshot(&plugin);
    assert!(Arc::ptr_eq(&old_false, &blocked));
    assert!(!blocked.loudness_range.unwrap().is_stable);

    drop(blocked_peak_generations);
    process_plugin_frames(&mut plugin, rate, &[0.0, 0.0]);
    let recovered = snapshot(&plugin);
    assert!(recovered.loudness_range.unwrap().is_stable);
    assert!(!old_false.loudness_range.unwrap().is_stable);
}

#[test]
fn stability_clock_pauses_on_the_wide_explicit_meter_route() {
    let rate = 8_000;
    let roles = [
        ChannelRole::FrontLeft,
        ChannelRole::FrontRight,
        ChannelRole::FrontCenter,
        ChannelRole::Lfe,
        ChannelRole::SideLeft,
        ChannelRole::SideRight,
        ChannelRole::BackLeft,
        ChannelRole::BackRight,
        ChannelRole::TopFrontLeft,
        ChannelRole::TopFrontRight,
        ChannelRole::TopBackLeft,
        ChannelRole::TopBackRight,
    ];
    let layout = ChannelLayout::new(
        roles
            .iter()
            .enumerate()
            .map(|(index, &role)| ChannelAssignment { index, role })
            .collect(),
    )
    .unwrap();
    let mut monitor = LoudnessMonitor::new_with_layout(roles.len() as u32, rate, layout)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig::default()))
        .unwrap();

    let first_half = feed_lra_tone(
        &mut monitor,
        rate,
        roles.len(),
        rate as usize * 30,
        &[rate as usize],
        true,
    );
    let first_range = first_half.loudness_range.unwrap();
    assert_eq!(first_range.status, LoudnessRangeStatus::Valid);
    assert!(!first_range.is_stable);

    monitor.set_integrated_measurement_running(false);
    let paused = feed_lra_tone(
        &mut monitor,
        rate,
        roles.len(),
        rate as usize * 30,
        &[rate as usize],
        true,
    )
    .loudness_range
    .unwrap();
    assert!(!paused.is_stable);

    monitor.set_integrated_measurement_running(true);
    let resumed_before_boundary = feed_lra_tone(
        &mut monitor,
        rate,
        roles.len(),
        rate as usize * 30 - 1,
        &[rate as usize],
        true,
    )
    .loudness_range
    .unwrap();
    assert!(!resumed_before_boundary.is_stable);
    monitor.add_frames(&[0.0; 12]).unwrap();
    assert!(monitor.get_loudness().loudness_range.unwrap().is_stable);

    monitor.set_integrated_measurement_running(false);
    monitor.reset().unwrap();
    let reset = monitor.get_loudness().loudness_range.unwrap();
    assert_eq!(reset.status, LoudnessRangeStatus::WarmingUp);
    assert!(!reset.is_stable);
}

#[test]
fn plugin_spatial_rebuild_preserves_clock_and_integrated_mode_restarts_it() {
    let rate = 8_000;
    let frames = rate as usize * 60;
    let mut plugin = LoudnessMonitorPlugin::new(2)
        .unwrap()
        .with_loudness_range(Some(LoudnessRangeConfig::default()))
        .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let input = stereo_tone(rate, frames, &[-20.0], usize::MAX);
    let mut output = vec![0.0; input.len()];
    plugin
        .process(&input, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    assert_eq!(output, input);
    let before_rebuild = snapshot(&plugin).loudness_range.unwrap();
    assert_eq!(before_rebuild.status, LoudnessRangeStatus::Valid);
    assert!(before_rebuild.is_stable);

    plugin.set_spatial_enabled(true);
    let spatial_rebuild = snapshot(&plugin).loudness_range.unwrap();
    assert!(spatial_rebuild.is_stable);

    let plugin = plugin
        .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
        .unwrap();
    let mode_rebuild = snapshot(&plugin).loudness_range.unwrap();
    assert_eq!(mode_rebuild.status, LoudnessRangeStatus::WarmingUp);
    assert!(!mode_rebuild.is_stable);
}

#[test]
fn analyzer_builders_reset_retained_snapshots_and_serialization_preserve_policy() {
    let range = config(LoudnessRangeMode::WholeProgram, 100);
    for reverse in [false, true] {
        let base = LoudnessMonitorPlugin::new(2).unwrap();
        assert_eq!(
            base.loudness_range_config(),
            Some(LoudnessRangeConfig::default())
        );
        let mut plugin = if reverse {
            base.with_loudness_range(Some(range))
                .unwrap()
                .with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
                .unwrap()
        } else {
            base.with_integrated_mode(IntegratedLoudnessMode::WholeProgram)
                .unwrap()
                .with_loudness_range(Some(range))
                .unwrap()
        }
        .with_spatial();
        plugin.initialize(48_000.0).unwrap();
        assert_eq!(
            snapshot(&plugin).loudness_range.unwrap().status,
            LoudnessRangeStatus::WarmingUp
        );
        let input = stereo_tone(48_000, 48_000 * 4, &[-20.0], usize::MAX);
        let mut output = vec![0.0; input.len()];
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(48_000, input.len() / 2),
            )
            .unwrap();
        assert_eq!(input, output);
        let held = snapshot(&plugin);
        let old = held.loudness_range.unwrap();
        assert_eq!(old.observed_windows, 11);
        assert_eq!(old.status, LoudnessRangeStatus::Valid);
        assert!(!old.is_stable);
        let mut json = serde_json::to_value(&*held).unwrap();
        let roundtrip: LoudnessData = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(roundtrip.loudness_range, Some(old));
        let mut legacy_range = serde_json::to_value(old).unwrap();
        legacy_range.as_object_mut().unwrap().remove("is_stable");
        let legacy: LoudnessRangeData = serde_json::from_value(legacy_range).unwrap();
        assert!(!legacy.is_stable);
        json.as_object_mut().unwrap().remove("loudness_range");
        assert_eq!(
            serde_json::from_value::<LoudnessData>(json)
                .unwrap()
                .loudness_range,
            None
        );
        let mut copied = LoudnessData::new(2);
        copied.update_from(&held);
        assert_eq!(copied.loudness_range, Some(old));
        plugin.reset();
        assert_eq!(held.loudness_range, Some(old));
        let cleared = snapshot(&plugin);
        assert_eq!(
            cleared.loudness_range.unwrap().status,
            LoudnessRangeStatus::WarmingUp
        );
        assert_eq!(cleared.loudness_range.unwrap().mode, range.mode);
        drop((held, cleared));
        let id = ParameterId::from("enabled");
        plugin
            .set_parameter(id.clone(), ParameterValue::Bool(false))
            .unwrap();
        assert_eq!(
            snapshot(&plugin).loudness_range.unwrap().observed_windows,
            0
        );
        plugin
            .set_parameter(id, ParameterValue::Bool(true))
            .unwrap();
        plugin.initialize(44_101.0).unwrap();
        assert_eq!(plugin.loudness_range_config(), Some(range));
        assert!(!snapshot(&plugin).loudness_range.unwrap().timebase_is_exact);
    }
}

#[test]
fn setup_validation_optional_default_and_capacity_status_are_explicit() {
    assert_eq!(
        LoudnessMonitor::new(2, 48_000)
            .unwrap()
            .loudness_range_config(),
        None
    );
    for capacity in [0, 36_001, usize::MAX] {
        assert!(
            LoudnessMonitorPlugin::new(2)
                .unwrap()
                .with_loudness_range(Some(config(LoudnessRangeMode::Rolling, capacity)))
                .is_err()
        );
    }
    let mut meter = LoudnessMonitor::new(1, 8_000)
        .unwrap()
        .with_loudness_range(Some(config(LoudnessRangeMode::WholeProgram, 2)))
        .unwrap();
    meter.add_frames(&vec![0.0; 24_800]).unwrap();
    assert_eq!(
        meter.get_loudness().loudness_range.unwrap().status,
        LoudnessRangeStatus::BelowGate
    );
    meter.add_frames(&vec![0.0; 800]).unwrap();
    let data = meter.get_loudness();
    assert_eq!(
        data.loudness_range.unwrap().status,
        LoudnessRangeStatus::CapacityExceeded
    );
    assert!(data.measurement_valid);
    assert_eq!(data.query_error, None);
    let encoded = serde_json::to_string(&data.loudness_range).unwrap();
    assert_eq!(
        serde_json::from_str::<Option<LoudnessRangeData>>(&encoded).unwrap(),
        data.loudness_range
    );
    assert!(meter.with_loudness_range(None).is_err());
}

#[test]
fn semantic_roles_and_channel_permutations_match_independent_energy_levels() {
    use ChannelRole::*;
    let roles = [
        FrontLeft,
        FrontRight,
        FrontCenter,
        Lfe,
        SideLeft,
        SideRight,
        BackLeft,
        BackRight,
        TopFrontLeft,
        TopFrontRight,
        TopBackLeft,
        TopBackRight,
    ];
    let rate = 8_000;
    let segment = 20 * rate as usize;
    // First segment has one front channel at -20 dBFS; the second has one
    // surround at -30 dBFS. BS.1770's independent 1.41 energy factor means the
    // two stationary levels differ by 10 - 10log10(1.41) LU.
    let expected = 10.0 - 10.0 * 1.41_f64.log10();
    for width in [6, 12] {
        for reverse in [false, true] {
            let selected: Vec<_> = if reverse {
                roles[..width].iter().rev().copied().collect()
            } else {
                roles[..width].to_vec()
            };
            let layout = ChannelLayout::new(
                selected
                    .iter()
                    .enumerate()
                    .map(|(index, &role)| ChannelAssignment { index, role })
                    .collect(),
            )
            .unwrap();
            for lfe_only in [false, true] {
                let mut meter =
                    LoudnessMonitor::new_with_layout(width as u32, rate, layout.clone())
                        .unwrap()
                        .with_loudness_range(Some(config(LoudnessRangeMode::WholeProgram, 500)))
                        .unwrap();
                let mut input = vec![0.0; 800 * width];
                for start in (0..segment * 2).step_by(800) {
                    for frame in 0..800 {
                        let n = start + frame;
                        let carrier = (std::f64::consts::TAU * 1_000.0 * n as f64 / f64::from(rate))
                            .sin() as f32;
                        for (channel, role) in selected.iter().enumerate() {
                            input[frame * width + channel] = carrier
                                * match role {
                                    Lfe => 0.8,
                                    FrontLeft if !lfe_only && n < segment => 0.1,
                                    SideLeft if !lfe_only && n >= segment => {
                                        10.0_f32.powf(-30.0 / 20.0)
                                    }
                                    _ => 0.0,
                                };
                        }
                    }
                    meter.add_frames(&input).unwrap();
                }
                let result = meter.get_loudness();
                assert!(result.channel_layout_is_compliant);
                let range = result.loudness_range.unwrap();
                if lfe_only {
                    assert_eq!(range.status, LoudnessRangeStatus::BelowGate);
                    assert_eq!(range.range_lu, None);
                } else {
                    assert_eq!(range.status, LoudnessRangeStatus::Valid);
                    assert!(
                        (range.range_lu.unwrap() - expected).abs() < 0.001,
                        "width={width}, reverse={reverse}: {range:?} expected={expected}"
                    );
                }
            }
        }
    }
}
