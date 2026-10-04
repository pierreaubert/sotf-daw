// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

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
fn cold_spectral_overflow_retry_rejection_and_recovery_are_allocation_free() {
    use nalgebra::Complex;
    use sotf_plugin_beamformer::mvdr::MvdrBeamformer;

    for algorithm in [0, 1] {
        let mut plugin = plugin(48_000, algorithm);
        let mut core = MvdrBeamformer::new(2, 257);
        let directions = vec![vec![Complex::new(1.0, 0.0); 2]; 257];
        let mut bins = vec![vec![Complex::new(0.0, 0.0); 257]; 2];
        let input: Vec<_> = (0..2048)
            .map(|i| if i % 4 < 2 { f32::MAX } else { -f32::MAX })
            .collect();
        let silence = vec![0.0; input.len()];
        let mut output = vec![0.0; input.len() / 2];
        let resources = std::thread::spawn(move || {
            let counts = callback_counts(|| {
                for amplitude in [2e19, 1e20, 0.25] {
                    bins[0].fill(Complex::new(amplitude, 0.0));
                    bins[1].fill(Complex::new(-amplitude, 0.0));
                    core.update_noise_covariance(&bins, &directions);
                    core.compute_weights(&directions);
                }
                bins[1][256].im = f32::INFINITY;
                assert!(!core.update_noise_covariance(&bins, &directions));
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 1024))
                    .unwrap();
                assert!(output.iter().all(|x| x.is_finite()));
                plugin
                    .process(&silence, &mut output, &ProcessContext::new(48_000, 1024))
                    .unwrap();
                assert!(output.iter().all(|x| x.is_finite()));
                plugin.reset();
                core.reset();
            });
            assert_eq!(counts, (0, 0));
            (plugin, core, directions, bins, input, silence, output)
        })
        .join()
        .unwrap();
        drop(resources);
    }
}

fn plugin(rate: u32, algorithm: usize) -> BeamformerPlugin {
    let mut plugin = BeamformerPlugin::from_params(
        rate,
        BeamformerPluginParams {
            num_mics: 2,
            beamformer_type: algorithm,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn identity_response(
    plugin: &mut BeamformerPlugin,
    source: &[f32],
    rate: u32,
    callback: usize,
) -> Vec<f32> {
    let total = source.len() + 768;
    let mut input = vec![0.0; total * 2];
    for (frame, &sample) in source.iter().enumerate() {
        input[frame * 2..frame * 2 + 2].fill(sample);
    }
    let mut output = vec![0.0; total];
    for start in (0..total).step_by(callback) {
        let end = (start + callback).min(total);
        assert_eq!(
            plugin
                .process(
                    &input[start * 2..end * 2],
                    &mut output[start..end],
                    &ProcessContext::new(rate, end - start)
                )
                .unwrap(),
            end - start
        );
    }
    output
}

#[test]
fn startup_dense_identity_preserves_every_sample_and_reset() {
    let source: Vec<f32> = (0..1609).map(|n| 0.125 + (n % 29) as f32 / 128.0).collect();
    for algorithm in 0..3 {
        let latency = if algorithm == 2 { 0 } else { 512 };
        for rate in [44_100, 48_000, 96_000] {
            let mut plugin = plugin(rate, algorithm);
            assert_eq!(plugin.latency_samples(), latency);
            for callback in [1, 17, 127, 513] {
                for _ in 0..2 {
                    plugin.reset();
                    let output = identity_response(&mut plugin, &source, rate, callback);
                    for (frame, sample) in output.iter().enumerate() {
                        let expected = frame
                            .checked_sub(latency)
                            .and_then(|n| source.get(n))
                            .copied()
                            .unwrap_or(0.0);
                        assert!(
                            (sample - expected).abs() < 2e-6,
                            "algorithm={algorithm},rate={rate},callback={callback},frame={frame}: {sample} != {expected}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn startup_impulse_at_every_hop_phase_preserves_amplitude() {
    for algorithm in 0..3 {
        let latency = if algorithm == 2 { 0 } else { 512 };
        for rate in [44_100, 48_000, 96_000] {
            let mut plugin = plugin(rate, algorithm);
            for callback in [1, 73, 1024] {
                for phase in 0..256 {
                    plugin.reset();
                    let mut source = vec![0.0; phase + 1];
                    source[phase] = 0.5;
                    let output = identity_response(&mut plugin, &source, rate, callback);
                    for (frame, sample) in output.iter().enumerate() {
                        let expected = if frame == phase + latency { 0.5 } else { 0.0 };
                        assert!(
                            (sample - expected).abs() < 2e-6,
                            "algorithm={algorithm},rate={rate},callback={callback},phase={phase},frame={frame}: {sample} != {expected}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn first_callback_and_reset_are_allocation_free_on_fresh_threads() {
    for algorithm in 0..3 {
        let plugin = plugin(48_000, algorithm);
        let input = vec![0.25; 1609 * 2];
        let output = vec![0.0; 1609];
        let plugin = std::thread::spawn(move || {
            let mut plugin = plugin;
            let mut output = output;
            sotf_host::assert_no_allocs("Beamformer cold startup/reset", || {
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 1609))
                    .unwrap();
                plugin.reset();
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, 1609))
                    .unwrap();
            });
            plugin
        })
        .join()
        .unwrap();
        drop(plugin);
    }
}

#[test]
fn eos_retains_the_final_identity_sample() {
    for algorithm in 0..2 {
        let mut plugin = plugin(48_000, algorithm);
        let mut input = vec![0.0; 997 * 2];
        input[996 * 2..].fill(0.5);
        let mut output = vec![0.0; 997];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 997))
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
            output.len() > 996 + 512,
            "tail ended before the final delayed input"
        );
        assert!((output[996 + 512] - 0.5).abs() < 2e-6);
    }
}

#[test]
fn drain_validation_is_transactional_and_reset_restores_continuation() {
    for algorithm in 0..3 {
        let make = || plugin(48_000, algorithm);
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
                            sotf_host::parameters::ParameterId::from("steer_angle_deg"),
                            sotf_host::parameters::ParameterValue::Float(0.0)
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
    for algorithm in 0..3 {
        let make = || plugin(48_000, algorithm);
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
            let tail_bound = plugin.tail_length();
            let (plugin, counts) = std::thread::spawn(move || {
                let mut scratch = [0.0; 256];
                let counts = callback_counts(|| {
                    for reset in [false, true] {
                        assert_eq!(plugin.tail_length(), tail_bound);
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
                            assert_eq!(plugin.tail_length(), tail_bound);
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

#[test]
fn cold_extreme_gsc_and_quiet_mvdr_callbacks_do_not_allocate_or_free() {
    for mics in [4, 8] {
        for algorithm in [0, 2] {
            let mut candidate = BeamformerPlugin::from_params(
                48_000,
                BeamformerPluginParams {
                    num_mics: mics,
                    beamformer_type: algorithm,
                    ..Default::default()
                },
            )
            .unwrap();
            candidate.initialize(48_000.0).unwrap();
            let mut hot = vec![0.0; 256 * mics];
            if algorithm == 2 {
                for frame in hot.chunks_exact_mut(mics) {
                    frame.fill(-f32::MAX);
                    frame[0] = f32::MAX;
                }
            }
            let zeros = vec![0.0; 256 * mics];
            let (candidate, counts) = std::thread::spawn(move || {
                let mut output = [0.0; 256];
                let context = ProcessContext::new(48_000, 256);
                let counts = callback_counts(|| {
                    for _ in 0..2 {
                        candidate.process(&hot, &mut output, &context).unwrap();
                        assert!(output.iter().all(|x| x.is_finite()));
                        // The first quiet MVDR solve at the overflow scale is
                        // measured too; no warmed numerical branch is skipped.
                        for _ in 0..1100 {
                            candidate.process(&zeros, &mut output, &context).unwrap();
                            assert!(output.iter().all(|x| x.is_finite()));
                        }
                        candidate.reset();
                    }
                });
                (candidate, counts)
            })
            .join()
            .unwrap();
            assert_eq!(counts, (0, 0), "mics={mics}, algorithm={algorithm}");
            drop(candidate);
        }
    }
}
