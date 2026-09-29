// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_aec::{AecPlugin, AecPluginParams};

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
fn eos_retains_the_final_identity_sample() {
    let mut plugin = AecPlugin::from_params(
        48_000,
        AecPluginParams {
            post_filter_enabled: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut input = vec![0.0; 257 * 2];
    input[256 * 2] = 0.5;
    let mut output = vec![0.0; 257];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 257))
        .unwrap();
    let mut scratch = vec![0.0; plugin.drain_output_frames_max()];
    loop {
        let result = plugin
            .drain(&mut scratch, &ProcessContext::new(48_000, 0))
            .unwrap();
        output.extend_from_slice(&scratch[..result.frames]);
        if result.complete {
            break;
        }
    }
    assert!(
        output.len() > 512,
        "tail ended before the final delayed input"
    );
    assert_eq!(output[512], 0.5);
}

#[test]
fn drain_validation_is_transactional_and_reset_restores_continuation() {
    for post_filter_enabled in [false, true] {
        let make = || {
            AecPlugin::from_params(
                48_000,
                AecPluginParams {
                    post_filter_enabled,
                    echo_tail_ms: 50.0,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let mut tested = make();
        let mut baseline = make();
        let empty_context = ProcessContext::new(48_000, 0);
        let empty = tested.drain(&mut [], &empty_context).unwrap();
        assert!(empty.complete);
        assert_eq!(empty.frames, 0);
        let input: Vec<f32> = (0..997)
            .flat_map(|n| [0.125 + (n % 7) as f32 / 64.0, (n % 13) as f32 / 64.0])
            .collect();
        let mut initial = vec![0.0; 997];
        for processor in [&mut tested, &mut baseline] {
            processor
                .process(
                    &input[..246],
                    &mut initial[..123],
                    &ProcessContext::new(48_000, 123),
                )
                .unwrap();
        }
        assert!(tested.drain(&mut [], &empty_context).is_err());
        let mut sentinel = [42.0; 256];
        assert!(
            tested
                .drain(&mut sentinel, &ProcessContext::new(44_100, 0))
                .is_err()
        );
        assert_eq!(sentinel, [42.0; 256]);
        // Invalid drain must not latch EOS; real input can still continue.
        for processor in [&mut tested, &mut baseline] {
            processor
                .process(
                    &input[246..],
                    &mut initial[123..],
                    &ProcessContext::new(48_000, 874),
                )
                .unwrap();
        }
        let mut reached_end = false;
        for iteration in 0..20_000 {
            let capacity = [1, 17, 513][iteration % 3];
            let mut a = [42.0; 513];
            let mut b = [42.0; 513];
            let ar = tested.drain(&mut a[..capacity], &empty_context).unwrap();
            let br = baseline.drain(&mut b[..capacity], &empty_context).unwrap();
            assert!(ar.frames <= tested.drain_output_frames_max());
            assert_eq!(ar.frames, br.frames);
            assert_eq!(ar.complete, br.complete);
            assert_eq!(a, b);
            assert!(a[ar.frames..].iter().all(|&x| x == 42.0));
            if iteration == 0 {
                assert!(
                    tested
                        .process(
                            &input[..2],
                            &mut sentinel[..1],
                            &ProcessContext::new(48_000, 1)
                        )
                        .is_err()
                );
                assert_eq!(sentinel, [42.0; 256]);
                assert!(
                    tested
                        .set_parameter(
                            sotf_host::parameters::ParameterId::from("post_filter_enabled"),
                            sotf_host::parameters::ParameterValue::Bool(true)
                        )
                        .is_err()
                );
                assert!(tested.drain(&mut [], &empty_context).is_err());
                assert!(
                    tested
                        .drain(&mut sentinel, &ProcessContext::new(44_100, 0))
                        .is_err()
                );
            }
            if ar.complete {
                reached_end = true;
                break;
            }
            assert!(ar.frames > 0);
        }
        assert!(reached_end);
        assert_eq!(tested.drain(&mut [], &empty_context).unwrap().frames, 0);
        assert!(tested.drain(&mut [], &empty_context).unwrap().complete);
        assert!(
            tested
                .process(
                    &input[..2],
                    &mut sentinel[..1],
                    &ProcessContext::new(48_000, 1)
                )
                .is_err()
        );
        tested.reset();
        let mut fresh = make();
        let mut actual = vec![0.0; 997];
        let mut expected = vec![0.0; 997];
        tested
            .process(&input, &mut actual, &ProcessContext::new(48_000, 997))
            .unwrap();
        fresh
            .process(&input, &mut expected, &ProcessContext::new(48_000, 997))
            .unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn cold_drain_and_reset_require_no_allocations_or_deallocations() {
    for post_filter_enabled in [false, true] {
        let make = || {
            AecPlugin::from_params(
                48_000,
                AecPluginParams {
                    post_filter_enabled,
                    echo_tail_ms: 50.0,
                    ..Default::default()
                },
            )
            .unwrap()
        };
        for input_frames in [1, 997] {
            let mut plugin = make();
            let input = vec![0.125; input_frames * 2];
            let mut initial = vec![0.0; input_frames];
            plugin
                .process(
                    &input,
                    &mut initial,
                    &ProcessContext::new(48_000, input_frames),
                )
                .unwrap();
            let (plugin, counts) = std::thread::spawn(move || {
                let mut scratch = [0.0; 256];
                let counts = callback_counts(|| {
                    for reset in [false, true] {
                        assert_eq!(
                            plugin.tail_length(),
                            sotf_host::plugin::TailLength::Finite(3072)
                        );
                        if reset {
                            plugin.reset();
                            plugin
                                .process(
                                    &input,
                                    &mut initial,
                                    &ProcessContext::new(48_000, input_frames),
                                )
                                .unwrap();
                        }
                        for iteration in 0..20_000 {
                            let capacity = [1, 17, 256][iteration % 3];
                            let result = plugin
                                .drain(&mut scratch[..capacity], &ProcessContext::new(48_000, 0))
                                .unwrap();
                            assert_eq!(
                                plugin.tail_length(),
                                sotf_host::plugin::TailLength::Finite(3072)
                            );
                            if result.complete {
                                break;
                            }
                            assert!(result.frames > 0);
                            assert!(iteration < 19_999);
                        }
                    }
                });
                (plugin, counts)
            })
            .join()
            .unwrap();
            drop(plugin);
            assert_eq!(counts, (0, 0), "cold/reset drain heap activity");
        }
    }
}
