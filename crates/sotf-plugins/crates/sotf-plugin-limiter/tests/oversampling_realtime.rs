//! Cold prepared processing and finite drain work without allocation or freeing.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize,usize)> = const { Cell::new((0,0)) };
}
struct Allocator;
// SAFETY: Original pointers and layouts are forwarded unchanged to System.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ACTIVE.with(|active| {
            if active.get() {
                COUNTS.with(|n| {
                    let (a, d) = n.get();
                    n.set((a + 1, d));
                });
            }
        });
        // SAFETY: Forwarding the valid request to System.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        ACTIVE.with(|active| {
            if active.get() {
                COUNTS.with(|n| {
                    let (a, d) = n.get();
                    n.set((a, d + 1));
                });
            }
        });
        // SAFETY: Forwarding the original pointer and allocation layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;
struct Stop;
impl Drop for Stop {
    fn drop(&mut self) {
        ACTIVE.with(|active| active.set(false));
    }
}
fn counted(run: impl FnOnce()) {
    COUNTS.with(|n| n.set((0, 0)));
    ACTIVE.with(|a| a.set(true));
    let stop = Stop;
    run();
    drop(stop);
    assert_eq!(COUNTS.with(Cell::get), (0, 0));
}

#[test]
fn cold_process_control_begin_partial_drain_bounds_and_reset_do_not_allocate_or_free() {
    let mut cases = Vec::new();
    for choice in [1, 2] {
        for phase in 0..256 {
            // All timeline phases, both modes/rate families, and the full 32
            // channel preparation limit. Each path is processed cold on thread.
            let isp = phase % 2 != 0;
            let rate = if phase % 3 == 0 { 192_000 } else { 48_000 };
            let channels = [1, 2, 6, 32][phase % 4];
            let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
                "oversampling": choice, "lookahead_ms": 0.25, "isp_mode": isp,
                "mix": 1.0, "threshold_db": -6.0,
            }))
            .unwrap();
            let mut plugin = LimiterPlugin::from_params(channels, params);
            plugin.initialize(f64::from(rate)).unwrap();
            let input = vec![0.7; (256 + phase) * channels];
            let output = vec![0.0; 256 * channels];
            cases.push((plugin, input, output, rate, channels, choice, isp));
        }
    }
    std::thread::spawn(move || {
        let ids = [
            "threshold",
            "release",
            "soft",
            "true_peak",
            "dual_release",
            "mix",
            "link_amount",
            "feed_forward",
            "oversampling",
        ]
        .map(ParameterId::from);
        for (mut plugin, mut input, mut output, rate, channels, choice, isp) in cases {
            counted(|| {
                let ctx = ProcessContext::new(
                    f64::from(u32::try_from(rate).unwrap()),
                    input.len() / channels,
                );
                assert_eq!(
                    plugin.process_in_place(&mut input, &ctx).unwrap(),
                    ctx.num_frames
                );
                let values = [
                    ParameterValue::Float(-9.0),
                    ParameterValue::Float(70.0),
                    ParameterValue::Bool(!isp),
                    ParameterValue::Bool(true),
                    ParameterValue::Bool(true),
                    ParameterValue::Float(if isp { 1.0 } else { 0.5 }),
                    ParameterValue::Float(0.2),
                    ParameterValue::Bool(true),
                    ParameterValue::Int(choice),
                ];
                for (id, value) in ids.iter().zip(values.iter()) {
                    plugin
                        .parametric_set_parameter(id.clone(), value.clone())
                        .unwrap();
                }
                // Maximum accepted-event density fills every pending slot with
                // a changed snapshot before the core consumes one full chunk.
                for frame in 0..256 {
                    plugin
                        .parametric_set_parameter(
                            ids[0].clone(),
                            ParameterValue::Float(if frame % 2 == 0 { -9.0 } else { -12.0 }),
                        )
                        .unwrap();
                    plugin
                        .parametric_set_parameter(
                            ids[1].clone(),
                            ParameterValue::Float(if frame % 2 == 0 { 70.0 } else { 80.0 }),
                        )
                        .unwrap();
                    plugin
                        .parametric_set_parameter(
                            ids[6].clone(),
                            ParameterValue::Float(if frame % 2 == 0 { 0.0 } else { 1.0 }),
                        )
                        .unwrap();
                    plugin
                        .process_in_place(&mut input[..channels], &ProcessContext::new(rate, 1))
                        .unwrap();
                }
                for (id, value) in ids.iter().zip(values.iter()) {
                    plugin
                        .parametric_set_parameter(id.clone(), value.clone())
                        .unwrap();
                }
                if [256, 257, 510, 511].contains(&ctx.num_frames) {
                    // These eight representative cases cover both rates,
                    // modes, factors and 1/2/6/32 channels. Cross first ordinary
                    // publication, then stop one frame before the next so EOS
                    // must publish inside this same allocation/free window.
                    let target = 2 * (rate as usize / 10) - 1;
                    let mut accepted = ctx.num_frames + 256;
                    while accepted < target {
                        let frames = (target - accepted).min(256);
                        output[..frames * channels].fill(0.1);
                        plugin
                            .process_in_place(
                                &mut output[..frames * channels],
                                &ProcessContext::new(rate, frames),
                            )
                            .unwrap();
                        accepted += frames;
                    }
                }
                let ctx = ProcessContext::new(rate, 0);
                plugin.begin_drain(&ctx).unwrap();
                plugin.begin_drain(&ctx).unwrap();
                let initial = plugin.drain_call_bound().unwrap().get();
                let mut calls = 0;
                loop {
                    let current = plugin.drain_call_bound().unwrap().get();
                    // First service exactly one frame, then advertised full capacity.
                    let capacity = if calls == 0 { channels } else { output.len() };
                    let result = plugin.drain(&mut output[..capacity], &ctx).unwrap();
                    calls += 1;
                    assert!(current > 0 && calls <= initial + 1);
                    if result.complete {
                        break;
                    }
                }
                for (id, value) in ids.iter().zip(values.iter()) {
                    plugin
                        .parametric_set_parameter(id.clone(), value.clone())
                        .unwrap();
                }
                assert!(plugin.drain(&mut output, &ctx).unwrap().complete);
                plugin.reset();
                assert!(plugin.drain(&mut [], &ctx).unwrap().complete);
                let frames = input.len() / channels;
                assert_eq!(
                    plugin
                        .process_in_place(&mut input, &ProcessContext::new(rate, frames))
                        .unwrap(),
                    frames
                );
            });
        }
    })
    .join()
    .unwrap();
}
