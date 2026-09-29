//! Callback allocation and ownership regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::plugin::{Plugin, ProcessContext};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: Pointer and layout handling is delegated unchanged to CountingAlloc.
// Instrumentation touches only constant-initialized, nonallocating TLS cells.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the caller's valid allocation layout unchanged.
        unsafe { sotf_host::CountingAlloc.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Forward the original allocation pointer and layout unchanged.
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

pub(super) fn callback_counts(f: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.set(0);
    DEALLOCATIONS.set(0);
    TRACKING.set(true);
    f();
    TRACKING.set(false);
    (ALLOCATIONS.get(), DEALLOCATIONS.get())
}

#[test]
fn first_callback_on_fresh_thread_has_no_heap_activity() {
    let mut plugin = crate::XtcPlugin::new(crate::XtcPluginParams::default(), 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    let input = vec![0.1; 4096 * 2];
    let output = vec![0.0; 4096 * 2];
    // Construction stays on this live thread. No callback or audio-thread
    // publication access is warmed before the measured first call.
    let (plugin, counts) = std::thread::spawn(move || {
        let mut output = output;
        let counts = callback_counts(|| {
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 4096))
                .unwrap();
        });
        (plugin, counts)
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "first callback allocations/deallocations");
}

fn update(
    plugin: &crate::XtcPlugin,
    generation: u64,
) -> std::sync::Arc<crate::types::PendingFilterUpdate> {
    std::sync::Arc::new(crate::types::PendingFilterUpdate {
        generation,
        filters: std::sync::Arc::new(crate::filters::compute_xtc_filters_full(
            &crate::XtcPluginParams::default(),
            48_000,
            plugin.fft.fft_size / 2 + 1,
        )),
        hrtf_transfer_functions: None,
        room_reflection_cache: None,
        room_params_hash: generation,
    })
}

fn publish(plugin: &crate::XtcPlugin, generation: u64) {
    let update = update(plugin, generation);
    plugin
        .filter_state
        .filter_update_generation
        .store(generation, std::sync::atomic::Ordering::Release);
    plugin.filter_state.exchange.lock().unwrap().pending = Some(update);
}

fn process_block(mut plugin: crate::XtcPlugin) -> crate::XtcPlugin {
    let input = vec![0.1; 4096 * 2];
    let output = vec![0.0; 4096 * 2];
    let (plugin, counts) = std::thread::spawn(move || {
        let mut output = output;
        let counts = callback_counts(|| {
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 4096))
                .unwrap();
        });
        assert!(output.iter().all(|sample| sample.is_finite()));
        (plugin, counts)
    })
    .join()
    .unwrap();
    assert_eq!(
        counts,
        (0, 0),
        "publication callback allocations/deallocations"
    );
    plugin
}

fn reclaim(plugin: &crate::XtcPlugin) {
    let retired = {
        let mut exchange = plugin.filter_state.exchange.lock().unwrap();
        (
            std::mem::take(&mut exchange.retired_updates),
            std::mem::take(&mut exchange.retired_snapshots),
        )
    };
    // This function runs only on this control thread, never inside counting.
    drop(retired);
}

#[test]
fn publication_contention_keeps_current_filters_and_retries() {
    let mut plugin = crate::XtcPlugin::new(crate::XtcPluginParams::default(), 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    publish(&plugin, 1);
    let exchange = std::sync::Arc::clone(&plugin.filter_state.exchange);
    let guard = exchange.lock().unwrap();
    plugin = process_block(plugin);
    assert_eq!(
        plugin
            .filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        1
    );
    drop(guard);
    plugin = process_block(plugin);
    assert_eq!(
        plugin
            .filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn interrupted_fades_and_full_retirement_slots_preserve_every_owner() {
    use std::sync::Arc;
    let params = crate::XtcPluginParams {
        room_reflections_enabled: true,
        ..Default::default()
    };
    let mut plugin = crate::XtcPlugin::new(params, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    // Force long fades so each subsequent adoption interrupts the previous.
    plugin.filter_state.progress_per_hop = 0.001;
    let initial = Arc::downgrade(plugin.filter_state.active_filter_update.as_ref().unwrap());
    publish(&plugin, 1);
    plugin = process_block(plugin);
    let first = Arc::downgrade(plugin.filter_state.active_filter_update.as_ref().unwrap());
    publish(&plugin, 2);
    plugin = process_block(plugin);
    let second = Arc::downgrade(plugin.filter_state.active_filter_update.as_ref().unwrap());
    // Both update retirement slots are full. Taking this pending update would
    // require losing an owner; it must remain pending until worker reclamation.
    publish(&plugin, 3);
    plugin = process_block(plugin);
    assert_eq!(
        plugin
            .filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert_eq!(
        plugin
            .filter_state
            .exchange
            .lock()
            .unwrap()
            .pending
            .as_ref()
            .unwrap()
            .generation,
        3
    );
    assert!(initial.upgrade().is_some());
    assert!(first.upgrade().is_some());
    reclaim(&plugin);
    assert!(initial.upgrade().is_none());
    assert!(first.upgrade().is_none());
    plugin = process_block(plugin);
    assert_eq!(
        plugin
            .filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        3
    );
    assert!(second.upgrade().is_some());
    reclaim(&plugin);
    assert!(second.upgrade().is_none());

    let previous = Arc::downgrade(plugin.filter_state.prev_filters.as_ref().unwrap());
    let (plugin, counts) = std::thread::spawn(move || {
        let counts = callback_counts(|| plugin.reset());
        (plugin, counts)
    })
    .join()
    .unwrap();
    assert_eq!(counts, (0, 0));
    assert!(previous.upgrade().is_some());
    reclaim(&plugin);
    assert!(
        previous.upgrade().is_none(),
        "reset fade must be destroyed on control thread"
    );
}

#[test]
fn completion_and_stale_publications_remain_allocation_free() {
    let mut plugin = crate::XtcPlugin::new(crate::XtcPluginParams::default(), 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    plugin.filter_state.progress_per_hop = 1.0;
    for generation in 1..17 {
        publish(&plugin, generation);
        plugin = process_block(plugin);
        assert_eq!(plugin.filter_state.crossfade_progress, 1.0);
        reclaim(&plugin);
        assert!(plugin.filter_state.prev_filters.is_none());
    }
    publish(&plugin, 17);
    plugin
        .filter_state
        .filter_update_generation
        .store(18, std::sync::atomic::Ordering::Release);
    plugin = process_block(plugin);
    assert_eq!(
        plugin
            .filter_state
            .active_filter_update
            .as_ref()
            .unwrap()
            .generation,
        16
    );
    assert!(
        plugin
            .filter_state
            .exchange
            .lock()
            .unwrap()
            .pending
            .is_none()
    );
    reclaim(&plugin);
}
