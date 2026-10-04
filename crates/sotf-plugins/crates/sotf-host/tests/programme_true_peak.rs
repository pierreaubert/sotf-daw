//! Programme-latched true-peak snapshots preserve measurement-epoch maxima.
// Rust guideline compliant 2026-02-21
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::{
    LoudnessData, LoudnessMonitorPlugin, ParameterId, ParameterValue, Plugin, ProcessContext,
};
use std::sync::Arc;

#[path = "common/true_peak_reference.rs"]
mod reference;

fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

fn prepare(channels: usize, rate: u32) -> LoudnessMonitorPlugin {
    let mut plugin = LoudnessMonitorPlugin::new(channels)
        .unwrap()
        .with_loudness_range(None)
        .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn feed(plugin: &mut LoudnessMonitorPlugin, input: &[f32], rate: u32, channels: usize) {
    assert_eq!(input.len() % channels, 0);
    let frames = input.len() / channels;
    let mut output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(input, &mut output, &ProcessContext::new(rate, frames))
            .unwrap(),
        frames
    );
    assert_eq!(output, input);
}

fn current_interval_max(data: &LoudnessData) -> Option<f64> {
    data.true_peaks_dbtp
        .iter()
        .copied()
        .filter(|peak| peak.is_finite())
        .reduce(f64::max)
}

fn reference_peak_dbtp(interleaved: &[f32], channels: usize, factor: usize) -> Option<f64> {
    (0..channels)
        .filter_map(|channel| {
            let mut lane: Vec<f32> = interleaved
                .iter()
                .skip(channel)
                .step_by(channels)
                .copied()
                .collect();
            lane.extend_from_slice(&[0.0; 11]);
            let peak = reference::frame_peaks(&lane, factor)
                .into_iter()
                .fold(0.0_f64, f64::max);
            (peak > 0.0).then_some(20.0 * peak.log10())
        })
        .reduce(f64::max)
}

fn independent_long_sinc_peak(samples: &[f32]) -> f64 {
    // Offline reconstruction oracle with a radius-64 Lanczos window, distinct
    // from the production coefficient bank and its phase evaluation grid.
    const RADIUS: isize = 64;
    const DENSE_PHASES: usize = 64;
    let start = -RADIUS;
    let end = isize::try_from(samples.len()).expect("fixture fits signed indices") + RADIUS;
    let point_count = usize::try_from(end - start)
        .expect("oracle range is nonnegative")
        .checked_mul(DENSE_PHASES)
        .expect("oracle grid fits usize");
    let mut peak = 0.0_f64;
    for point in 0..point_count {
        let time = start as f64 + point as f64 / DENSE_PHASES as f64;
        let center = time.floor() as isize;
        let mut value = 0.0;
        let mut dc_gain = 0.0;
        for source_index in (center - RADIUS)..=(center + RADIUS) {
            let distance = time - source_index as f64;
            if distance.abs() >= RADIUS as f64 {
                continue;
            }
            let sinc = |x: f64| {
                if x == 0.0 {
                    1.0
                } else {
                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                }
            };
            let weight = sinc(distance) * sinc(distance / RADIUS as f64);
            dc_gain += weight;
            if let Some(&sample) = usize::try_from(source_index)
                .ok()
                .and_then(|index| samples.get(index))
            {
                value += f64::from(sample) * weight;
            }
        }
        peak = peak.max((value / dc_gain).abs());
    }
    peak
}

fn expected_final_impulse_peak_dbtp(input: &[f32], rate: u32) -> f64 {
    match rate {
        48_000 => reference_peak_dbtp(input, 1, 4).unwrap(),
        96_000 => reference_peak_dbtp(input, 1, 2).unwrap(),
        8_000 => {
            let mut complete_support = input.to_vec();
            complete_support.extend_from_slice(&[0.0; 64]);
            20.0 * independent_long_sinc_peak(&complete_support).log10()
        }
        _ => unreachable!("this regression covers published and custom-rate paths"),
    }
}

fn process_program(rate: u32, partitions: &[usize]) -> LoudnessData {
    let channels = 2;
    let mut input = vec![0.0_f32; 320 * channels];
    input[20 * channels] = 0.8;
    input[90 * channels + 1] = 0.5;
    input[190 * channels] = -0.4;
    input[319 * channels + 1] = 0.3;
    let factor = match rate {
        48_000 => 4,
        96_000 => 2,
        _ => unreachable!("published reference is used only for 48 and 96 kHz"),
    };
    let expected = reference_peak_dbtp(&input, channels, factor).unwrap();
    let mut plugin = prepare(channels, rate);
    let mut offset = 0;
    let mut observed_maximum = None;
    let mut early_maximum = None;
    let mut later_interval_maximum = None;
    for &frames in partitions {
        let start = offset * channels;
        let end = (offset + frames) * channels;
        feed(&mut plugin, &input[start..end], rate, channels);
        offset += frames;
        let data = snapshot(&plugin);
        if let Some(interval_maximum) = current_interval_max(&data) {
            observed_maximum = Some(observed_maximum.map_or(interval_maximum, |maximum: f64| {
                maximum.max(interval_maximum)
            }));
        }
        assert_eq!(data.maximum_true_peak_dbtp, observed_maximum);
        if offset == 40 {
            early_maximum = observed_maximum;
        }
        if offset == 128 {
            later_interval_maximum = current_interval_max(&data);
        }
    }
    assert_eq!(offset, 320);
    if partitions.len() > 1 {
        let early = early_maximum.expect("first interval contains the loud impulse");
        assert!(
            later_interval_maximum.is_none_or(|later| later < early),
            "later interval={later_interval_maximum:?}, early maximum={early}"
        );
        assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, early_maximum);
    }

    let result = plugin
        .drain(&mut [], &ProcessContext::new(rate, 0))
        .unwrap();
    assert!(result.complete);
    let final_data = snapshot(&plugin);
    let actual = final_data
        .maximum_true_peak_dbtp
        .expect("the programme contains non-silent audio");
    assert!(
        (actual - expected).abs() < 2.0e-12,
        "rate={rate}, actual={actual}, independent full convolution={expected}"
    );
    plugin
        .drain(&mut [], &ProcessContext::new(rate, 0))
        .unwrap();
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, Some(actual));
    final_data.as_ref().clone()
}

#[test]
fn programme_maximum_matches_independent_full_convolution_across_queries() {
    let whole = process_program(48_000, &[320]);
    let partitioned = process_program(48_000, &[40, 24, 64, 32, 64, 96]);
    assert_eq!(
        whole.maximum_true_peak_dbtp,
        partitioned.maximum_true_peak_dbtp
    );

    let high_rate = process_program(96_000, &[40, 24, 64, 32, 64, 96]);
    assert!(high_rate.maximum_true_peak_dbtp.unwrap().is_finite());
}

#[test]
fn final_largest_impulse_updates_programme_maximum_after_drain_with_retained_readers() {
    for rate in [48_000, 96_000, 8_000] {
        let mut input = vec![0.0_f32; 128];
        input[127] = 1.0;
        let expected = expected_final_impulse_peak_dbtp(&input, rate);

        for retain_weak in [false, true] {
            let mut plugin = prepare(1, rate);
            let mut strong_readers = Vec::new();
            let mut weak_readers = Vec::new();
            for _ in 0..3 {
                feed(&mut plugin, &[0.0; 16], rate, 1);
                let data = snapshot(&plugin);
                if retain_weak {
                    weak_readers.push(Arc::downgrade(&data.true_peaks_dbtp));
                } else {
                    strong_readers.push(data);
                }
            }

            feed(&mut plugin, &input, rate, 1);
            let before_drain = snapshot(&plugin);
            assert!(
                before_drain
                    .maximum_true_peak_dbtp
                    .is_none_or(|maximum| maximum < expected - 0.1),
                "rate={rate}, weak_readers={retain_weak}, pre-drain maximum={:?}, final response={expected}",
                before_drain.maximum_true_peak_dbtp
            );

            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            let blocked = snapshot(&plugin);
            assert!(Arc::ptr_eq(&before_drain, &blocked));
            assert_eq!(
                blocked.maximum_true_peak_dbtp,
                before_drain.maximum_true_peak_dbtp
            );
            if retain_weak {
                assert!(weak_readers.iter().all(|reader| reader.upgrade().is_some()));
            } else {
                assert!(
                    strong_readers
                        .iter()
                        .all(|reader| reader.maximum_true_peak_dbtp.is_none())
                );
            }

            drop((before_drain, blocked, strong_readers, weak_readers));
            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            let recovered = snapshot(&plugin);
            let actual = recovered
                .maximum_true_peak_dbtp
                .expect("draining publishes the final finite peak");
            let tolerance = if rate == 8_000 { 0.03 } else { 2.0e-12 };
            assert!(
                (actual - expected).abs() < tolerance,
                "rate={rate}, weak_readers={retain_weak}, recovered={actual}, reference={expected}"
            );

            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap();
            assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, Some(actual));
        }
    }
}

#[test]
fn finite_maximum_reduces_supported_rates_and_none_covers_silence() {
    for rate in [8_000, 12_000, 44_100, 48_000, 96_000] {
        let mut monitor = LoudnessMonitor::new(2, rate).unwrap();
        let mut data = LoudnessData::new(2);
        monitor.add_frames(&vec![0.0; 64 * 2]).unwrap();
        monitor.update_loudness_data(&mut data);
        assert_eq!(data.maximum_true_peak_dbtp, None, "silence at {rate} Hz");

        let mut loud = vec![0.0_f32; 64 * 2];
        loud[31 * 2] = 0.75;
        monitor.add_frames(&loud).unwrap();
        monitor.update_loudness_data(&mut data);
        let first_maximum = current_interval_max(&data).unwrap();
        assert_eq!(data.maximum_true_peak_dbtp, Some(first_maximum));

        monitor.add_frames(&vec![0.0; 64 * 2]).unwrap();
        monitor.update_loudness_data(&mut data);
        assert!(
            current_interval_max(&data).is_none_or(|tail| tail <= first_maximum),
            "silence tail exceeded the preceding maximum at {rate} Hz: {:?}",
            data.true_peaks_dbtp
        );
        assert_eq!(data.maximum_true_peak_dbtp, Some(first_maximum));
        assert!(data.maximum_true_peak_dbtp.unwrap().is_finite());
    }

    let mut unsupported = prepare(1, 7_999);
    let mut input = vec![0.0_f32; 64];
    input[31] = 1.0;
    feed(&mut unsupported, &input, 7_999, 1);
    let unsupported_data = snapshot(&unsupported);
    assert!(!unsupported_data.true_peak_is_compliant);
    assert_eq!(unsupported_data.maximum_true_peak_dbtp, None);
}

#[test]
fn rejected_callbacks_and_empty_drain_do_not_create_a_maximum() {
    let mut plugin = prepare(1, 48_000);
    let mut impulse = vec![0.0_f32; 64];
    impulse[31] = 1.0;
    let mut output = vec![0.0; impulse.len()];
    assert!(
        plugin
            .process(
                &impulse,
                &mut output,
                &ProcessContext::new(44_100, impulse.len())
            )
            .is_err()
    );
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);

    plugin
        .drain(&mut [], &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);

    impulse[31] = 0.1;
    feed(&mut plugin, &impulse, 48_000, 1);
    let data = snapshot(&plugin);
    assert!(data.maximum_true_peak_dbtp.unwrap() < -10.0);
}

#[test]
fn reset_reinitialize_and_disable_start_fresh_epochs_with_retained_generations() {
    let mut plugin = prepare(1, 48_000);
    let cold = snapshot(&plugin);
    let mut high = vec![0.0_f32; 64];
    high[31] = 0.8;
    feed(&mut plugin, &high, 48_000, 1);
    let first = snapshot(&plugin);
    let high_maximum = first.maximum_true_peak_dbtp.unwrap();

    let mut low = vec![0.0_f32; 64];
    low[31] = 0.2;
    feed(&mut plugin, &low, 48_000, 1);
    let second = snapshot(&plugin);
    assert_eq!(second.maximum_true_peak_dbtp, Some(high_maximum));
    let weak_cold = Arc::downgrade(&cold.true_peaks_dbtp);
    let weak_first = Arc::downgrade(&first.true_peaks_dbtp);
    let weak_second = Arc::downgrade(&second.true_peaks_dbtp);

    plugin.reset();
    let still_published = snapshot(&plugin);
    assert_eq!(still_published.maximum_true_peak_dbtp, Some(high_maximum));
    assert_eq!(first.maximum_true_peak_dbtp, Some(high_maximum));
    assert_eq!(second.maximum_true_peak_dbtp, Some(high_maximum));
    drop(still_published);
    drop((cold, first, second));

    let silence = vec![0.0_f32; 64];
    feed(&mut plugin, &silence, 48_000, 1);
    drop((weak_cold, weak_first, weak_second));
    feed(&mut plugin, &silence, 48_000, 1);
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);

    feed(&mut plugin, &low, 48_000, 1);
    let post_reset = snapshot(&plugin).maximum_true_peak_dbtp.unwrap();
    assert!(post_reset < high_maximum);

    let old_epoch = snapshot(&plugin);
    plugin.initialize(44_100.0).unwrap();
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);
    assert_eq!(old_epoch.maximum_true_peak_dbtp, Some(post_reset));
    let mut lower_rate_peak = vec![0.0_f32; 64];
    lower_rate_peak[31] = 0.1;
    feed(&mut plugin, &lower_rate_peak, 44_100, 1);
    let reinitialized_maximum = snapshot(&plugin).maximum_true_peak_dbtp.unwrap();
    assert!(reinitialized_maximum < post_reset);

    let enabled = ParameterId::from("enabled");
    plugin
        .set_parameter(enabled.clone(), ParameterValue::Bool(false))
        .unwrap();
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);
    plugin
        .set_parameter(enabled, ParameterValue::Bool(true))
        .unwrap();
    assert_eq!(snapshot(&plugin).maximum_true_peak_dbtp, None);
    feed(&mut plugin, &lower_rate_peak, 44_100, 1);
    assert_eq!(
        snapshot(&plugin).maximum_true_peak_dbtp,
        Some(reinitialized_maximum)
    );
}

#[test]
fn maximum_field_defaults_updates_and_serializes_without_nonfinite_values() {
    let cold = LoudnessData::new(1);
    assert_eq!(cold.maximum_true_peak_dbtp, None);
    assert_eq!(LoudnessData::default().maximum_true_peak_dbtp, None);
    let cold_json = serde_json::to_value(&cold).unwrap();
    assert!(
        cold_json
            .as_object()
            .unwrap()
            .get("maximum_true_peak_dbtp")
            .is_none()
    );

    let mut finite = LoudnessData::new(1);
    finite.momentary_lufs = -20.0;
    finite.shortterm_lufs = -20.0;
    finite.integrated_lufs = -20.0;
    finite.true_peaks_dbtp = Arc::new(vec![-12.0]);
    finite.maximum_true_peak_dbtp = Some(-12.0);
    let json = serde_json::to_value(&finite).unwrap();
    assert_eq!(json["maximum_true_peak_dbtp"], -12.0);
    let roundtrip: LoudnessData = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(roundtrip.maximum_true_peak_dbtp, Some(-12.0));

    let mut legacy = json;
    legacy
        .as_object_mut()
        .unwrap()
        .remove("maximum_true_peak_dbtp");
    let old_payload: LoudnessData = serde_json::from_value(legacy).unwrap();
    assert_eq!(old_payload.maximum_true_peak_dbtp, None);
    assert_eq!(old_payload.integrated_lufs, -20.0);

    let mut copied = LoudnessData::new(1);
    copied.update_from(&finite);
    assert_eq!(copied.maximum_true_peak_dbtp, Some(-12.0));

    let mut monitor = LoudnessMonitor::new(1, 48_000).unwrap();
    monitor.add_frames(&[0.0; 64]).unwrap();
    let silence = monitor.get_loudness();
    assert_eq!(silence.maximum_true_peak_dbtp, None);
    let silence_json = serde_json::to_value(&silence).unwrap();
    assert!(
        silence_json
            .as_object()
            .unwrap()
            .get("maximum_true_peak_dbtp")
            .is_none()
    );
}
