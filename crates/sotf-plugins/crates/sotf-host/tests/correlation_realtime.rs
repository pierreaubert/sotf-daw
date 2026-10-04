//! Correlation callbacks preserve snapshots without allocating or freeing heap storage.
// Rust guideline compliant 2026-02-21
use sotf_host::{
    ChannelCorrelationMonitor, ChannelCorrelationPlugin, CorrelationData, Plugin, ProcessContext,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::{Arc, Barrier, Weak};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
// SAFETY: Allocation requests, live pointers, and layouts are forwarded unchanged to System.
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
        // SAFETY: The valid allocation request is unchanged.
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
        // SAFETY: The pointer and its original allocation layout are unchanged.
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
fn input(channels: usize, anti_phase: bool) -> Vec<f32> {
    (0..32 * channels)
        .map(|index| {
            let frame = index / channels;
            let channel = index % channels;
            let signal = ((frame * 7 % 19) as f32 - 9.0) / 16.0;
            signal
                * if anti_phase && channel % 2 == 1 {
                    -1.0
                } else {
                    1.0
                }
        })
        .collect()
}
fn prepared(channels: usize) -> ChannelCorrelationPlugin {
    let mut plugin = ChannelCorrelationPlugin::new(channels).unwrap();
    plugin.initialize(48_000.0).unwrap();
    plugin
}
fn process(plugin: &mut ChannelCorrelationPlugin, values: &[f32], output: &mut [f32]) {
    let frames = values.len() / plugin.input_channels();
    let result =
        without_heap(|| plugin.process(values, output, &ProcessContext::new(48_000, frames)));
    assert_eq!(result.unwrap(), frames);
    assert_eq!(output, values);
}
fn assert_identity(data: &CorrelationData) {
    assert_eq!(data.samples_seen, 0);
    for row in 0..data.channels {
        for column in 0..data.channels {
            assert_eq!(
                data.matrix[row * data.channels + column],
                if row == column { 1.0 } else { 0.0 }
            );
        }
    }
}
fn assert_signs(data: &CorrelationData, anti_phase: bool) {
    for row in 0..data.channels {
        for column in 0..data.channels {
            let expected = if anti_phase && (row + column) % 2 == 1 {
                -1.0
            } else {
                1.0
            };
            assert!((data.matrix[row * data.channels + column] - expected).abs() < 1.0e-6);
        }
    }
}

#[test]
fn first_split_completion_at_every_offset_and_reset_have_no_heap_activity() {
    for channels in [2, 7, 32, 40] {
        for split in 1..channels {
            let mut monitor = ChannelCorrelationMonitor::new(channels, 48_000);
            let values = input(channels, true);
            let mut expected = CorrelationData::new(channels);
            let mut reference = ChannelCorrelationMonitor::new(channels, 48_000);
            reference.add_frames(&values);
            reference.update_correlation_data(&mut expected);
            let mut actual = CorrelationData::new(channels);
            std::thread::spawn(move || {
                without_heap(|| monitor.add_frames(&values[..split]));
                without_heap(|| monitor.add_frames(&values[split..]));
                without_heap(|| monitor.update_correlation_data(&mut actual));
                assert_eq!(*actual.matrix, *expected.matrix);
                assert_eq!(actual.samples_seen, 32);
                without_heap(|| monitor.reset());
                // Reset also clears an incomplete frame without losing prepared capacity.
                without_heap(|| monitor.add_frames(&values[..split]));
                without_heap(|| monitor.reset());
                for fragment in values.chunks(1) {
                    without_heap(|| monitor.add_frames(fragment));
                }
                without_heap(|| monitor.update_correlation_data(&mut actual));
                assert_eq!(*actual.matrix, *expected.matrix);
                assert_eq!(actual.samples_seen, 32);
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn cold_first_process_cold_reset_and_reuse_have_no_heap_activity() {
    for channels in [2, 7, 32, 40] {
        for reset_first in [false, true] {
            let mut plugin = prepared(channels);
            let values = input(channels, true);
            let mut output = vec![0.0; values.len()];
            std::thread::spawn(move || {
                if reset_first {
                    without_heap(|| plugin.reset());
                    assert_identity(&plugin.cache().load());
                }
                for _ in 0..3 {
                    process(&mut plugin, &values, &mut output);
                    let data = without_heap(|| plugin.cache().load());
                    assert_eq!(data.samples_seen, 32);
                    assert_signs(&data, true);
                    drop(data);
                    without_heap(|| plugin.reset());
                    assert_identity(&plugin.cache().load());
                }
            })
            .join()
            .unwrap();
        }
    }
}

#[derive(Clone, Copy)]
enum Reader {
    Outer,
    Inner,
    WeakInner,
    WeakOuter,
    Both,
}
enum Retained {
    Outer(Arc<CorrelationData>),
    Inner(Arc<Vec<f32>>),
    WeakInner(Weak<Vec<f32>>),
    WeakOuter(Weak<CorrelationData>),
    Both(Arc<CorrelationData>, Arc<Vec<f32>>),
}
impl Retained {
    fn new(plugin: &ChannelCorrelationPlugin, reader: Reader) -> Self {
        let data = plugin.cache().load();
        match reader {
            Reader::Outer => Self::Outer(data),
            Reader::Inner => Self::Inner(Arc::clone(&data.matrix)),
            Reader::WeakInner => Self::WeakInner(Arc::downgrade(&data.matrix)),
            Reader::WeakOuter => Self::WeakOuter(Arc::downgrade(&data)),
            Reader::Both => Self::Both(Arc::clone(&data), Arc::clone(&data.matrix)),
        }
    }
    fn assert_initial(&self) {
        let matrix = match self {
            Self::Outer(data) => {
                assert_identity(data);
                Arc::clone(&data.matrix)
            }
            Self::Inner(matrix) => Arc::clone(matrix),
            Self::WeakInner(weak) => weak.upgrade().expect("producer retains skipped matrix"),
            Self::WeakOuter(weak) => {
                let data = weak.upgrade().expect("producer retains skipped payload");
                assert_identity(&data);
                Arc::clone(&data.matrix)
            }
            Self::Both(data, matrix) => {
                assert_identity(data);
                assert!(Arc::ptr_eq(&data.matrix, matrix));
                Arc::clone(matrix)
            }
        };
        let channels = (matrix.len() as f64).sqrt() as usize;
        for row in 0..channels {
            for column in 0..channels {
                assert_eq!(
                    matrix[row * channels + column],
                    if row == column { 1.0 } else { 0.0 }
                );
            }
        }
    }
}

#[test]
fn retained_outer_inner_and_weak_readers_use_fallback_without_heap_activity() {
    for channels in [2, 7, 32, 40] {
        for reader in [
            Reader::Outer,
            Reader::Inner,
            Reader::WeakInner,
            Reader::WeakOuter,
            Reader::Both,
        ] {
            let mut plugin = prepared(channels);
            let retained = Retained::new(&plugin, reader);
            let values = input(channels, true);
            let mut output = vec![0.0; values.len()];
            std::thread::spawn(move || {
                for block in 1..=8 {
                    process(&mut plugin, &values, &mut output);
                    assert_eq!(plugin.cache().load().samples_seen, 32 * block);
                    retained.assert_initial();
                }
                without_heap(|| plugin.reset());
                assert_identity(&plugin.cache().load());
                retained.assert_initial();
                process(&mut plugin, &values, &mut output);
                assert_eq!(plugin.cache().load().samples_seen, 32);
                assert_signs(&plugin.cache().load(), true);
                retained.assert_initial();
            })
            .join()
            .unwrap();
        }
    }
}

#[test]
fn all_retained_generations_skip_atomically_then_reset_and_resume_with_current_history() {
    for channels in [2, 7, 32, 40] {
        let mut plugin = prepared(channels);
        let negative = input(channels, true);
        let positive = input(channels, false);
        let mut output = vec![0.0; positive.len()];
        std::thread::spawn(move || {
            let old_outer = plugin.cache().load();
            process(&mut plugin, &negative, &mut output);
            let old_inner = Arc::clone(&plugin.cache().load().matrix);
            process(&mut plugin, &negative, &mut output);
            let current = plugin.cache().load();
            let snapshot = current.matrix.as_ref().clone();
            assert_eq!(current.samples_seen, 64);
            process(&mut plugin, &negative, &mut output);
            assert!(Arc::ptr_eq(&current, &plugin.cache().load()));
            assert_eq!(*current.matrix, snapshot);
            assert_eq!(plugin.take_cache_contention_stats(), (1, 3));
            without_heap(|| plugin.reset());
            assert!(Arc::ptr_eq(&current, &plugin.cache().load()));
            process(&mut plugin, &positive, &mut output);
            assert!(Arc::ptr_eq(&current, &plugin.cache().load()));
            assert_eq!(plugin.take_cache_contention_stats(), (2, 2));
            // Release a nested reader on the control side. The next publication
            // must contain both post-reset blocks, with no pre-reset covariance.
            drop(old_inner);
            process(&mut plugin, &positive, &mut output);
            let resumed = plugin.cache().load();
            assert!(!Arc::ptr_eq(&current, &resumed));
            assert_eq!(resumed.samples_seen, 64);
            assert_signs(&resumed, false);
            assert_identity(&old_outer);
            assert_eq!(*current.matrix, snapshot);
            assert_eq!(plugin.take_cache_contention_stats(), (0, 1));
        })
        .join()
        .unwrap();
    }
}

#[test]
fn authoritative_nested_access_rejects_a_deterministic_weak_upgrade_interleaving() {
    let mut candidate = CorrelationData::new(2);
    let observer = Arc::downgrade(&candidate.matrix);
    let barrier = Arc::new(Barrier::new(2));
    let reader_barrier = Arc::clone(&barrier);
    let reader = std::thread::spawn(move || {
        reader_barrier.wait();
        let retained = observer.upgrade().unwrap();
        drop(observer);
        reader_barrier.wait();
        reader_barrier.wait();
        drop(retained);
    });
    // This is a deliberately split observation, not a readiness implementation.
    // The reader upgrades its Weak and drops it between the two counter reads.
    let earlier_strong = Arc::strong_count(&candidate.matrix);
    barrier.wait();
    barrier.wait();
    let later_weak = Arc::weak_count(&candidate.matrix);
    assert_eq!((earlier_strong, later_weak), (1, 0));
    assert!(!without_heap(
        || Arc::get_mut(&mut candidate.matrix).is_some()
    ));
    barrier.wait();
    reader.join().unwrap();
    assert!(without_heap(
        || Arc::get_mut(&mut candidate.matrix).is_some()
    ));
}
