//! Independent asymmetric layout, precision, state, and cold heap oracles.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::{InPlacePlugin, InPlacePluginAdapter, PluginInfo, PluginResult};
use sotf_host::{
    Parameter, ParameterId, ParameterSchema, ParameterSet, ParameterValue, ParametricInPlacePlugin,
    ParametricInPlacePluginAdapter, Plugin, ProcessContext,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}
struct CountingAllocator;
// SAFETY: This allocator forwards every original pointer and layout to System.
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
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        COUNTING.with(|active| {
            if active.get() {
                COUNTS.with(|counts| {
                    let (allocations, frees) = counts.get();
                    counts.set((allocations, frees + 1));
                });
            }
        });
        // SAFETY: The pointer and its original allocation layout are unchanged.
        unsafe { System.dealloc(ptr, layout) }
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
fn without_heap<T>(operation: impl FnOnce() -> T) -> T {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    assert_eq!(COUNTS.with(Cell::get), (0, 0), "allocations and frees");
    value
}

#[derive(Default)]
struct Probe {
    native: bool,
    opt_in: bool,
    return_delta: isize,
    previous32: [f32; 2],
    previous64: [f64; 2],
    frames: usize,
}
impl ParametricInPlacePlugin for Probe {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("probe", "0", "tests")
    }
    fn channels(&self) -> usize {
        2
    }
    fn input_channels(&self) -> usize {
        4
    }
    fn supports_bounded_subdivision(&self) -> bool {
        self.opt_in
    }
    fn supports_f64(&self) -> bool {
        self.native
    }
    fn parameter_schema(&self) -> ParameterSchema {
        Vec::new()
    }
    fn current_values(&self) -> ParameterSet {
        ParameterSet::new()
    }
    fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
        Ok(())
    }
    fn parametric_get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        Some(ParameterValue::Int(self.frames as i32))
    }
    fn reset(&mut self) {
        self.previous32 = [0.0; 2];
        self.previous64 = [0.0; 2];
        self.frames = 0;
    }
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        assert_eq!(context.sample_rate, 48_000);
        assert_eq!(buffer.len(), context.num_frames * 4);
        assert_eq!(context.transport.sample_position, 987);
        if self.return_delta != 0 {
            return Ok(context.num_frames.saturating_add_signed(self.return_delta));
        }
        for frame in buffer.as_chunks_mut::<4>().0 {
            for channel in 0..2 {
                let input = frame[channel];
                frame[channel] = input + frame[channel + 2] * 0.5 + self.previous32[channel] * 0.25;
                self.previous32[channel] = input;
            }
        }
        self.frames += context.num_frames;
        Ok(context.num_frames)
    }
    fn process_in_place_f64(
        &mut self,
        buffer: &mut [f64],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        assert!(self.native, "fallback must call f32 directly");
        assert_eq!(context.sample_rate, 48_000);
        assert_eq!(buffer.len(), context.num_frames * 4);
        assert_eq!(context.transport.sample_position, 987);
        if self.return_delta != 0 {
            return Ok(context.num_frames.saturating_add_signed(self.return_delta));
        }
        for frame in buffer.as_chunks_mut::<4>().0 {
            for channel in 0..2 {
                let input = frame[channel];
                frame[channel] = input + frame[channel + 2] * 0.5 + self.previous64[channel] * 0.25;
                self.previous64[channel] = input;
            }
        }
        self.frames += context.num_frames;
        Ok(context.num_frames)
    }
}
struct Plain(Probe);
impl InPlacePlugin for Plain {
    fn info(&self) -> PluginInfo {
        self.0.info()
    }
    fn channels(&self) -> usize {
        self.0.channels()
    }
    fn input_channels(&self) -> usize {
        self.0.input_channels()
    }
    fn supports_bounded_subdivision(&self) -> bool {
        self.0.opt_in
    }
    fn supports_f64(&self) -> bool {
        self.0.native
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
        Ok(())
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.0.parametric_get_parameter(id)
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn process_in_place(&mut self, b: &mut [f32], c: &ProcessContext) -> PluginResult<usize> {
        self.0.process_in_place(b, c)
    }
    fn process_in_place_f64(&mut self, b: &mut [f64], c: &ProcessContext) -> PluginResult<usize> {
        self.0.process_in_place_f64(b, c)
    }
}
fn wrap(route: usize, probe: Probe) -> Box<dyn Plugin> {
    match route {
        0 => Box::new(InPlacePluginAdapter::new(Plain(probe))),
        1 => Box::new(ParametricInPlacePluginAdapter::new(probe)),
        _ => Box::new(InPlacePluginAdapter::new(
            ParametricInPlacePluginAdapter::new(probe),
        )),
    }
}
fn make(route: usize, native: bool, delta: isize) -> Box<dyn Plugin> {
    let mut plugin = wrap(
        route,
        Probe {
            native,
            opt_in: true,
            return_delta: delta,
            ..Probe::default()
        },
    );
    plugin.initialize(48_000).unwrap();
    plugin
}
fn context(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(48_000, frames).with_sample_position(987)
}
fn input(frames: usize) -> Vec<f64> {
    (0..frames)
        .flat_map(|i| {
            let v = (i % 31) as f64 / 128.0;
            [v + 2.0_f64.powi(-35), -v - 2.0_f64.powi(-36), 0.25, -0.125]
        })
        .collect()
}
fn reference(input: &[f64], native: bool) -> Vec<f64> {
    let mut prev = [0.0; 2];
    input
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|frame| {
            let mut out = [0.0; 2];
            for ch in 0..2 {
                out[ch] = if native {
                    frame[ch] + frame[ch + 2] * 0.5 + prev[ch] * 0.25
                } else {
                    f64::from(
                        frame[ch] as f32 + frame[ch + 2] as f32 * 0.5 + prev[ch] as f32 * 0.25,
                    )
                };
                prev[ch] = frame[ch];
            }
            out
        })
        .collect()
}
#[test]
fn both_adapters_compact_program_lanes_and_preserve_native_f64_precision() {
    for route in 0..3 {
        for native in [false, true] {
            for frames in [1, 17, 255, 256, 257, 8192, 17003] {
                let input = input(frames);
                let mut output = vec![99.0; frames * 2];
                let mut plugin = make(route, native, 0);
                assert_eq!(
                    plugin
                        .process_f64(&input, &mut output, &context(frames))
                        .unwrap(),
                    frames
                );
                assert_eq!(output, reference(&input, native));
                let input32: Vec<_> = input.iter().map(|x| *x as f32).collect();
                let mut output32 = vec![99.0; frames * 2];
                plugin.reset();
                assert_eq!(
                    plugin
                        .process(&input32, &mut output32, &context(frames))
                        .unwrap(),
                    frames
                );
                assert_eq!(
                    output32,
                    reference(&input, false)
                        .iter()
                        .map(|x| *x as f32)
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}
#[test]
fn invalid_whole_blocks_leave_output_and_history_untouched() {
    for route in 0..3 {
        for native in [false, true] {
            let mut plugin = make(route, native, 0);
            let input = input(1025);
            let mut invalid = input.clone();
            *invalid.last_mut().unwrap() = f64::NAN;
            let mut output = vec![99.0; 2050];
            assert!(
                plugin
                    .process_f64(&invalid, &mut output, &context(1025))
                    .is_err()
            );
            assert!(
                plugin
                    .process_f64(&input, &mut output[..2048], &context(1025))
                    .is_err()
            );
            assert!(
                plugin
                    .process_f64(&input, &mut output, &ProcessContext::new(44_100, 1025))
                    .is_err()
            );
            assert!(
                plugin
                    .process_f64(&[], &mut [], &context(usize::MAX))
                    .is_err()
            );
            assert!(output.iter().all(|x| *x == 99.0));
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("frames")),
                Some(ParameterValue::Int(0))
            );
            plugin
                .process_f64(&input, &mut output, &context(1025))
                .unwrap();
            assert_eq!(output, reference(&input, native));
            plugin.reset();
            let mut invalid32: Vec<_> = input.iter().map(|x| *x as f32).collect();
            *invalid32.last_mut().unwrap() = f32::INFINITY;
            let mut output32 = vec![99.0; 2050];
            assert!(
                plugin
                    .process(&invalid32, &mut output32, &context(1025))
                    .is_err()
            );
            assert!(output32.iter().all(|x| *x == 99.0));
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("frames")),
                Some(ParameterValue::Int(0))
            );
        }
    }
}
#[test]
fn cold_large_process_and_reset_do_not_allocate_or_free() {
    for route in 0..3 {
        for native in [false, true] {
            for double in [false, true] {
                let mut plugin = make(route, native, 0);
                let input64 = input(17003);
                let input32: Vec<_> = input64.iter().map(|x| *x as f32).collect();
                let mut output64 = vec![0.0; 34006];
                let mut output32 = vec![0.0; 34006];
                std::thread::spawn(move || {
                    without_heap(|| {
                        if double {
                            plugin
                                .process_f64(&input64, &mut output64, &context(17003))
                                .unwrap();
                        } else {
                            plugin
                                .process(&input32, &mut output32, &context(17003))
                                .unwrap();
                        }
                        plugin.reset();
                    });
                })
                .join()
                .unwrap();
            }
        }
    }
}
#[test]
fn frame_count_contract_errors_never_publish_the_bad_chunk() {
    for route in 0..3 {
        for native in [false, true] {
            for delta in [-1, 1] {
                let mut plugin = make(route, native, delta);
                let input = input(257);
                let mut output = vec![99.0; 514];
                let error = plugin
                    .process_f64(&input, &mut output, &context(257))
                    .unwrap_err();
                assert!(error.contains("returned") && error.contains("expected 256"));
                assert!(output.iter().all(|x| *x == 99.0));
                let input32: Vec<_> = input.iter().map(|x| *x as f32).collect();
                let mut output32 = vec![99.0; 514];
                assert!(
                    plugin
                        .process(&input32, &mut output32, &context(257))
                        .is_err()
                );
                assert!(output32.iter().all(|x| *x == 99.0));
            }
        }
    }
}
#[test]
fn unsupported_asymmetry_and_uninitialized_processing_fail_explicitly() {
    for route in 0..3 {
        assert!(
            wrap(route, Probe::default())
                .initialize(48_000)
                .unwrap_err()
                .contains("subdivision")
        );
        let mut plugin = wrap(
            route,
            Probe {
                opt_in: true,
                ..Probe::default()
            },
        );
        let mut output = [99.0; 2];
        assert!(plugin.process(&[0.0; 4], &mut output, &context(1)).is_err());
        assert_eq!(output, [99.0; 2]);
    }
}
#[test]
fn direct_parametric_in_place_fallback_retains_keys_and_input_stride() {
    let mut plugin = ParametricInPlacePluginAdapter::new(Probe {
        opt_in: true,
        ..Probe::default()
    });
    InPlacePlugin::initialize(&mut plugin, 48_000).unwrap();
    let original = input(1025);
    let mut buffer = original.clone();
    without_heap(|| {
        InPlacePlugin::process_in_place_f64(&mut plugin, &mut buffer, &context(1025)).unwrap()
    });
    let expected = reference(&original, false);
    for (index, frame) in buffer.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(&frame[..2], &expected[index * 2..index * 2 + 2]);
        assert_eq!(&frame[2..], &original[index * 4 + 2..index * 4 + 4]);
    }
}
