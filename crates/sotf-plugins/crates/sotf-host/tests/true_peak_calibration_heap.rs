//! Cold true-peak processing, interval queries and resets reuse prepared storage.
// Rust guideline compliant 2026-02-21
use math_audio_dsp::ebur128::{EbuR128, Mode};
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::{LoudnessData, LoudnessMonitorPlugin, Plugin, ProcessContext};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CallbackAllocator;

// SAFETY: Allocation and deallocation contracts are forwarded unchanged to System.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations + 1, frees));
            });
        }
        // SAFETY: System receives the original valid allocation layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            COUNTS.with(|counts| {
                let (allocations, frees) = counts.get();
                counts.set((allocations, frees + 1));
            });
        }
        // SAFETY: The pointer and its original allocation layout are unchanged.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

struct StopCounting;

impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNTING.with(|active| active.set(false));
    }
}

fn measured(operation: impl FnOnce()) -> (usize, usize) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    operation();
    drop(guard);
    COUNTS.with(Cell::get)
}

#[test]
fn public_host_true_peak_is_prepared_at_every_supported_rate() {
    for channels in [1, 2, 6, 24] {
        for rate in [
            8_000, 16_000, 44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 384_000, 2_822_400,
        ] {
            let mut monitor = LoudnessMonitor::new(channels, rate).unwrap();
            let mut snapshot = LoudnessData::new(channels as usize);
            let frames = if rate <= 16_000 { 64 } else { 8193 };
            let audio: Vec<f32> = (0..frames * channels as usize)
                .map(|i| ((i * 17 % 251) as f32 - 125.0) / 256.0)
                .collect();
            let (monitor, snapshot, audio, counts) = std::thread::spawn(move || {
                let mut counts = [(0, 0); 2];
                for count in &mut counts {
                    *count = measured(|| {
                        monitor.add_frames(&audio).unwrap();
                        monitor.finish_true_peak();
                        monitor.finish_true_peak();
                        monitor.update_loudness_data(&mut snapshot);
                        assert!(snapshot.true_peaks_dbtp.iter().all(|peak| peak.is_finite()));
                        let programme_maximum = snapshot
                            .maximum_true_peak_dbtp
                            .expect("non-silent supported input has a finite maximum");
                        assert!(programme_maximum.is_finite());
                        monitor.update_loudness_data(&mut snapshot);
                        assert_eq!(snapshot.maximum_true_peak_dbtp, Some(programme_maximum));
                        assert!(
                            snapshot
                                .true_peaks_dbtp
                                .iter()
                                .all(|&peak| peak == f64::NEG_INFINITY)
                        );
                        monitor.finish_true_peak();
                        monitor.update_loudness_data(&mut snapshot);
                        assert_eq!(snapshot.maximum_true_peak_dbtp, Some(programme_maximum));
                        assert!(
                            snapshot
                                .true_peaks_dbtp
                                .iter()
                                .all(|&p| p == f64::NEG_INFINITY)
                        );
                        monitor.reset().unwrap();
                        monitor.update_loudness_data(&mut snapshot);
                        assert_eq!(snapshot.maximum_true_peak_dbtp, None);
                    });
                }
                (monitor, snapshot, audio, counts)
            })
            .join()
            .unwrap();
            assert_eq!(counts, [(0, 0); 2], "channels={channels}, rate={rate}");
            drop((monitor, snapshot, audio));
        }
    }
}

#[test]
fn public_backend_true_peak_is_prepared_from_first_sample() {
    for channels in [1, 2, 6, 24] {
        let mut meter = EbuR128::new(channels, 48_000, Mode::TRUE_PEAK).unwrap();
        let audio = vec![0.25; 8193 * channels as usize];
        let (meter, audio, counts) = std::thread::spawn(move || {
            let mut counts = [(0, 0); 2];
            for count in &mut counts {
                *count = measured(|| {
                    meter.add_frames_f32(&audio).unwrap();
                    meter.finish_true_peak();
                    meter.finish_true_peak();
                    for ch in 0..channels {
                        assert!(meter.prev_true_peak(ch).unwrap() > 0.0);
                        assert_eq!(meter.prev_true_peak(ch).unwrap(), 0.0);
                    }
                    meter.finish_true_peak();
                    for ch in 0..channels {
                        assert_eq!(meter.prev_true_peak(ch).unwrap(), 0.0);
                    }
                    meter.reset();
                });
            }
            (meter, audio, counts)
        })
        .join()
        .unwrap();
        assert_eq!(counts, [(0, 0); 2], "channels={channels}");
        drop((meter, audio));
    }
}

#[test]
fn plugin_first_drain_retries_and_reset_do_not_allocate_or_free() {
    for channels in [1, 2, 6, 24] {
        for rate in [44_100, 48_000, 88_200, 96_000] {
            for spatial in [false, true] {
                // No readers, outer readers, or nested Weak readers.
                for retained in 0..3 {
                    let mut plugin = LoudnessMonitorPlugin::new(channels).unwrap();
                    plugin.set_spatial_enabled(spatial);
                    plugin.initialize(rate).unwrap();
                    let mut audio = vec![0.0; 64 * channels];
                    audio[63 * channels..].fill(1.0);
                    let mut output = vec![0.0; audio.len()];
                    let context = ProcessContext::new(rate, 64);
                    let ending = ProcessContext::new(rate, 0);
                    let mut readers = Vec::new();
                    let mut weak_readers = Vec::new();
                    for _ in 0..3 {
                        plugin.process(&audio, &mut output, &context).unwrap();
                        if retained == 1 {
                            readers.push(plugin.get_data().unwrap());
                        } else if retained == 2 {
                            let data = plugin
                                .get_data()
                                .unwrap()
                                .downcast::<LoudnessData>()
                                .unwrap();
                            weak_readers.push(std::sync::Arc::downgrade(&data.true_peaks_dbtp));
                        }
                    }
                    let (plugin, audio, output, counts) = std::thread::spawn(move || {
                        let first = measured(|| {
                            let drain = plugin.drain(&mut [], &ending).unwrap();
                            assert!(drain.complete);
                            assert_eq!(drain.frames, 0);
                            plugin.drain(&mut [], &ending).unwrap();
                        });
                        drop((readers, weak_readers));
                        let retry_and_reset = measured(|| {
                            plugin.drain(&mut [], &ending).unwrap();
                            plugin.reset();
                            plugin.process(&audio, &mut output, &context).unwrap();
                            plugin.drain(&mut [], &ending).unwrap();
                            plugin.drain(&mut [], &ending).unwrap();
                        });
                        (plugin, audio, output, [first, retry_and_reset])
                    })
                    .join()
                    .unwrap();
                    assert_eq!(
                        counts,
                        [(0, 0); 2],
                        "channels={channels}, rate={rate}, spatial={spatial}, retained={retained}"
                    );
                    drop((plugin, audio, output));
                }
            }
        }
    }
}
