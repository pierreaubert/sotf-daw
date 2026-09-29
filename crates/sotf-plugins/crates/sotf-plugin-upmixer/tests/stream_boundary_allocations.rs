//! Cold callback and reset heap checks for prefixed analysis windows.

use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: Layout/pointer handling is delegated unchanged to CountingAlloc;
// instrumentation uses only constant-initialized nonallocating TLS cells.
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
        // SAFETY: Forward the original pointer and layout unchanged.
        unsafe { sotf_host::CountingAlloc.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

#[test]
fn first_callback_and_reset_have_no_heap_activity() {
    let mut plugin = UpmixerPlugin::from_params(UpmixerPluginParams::default());
    plugin.initialize(48_000).unwrap();
    let input = vec![0.1; 4096 * 2];
    let mut output = vec![0.0; 4096 * plugin.output_channels()];
    let (plugin, counts) = std::thread::spawn(move || {
        let context = ProcessContext::new(48_000, 4096);
        TRACKING.set(true);
        plugin.process(&input, &mut output, &context).unwrap();
        while !plugin.drain(&mut output, &context).unwrap().complete {}
        plugin.reset();
        plugin.process(&input, &mut output, &context).unwrap();
        while !plugin.drain(&mut output, &context).unwrap().complete {}
        TRACKING.set(false);
        (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "cold/reset allocations/deallocations");
}

#[test]
fn small_fft_callbacks_and_reset_have_no_heap_activity() {
    for fft_size in [2, 4, 8, 16, 32] {
        let params = serde_json::from_value(serde_json::json!({
            "fft_size": fft_size,
            "speaker_config": "7.1.4",
            "enable_hr_direct": true,
            "auto_gain_enabled": true
        }))
        .unwrap();
        let mut plugin = UpmixerPlugin::from_params(params);
        plugin.initialize(48_000).unwrap();
        let frames = 512;
        let input = vec![0.1; frames * 2];
        let mut output = vec![0.0; frames * plugin.output_channels()];
        let (plugin, counts) = std::thread::spawn(move || {
            let context = ProcessContext::new(48_000, frames);
            TRACKING.set(true);
            assert_eq!(
                plugin.process(&input, &mut output, &context).unwrap(),
                frames
            );
            plugin.reset();
            assert_eq!(
                plugin.process(&input, &mut output, &context).unwrap(),
                frames
            );
            TRACKING.set(false);
            (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
        })
        .join()
        .unwrap();
        drop(plugin);
        assert_eq!(
            counts,
            (0, 0),
            "small FFT callback allocations/deallocations at N={fft_size}"
        );
    }
}

#[test]
fn first_drain_on_fresh_callback_thread_has_no_heap_activity() {
    let params = serde_json::from_value(serde_json::json!({
        "enable_subharmonic_synth": true,
        "subharmonic_release_ms": 10.0,
        "auto_gain_enabled": true
    }))
    .unwrap();
    let mut plugin = UpmixerPlugin::from_params(params);
    plugin.initialize(48_000).unwrap();
    plugin
        .process(&[0.2, -0.1], &mut [0.0; 6], &ProcessContext::new(48_000, 1))
        .unwrap();
    let mut output = vec![0.0; plugin.drain_output_frames_max() * plugin.output_channels()];
    let (plugin, counts) = std::thread::spawn(move || {
        TRACKING.set(true);
        loop {
            assert!(plugin.drain_call_bound().is_some());
            let result = plugin
                .drain(&mut output, &ProcessContext::new(48_000, 1))
                .unwrap();
            if result.complete {
                break;
            }
            assert!(result.frames > 0);
        }
        TRACKING.set(false);
        (plugin, (ALLOCATIONS.get(), DEALLOCATIONS.get()))
    })
    .join()
    .unwrap();
    drop(plugin);
    assert_eq!(counts, (0, 0), "cold drain allocations/deallocations");
}
