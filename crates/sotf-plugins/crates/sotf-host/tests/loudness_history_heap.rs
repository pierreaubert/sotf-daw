//! Integrated history remains prepared through growth boundaries and rolling wrap.
// Rust guideline compliant 2026-02-21
use math_audio_dsp::ebur128::{EbuR128, Mode};
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::{
    LoudnessData, LoudnessMonitorPlugin, LoudnessRangeConfig, LoudnessRangeMode,
    LoudnessRangeStatus, Plugin, ProcessContext,
};
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

// A 400 ms momentary window delays the first retained energy until sub-block 4.
// These checkpoints cross every old growth boundary and the production wrap.
const CHECKPOINTS: [usize; 5] = [6_003, 12_003, 24_003, 36_003, 36_013];

#[test]
fn dependency_history_never_grows_on_the_callback_thread() {
    for (rate, channels, integrated) in [(20, 1, true), (8_000, 2, true), (20, 6, false)] {
        let mut mode = Mode::M | Mode::S | Mode::SAMPLE_PEAK;
        if integrated {
            mode = mode | Mode::I;
        }
        let mut meter = EbuR128::new(channels, rate, mode).unwrap();
        let silence = vec![0.0; rate as usize / 10 * channels as usize];
        let (meter, counts) = std::thread::spawn(move || {
            let mut counts = [[(0, 0); CHECKPOINTS.len()]; 2];
            for epoch in &mut counts {
                let mut previous = 0;
                for (index, end) in CHECKPOINTS.into_iter().enumerate() {
                    epoch[index] = measured(|| {
                        for _ in previous..end {
                            meter.add_frames_f32(&silence).unwrap();
                        }
                        assert_eq!(meter.loudness_momentary().unwrap(), f64::NEG_INFINITY);
                        assert_eq!(meter.loudness_shortterm().unwrap(), f64::NEG_INFINITY);
                        assert_eq!(meter.loudness_global().unwrap(), f64::NEG_INFINITY);
                    });
                    previous = end;
                }
                assert_eq!(measured(|| meter.reset()), (0, 0));
            }
            (meter, counts)
        })
        .join()
        .unwrap();
        assert_eq!(
            counts,
            [[(0, 0); CHECKPOINTS.len()]; 2],
            "rate={rate}, channels={channels}, integrated={integrated}: allocations/frees"
        );
        drop(meter);
    }
}

#[test]
fn public_monitor_retains_history_storage_after_an_hour_and_reset() {
    // 20 Hz is accepted and has exact 100 ms blocks. Silence isolates storage
    // behavior; this deliberately makes no frequency-response accuracy claim.
    for channels in [2, 6] {
        let mut monitor = LoudnessMonitor::new(channels, 20).unwrap();
        let mut snapshot = sotf_host::analyzer::LoudnessData::new(channels as usize);
        let silence = vec![0.0; 2 * channels as usize];
        let (monitor, snapshot, counts) = std::thread::spawn(move || {
            let mut counts = [[(0, 0); CHECKPOINTS.len()]; 2];
            for epoch in &mut counts {
                let mut previous = 0;
                for (index, end) in CHECKPOINTS.into_iter().enumerate() {
                    epoch[index] = measured(|| {
                        for _ in previous..end {
                            monitor.add_frames(&silence).unwrap();
                        }
                        monitor.update_loudness_data(&mut snapshot);
                    });
                    assert_eq!(snapshot.integrated_lufs, f64::NEG_INFINITY);
                    assert_eq!(snapshot.peak, 0.0);
                    assert!(snapshot.integrated_valid);
                    previous = end;
                }
                assert_eq!(measured(|| monitor.reset().unwrap()), (0, 0));
            }
            (monitor, snapshot, counts)
        })
        .join()
        .unwrap();
        assert_eq!(
            counts,
            [[(0, 0); CHECKPOINTS.len()]; 2],
            "channels={channels}: allocations/frees"
        );
        drop((monitor, snapshot));
    }
}

#[test]
fn loudness_range_cold_full_dirty_queries_and_reset_reuse_prepared_storage() {
    for mode in [LoudnessRangeMode::Rolling, LoudnessRangeMode::WholeProgram] {
        for capacity in [2, 36_000] {
            // Use a normal audio rate for nonzero signals and full percentile queries.
            // A large offline callback crosses the full history only once.
            let mut plugin = LoudnessMonitorPlugin::new(2)
                .unwrap()
                .with_loudness_range(Some(LoudnessRangeConfig {
                    mode,
                    capacity_windows: capacity,
                }))
                .unwrap();
            plugin.initialize(8_000).unwrap();
            let mut input = vec![0.0; (capacity + 35) * 1_600];
            for (index, sample) in input.iter_mut().enumerate() {
                *sample = if index % 4 < 2 { 0.001 } else { -0.001 };
            }
            let mut output = vec![0.0; input.len()];
            let (plugin, input, output) = std::thread::spawn(move || {
                // First dirty publication after exactly C completed windows.
                let used = (capacity + 29) * 1_600;
                assert_eq!(
                    measured(|| {
                        plugin
                            .process(
                                &input[..used],
                                &mut output[..used],
                                &ProcessContext::new(8_000, used / 2),
                            )
                            .unwrap();
                    }),
                    (0, 0)
                );
                let held = plugin
                    .get_data()
                    .unwrap()
                    .downcast::<LoudnessData>()
                    .unwrap();
                assert_eq!(held.loudness_range.unwrap().retained_windows, capacity);
                assert_eq!(
                    held.loudness_range.unwrap().status,
                    LoudnessRangeStatus::Valid
                );
                // A retained nested owner must not cause scalar-only publication.
                let nested = held.channel_peaks.clone();
                let prior = held.loudness_range;
                assert_eq!(
                    measured(|| {
                        for frames in [0, 1, 1_600] {
                            plugin
                                .process(
                                    &input[..frames * 2],
                                    &mut output[..frames * 2],
                                    &ProcessContext::new(8_000, frames),
                                )
                                .unwrap();
                        }
                        plugin.reset();
                    }),
                    (0, 0)
                );
                assert_eq!(held.loudness_range, prior);
                drop((held, nested));
                assert_eq!(
                    measured(|| {
                        plugin
                            .process(
                                &input[..used],
                                &mut output[..used],
                                &ProcessContext::new(8_000, used / 2),
                            )
                            .unwrap();
                    }),
                    (0, 0)
                );
                assert_eq!(
                    plugin
                        .get_data()
                        .unwrap()
                        .downcast::<LoudnessData>()
                        .unwrap()
                        .loudness_range
                        .unwrap()
                        .status,
                    LoudnessRangeStatus::Valid
                );
                (plugin, input, output)
            })
            .join()
            .unwrap();
            drop((plugin, input, output));
        }
    }
}
