//! End-of-stream measurement includes the final interpolation response.
// Rust guideline compliant 2026-02-21
use math_audio_dsp::ebur128::{EbuR128, Mode};
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::plugin::PluginCompiledOp;
use sotf_host::{DawHost, Host, LoudnessData, LoudnessMonitorPlugin, Plugin, ProcessContext};
use sotf_host::{ParameterId, ParameterValue};
use std::sync::Arc;

#[path = "common/true_peak_reference.rs"]
mod reference;

fn final_impulse() -> Vec<f32> {
    let mut input = vec![0.0; 64];
    input[63] = 1.0;
    input
}

fn expected_peak(input: &[f32]) -> f64 {
    let mut padded = input.to_vec();
    padded.extend_from_slice(&[0.0; 11]);
    let peak = reference::frame_peaks(&padded, 4)
        .into_iter()
        .fold(0.0, f64::max);
    20.0 * peak.log10()
}

fn expected_custom_peak_by_zero_continuation(input: &[f32], rate: u32) -> f64 {
    let mut meter = LoudnessMonitor::new(1, rate).unwrap();
    let mut continued = input.to_vec();
    continued.extend_from_slice(&[0.0; 63]);
    meter.add_frames(&continued).unwrap();
    let data = meter.get_loudness();
    assert!(data.true_peak_valid && data.true_peak_is_compliant);
    data.true_peaks_dbtp[0]
}

#[test]
fn direct_plugin_drain_measures_final_impulse_without_emitting_audio() {
    let input = final_impulse();
    let expected = expected_peak(&input);
    let mut plugin = LoudnessMonitorPlugin::new(1).unwrap();
    plugin.initialize(48_000).unwrap();
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(48_000, input.len())
            )
            .unwrap(),
        input.len()
    );
    assert_eq!(output, input);
    let mut destination = [123.0; 17];
    let drained = plugin
        .drain(&mut destination, &ProcessContext::new(48_000, 0))
        .unwrap();
    assert!(drained.complete);
    assert_eq!(drained.frames, 0);
    assert_eq!(destination, [123.0; 17]);
    let snapshot = plugin
        .get_data()
        .unwrap()
        .downcast::<LoudnessData>()
        .unwrap();
    assert!(
        (snapshot.true_peaks_dbtp[0] - expected).abs() < 2e-12,
        "final measurement={}, complete FIR response={expected}",
        snapshot.true_peaks_dbtp[0]
    );
}

#[test]
fn linear_host_drain_measures_final_impulse_without_extending_programme() {
    for compiled in [false, true] {
        let input = final_impulse();
        let expected = expected_peak(&input);
        let mut host = DawHost::new(1, 48_000);
        host.set_compiled_linear_enabled(compiled);
        host.add_plugin(Box::new(LoudnessMonitorPlugin::new(1).unwrap()))
            .unwrap();
        let mut output = vec![f32::NAN; input.len()];
        assert_eq!(host.process(&input, &mut output).unwrap(), input.len());
        assert_eq!(output, input);
        assert_eq!(host.drain_output_frames_max(), 0);
        let drained = host.drain(&mut []).unwrap();
        assert!(drained.complete);
        assert_eq!(drained.frames, 0);
        let snapshot = host
            .get_plugin_data(0)
            .unwrap()
            .downcast::<LoudnessData>()
            .unwrap();
        assert!(
            (snapshot.true_peaks_dbtp[0] - expected).abs() < 2e-12,
            "final measurement={}, complete FIR response={expected}",
            snapshot.true_peaks_dbtp[0]
        );
    }
}

fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn feed(
    plugin: &mut LoudnessMonitorPlugin,
    input: &[f32],
    rate: u32,
    channels: usize,
    path: usize,
) {
    let context = ProcessContext::new(rate, input.len() / channels);
    let mut output = vec![f32::NAN; input.len()];
    let count = match path {
        0 => plugin.process(input, &mut output, &context).unwrap(),
        1 => plugin
            .process_analyzer_tap_f32(input, &context)
            .unwrap()
            .unwrap(),
        2 => plugin
            .process_compiled_f32(PluginCompiledOp::AnalyzerTap, input, &mut output, &context)
            .unwrap()
            .unwrap(),
        _ => unreachable!(),
    };
    assert_eq!(count, context.num_frames);
    if path != 1 {
        assert_eq!(output, input);
    }
}

fn finish(plugin: &mut LoudnessMonitorPlugin, rate: u32) {
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    assert_eq!(plugin.drain_output_frames_max(), 0);
    let result = plugin
        .drain(&mut [], &ProcessContext::new(rate, 0))
        .unwrap();
    assert!(result.complete);
    assert_eq!(result.frames, 0);
}

fn without_true_peaks(data: &LoudnessData) -> serde_json::Value {
    let mut value = serde_json::to_value(data).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("true_peaks_dbtp");
    object.remove("maximum_true_peak_dbtp");
    value
}

#[test]
fn every_final_impulse_position_matches_full_convolution_across_public_paths() {
    for rate in [48_000, 88_200, 96_000] {
        let factor = if rate == 96_000 { 2 } else { 4 };
        for channels in [1, 2, 6, 24] {
            for path in 0..3 {
                let mut plugin = LoudnessMonitorPlugin::new(channels)
                    .unwrap()
                    .with_loudness_range(None)
                    .unwrap();
                plugin.initialize(rate).unwrap();
                for offset in 0..12 {
                    plugin.reset();
                    let mut input = vec![0.0; 173 * channels];
                    for ch in 0..channels {
                        input[(172 - offset) * channels + ch] = (ch + 1) as f32 / channels as f32;
                    }
                    let mut seen = vec![f64::NEG_INFINITY; channels];
                    let mut start = 0;
                    for frames in [1, 17, 137, 18] {
                        feed(
                            &mut plugin,
                            &input[start * channels..(start + frames) * channels],
                            rate,
                            channels,
                            path,
                        );
                        start += frames;
                        for (peak, value) in seen
                            .iter_mut()
                            .zip(snapshot(&plugin).true_peaks_dbtp.iter())
                        {
                            *peak = peak.max(*value);
                        }
                    }
                    finish(&mut plugin, rate);
                    let final_data = snapshot(&plugin);
                    for (ch, &previous_peak) in seen.iter().enumerate() {
                        let mut mono: Vec<f32> =
                            input.iter().skip(ch).step_by(channels).copied().collect();
                        mono.extend_from_slice(&[0.0; 11]);
                        let peak = reference::frame_peaks(&mono, factor)
                            .into_iter()
                            .fold(0.0, f64::max);
                        let expected = 20.0 * peak.log10();
                        let measured = previous_peak.max(final_data.true_peaks_dbtp[ch]);
                        assert!(
                            (measured - expected).abs() < 2e-12,
                            "rate={rate}, channels={channels}, path={path}, offset={offset}, ch={ch}: {measured} != {expected}"
                        );
                    }
                    finish(&mut plugin, rate);
                    assert!(Arc::ptr_eq(&final_data, &snapshot(&plugin)));
                    feed(&mut plugin, &[], rate, channels, path);
                    assert!(Arc::ptr_eq(&final_data, &snapshot(&plugin)));
                }
            }
        }
    }
}

#[test]
fn low_level_host_finish_preserves_other_measurements_and_query_intervals() {
    for rate in [8_000, 12_000, 44_100, 48_000, 88_200, 96_000] {
        for channels in [1, 2, 6, 24] {
            let mut meter = LoudnessMonitor::new(channels, rate).unwrap().with_spatial();
            let mut control = LoudnessMonitor::new(channels, rate).unwrap().with_spatial();
            let mut input = vec![0.0; 73 * channels as usize];
            for (i, value) in input.iter_mut().enumerate() {
                *value = ((i * 73 % 113) as f32 - 56.0) / 64.0;
            }
            let continued_peaks = if matches!(rate, 48_000 | 88_200 | 96_000) {
                None
            } else {
                let mut continued = input.clone();
                continued.extend(vec![0.0; 63 * channels as usize]);
                let mut complete = LoudnessMonitor::new(channels, rate).unwrap();
                complete.add_frames(&continued).unwrap();
                Some(complete.get_loudness().true_peaks_dbtp)
            };
            meter.add_frames(&input).unwrap();
            control.add_frames(&input).unwrap();
            meter.finish_true_peak();
            meter.finish_true_peak();
            let finished = meter.get_loudness();
            assert_eq!(
                without_true_peaks(&finished),
                without_true_peaks(&control.get_loudness())
            );
            for ch in 0..channels as usize {
                let mut mono: Vec<f32> = input
                    .iter()
                    .skip(ch)
                    .step_by(channels as usize)
                    .copied()
                    .collect();
                let expected = if matches!(rate, 48_000 | 88_200) {
                    mono.extend_from_slice(&[0.0; 11]);
                    let peak = reference::frame_peaks(&mono, 4)
                        .into_iter()
                        .fold(0.0, f64::max);
                    20.0 * peak.log10()
                } else if rate == 96_000 {
                    mono.extend_from_slice(&[0.0; 11]);
                    let peak = reference::frame_peaks(&mono, 2)
                        .into_iter()
                        .fold(0.0, f64::max);
                    20.0 * peak.log10()
                } else {
                    continued_peaks.as_ref().unwrap()[ch]
                };
                assert!(
                    (finished.true_peaks_dbtp[ch] - expected).abs() < 2e-12,
                    "rate={rate}, channels={channels}, channel={ch}"
                );
            }
            meter.finish_true_peak();
            assert!(
                meter
                    .get_loudness()
                    .true_peaks_dbtp
                    .iter()
                    .all(|&p| p == f64::NEG_INFINITY)
            );
            meter.add_frames(&input).unwrap();
            meter.finish_true_peak();
            assert_eq!(
                *meter.get_loudness().true_peaks_dbtp,
                *finished.true_peaks_dbtp
            );
            meter.reset().unwrap();
            meter.finish_true_peak();
            assert!(
                meter
                    .get_loudness()
                    .true_peaks_dbtp
                    .iter()
                    .all(|&p| p == f64::NEG_INFINITY)
            );
        }
    }
}

#[test]
fn backend_finish_only_changes_true_peak_and_starts_next_segment_cleanly() {
    for channels in [1, 2, 6, 24] {
        let mode = Mode::M | Mode::S | Mode::I | Mode::SAMPLE_PEAK | Mode::TRUE_PEAK;
        let mut meter = EbuR128::new(channels, 48_000, mode).unwrap();
        let mut control = EbuR128::new(channels, 48_000, mode).unwrap();
        let input: Vec<f32> = (0..144_007 * channels as usize)
            .map(|i| ((i * 17 % 251) as f32 - 125.0) / 256.0)
            .collect();
        meter.add_frames_f32(&input).unwrap();
        control.add_frames_f32(&input).unwrap();
        meter.finish_true_peak();
        meter.finish_true_peak();
        assert_eq!(meter.loudness_momentary(), control.loudness_momentary());
        assert_eq!(meter.loudness_shortterm(), control.loudness_shortterm());
        assert_eq!(meter.loudness_global(), control.loudness_global());
        assert_eq!(
            meter.gating_block_count_and_energy(),
            control.gating_block_count_and_energy()
        );
        for ch in 0..channels {
            assert_eq!(meter.prev_sample_peak(ch), control.prev_sample_peak(ch));
            let mut mono: Vec<f32> = input
                .iter()
                .skip(ch as usize)
                .step_by(channels as usize)
                .copied()
                .collect();
            mono.extend_from_slice(&[0.0; 11]);
            let expected = reference::frame_peaks(&mono, 4)
                .into_iter()
                .fold(0.0, f64::max);
            assert!((meter.prev_true_peak(ch).unwrap() - expected).abs() < 2e-12);
        }
        meter.finish_true_peak();
        for ch in 0..channels {
            assert_eq!(meter.prev_true_peak(ch).unwrap(), 0.0);
        }
        // Resume with silence: neither delayed interpolation nor loudness time
        // should leak across a finished segment.
        let silence = vec![0.0; 48_000 * channels as usize];
        meter.add_frames_f32(&silence).unwrap();
        control.add_frames_f32(&silence).unwrap();
        meter.finish_true_peak();
        assert_eq!(meter.loudness_global(), control.loudness_global());
        for ch in 0..channels {
            assert_eq!(meter.prev_true_peak(ch).unwrap(), 0.0);
        }
        meter.reset();
        meter.finish_true_peak();
        for ch in 0..channels {
            assert_eq!(meter.prev_true_peak(ch).unwrap(), 0.0);
        }
    }
    let mut disabled = EbuR128::new(1, 48_000, Mode::M).unwrap();
    disabled.finish_true_peak();
    assert!(disabled.prev_true_peak(0).is_err());
}

#[test]
fn plugin_finalization_preserves_loudness_lra_correlation_and_sample_peaks() {
    let mut plugin = LoudnessMonitorPlugin::new(2).unwrap().with_spatial();
    plugin.initialize(48_000).unwrap();
    let mut input: Vec<f32> = (0..148_803 * 2)
        .map(|i| ((i * 13 % 251) as f32 - 125.0) / 256.0)
        .collect();
    *input.last_mut().unwrap() = 1.0;
    feed(&mut plugin, &input, 48_000, 2, 0);
    let before = snapshot(&plugin);
    assert!(before.measurement_valid);
    assert!(before.loudness_range.is_some());
    assert_eq!(before.correlation_samples_seen, 148_803);
    finish(&mut plugin, 48_000);
    assert_eq!(
        without_true_peaks(&before),
        without_true_peaks(&snapshot(&plugin))
    );
}

#[test]
fn invalid_drain_and_lifecycle_changes_do_not_consume_or_replay_peaks() {
    for rate in [8_000, 12_000, 44_100, 48_000] {
        let mut plugin = LoudnessMonitorPlugin::new(1).unwrap();
        let context = ProcessContext::new(rate, 0);
        assert!(plugin.drain(&mut [], &context).is_err());
        plugin.initialize(rate).unwrap();
        feed(&mut plugin, &final_impulse(), rate, 1, 0);
        let before = snapshot(&plugin);
        for invalid in [
            ProcessContext::new(rate.saturating_add(1), 0),
            ProcessContext::new(rate, 1),
        ] {
            assert!(plugin.drain(&mut [], &invalid).is_err());
            assert!(Arc::ptr_eq(&before, &snapshot(&plugin)));
        }
        finish(&mut plugin, rate);
        let initial_peak = snapshot(&plugin).true_peaks_dbtp[0];
        assert!(initial_peak.is_finite());
        assert!(snapshot(&plugin).true_peak_valid && snapshot(&plugin).true_peak_is_compliant);
        let expected = if rate == 48_000 {
            expected_peak(&final_impulse())
        } else {
            expected_custom_peak_by_zero_continuation(&final_impulse(), rate)
        };
        assert!(
            (initial_peak - expected).abs() < 2e-12,
            "rate={rate} Hz: initial peak {initial_peak} differs from complete-support reference {expected}"
        );
        feed(&mut plugin, &[0.0; 64], rate, 1, 0);
        finish(&mut plugin, rate);
        assert_eq!(snapshot(&plugin).true_peaks_dbtp[0], f64::NEG_INFINITY);
        for action in 0..3 {
            feed(&mut plugin, &final_impulse(), rate, 1, 0);
            finish(&mut plugin, rate);
            match action {
                0 => plugin.reset(),
                1 => plugin.initialize(rate).unwrap(),
                2 => plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
                    .unwrap(),
                _ => unreachable!(),
            }
            finish(&mut plugin, rate);
            assert_eq!(snapshot(&plugin).true_peaks_dbtp[0], f64::NEG_INFINITY);
            assert_eq!(snapshot(&plugin).peak, 0.0);
            if action == 2 {
                plugin
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
                    .unwrap();
                feed(&mut plugin, &final_impulse(), rate, 1, 0);
                finish(&mut plugin, rate);
                assert!(
                    (snapshot(&plugin).true_peaks_dbtp[0] - initial_peak).abs() < 2e-12,
                    "rate={rate} Hz: peak after enable differs from initial epoch"
                );
            }
        }
    }
}

#[test]
fn retained_outer_and_nested_readers_delay_final_publication_without_losing_peaks() {
    for rate in [8_000, 12_000, 44_100, 48_000] {
        for weak in [false, true] {
            let mut plugin = LoudnessMonitorPlugin::new(1).unwrap();
            plugin.initialize(rate).unwrap();
            let mut strong_readers = Vec::new();
            let mut weak_readers = Vec::new();
            for _ in 0..3 {
                feed(&mut plugin, &[0.0; 16], rate, 1, 0);
                let data = snapshot(&plugin);
                if weak {
                    weak_readers.push(Arc::downgrade(&data.true_peaks_dbtp));
                } else {
                    strong_readers.push(data);
                }
            }
            feed(&mut plugin, &final_impulse(), rate, 1, 0);
            let old = snapshot(&plugin);
            finish(&mut plugin, rate);
            assert!(Arc::ptr_eq(&old, &snapshot(&plugin)));
            assert!(strong_readers.iter().all(|d| d.peak == 0.0));
            assert!(
                weak_readers
                    .iter()
                    .all(|w| w.upgrade().unwrap()[0] == f64::NEG_INFINITY)
            );
            drop((strong_readers, weak_readers));
            finish(&mut plugin, rate);
            let final_data = snapshot(&plugin);
            assert!(final_data.true_peak_valid && final_data.true_peak_is_compliant);
            let expected = if rate == 48_000 {
                expected_peak(&final_impulse())
            } else {
                expected_custom_peak_by_zero_continuation(&final_impulse(), rate)
            };
            assert!(
                (final_data.true_peaks_dbtp[0] - expected).abs() < 2e-12,
                "rate={rate} Hz: recovered peak {} differs from complete-support reference {expected}",
                final_data.true_peaks_dbtp[0]
            );
            assert_eq!(snapshot(&plugin).peak, 1.0);
        }
    }
}

#[test]
fn reset_under_retained_readers_never_merges_previous_epoch() {
    let mut plugin = LoudnessMonitorPlugin::new(1).unwrap();
    plugin.initialize(48_000).unwrap();
    let mut retained = Vec::new();
    for _ in 0..3 {
        feed(&mut plugin, &[1.0; 16], 48_000, 1, 0);
        retained.push(snapshot(&plugin));
    }
    plugin.reset();
    finish(&mut plugin, 48_000);
    assert!(snapshot(&plugin).peak > 0.0); // All three prepared slots are held.
    retained.clear();
    finish(&mut plugin, 48_000);
    assert_eq!(snapshot(&plugin).peak, 0.0);
    assert_eq!(snapshot(&plugin).true_peaks_dbtp[0], f64::NEG_INFINITY);
    assert!(!snapshot(&plugin).momentary_valid);
}

struct FinalImpulseTail(bool);

impl Plugin for FinalImpulseTail {
    fn info(&self) -> sotf_host::PluginInfo {
        sotf_host::PluginInfo::new("tail impulse", "1", "test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<sotf_host::Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("no parameters".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        self.0 = true;
        Ok(input.len())
    }
    fn drain_output_frames_max(&self) -> usize {
        1
    }
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(2)
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        _: &ProcessContext,
    ) -> Result<sotf_host::plugin::PluginDrainResult, String> {
        if !self.0 {
            return Ok(sotf_host::plugin::PluginDrainResult::COMPLETE);
        }
        output[0] = 0.75;
        self.0 = false;
        Ok(sotf_host::plugin::PluginDrainResult {
            frames: 1,
            complete: false,
        })
    }
}

#[test]
fn host_includes_upstream_tail_before_finishing_downstream_meter() {
    for compiled in [false, true] {
        let mut host = DawHost::new(1, 48_000);
        host.set_compiled_linear_enabled(compiled);
        host.add_plugin(Box::new(FinalImpulseTail(false))).unwrap();
        host.add_plugin(Box::new(LoudnessMonitorPlugin::new(1).unwrap()))
            .unwrap();
        let mut output = [f32::NAN; 64];
        assert_eq!(host.process(&[0.0; 64], &mut output).unwrap(), 64);
        assert_eq!(output, [0.0; 64]);
        let mut emitted = Vec::new();
        let mut complete = false;
        for _ in 0..3 {
            let result = host.drain(&mut output).unwrap();
            emitted.extend_from_slice(&output[..result.frames]);
            if result.complete {
                complete = true;
                break;
            }
        }
        assert!(complete);
        assert_eq!(emitted, [0.75]);
        let data = host
            .get_plugin_data(1)
            .unwrap()
            .downcast::<LoudnessData>()
            .unwrap();
        assert!((data.true_peaks_dbtp[0] - expected_peak(&[0.75])).abs() < 2e-12);
        assert_eq!(data.peak, 0.75);
    }
}

#[test]
fn unsupported_rate_finish_keeps_true_peak_unavailable() {
    let mut plugin = LoudnessMonitorPlugin::new(1).unwrap();
    plugin.initialize(7_999).unwrap();
    feed(&mut plugin, &final_impulse(), 7_999, 1, 0);
    finish(&mut plugin, 7_999);
    let data = snapshot(&plugin);
    assert!(!data.true_peak_valid);
    assert!(!data.true_peak_is_compliant);
    assert_eq!(data.true_peaks_dbtp[0], f64::NEG_INFINITY);
    assert_eq!(data.peak, 1.0);
}
