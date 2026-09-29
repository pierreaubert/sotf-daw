//! Prepared loudness snapshots remain coherent without callback allocation or destruction.
// Rust guideline compliant 2026-02-21
use sotf_host::{
    IntegratedLoudnessMode, LoudnessData, LoudnessMonitorPlugin, ParameterId, ParameterValue,
    Plugin, ProcessContext,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Arc, Weak};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
// SAFETY: All requests, live pointers and original layouts are forwarded unchanged to System.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        COUNTING.with(|active| {
            if active.get() {
                COUNTS.with(|counts| {
                    let (allocations, frees) = counts.get();
                    counts.set((allocations + 1, frees));
                });
            }
        });
        // SAFETY: The valid allocation request is forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        COUNTING.with(|active| {
            if active.get() {
                COUNTS.with(|counts| {
                    let (allocations, frees) = counts.get();
                    counts.set((allocations, frees + 1));
                });
            }
        });
        // SAFETY: The live pointer and its original layout are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

struct StopCounting;
impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNTING.with(|active| active.set(false));
    }
}
fn without_heap<T>(call: impl FnOnce() -> T) -> T {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let result = call();
    drop(guard);
    assert_eq!(COUNTS.with(Cell::get), (0, 0), "allocations and frees");
    result
}
fn prepared(channels: usize, spatial: bool) -> LoudnessMonitorPlugin {
    let mut plugin = LoudnessMonitorPlugin::new(channels).unwrap();
    plugin.set_spatial_enabled(spatial);
    plugin.initialize(48_000).unwrap();
    plugin
}
fn input(channels: usize, scale: f32) -> Vec<f32> {
    (0..32 * channels)
        .map(|index| {
            let frame = index / channels;
            let channel = index % channels;
            ((frame * 7 % 19) as f32 - 9.0) / 16.0
                * scale
                * if channel % 2 == 1 { -1.0 } else { 1.0 }
        })
        .collect()
}
fn sustained_tone(channels: usize, frames: usize, peak_dbfs: f64) -> Vec<f32> {
    let amplitude = 10.0_f64.powf(peak_dbfs / 20.0);
    (0..frames)
        .flat_map(|frame| {
            let sample = (amplitude
                * (std::f64::consts::TAU * 997.0 * frame as f64 / 48_000.0).sin())
                as f32;
            std::iter::repeat_n(sample, channels)
        })
        .collect()
}
fn process(plugin: &mut LoudnessMonitorPlugin, values: &[f32], output: &mut [f32], tap: bool) {
    let frames = values.len() / plugin.input_channels();
    let context = ProcessContext::new(48_000, frames);
    let result = if tap {
        without_heap(|| plugin.process_analyzer_tap_f32(values, &context)).unwrap()
    } else {
        let result = without_heap(|| plugin.process(values, output, &context));
        assert_eq!(output, values);
        result
    };
    assert_eq!(result.unwrap(), frames);
}
fn snapshot(plugin: &LoudnessMonitorPlugin) -> Arc<LoudnessData> {
    plugin.get_data().unwrap().downcast().unwrap()
}
fn enabled(plugin: &mut LoudnessMonitorPlugin, value: bool) {
    // The caller retains its prepared ID so consuming the setter's Arc clone
    // cannot destroy the caller's last allocation on the callback thread.
    let id = ParameterId::from("enabled");
    without_heap(|| plugin.set_parameter(id.clone(), ParameterValue::Bool(value))).unwrap();
}
fn assert_clear(data: &LoudnessData, channels: usize, spatial: bool, enabled: bool) {
    assert_eq!(data.measurement_enabled, enabled);
    assert!(!data.measurement_valid);
    assert!(!data.momentary_valid && !data.shortterm_valid && !data.integrated_valid);
    assert!(!data.sample_peak_valid && !data.true_peak_valid && !data.true_peak_is_compliant);
    assert_eq!(data.query_error, None);
    assert_eq!(data.query_error_generation, 0);
    assert_eq!(data.momentary_lufs, f64::NEG_INFINITY);
    assert_eq!(data.shortterm_lufs, f64::NEG_INFINITY);
    assert_eq!(data.integrated_lufs, f64::NEG_INFINITY);
    assert_eq!(data.maximum_momentary_lufs, None);
    assert_eq!(data.maximum_shortterm_lufs, None);
    assert_eq!(data.peak, 0.0);
    assert_eq!(data.maximum_true_peak_dbtp, None);
    assert_eq!(data.correlation_lr, None);
    assert_eq!(data.correlation_samples_seen, 0);
    let range = data
        .loudness_range
        .expect("analyzer prepares LRA by default");
    assert_eq!(range.status, sotf_host::LoudnessRangeStatus::WarmingUp);
    assert_eq!(range.observed_windows, 0);
    assert_eq!(range.range_lu, None);
    assert_eq!(data.channel_peaks.len(), channels);
    assert_eq!(data.true_peaks_dbtp.len(), channels);
    assert!(data.channel_peaks.iter().all(|x| *x == 0.0));
    assert!(data.true_peaks_dbtp.iter().all(|x| *x == f64::NEG_INFINITY));
    assert_eq!(
        data.correlation_matrix.len(),
        if spatial { channels * channels } else { 0 }
    );
    assert!(data.correlation_matrix.iter().all(|x| *x == 0.0));
}
fn assert_same(actual: &LoudnessData, expected: &LoudnessData) {
    assert_eq!(actual.loudness_range, expected.loudness_range);
    assert_eq!(actual.measurement_enabled, expected.measurement_enabled);
    assert_eq!(actual.measurement_valid, expected.measurement_valid);
    assert_eq!(actual.momentary_valid, expected.momentary_valid);
    assert_eq!(actual.shortterm_valid, expected.shortterm_valid);
    assert_eq!(actual.integrated_valid, expected.integrated_valid);
    assert_eq!(actual.sample_peak_valid, expected.sample_peak_valid);
    assert_eq!(actual.true_peak_valid, expected.true_peak_valid);
    assert_eq!(
        actual.true_peak_is_compliant,
        expected.true_peak_is_compliant
    );
    assert_eq!(actual.query_error, expected.query_error);
    assert_eq!(
        actual.query_error_generation,
        expected.query_error_generation
    );
    assert_eq!(actual.momentary_lufs, expected.momentary_lufs);
    assert_eq!(actual.shortterm_lufs, expected.shortterm_lufs);
    assert_eq!(actual.integrated_lufs, expected.integrated_lufs);
    assert_eq!(
        actual.maximum_momentary_lufs,
        expected.maximum_momentary_lufs
    );
    assert_eq!(
        actual.maximum_shortterm_lufs,
        expected.maximum_shortterm_lufs
    );
    assert_eq!(actual.integrated_mode, expected.integrated_mode);
    assert_eq!(
        actual.integrated_window_seconds,
        expected.integrated_window_seconds
    );
    assert_eq!(
        actual.channel_layout_is_compliant,
        expected.channel_layout_is_compliant
    );
    assert_eq!(actual.peak, expected.peak);
    assert_eq!(
        actual.maximum_true_peak_dbtp,
        expected.maximum_true_peak_dbtp
    );
    assert_eq!(actual.correlation_lr, expected.correlation_lr);
    assert_eq!(
        actual.correlation_samples_seen,
        expected.correlation_samples_seen
    );
    assert_eq!(*actual.channel_peaks, *expected.channel_peaks);
    assert_eq!(*actual.true_peaks_dbtp, *expected.true_peaks_dbtp);
    assert_eq!(*actual.correlation_matrix, *expected.correlation_matrix);
}

#[derive(Clone, Copy)]
enum Field {
    Matrix,
    SamplePeak,
    TruePeak,
}
enum Retained {
    Matrix(Arc<Vec<f32>>),
    WeakMatrix(Weak<Vec<f32>>),
    Peaks(Arc<Vec<f64>>),
    WeakPeaks(Weak<Vec<f64>>),
    Outer(Arc<LoudnessData>),
    WeakOuter(Weak<LoudnessData>),
}
impl Retained {
    fn new(plugin: &LoudnessMonitorPlugin, field: Field, weak: bool) -> Self {
        let data = snapshot(plugin);
        match (field, weak) {
            (Field::Matrix, false) => Self::Matrix(Arc::clone(&data.correlation_matrix)),
            (Field::Matrix, true) => Self::WeakMatrix(Arc::downgrade(&data.correlation_matrix)),
            (Field::SamplePeak, false) => Self::Peaks(Arc::clone(&data.channel_peaks)),
            (Field::SamplePeak, true) => Self::WeakPeaks(Arc::downgrade(&data.channel_peaks)),
            (Field::TruePeak, false) => Self::Peaks(Arc::clone(&data.true_peaks_dbtp)),
            (Field::TruePeak, true) => Self::WeakPeaks(Arc::downgrade(&data.true_peaks_dbtp)),
        }
    }
    fn values(&self) -> Vec<f64> {
        match self {
            Self::Matrix(data) => data.iter().map(|x| f64::from(*x)).collect(),
            Self::WeakMatrix(data) => data
                .upgrade()
                .expect("retained producer matrix")
                .iter()
                .map(|x| f64::from(*x))
                .collect(),
            Self::Peaks(data) => data.to_vec(),
            Self::WeakPeaks(data) => data.upgrade().expect("retained producer peaks").to_vec(),
            Self::Outer(data) => data.channel_peaks.to_vec(),
            Self::WeakOuter(data) => data
                .upgrade()
                .expect("retained producer snapshot")
                .channel_peaks
                .to_vec(),
        }
    }
}

#[test]
fn retained_nested_strong_and_weak_readers_use_prepared_fallback_without_heap() {
    for channels in [2, 7, 32, 40] {
        for spatial in [false, true] {
            for field in [Field::Matrix, Field::SamplePeak, Field::TruePeak] {
                for weak in [false, true] {
                    let mut plugin = prepared(channels, spatial);
                    let values = input(channels, 1.0);
                    let mut output = vec![0.0; values.len()];
                    std::thread::spawn(move || {
                        process(&mut plugin, &values, &mut output, false);
                        let retained = Retained::new(&plugin, field, weak);
                        let original = retained.values();
                        for tap in [true, false, true] {
                            process(&mut plugin, &values, &mut output, tap);
                            assert_eq!(retained.values(), original);
                        }
                        assert_eq!(
                            snapshot(&plugin).correlation_samples_seen,
                            if spatial { 128 } else { 0 }
                        );
                    })
                    .join()
                    .unwrap();
                }
            }
        }
    }
}

#[test]
fn reset_publishes_all_fields_together_with_held_nested_readers() {
    for channels in [2, 7, 32, 40] {
        for spatial in [false, true] {
            for field in [Field::Matrix, Field::SamplePeak, Field::TruePeak] {
                for weak in [false, true] {
                    let mut plugin = prepared(channels, spatial);
                    let values = input(channels, 1.0);
                    let mut output = vec![0.0; values.len()];
                    std::thread::spawn(move || {
                        process(&mut plugin, &values, &mut output, false);
                        let retained = Retained::new(&plugin, field, weak);
                        let original = retained.values();
                        process(&mut plugin, &values, &mut output, true);
                        without_heap(|| plugin.reset());
                        assert_clear(&snapshot(&plugin), channels, spatial, true);
                        assert_eq!(retained.values(), original);
                        process(&mut plugin, &values, &mut output, false);
                        assert_eq!(
                            snapshot(&plugin).correlation_samples_seen,
                            if spatial { 32 } else { 0 }
                        );
                    })
                    .join()
                    .unwrap();
                }
            }
        }
    }
}

#[test]
fn all_retained_generations_skip_atomically_then_resume_cleared_or_current_epoch() {
    for channels in [2, 7, 32, 40] {
        for spatial in [false, true] {
            for reader in 0..4 {
                for reenable_before_release in [false, true] {
                    let mut plugin = prepared(channels, spatial);
                    let mut reference = prepared(channels, spatial);
                    let loud = input(channels, 1.0);
                    let quiet = input(channels, 0.125);
                    let mut output = vec![0.0; loud.len()];
                    let mut reference_output = output.clone();
                    std::thread::spawn(move || {
                        let mut retained = Vec::with_capacity(9);
                        for _ in 0..3 {
                            process(&mut plugin, &loud, &mut output, false);
                            match reader {
                                0 | 1 => {
                                    for field in [Field::Matrix, Field::SamplePeak, Field::TruePeak]
                                    {
                                        retained.push(Retained::new(&plugin, field, reader == 1));
                                    }
                                }
                                2 => retained.push(Retained::Outer(snapshot(&plugin))),
                                _ => retained
                                    .push(Retained::WeakOuter(Arc::downgrade(&snapshot(&plugin)))),
                            }
                        }
                        let before = snapshot(&plugin);
                        process(&mut plugin, &loud, &mut output, true);
                        assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        without_heap(|| plugin.reset());
                        assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        without_heap(|| plugin.reset());
                        assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        enabled(&mut plugin, false);
                        assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        process(&mut plugin, &loud, &mut output, false);
                        assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        if reenable_before_release {
                            enabled(&mut plugin, true);
                            process(&mut plugin, &quiet, &mut output, true);
                            assert!(Arc::ptr_eq(&snapshot(&plugin), &before));
                        }
                        // Release one old generation, leaving current and another old one held.
                        let count = if reader < 2 { 3 } else { 1 };
                        drop(retained.drain(..count));
                        process(&mut plugin, &quiet, &mut output, false);
                        assert!(!Arc::ptr_eq(&snapshot(&plugin), &before));
                        if reenable_before_release {
                            // The first post-reset update was skipped, so match the true-peak
                            // interval by feeding the twin both accepted blocks before querying.
                            let both: Vec<_> = quiet.iter().chain(&quiet).copied().collect();
                            let mut both_output = vec![0.0; both.len()];
                            process(&mut reference, &both, &mut both_output, false);
                            assert_same(&snapshot(&plugin), &snapshot(&reference));
                        } else {
                            assert_clear(&snapshot(&plugin), channels, spatial, false);
                            // Releasing the remaining readers permits the enable snapshot;
                            // with those readers held, deferral would remain legitimate.
                            drop(retained);
                            drop(before);
                            enabled(&mut plugin, true);
                            assert_clear(&snapshot(&plugin), channels, spatial, true);
                            process(&mut plugin, &quiet, &mut output, false);
                            process(&mut reference, &quiet, &mut reference_output, true);
                            assert_same(&snapshot(&plugin), &snapshot(&reference));
                        }
                    })
                    .join()
                    .unwrap();
                }
            }
        }
    }
}

#[test]
fn cold_reset_and_spatial_builders_prepare_every_callback_buffer() {
    for channels in [2, 7, 32, 40] {
        for after_initialize in [false, true] {
            let mut plugin = LoudnessMonitorPlugin::new(channels).unwrap();
            if after_initialize {
                plugin.initialize(48_000).unwrap();
            }
            plugin = plugin.with_spatial();
            if !after_initialize {
                plugin.initialize(48_000).unwrap();
            }
            let values = input(channels, 0.5);
            let mut output = vec![0.0; values.len()];
            std::thread::spawn(move || {
                for reset_first in [false, true] {
                    if reset_first {
                        without_heap(|| plugin.reset());
                    }
                    process(&mut plugin, &values, &mut output, reset_first);
                    assert_eq!(
                        snapshot(&plugin).correlation_matrix.len(),
                        channels * channels
                    );
                }
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn warm_range_survives_retained_generations_and_resumes_the_new_epoch() {
    for weak in [false, true] {
        let mut plugin = prepared(2, false);
        let values: Vec<_> = (0..48_000 * 3)
            .flat_map(|n| {
                let sample = 0.05 * (std::f32::consts::TAU * 997.0 * n as f32 / 48_000.0).sin();
                [sample, sample]
            })
            .collect();
        let mut output = vec![0.0; values.len()];
        std::thread::spawn(move || {
            let mut retained = Vec::with_capacity(3);
            for _ in 0..3 {
                process(&mut plugin, &values, &mut output, false);
                retained.push(Retained::new(&plugin, Field::SamplePeak, weak));
            }
            let held = snapshot(&plugin);
            assert_eq!(held.loudness_range.unwrap().observed_windows, 61);
            let held_momentary = held
                .maximum_momentary_lufs
                .expect("three seconds of accepted input warms Momentary maximum");
            let held_shortterm = held
                .maximum_shortterm_lufs
                .expect("three seconds of accepted input warms Short-term maximum");
            assert!(held_momentary.is_finite() && held_shortterm.is_finite());
            process(&mut plugin, &values, &mut output, true);
            assert!(Arc::ptr_eq(&snapshot(&plugin), &held));
            without_heap(|| plugin.reset());
            enabled(&mut plugin, false);
            enabled(&mut plugin, true);
            process(&mut plugin, &values, &mut output, false);
            assert!(Arc::ptr_eq(&snapshot(&plugin), &held));
            assert_eq!(held.loudness_range.unwrap().observed_windows, 61);
            drop(retained.remove(0));
            process(&mut plugin, &[], &mut [], true);
            let current = snapshot(&plugin);
            assert_eq!(current.loudness_range.unwrap().observed_windows, 1);
            assert_eq!(
                current.loudness_range.unwrap().status,
                sotf_host::LoudnessRangeStatus::Valid
            );
            assert_eq!(held.loudness_range.unwrap().observed_windows, 61);
            assert!(current.maximum_momentary_lufs.is_some_and(f64::is_finite));
            assert!(current.maximum_shortterm_lufs.is_some_and(f64::is_finite));
            assert!(
                (current.maximum_momentary_lufs.unwrap() - held_momentary).abs() <= 0.01,
                "a populated disable/enable lifecycle starts a fresh Momentary maximum"
            );
            assert!(
                (current.maximum_shortterm_lufs.unwrap() - held_shortterm).abs() <= 0.01,
                "a populated disable/enable lifecycle starts a fresh Short-term maximum"
            );
        })
        .join()
        .unwrap();
    }
}

#[test]
fn blocked_same_epoch_maxima_recover_without_heap_after_readers_release() {
    const CHANNELS: usize = 2;
    const QUANTUM_FRAMES: usize = 480;
    let low = sustained_tone(CHANNELS, 48_000 * 4, -30.0);
    let high = sustained_tone(CHANNELS, 48_000 * 4, -12.0);
    let mut plugin = prepared(CHANNELS, false);
    let mut output = vec![0.0; QUANTUM_FRAMES * CHANNELS];

    std::thread::spawn(move || {
        let mut retained = Vec::with_capacity(3);
        for block in low.chunks(QUANTUM_FRAMES * CHANNELS) {
            process(&mut plugin, block, &mut output, false);
        }

        let first = snapshot(&plugin);
        let low_momentary = first
            .maximum_momentary_lufs
            .expect("four seconds of accepted input warms Momentary maximum");
        let low_shortterm = first
            .maximum_shortterm_lufs
            .expect("four seconds of accepted input warms Short-term maximum");
        assert!(low_momentary.is_finite() && low_shortterm.is_finite());
        retained.push(first);

        // Retain all three prepared generations after the maxima are populated.
        for _ in 0..2 {
            process(
                &mut plugin,
                &low[..QUANTUM_FRAMES * CHANNELS],
                &mut output,
                false,
            );
            let next = snapshot(&plugin);
            assert!(!retained.iter().any(|old| Arc::ptr_eq(old, &next)));
            retained.push(next);
        }
        for left in 0..retained.len() {
            for right in left + 1..retained.len() {
                assert!(!Arc::ptr_eq(&retained[left], &retained[right]));
            }
        }

        // The louder programme advances the private monitor, but publication
        // must stay on the retained generation until storage becomes available.
        let published_before = retained.last().expect("three snapshots retained");
        for block in high.chunks(QUANTUM_FRAMES * CHANNELS) {
            process(&mut plugin, block, &mut output, false);
        }
        let blocked = snapshot(&plugin);
        assert!(Arc::ptr_eq(&blocked, published_before));
        assert_eq!(blocked.maximum_momentary_lufs, Some(low_momentary));
        assert_eq!(blocked.maximum_shortterm_lufs, Some(low_shortterm));
        drop(blocked);
        drop(retained);

        // Empty analyzer input publishes the already accumulated same-epoch
        // maxima. The process calls above ran under the allocator guard.
        process(&mut plugin, &[], &mut [], true);
        let recovered = snapshot(&plugin);
        let recovered_momentary = recovered.maximum_momentary_lufs.unwrap();
        let recovered_shortterm = recovered.maximum_shortterm_lufs.unwrap();
        assert!(recovered_momentary > low_momentary + 10.0);
        assert!(recovered_shortterm > low_shortterm + 10.0);
    })
    .join()
    .unwrap();
}

#[test]
fn disabled_spatial_and_integrated_preparation_preserve_control_state() {
    for channels in [2, 7, 32, 40] {
        for initial in [false, true] {
            let mut plugin = prepared(channels, initial);
            let values = input(channels, 0.5);
            let mut output = vec![0.0; values.len()];
            std::thread::spawn(move || {
                without_heap(|| plugin.reset());
                process(&mut plugin, &values, &mut output, false);
                let old = snapshot(&plugin);
                enabled(&mut plugin, false);
                // Structural preparation is deliberately outside heap measurement.
                for spatial in [!initial, initial] {
                    plugin.set_spatial_enabled(spatial);
                    assert_clear(&snapshot(&plugin), channels, spatial, false);
                    process(&mut plugin, &values, &mut output, false);
                    assert_clear(&snapshot(&plugin), channels, spatial, false);
                }
                plugin = plugin
                    .with_integrated_mode(IntegratedLoudnessMode::Rolling)
                    .unwrap();
                assert_clear(&snapshot(&plugin), channels, initial, false);
                process(&mut plugin, &values, &mut output, true);
                assert_clear(&snapshot(&plugin), channels, initial, false);
                assert!(old.peak > 0.0);
                enabled(&mut plugin, true);
                process(&mut plugin, &values, &mut output, false);
                let current = snapshot(&plugin);
                // A same-value setter must preserve the published measurement and history.
                enabled(&mut plugin, true);
                assert!(Arc::ptr_eq(&current, &snapshot(&plugin)));
                process(&mut plugin, &values, &mut output, true);
                assert_eq!(
                    snapshot(&plugin).correlation_samples_seen,
                    if initial { 64 } else { 0 }
                );
            })
            .join()
            .unwrap();
        }
    }
}
