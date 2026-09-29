//! Programme M/S maxima follow the monitor's fixed 100 ms observation grid.
// Rust guideline compliant 2026-09-28
use sotf_host::speaker_config::{ChannelLayout, ChannelRole, get_speaker_config};
use sotf_host::{
    LoudnessData, LoudnessMonitor, LoudnessMonitorPlugin, LoudnessRangeConfig, Plugin,
    ProcessContext,
};
use std::sync::{Arc, Weak};

const RATE: u32 = 48_000;
const OBSERVATION_FRAMES: usize = RATE as usize / 10;

fn tone(rate: u32, start_frame: usize, frames: usize, channels: usize, peak_dbfs: f64) -> Vec<f32> {
    let amplitude = 10.0_f64.powf(peak_dbfs / 20.0);
    (0..frames)
        .flat_map(|offset| {
            let phase =
                std::f64::consts::TAU * 997.0 * (start_frame + offset) as f64 / f64::from(rate);
            let sample = (amplitude * phase.sin()) as f32;
            std::iter::repeat_n(sample, channels)
        })
        .collect()
}

fn programme(rate: u32, channels: usize) -> Vec<f32> {
    let total_frames = rate as usize * 7;
    let mut input = Vec::with_capacity(total_frames * channels);
    for frame in 0..total_frames {
        let level = if frame < rate as usize * 4 {
            -18.0
        } else {
            -30.0
        };
        let amplitude = 10.0_f64.powf(level / 20.0);
        let phase = std::f64::consts::TAU * 997.0 * frame as f64 / f64::from(rate);
        let sample = (amplitude * phase.sin()) as f32;
        input.extend(std::iter::repeat_n(sample, channels));
    }
    input
}

fn monitor(channels: usize, layout_id: Option<&str>, lra: bool) -> LoudnessMonitor {
    let mut meter = if let Some(layout_id) = layout_id {
        let config = get_speaker_config(layout_id).expect("speaker configuration exists");
        let layout = ChannelLayout::from_speaker_config(config).expect("speaker layout is valid");
        LoudnessMonitor::new_with_layout(channels as u32, RATE, layout)
            .expect("explicit monitor construction succeeds")
    } else {
        LoudnessMonitor::new(channels as u32, RATE).expect("monitor construction succeeds")
    };
    if lra {
        meter = meter
            .with_loudness_range(Some(LoudnessRangeConfig::default()))
            .expect("LRA storage prepares");
    }
    meter
}

fn feed_partitioned(
    meter: &mut LoudnessMonitor,
    input: &[f32],
    channels: usize,
    pattern: &[usize],
) {
    let total_frames = input.len() / channels;
    let mut frame_offset = 0;
    let mut pattern_index = 0;
    while frame_offset < total_frames {
        let frames = pattern[pattern_index % pattern.len()].min(total_frames - frame_offset);
        let sample_start = frame_offset * channels;
        let sample_end = (frame_offset + frames) * channels;
        meter
            .add_frames(&input[sample_start..sample_end])
            .expect("partition contains whole frames");
        frame_offset += frames;
        pattern_index += 1;
    }
}

fn include_finite(maximum: &mut Option<f64>, value: f64) {
    if value.is_finite() {
        *maximum = Some(maximum.map_or(value, |current| current.max(value)));
    }
}

fn assert_near(actual: Option<f64>, expected: Option<f64>, tolerance: f64, context: &str) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert!(
            (actual - expected).abs() <= tolerance,
            "{context}: actual={actual}, expected={expected}, tolerance={tolerance}"
        ),
        (None, None) => {}
        (actual, expected) => panic!("{context}: actual={actual:?}, expected={expected:?}"),
    }
}

#[derive(Default)]
struct GridMaximum {
    momentary: Option<f64>,
    shortterm: Option<f64>,
}

impl GridMaximum {
    fn observe(&mut self, data: &LoudnessData) {
        if data.momentary_valid {
            include_finite(&mut self.momentary, data.momentary_lufs);
        }
        if data.shortterm_valid {
            include_finite(&mut self.shortterm, data.shortterm_lufs);
        }
    }
}

#[test]
fn maxima_reduce_an_independent_fixed_grid_for_partitioned_programmes() {
    let input = programme(RATE, 2);
    let mut reference = monitor(2, None, false);
    let mut expected = GridMaximum::default();
    let mut final_reference = LoudnessData::new(2);
    for block in input.chunks(OBSERVATION_FRAMES * 2) {
        reference.add_frames(block).unwrap();
        final_reference = reference.get_loudness();
        expected.observe(&final_reference);
    }

    assert!((expected.momentary.unwrap() + 18.0).abs() <= 0.2);
    assert!((expected.shortterm.unwrap() + 18.0).abs() <= 0.2);
    assert!(final_reference.momentary_lufs < expected.momentary.unwrap() - 5.0);
    assert!(final_reference.shortterm_lufs < expected.shortterm.unwrap() - 5.0);

    // One-frame, irregular, 100 ms, and multi-second callback partitions all
    // reduce the same grid observations. LRA on/off must not change the maxima.
    for (pattern, lra) in [
        (&[1][..], false),
        (&[137, 509, 4_096, 2_031][..], false),
        (&[OBSERVATION_FRAMES][..], true),
        (&[RATE as usize * 2][..], true),
        (&[137, 509, 4_096, 2_031][..], true),
    ] {
        let mut candidate = monitor(2, None, lra);
        feed_partitioned(&mut candidate, &input, 2, pattern);
        let actual = candidate.get_loudness();
        assert_near(
            actual.maximum_momentary_lufs,
            expected.momentary,
            0.01,
            "stereo maximum Momentary",
        );
        assert_near(
            actual.maximum_shortterm_lufs,
            expected.shortterm,
            0.01,
            "stereo maximum Short-term",
        );
        assert!((actual.momentary_lufs - final_reference.momentary_lufs).abs() <= 0.01);
        assert!((actual.shortterm_lufs - final_reference.shortterm_lufs).abs() <= 0.01);
    }
}

#[test]
fn explicit_5_1_maxima_follow_bs1770_weights_and_exclude_lfe() {
    let config = get_speaker_config("5.1").unwrap();
    let layout = ChannelLayout::from_speaker_config(config).unwrap();
    let roles: Vec<_> = (0..6).map(|index| layout.role_at(index).unwrap()).collect();

    let run = |active_roles: &[ChannelRole], peak_dbfs: f64| {
        let mut meter = LoudnessMonitor::new_with_layout(6, RATE, layout.clone()).unwrap();
        for start in (0..RATE as usize * 4).step_by(OBSERVATION_FRAMES) {
            let frames = (RATE as usize * 4 - start).min(OBSERVATION_FRAMES);
            let mut input = Vec::with_capacity(frames * roles.len());
            for frame in 0..frames {
                let carrier = (std::f64::consts::TAU * 997.0 * (start + frame) as f64
                    / f64::from(RATE))
                .sin() as f32;
                for role in &roles {
                    let enabled = active_roles.contains(role);
                    let amplitude = if enabled {
                        10.0_f64.powf(peak_dbfs / 20.0) as f32
                    } else {
                        0.0
                    };
                    input.push(carrier * amplitude);
                }
            }
            meter.add_frames(&input).unwrap();
        }
        meter.get_loudness()
    };

    let lr = [ChannelRole::FrontLeft, ChannelRole::FrontRight];
    let left_right = run(&lr, -18.0);
    assert!((left_right.maximum_momentary_lufs.unwrap() + 18.0).abs() <= 0.2);
    assert!((left_right.maximum_shortterm_lufs.unwrap() + 18.0).abs() <= 0.2);

    let surround = [
        ChannelRole::FrontLeft,
        ChannelRole::FrontRight,
        ChannelRole::SideLeft,
        ChannelRole::SideRight,
    ];
    let surround_programme = run(&surround, -18.0);
    let expected_surround = -18.0 + 10.0 * ((2.0 + 2.0 * 1.41) / 2.0_f64).log10();
    assert!((surround_programme.maximum_momentary_lufs.unwrap() - expected_surround).abs() <= 0.2);
    assert!((surround_programme.maximum_shortterm_lufs.unwrap() - expected_surround).abs() <= 0.2);

    let lfe_only = run(&[ChannelRole::Lfe], -3.0);
    assert_eq!(lfe_only.maximum_momentary_lufs, None);
    assert_eq!(lfe_only.maximum_shortterm_lufs, None);
}

fn weighted_mono_reference(
    readings: &[LoudnessData],
    roles: &[ChannelRole],
    shortterm: bool,
) -> Option<f64> {
    let mut energy = 0.0;
    for (reading, role) in readings.iter().zip(roles) {
        let valid = if shortterm {
            reading.shortterm_valid
        } else {
            reading.momentary_valid
        };
        let loudness = if shortterm {
            reading.shortterm_lufs
        } else {
            reading.momentary_lufs
        };
        // Keep this reference independent from the production layout-weight
        // method so a shared weighting defect cannot make the comparison pass.
        let weight = match role {
            ChannelRole::Lfe => 0.0,
            ChannelRole::SideLeft
            | ChannelRole::SideRight
            | ChannelRole::BackLeft
            | ChannelRole::BackRight => 1.41,
            ChannelRole::Mono
            | ChannelRole::FrontLeft
            | ChannelRole::FrontRight
            | ChannelRole::FrontCenter
            | ChannelRole::WideLeft
            | ChannelRole::WideRight
            | ChannelRole::TopFrontLeft
            | ChannelRole::TopFrontRight
            | ChannelRole::TopMiddleLeft
            | ChannelRole::TopMiddleRight
            | ChannelRole::TopBackLeft
            | ChannelRole::TopBackRight => 1.0,
        };
        if valid && weight > 0.0 && loudness.is_finite() {
            energy += weight * 10.0_f64.powf((loudness + 0.691) / 10.0);
        }
    }
    (energy > 0.0).then(|| -0.691 + 10.0 * energy.log10())
}

#[test]
fn explicit_7_1_4_aggregation_matches_independent_weighted_mono_meters() {
    let config = get_speaker_config("7.1.4").unwrap();
    let layout = ChannelLayout::from_speaker_config(config).unwrap();
    let roles: Vec<_> = (0..12)
        .map(|index| layout.role_at(index).unwrap())
        .collect();
    let mut programme_meter = LoudnessMonitor::new_with_layout(12, RATE, layout).unwrap();
    let mut mono_meters: Vec<_> = (0..12)
        .map(|_| LoudnessMonitor::new(1, RATE).unwrap())
        .collect();
    let mut expected = GridMaximum::default();

    for start in (0..RATE as usize * 4).step_by(OBSERVATION_FRAMES) {
        let mut interleaved = Vec::with_capacity(OBSERVATION_FRAMES * 12);
        let mut mono_blocks: Vec<_> = (0..12)
            .map(|_| Vec::with_capacity(OBSERVATION_FRAMES))
            .collect();
        for frame in 0..OBSERVATION_FRAMES {
            let phase = std::f64::consts::TAU * 997.0 * (start + frame) as f64 / f64::from(RATE);
            for (channel, role) in roles.iter().enumerate() {
                let peak_dbfs = match role {
                    ChannelRole::Lfe => -3.0,
                    ChannelRole::FrontLeft | ChannelRole::FrontRight => -18.0,
                    ChannelRole::SideLeft | ChannelRole::SideRight => -22.0,
                    ChannelRole::BackLeft | ChannelRole::BackRight => -24.0,
                    _ => -27.0,
                };
                let sample = (10.0_f64.powf(peak_dbfs / 20.0) * phase.sin()) as f32;
                interleaved.push(sample);
                mono_blocks[channel].push(sample);
            }
        }
        programme_meter.add_frames(&interleaved).unwrap();
        for (meter, block) in mono_meters.iter_mut().zip(mono_blocks) {
            meter.add_frames(&block).unwrap();
        }
        let mono_readings: Vec<_> = mono_meters
            .iter_mut()
            .map(LoudnessMonitor::get_loudness)
            .collect();
        include_finite(
            &mut expected.momentary,
            weighted_mono_reference(&mono_readings, &roles, false).unwrap_or(f64::NEG_INFINITY),
        );
        include_finite(
            &mut expected.shortterm,
            weighted_mono_reference(&mono_readings, &roles, true).unwrap_or(f64::NEG_INFINITY),
        );
    }

    let actual = programme_meter.get_loudness();
    assert_near(
        actual.maximum_momentary_lufs,
        expected.momentary,
        0.01,
        "7.1.4 maximum Momentary versus weighted mono reference",
    );
    assert_near(
        actual.maximum_shortterm_lufs,
        expected.shortterm,
        0.01,
        "7.1.4 maximum Short-term versus weighted mono reference",
    );
}

#[test]
fn explicit_7_1_4_maxima_match_the_fixed_grid_with_lra_on_and_off() {
    let input = programme(RATE, 12);
    let mut reference = monitor(12, Some("7.1.4"), false);
    let mut expected = GridMaximum::default();
    for block in input.chunks(OBSERVATION_FRAMES * 12) {
        reference.add_frames(block).unwrap();
        expected.observe(&reference.get_loudness());
    }

    for lra in [false, true] {
        let mut candidate = monitor(12, Some("7.1.4"), lra);
        feed_partitioned(&mut candidate, &input, 12, &[137, 509, 4_096, 2_031]);
        let actual = candidate.get_loudness();
        assert_near(
            actual.maximum_momentary_lufs,
            expected.momentary,
            0.01,
            "7.1.4 maximum Momentary",
        );
        assert_near(
            actual.maximum_shortterm_lufs,
            expected.shortterm,
            0.01,
            "7.1.4 maximum Short-term",
        );
    }
}

#[test]
fn exact_window_eligibility_uses_source_frames_and_true_peak_finish_is_m_s_neutral() {
    const RATE_11025: u32 = 11_025;
    let mut meter = LoudnessMonitor::new(1, RATE_11025).unwrap();

    meter
        .add_frames(&tone(RATE_11025, 0, 4_409, 1, -18.0))
        .unwrap();
    let data = meter.get_loudness();
    assert_eq!(data.maximum_momentary_lufs, None);
    assert_eq!(data.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE_11025, 4_409, 1, 1, -18.0))
        .unwrap();
    let data = meter.get_loudness();
    assert!(data.momentary_valid);
    assert_eq!(data.maximum_momentary_lufs, None);

    meter
        .add_frames(&tone(RATE_11025, 4_410, 1_100, 1, -18.0))
        .unwrap();
    let data = meter.get_loudness();
    assert!(data.maximum_momentary_lufs.is_some_and(f64::is_finite));
    assert_eq!(data.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE_11025, 5_510, 33_074 - 5_510, 1, -18.0))
        .unwrap();
    let data = meter.get_loudness();
    assert!(!data.shortterm_valid);
    assert_eq!(data.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE_11025, 33_074, 1, 1, -18.0))
        .unwrap();
    let data = meter.get_loudness();
    assert!(data.shortterm_valid);
    assert_eq!(data.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE_11025, 33_075, 1_087, 1, -18.0))
        .unwrap();
    let before_finish = meter.get_loudness();
    let maximum_shortterm = before_finish.maximum_shortterm_lufs;
    assert!(maximum_shortterm.is_some_and(f64::is_finite));
    meter.finish_true_peak();
    let after_finish = meter.get_loudness();
    assert_eq!(
        after_finish.maximum_momentary_lufs,
        before_finish.maximum_momentary_lufs
    );
    assert_eq!(after_finish.maximum_shortterm_lufs, maximum_shortterm);

    meter.reset().unwrap();
    let reset = meter.get_loudness();
    assert_eq!(reset.maximum_momentary_lufs, None);
    assert_eq!(reset.maximum_shortterm_lufs, None);
}

#[test]
fn exact_48khz_window_eligibility_boundaries_use_source_frames() {
    let mut meter = LoudnessMonitor::new(1, RATE).unwrap();

    meter.add_frames(&tone(RATE, 0, 19_199, 1, -18.0)).unwrap();
    let below_momentary = meter.get_loudness();
    assert_eq!(below_momentary.maximum_momentary_lufs, None);
    assert_eq!(below_momentary.maximum_shortterm_lufs, None);

    meter.add_frames(&tone(RATE, 19_199, 1, 1, -18.0)).unwrap();
    let at_momentary = meter.get_loudness();
    assert!(at_momentary.momentary_valid);
    assert!(
        at_momentary
            .maximum_momentary_lufs
            .is_some_and(f64::is_finite)
    );
    assert_eq!(at_momentary.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE, 19_200, 143_999 - 19_200, 1, -18.0))
        .unwrap();
    let below_shortterm = meter.get_loudness();
    assert!(!below_shortterm.shortterm_valid);
    assert!(
        below_shortterm
            .maximum_momentary_lufs
            .is_some_and(f64::is_finite)
    );
    assert_eq!(below_shortterm.maximum_shortterm_lufs, None);

    meter.add_frames(&tone(RATE, 143_999, 1, 1, -18.0)).unwrap();
    let at_shortterm = meter.get_loudness();
    assert!(at_shortterm.shortterm_valid);
    assert!(
        at_shortterm
            .maximum_momentary_lufs
            .is_some_and(f64::is_finite)
    );
    assert!(
        at_shortterm
            .maximum_shortterm_lufs
            .is_some_and(f64::is_finite)
    );
}

#[test]
fn cold_silence_and_empty_queries_do_not_create_maxima() {
    let mut meter = LoudnessMonitor::new(2, RATE).unwrap();
    for _ in 0..3 {
        let cold = meter.get_loudness();
        assert_eq!(cold.maximum_momentary_lufs, None);
        assert_eq!(cold.maximum_shortterm_lufs, None);
    }
    meter.finish_true_peak();
    meter.add_frames(&vec![0.0; RATE as usize * 4 * 2]).unwrap();
    let silent = meter.get_loudness();
    assert_eq!(silent.maximum_momentary_lufs, None);
    assert_eq!(silent.maximum_shortterm_lufs, None);

    meter
        .add_frames(&tone(RATE, RATE as usize * 4, RATE as usize, 2, -18.0))
        .unwrap();
    let finite = meter.get_loudness();
    assert!(finite.maximum_momentary_lufs.is_some_and(f64::is_finite));
    assert!(finite.maximum_shortterm_lufs.is_some_and(f64::is_finite));
}

#[test]
fn loudness_data_defaults_copy_and_legacy_serde_preserve_optional_maxima() {
    let constructed = LoudnessData::new(2);
    let defaulted = LoudnessData::default();
    assert_eq!(constructed.maximum_momentary_lufs, None);
    assert_eq!(constructed.maximum_shortterm_lufs, None);
    assert_eq!(defaulted.maximum_momentary_lufs, None);
    assert_eq!(defaulted.maximum_shortterm_lufs, None);
    let cold_json = serde_json::to_value(&constructed).unwrap();
    assert!(cold_json.get("maximum_momentary_lufs").is_none());
    assert!(cold_json.get("maximum_shortterm_lufs").is_none());

    let mut meter = LoudnessMonitor::new(2, RATE).unwrap();
    meter
        .add_frames(&tone(RATE, 0, RATE as usize * 4, 2, -18.0))
        .unwrap();
    let mut data = meter.get_loudness();
    assert!(data.maximum_momentary_lufs.is_some_and(f64::is_finite));
    assert!(data.maximum_shortterm_lufs.is_some_and(f64::is_finite));

    let encoded = serde_json::to_value(&data).unwrap();
    for key in ["maximum_momentary_lufs", "maximum_shortterm_lufs"] {
        assert!(encoded[key].as_f64().is_some_and(f64::is_finite));
    }
    let roundtrip: LoudnessData = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(
        roundtrip.maximum_momentary_lufs,
        data.maximum_momentary_lufs
    );
    assert_eq!(
        roundtrip.maximum_shortterm_lufs,
        data.maximum_shortterm_lufs
    );

    let mut legacy = encoded;
    legacy
        .as_object_mut()
        .unwrap()
        .remove("maximum_momentary_lufs");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("maximum_shortterm_lufs");
    let legacy: LoudnessData = serde_json::from_value(legacy).unwrap();
    assert_eq!(legacy.maximum_momentary_lufs, None);
    assert_eq!(legacy.maximum_shortterm_lufs, None);

    let expected_momentary = data.maximum_momentary_lufs;
    let expected_shortterm = data.maximum_shortterm_lufs;
    data.maximum_momentary_lufs = Some(-99.0);
    data.maximum_shortterm_lufs = Some(-99.0);
    data.update_from(&roundtrip);
    assert_eq!(data.maximum_momentary_lufs, expected_momentary);
    assert_eq!(data.maximum_shortterm_lufs, expected_shortterm);
}

fn plugin_snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn process_plugin(plugin: &mut LoudnessMonitorPlugin, input: &[f32], channels: usize) {
    let mut output = vec![0.0; input.len()];
    plugin
        .process(
            input,
            &mut output,
            &ProcessContext::new(RATE, input.len() / channels),
        )
        .unwrap();
    assert_eq!(input, output);
}

#[test]
fn plugin_cache_rebuild_and_retained_generations_do_not_leak_old_epoch_maxima() {
    let channels = 2;
    let mut plugin = LoudnessMonitorPlugin::new(channels).unwrap();
    plugin.initialize(RATE).unwrap();
    let high = tone(RATE, 0, RATE as usize * 4, channels, -18.0);
    for block in high.chunks(OBSERVATION_FRAMES * channels) {
        process_plugin(&mut plugin, block, channels);
    }
    let before_rebuild = plugin_snapshot(&plugin);
    let old_momentary = before_rebuild.maximum_momentary_lufs.unwrap();
    let old_shortterm = before_rebuild.maximum_shortterm_lufs.unwrap();

    plugin.set_spatial_enabled(true);
    let spatial_snapshot = plugin_snapshot(&plugin);
    assert_eq!(spatial_snapshot.maximum_momentary_lufs, Some(old_momentary));
    assert_eq!(spatial_snapshot.maximum_shortterm_lufs, Some(old_shortterm));
    assert_eq!(before_rebuild.maximum_momentary_lufs, Some(old_momentary));

    // Keep each prepared cache generation alive with both strong and Weak
    // readers. After reset, no new publication can reuse those old snapshots.
    let mut retained = vec![spatial_snapshot];
    let followup = tone(RATE, RATE as usize * 4, OBSERVATION_FRAMES, channels, -18.0);
    for _ in 0..2 {
        process_plugin(&mut plugin, &followup, channels);
        retained.push(plugin_snapshot(&plugin));
    }
    for left in 0..retained.len() {
        for right in left + 1..retained.len() {
            assert!(!Arc::ptr_eq(&retained[left], &retained[right]));
        }
    }
    let retained_momentary = retained.last().unwrap().maximum_momentary_lufs.unwrap();
    let retained_shortterm = retained.last().unwrap().maximum_shortterm_lufs.unwrap();
    let weak_readers: Vec<Weak<LoudnessData>> = retained.iter().map(Arc::downgrade).collect();

    plugin.reset();
    let stale = plugin_snapshot(&plugin);
    assert_eq!(stale.maximum_momentary_lufs, Some(retained_momentary));
    assert_eq!(stale.maximum_shortterm_lufs, Some(retained_shortterm));
    drop(stale);

    let bad_input = [0.25_f32];
    let mut bad_output = [0.0_f32; 2];
    assert!(
        plugin
            .process(&bad_input, &mut bad_output, &ProcessContext::new(RATE, 1),)
            .is_err()
    );
    drop(retained);

    // Weak readers still prevent mutation of all three old cache generations.
    let post_reset_low = tone(RATE, 0, OBSERVATION_FRAMES, channels, -30.0);
    process_plugin(&mut plugin, &post_reset_low, channels);
    let still_stale = plugin_snapshot(&plugin);
    assert_eq!(still_stale.maximum_momentary_lufs, Some(retained_momentary));
    assert_eq!(still_stale.maximum_shortterm_lufs, Some(retained_shortterm));
    drop(still_stale);
    drop(weak_readers);

    process_plugin(&mut plugin, &[], channels);
    let cleared = plugin_snapshot(&plugin);
    assert_eq!(cleared.maximum_momentary_lufs, None);
    assert_eq!(cleared.maximum_shortterm_lufs, None);

    let low = tone(RATE, 0, RATE as usize * 4, channels, -30.0);
    for block in low.chunks(OBSERVATION_FRAMES * channels) {
        process_plugin(&mut plugin, block, channels);
    }
    let new_epoch = plugin_snapshot(&plugin);
    let new_momentary = new_epoch.maximum_momentary_lufs.unwrap();
    let new_shortterm = new_epoch.maximum_shortterm_lufs.unwrap();
    assert!(
        retained_momentary > new_momentary + 8.0,
        "old Momentary maximum {retained_momentary} must exceed new epoch {new_momentary}"
    );
    assert!(
        retained_shortterm > new_shortterm + 8.0,
        "old Short-term maximum {retained_shortterm} must exceed new epoch {new_shortterm}"
    );

    plugin.initialize(RATE).unwrap();
    let reinitialized = plugin_snapshot(&plugin);
    assert_eq!(reinitialized.maximum_momentary_lufs, None);
    assert_eq!(reinitialized.maximum_shortterm_lufs, None);
}
