//! Cold reference-clock, measurement publication, and tail work retain storage.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_eq::{EqPlugin, EqPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Requests are forwarded unchanged to System. Const TLS counters do
// not allocate, retain pointers, or access the allocation contents.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|counts| {
                let (a, d) = counts.get();
                counts.set((a + 1, d));
            });
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if ACTIVE.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|counts| {
                let (a, d) = counts.get();
                counts.set((a, d + 1));
            });
        }
        // SAFETY: Pointer and layout match an allocation forwarded to System.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

fn make(rate: u32, channels: usize, factor: i32, enabled: bool, finite: bool) -> Box<dyn Plugin> {
    let filters = if finite {
        Vec::new()
    } else {
        vec![serde_json::json!({
            "filter_type":"peak", "freq":1000.0, "q":1.0, "db_gain":9.0
        })]
    };
    let params: EqPluginParams = serde_json::from_value(serde_json::json!({
        "filters":filters, "auto_gain":{"enabled":enabled,"max_gain_db":12.0}
    }))
    .unwrap();
    let mut plugin = EqPlugin::from_params(channels, rate, params)
        .unwrap()
        .into_boxed_plugin();
    plugin
        .set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(factor),
        )
        .unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

#[test]
fn cold_process_publication_live_control_and_reset_reuse_prepared_storage() {
    for rate in [48_000, 192_000] {
        for channels in [1, 2] {
            for factor in [1, 2, 4] {
                for enabled in [false, true] {
                    for topology in 0..if factor == 1 { 3 } else { 2 } {
                        let mut plugin = make(rate, channels, factor, enabled, false);
                        if topology == 1 {
                            plugin
                                .set_parameter(
                                    ParameterId::from("tdf2"),
                                    ParameterValue::Bool(true),
                                )
                                .unwrap();
                        } else if topology == 2 {
                            plugin
                                .set_parameter(
                                    ParameterId::from("topology"),
                                    ParameterValue::Int(1),
                                )
                                .unwrap();
                        }
                        let maximum = if factor == 1 { 8193 } else { 4096 };
                        let input: Vec<f32> = (0..maximum * channels)
                            .map(|i| (i % 37) as f32 / 100.0 - 0.18)
                            .collect();
                        let mut output = vec![0.0; input.len()];
                        let control = ParameterId::from("band_0_gain");
                        // Keep the prepared ID owner alive: dropping a caller's
                        // final owned ParameterId is outside the DSP contract.
                        let counts = std::thread::spawn(move || {
                            ACTIVE.set(true);
                            for _ in 0..2 {
                                plugin
                                    .set_parameter(control.clone(), ParameterValue::Float(-9.0))
                                    .unwrap();
                                let mut frames = 0;
                                while frames < rate as usize / 10 + maximum {
                                    plugin
                                        .process(
                                            &input,
                                            &mut output,
                                            &ProcessContext::new(rate, maximum),
                                        )
                                        .unwrap();
                                    frames += maximum;
                                }
                                plugin.reset();
                            }
                            ACTIVE.set(false);
                            COUNTS.get()
                        })
                        .join()
                        .unwrap();
                        assert_eq!(
                            counts,
                            (0, 0),
                            "rate{rate}/channels{channels}/factor{factor}/enabled{enabled}/topology{topology}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn first_thread_tail_crosses_measurement_publication_without_heap_operations() {
    for rate in [48_000, 192_000] {
        for channels in [1, 2] {
            for factor in [2, 4] {
                for capacity in [1, 17, 256] {
                    let mut plugin = make(rate, channels, factor, true, true);
                    // First meter refresh is precisely the first drain frame.
                    let frames = rate as usize / 10 - 1;
                    let input = vec![0.125; 4096 * channels];
                    let mut output = vec![0.0; input.len()];
                    let mut offset = 0;
                    while offset < frames {
                        let n = (frames - offset).min(4096);
                        plugin
                            .process(
                                &input[..n * channels],
                                &mut output[..n * channels],
                                &ProcessContext::new(rate, n),
                            )
                            .unwrap();
                        offset += n;
                    }
                    let mut buffer = vec![123.0; capacity * channels];
                    let counts = std::thread::spawn(move || {
                        ACTIVE.set(true);
                        plugin.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
                        loop {
                            let _ = plugin.drain_call_bound();
                            if plugin
                                .drain(&mut buffer, &ProcessContext::new(rate, 0))
                                .unwrap()
                                .complete
                            {
                                break;
                            }
                        }
                        plugin.reset();
                        ACTIVE.set(false);
                        COUNTS.get()
                    })
                    .join()
                    .unwrap();
                    assert_eq!(
                        counts,
                        (0, 0),
                        "rate{rate}/channels{channels}/factor{factor}/capacity{capacity}"
                    );
                }
            }
        }
    }
}
