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

fn callback_counts(f: impl FnOnce()) -> (usize, usize) {
    ALLOCATIONS.set(0);
    DEALLOCATIONS.set(0);
    TRACKING.set(true);
    f();
    TRACKING.set(false);
    (ALLOCATIONS.get(), DEALLOCATIONS.get())
}

#[test]
fn first_drain_and_reset_drain_have_no_heap_activity() {
    let mut plugin = small_plugin(0);
    plugin
        .process(&[0.4, -0.1], &mut [0.0; 2], &ProcessContext::new(48_000, 1))
        .unwrap();
    let (plugin, counts) = std::thread::spawn(move || {
        let mut output = [0.0; 512];
        let counts = callback_counts(|| {
            for reset in [false, true] {
                if reset {
                    plugin.reset();
                    plugin
                        .process(&[0.4, -0.1], &mut [0.0; 2], &ProcessContext::new(48_000, 1))
                        .unwrap();
                }
                loop {
                    assert!(plugin.drain_call_bound().is_some());
                    let result = plugin
                        .drain(&mut output, &ProcessContext::new(48_000, 256))
                        .unwrap();
                    if result.complete {
                        break;
                    }
                    assert!(result.frames > 0);
                }
            }
        });
        (plugin, counts)
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "cold/reset drain allocations/deallocations");
}

#[test]
fn first_callback_on_fresh_thread_has_no_heap_activity() {
    let params = serde_json::from_value(serde_json::json!({"input_channels": 2})).unwrap();
    let mut plugin = crate::BinauralDecoderPlugin::try_from_params(params).unwrap();
    plugin.initialize(48_000.0).unwrap();
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

fn process_hop(mut plugin: crate::BinauralDecoderPlugin) -> crate::BinauralDecoderPlugin {
    let frames = plugin.config.hop_size;
    let input = vec![0.1; frames * 2];
    let output = vec![0.0; frames * 2];
    let (plugin, counts) = std::thread::spawn(move || {
        let mut output = output;
        let counts = callback_counts(|| {
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, frames))
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

fn make_state(gain: f32) -> std::sync::Arc<crate::types::BinauralState> {
    std::sync::Arc::new(crate::types::BinauralState {
        hrtf_filters_freq: vec![vec![rustfft::num_complex::Complex::new(gain, 0.0); 66]; 2],
        diffuse_field_eq_filter: None,
        _hrtf_data: None,
    })
}

fn small_plugin(mode: usize) -> crate::BinauralDecoderPlugin {
    let params = serde_json::from_value(serde_json::json!({
        "input_channels": 2, "fft_size": 64, "crossfade_mode": mode,
        "crossfade_ms": 10.0
    }))
    .unwrap();
    let mut plugin = crate::BinauralDecoderPlugin::try_from_params(params).unwrap();
    plugin.initialize(48_000.0).unwrap();
    // One-hop tests shorten the already prepared transition without changing
    // transform storage. Both linear and spectral modes are exercised.
    plugin.config.crossfade_ms = plugin.config.hop_size as f32 / 48.0;
    plugin
}

#[test]
fn state_contention_keeps_audio_snapshot_and_retries() {
    use std::sync::Arc;
    let mut plugin = small_plugin(0);
    let original = Arc::downgrade(&plugin.crossfade.current_state_snapshot);
    plugin.state.store(make_state(0.3));
    let shared = Arc::clone(&plugin.state);
    let guard = shared.try_lock().unwrap();
    plugin = process_hop(plugin);
    assert!(Arc::ptr_eq(
        &plugin.crossfade.current_state_snapshot,
        &original.upgrade().unwrap()
    ));
    drop(guard);
    plugin = process_hop(plugin);
    assert!(!Arc::ptr_eq(
        &plugin.crossfade.current_state_snapshot,
        &original.upgrade().unwrap_or_else(|| make_state(0.0))
    ));
}

#[test]
fn interrupted_fade_waits_for_retirement_capacity_without_dropping_owners() {
    use std::sync::Arc;
    let mut plugin = small_plugin(0);
    plugin.config.crossfade_ms = 100.0;
    // Replace the background receiver with a deliberately stalled one, so
    // backpressure and every transfer are deterministic.
    plugin.retirement.tx.take();
    plugin.retirement.thread.take().unwrap().join().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    tx.try_send(make_state(0.0)).unwrap();
    plugin.retirement.tx = Some(tx);
    let first = Arc::downgrade(&plugin.crossfade.current_state_snapshot);
    plugin.state.store(make_state(0.3));
    plugin = process_hop(plugin);
    let second = Arc::downgrade(&plugin.crossfade.current_state_snapshot);
    plugin.state.store(make_state(0.6));
    plugin = process_hop(plugin);
    assert!(Arc::ptr_eq(
        &plugin.crossfade.current_state_snapshot,
        &second.upgrade().unwrap()
    ));
    assert!(Arc::ptr_eq(
        plugin.crossfade.crossfade_prev_state.as_ref().unwrap(),
        &first.upgrade().unwrap()
    ));
    drop(rx.try_recv().unwrap());
    plugin = process_hop(plugin);
    assert!(Arc::ptr_eq(
        plugin.crossfade.crossfade_prev_state.as_ref().unwrap(),
        &second.upgrade().unwrap()
    ));
    drop(rx.try_recv().unwrap());
    assert!(
        first.upgrade().is_none(),
        "old filter must be reclaimed off callback"
    );

    let (mut plugin, counts) = std::thread::spawn(move || {
        let counts = callback_counts(|| plugin.reset());
        (plugin, counts)
    })
    .join()
    .unwrap();
    assert_eq!(counts, (0, 0), "reset must transfer, not free, the fade");
    drop(rx.try_recv().unwrap());
    assert!(second.upgrade().is_none());
    plugin = process_hop(plugin);
    assert!(plugin.crossfade.crossfade_prev_state.is_none());
}

#[test]
fn completed_linear_and_spectral_fades_never_free_filters_on_callback() {
    for mode in [0, 1] {
        let mut plugin = small_plugin(mode);
        for update in 0..32 {
            plugin.state.store(make_state(0.2 + update as f32 * 0.01));
            plugin = process_hop(plugin);
            assert_eq!(plugin.crossfade.crossfade_remaining, 0);
        }
    }
}
