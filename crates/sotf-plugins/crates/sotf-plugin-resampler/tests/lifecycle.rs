//! Public stream lifecycle and exact retry/history regressions.

// Rust guideline compliant 2026-02-21
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};

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

fn mode(plugin: &mut ResamplerPlugin, enabled: bool) -> Result<(), String> {
    plugin.set_parameter(
        ParameterId::from("dynamic_ratio"),
        ParameterValue::Bool(enabled),
    )
}

fn make(
    rate: u32,
    output_rate: u32,
    quality: ResamplerQuality,
    chunk: usize,
    dynamic: bool,
) -> ResamplerPlugin {
    let mut plugin = ResamplerPlugin::with_quality(1, rate, output_rate, chunk, quality).unwrap();
    plugin.initialize(rate).unwrap();
    mode(&mut plugin, dynamic).unwrap();
    plugin
}

fn feed(plugin: &mut ResamplerPlugin, input: &[f32], rate: u32, output: &mut Vec<f32>) {
    let mut block = vec![f32::NAN; plugin.output_frames_for_input(input.len())];
    let frames = plugin
        .process(input, &mut block, &ProcessContext::new(rate, input.len()))
        .unwrap();
    output.extend_from_slice(&block[..frames]);
    assert!(block[frames..].iter().all(|x| x.is_nan()));
}

fn finish(plugin: &mut ResamplerPlugin, rate: u32, output: &mut Vec<f32>) {
    for _ in 0..4096 {
        let mut block = vec![f32::NAN; plugin.drain_output_frames_max()];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(rate, 0))
            .unwrap();
        output.extend_from_slice(&block[..result.frames]);
        assert!(block[result.frames..].iter().all(|x| x.is_nan()));
        if result.complete {
            return;
        }
    }
    panic!("drain did not complete");
}

#[test]
fn same_rate_mode_changes_reject_without_losing_or_reordering_history() {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for rate in [44_100, 48_000, 96_000] {
            for dynamic in [false, true] {
                for length in [1, 123, 1024, 1301] {
                    let mut tested = make(rate, rate, quality, 1024, dynamic);
                    let mut control = make(rate, rate, quality, 1024, dynamic);
                    let mut input: Vec<f32> = (0..length).map(|n| (n % 17) as f32 / 64.0).collect();
                    input[length - 1] = 0.5;
                    let mut actual = vec![];
                    let mut expected = vec![];
                    feed(&mut tested, &input, rate, &mut actual);
                    feed(&mut control, &input, rate, &mut expected);
                    let state = (
                        tested.current_ratio(),
                        tested.latency_samples(),
                        tested.output_frames_for_input(333),
                        tested.drain_output_frames_max(),
                    );
                    assert!(
                        mode(&mut tested, !dynamic).is_err(),
                        "quality={quality:?},rate={rate},dynamic={dynamic},length={length}"
                    );
                    mode(&mut tested, dynamic).unwrap();
                    assert_eq!(tested.is_dynamic_ratio(), dynamic);
                    assert_eq!(
                        (
                            tested.current_ratio(),
                            tested.latency_samples(),
                            tested.output_frames_for_input(333),
                            tested.drain_output_frames_max()
                        ),
                        state
                    );
                    // initialize() validates the clock; it does not clear a stream.
                    tested.initialize(rate).unwrap();
                    assert!(mode(&mut tested, !dynamic).is_err());
                    feed(&mut tested, &[0.25; 333], rate, &mut actual);
                    feed(&mut control, &[0.25; 333], rate, &mut expected);
                    finish(&mut tested, rate, &mut actual);
                    finish(&mut control, rate, &mut expected);
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}

#[test]
fn fresh_and_reset_mode_changes_clear_dormant_ratio_ramps() {
    for quality in [
        ResamplerQuality::Fast,
        ResamplerQuality::Medium,
        ResamplerQuality::High,
    ] {
        for length in [123, 1301] {
            let mut tested = make(48_000, 48_000, quality, 256, true);
            let mut control = make(48_000, 48_000, quality, 256, true);
            tested.set_ratio(2.0, false).unwrap();
            mode(&mut tested, false).unwrap();
            mode(&mut tested, true).unwrap();
            assert_eq!(tested.current_ratio(), 1.0);
            let input: Vec<f32> = (0..length).map(|n| (n % 19) as f32 / 64.0).collect();
            let mut actual = vec![];
            let mut expected = vec![];
            feed(&mut tested, &input, 48_000, &mut actual);
            feed(&mut control, &input, 48_000, &mut expected);
            finish(&mut tested, 48_000, &mut actual);
            finish(&mut control, 48_000, &mut expected);
            assert_eq!(actual, expected);
            tested.reset();
            mode(&mut tested, false).unwrap();
            assert_eq!(tested.latency_samples(), 0);
            actual.clear();
            feed(&mut tested, &input, 48_000, &mut actual);
            assert_eq!(actual, input);
            tested.reset();
            mode(&mut tested, true).unwrap();
            actual.clear();
            feed(&mut tested, &input, 48_000, &mut actual);
            finish(&mut tested, 48_000, &mut actual);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn unequal_rate_disable_preserves_the_backend_and_matches_explicit_nominal_ramp() {
    for (rate, output_rate) in [(44_100, 48_000), (48_000, 44_100)] {
        for length in [123, 1024, 1301] {
            let mut tested = make(rate, output_rate, ResamplerQuality::Medium, 256, true);
            let mut control = make(rate, output_rate, ResamplerQuality::Medium, 256, true);
            let nominal = tested.ratio();
            let mut actual = vec![];
            let mut expected = vec![];
            let input: Vec<f32> = (0..length).map(|n| (n % 29) as f32 / 64.0).collect();
            for (plugin, output) in [(&mut tested, &mut actual), (&mut control, &mut expected)] {
                plugin.set_ratio(nominal * 1.01, false).unwrap();
                feed(plugin, &input, rate, output);
            }
            mode(&mut tested, false).unwrap();
            control.set_ratio(nominal, true).unwrap();
            for part in [&input[..17], &input[..91], &input[..]] {
                feed(&mut tested, part, rate, &mut actual);
                feed(&mut control, part, rate, &mut expected);
            }
            mode(&mut tested, true).unwrap();
            feed(&mut tested, &[0.25; 333], rate, &mut actual);
            feed(&mut control, &[0.25; 333], rate, &mut expected);
            finish(&mut tested, rate, &mut actual);
            finish(&mut control, rate, &mut expected);
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn mode_metadata_matches_the_instance_clock_topology() {
    for (output_rate, expected) in [
        (48_000, UpdateMode::Structural),
        (44_100, UpdateMode::Realtime),
    ] {
        let plugin = make(48_000, output_rate, ResamplerQuality::Medium, 256, false);
        let parameter = plugin
            .parameters()
            .into_iter()
            .find(|p| p.id == ParameterId::from("dynamic_ratio"))
            .unwrap();
        assert_eq!(parameter.update_mode, expected);
    }
}

#[test]
fn invalid_capacity_or_clock_never_latches_eof_or_changes_audio() {
    let mut tested = make(48_000, 48_000, ResamplerQuality::High, 32, true);
    let mut control = make(48_000, 48_000, ResamplerQuality::High, 32, true);
    let mut actual = vec![];
    let mut expected = vec![];
    feed(&mut tested, &[0.5; 19], 48_000, &mut actual);
    feed(&mut control, &[0.5; 19], 48_000, &mut expected);
    assert!(
        tested
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .is_err()
    );
    let mut sentinel = [42.0; 512];
    assert!(
        tested
            .drain(&mut sentinel, &ProcessContext::new(44_100, 0))
            .is_err()
    );
    assert!(
        tested
            .process(&[0.25; 64], &mut sentinel, &ProcessContext::new(44_100, 64))
            .is_err()
    );
    assert_eq!(sentinel, [42.0; 512]);
    feed(&mut tested, &[0.25; 23], 48_000, &mut actual);
    feed(&mut control, &[0.25; 23], 48_000, &mut expected);
    let mut reached_end = false;
    let mut checked_pending = false;
    for _ in 0..64 {
        let mut a = vec![f32::NAN; tested.drain_output_frames_max()];
        let mut b = vec![f32::NAN; control.drain_output_frames_max()];
        let ar = tested
            .drain(&mut a, &ProcessContext::new(48_000, 0))
            .unwrap();
        let br = control
            .drain(&mut b, &ProcessContext::new(48_000, 0))
            .unwrap();
        assert_eq!((ar.frames, ar.complete), (br.frames, br.complete));
        assert_eq!(&a[..ar.frames], &b[..br.frames]);
        assert!(a[ar.frames..].iter().all(|x| x.is_nan()));
        actual.extend_from_slice(&a[..ar.frames]);
        expected.extend_from_slice(&b[..br.frames]);
        assert!(tested.set_ratio(1.01, true).is_err());
        assert!(tested.set_ratio_relative(1.01, false).is_err());
        assert!(
            tested
                .set_parameter(ParameterId::from("ratio"), ParameterValue::Float(1.01))
                .is_err()
        );
        assert!(mode(&mut tested, false).is_err());
        mode(&mut tested, true).unwrap();
        if ar.complete {
            reached_end = true;
            break;
        }
        checked_pending = true;
        assert!(
            tested
                .drain(&mut [], &ProcessContext::new(48_000, 0))
                .is_err()
        );
        assert!(
            tested
                .drain(&mut sentinel, &ProcessContext::new(44_100, 0))
                .is_err()
        );
        assert_eq!(sentinel, [42.0; 512]);
    }
    assert!(reached_end && checked_pending);
    assert_eq!(actual, expected);
    assert!(
        tested
            .drain(&mut [], &ProcessContext::new(48_000, 0))
            .unwrap()
            .complete
    );
}

#[test]
fn empty_drain_finishes_the_stream_and_reset_permits_fresh_controls() {
    for (rate, output_rate, dynamic) in [
        (48_000, 48_000, false),
        (48_000, 48_000, true),
        (44_100, 48_000, false),
        (44_100, 48_000, true),
    ] {
        // Leave initialize uncalled to exercise quality's EOS guard separately
        // from the already existing post-initialize structural restriction.
        let mut plugin = ResamplerPlugin::new(1, rate, output_rate, 64).unwrap();
        mode(&mut plugin, dynamic).unwrap();
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(rate + 1, 0))
                .is_err()
        );
        let complete = plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap();
        assert_eq!(complete.frames, 0);
        assert!(complete.complete);
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(rate, 0))
                .unwrap()
                .complete
        );
        assert!(mode(&mut plugin, !dynamic).is_err());
        mode(&mut plugin, dynamic).unwrap();
        assert!(plugin.set_ratio(plugin.ratio(), false).is_err());
        assert!(
            plugin
                .set_parameter(ParameterId::from("quality"), ParameterValue::Int(2))
                .is_err()
        );
        assert_eq!(plugin.quality(), ResamplerQuality::Medium);
        assert!(
            plugin
                .process(&[0.5], &mut [0.0; 128], &ProcessContext::new(rate, 1))
                .is_err()
        );
        plugin.reset();
        mode(&mut plugin, !dynamic).unwrap();
        plugin
            .process(&[0.5], &mut [0.0; 128], &ProcessContext::new(rate, 1))
            .unwrap();
    }
}

#[test]
fn cold_valid_setters_process_drain_and_reset_have_no_heap_activity() {
    for output_rate in [44_100, 48_000] {
        for channels in [1, 8] {
            let mut plugin = ResamplerPlugin::with_quality(
                channels,
                48_000,
                output_rate,
                256,
                ResamplerQuality::High,
            )
            .unwrap();
            plugin.initialize(48_000).unwrap();
            let dynamic_id = ParameterId::from("dynamic_ratio");
            let ratio_id = ParameterId::from("ratio");
            let input = vec![0.125; 1301 * channels];
            let capacity = plugin.output_frames_for_input(8192).max(16_384) * channels;
            let mut output = vec![0.0; capacity];
            let (plugin, counts) = std::thread::spawn(move || {
                let counts = callback_counts(|| {
                    // First setup transition on this audio thread. Clearing a
                    // queued setup ramp must only reuse existing storage.
                    plugin
                        .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                        .unwrap();
                    plugin.set_ratio(1.5 * plugin.ratio(), false).unwrap();
                    if output_rate == 48_000 {
                        plugin
                            .set_parameter(dynamic_id.clone(), ParameterValue::Bool(false))
                            .unwrap();
                        plugin
                            .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                            .unwrap();
                    } else {
                        plugin.set_ratio(plugin.ratio(), false).unwrap();
                    }
                    for reset in [false, true] {
                        if reset {
                            plugin.reset();
                        }
                        for (offset, frames) in [(0, 123), (123, 1178)] {
                            plugin
                                .set_parameter(
                                    ratio_id.clone(),
                                    ParameterValue::Float((plugin.ratio() * 1.001) as f32),
                                )
                                .unwrap();
                            plugin
                                .process(
                                    &input[offset * channels..(offset + frames) * channels],
                                    &mut output,
                                    &ProcessContext::new(48_000, frames),
                                )
                                .unwrap();
                        }
                        // Unequal-rate mode changes keep the live backend.
                        if output_rate != 48_000 {
                            plugin
                                .set_parameter(dynamic_id.clone(), ParameterValue::Bool(false))
                                .unwrap();
                            plugin
                                .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                                .unwrap();
                        }
                        for step in 0..4096 {
                            let result = plugin
                                .drain(&mut output, &ProcessContext::new(48_000, 0))
                                .unwrap();
                            if result.complete {
                                break;
                            }
                            assert!(step < 4095);
                        }
                        plugin
                            .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                            .unwrap();
                    }
                });
                (plugin, counts)
            })
            .join()
            .unwrap();
            drop(plugin);
            assert_eq!(
                counts,
                (0, 0),
                "rate={output_rate},channels={channels}: cold heap activity"
            );
        }
    }
}
