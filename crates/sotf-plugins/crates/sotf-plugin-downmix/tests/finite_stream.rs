//! Independent finite-support, delayed-waveform and lifecycle checks.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::TailLength;
use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_downmix::{DownmixPlugin, DownmixPluginParams};

const N: usize = 2048;
const H: usize = N / 2;
const RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Simple,
    Phase,
    LtRt,
}

fn make(layout: &str, channels: usize, mode: Mode, itu: bool) -> DownmixPlugin {
    let params: DownmixPluginParams = serde_json::from_value(serde_json::json!({
        "input_channels": channels,
        "input_layout": layout,
        "phase_coherence": matches!(mode, Mode::Phase),
        "matrix_ltrt": matches!(mode, Mode::LtRt),
        "itu_mode": itu,
        "lfe_gain_db": 0.0,
    }))
    .unwrap();
    DownmixPlugin::try_from_params(params).unwrap()
}

fn render(plugin: &mut DownmixPlugin, rate: u32, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    let channels = plugin.input_channels();
    let frames = input.len() / channels;
    let mut output = vec![0.0; frames * 2];
    let mut position = 0;
    for &chunk in chunks.iter().cycle() {
        let n = chunk.min(frames - position);
        if n == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[position * channels..(position + n) * channels],
                    &mut output[position * 2..(position + n) * 2],
                    &ProcessContext::new(rate, n),
                )
                .unwrap(),
            n,
        );
        position += n;
    }
    output
}

fn finish(plugin: &mut DownmixPlugin, rate: u32, capacities: &[usize]) -> Vec<f32> {
    let mut output = Vec::new();
    for &capacity in capacities.iter().cycle().take(N * 3) {
        let mut block = vec![987.0; capacity * 2];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(result.frames <= capacity.min(H));
        assert!(block[result.frames * 2..].iter().all(|&v| v == 987.0));
        output.extend_from_slice(&block[..result.frames * 2]);
        if result.complete {
            assert!(
                plugin
                    .drain(&mut [], &ProcessContext::new(rate, 0))
                    .unwrap()
                    .complete
            );
            return output;
        }
        assert!(result.frames > 0, "finite Downmix must make progress");
    }
    panic!("finite Downmix did not complete");
}

fn tail_frames(source: usize) -> usize {
    // Independent absolute endpoint: last occupied window's origin + D + N.
    N + ((source - 1) / H) * H + N - source
}

fn assert_close(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        assert!((a - e).abs() < 2e-6, "sample {index}: {a} != {e}");
    }
}

#[test]
fn first_and_final_markers_survive_every_final_hop_phase() {
    for mode in [Mode::Phase, Mode::LtRt] {
        let mut plugin = make("2.0", 2, mode, false);
        plugin.initialize(f64::from(RATE)).unwrap();
        assert_eq!(plugin.latency_samples(), N);
        for source in 1..=H {
            plugin.reset();
            let mut input = vec![0.0; source * 2];
            input[0] = 0.25;
            input[(source - 1) * 2 + 1] = -0.5;
            let mut actual = render(&mut plugin, RATE, &input, &[17, 1, 137]);
            actual.extend(finish(&mut plugin, RATE, &[1, 17, 257, 4097]));
            let mut expected = vec![0.0; (source + tail_frames(source)) * 2];
            expected[N * 2] = 0.25;
            expected[(N + source - 1) * 2 + 1] = -0.5;
            assert_close(&actual, &expected);
        }
    }
}

#[test]
fn dense_front_channels_have_exact_delay_and_endpoint_across_layouts() {
    for (layout, channels) in [("2.0", 2), ("5.0", 5), ("5.1", 6), ("7.1.4", 12)] {
        for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
            for rate in [44_100, RATE, 96_000, 192_000] {
                for chunks in [&[1][..], &[137, 4096, 17][..], &[8193][..]] {
                    let mut plugin = make(layout, channels, mode, true);
                    plugin.initialize(f64::from(rate)).unwrap();
                    let frames = N * 3 + 73;
                    let delay = if matches!(mode, Mode::Simple) { 0 } else { N };
                    let tail = if delay == 0 { 0 } else { tail_frames(frames) };
                    let mut input = vec![0.0; frames * channels];
                    let mut expected = vec![0.0; (frames + tail) * 2];
                    for frame in 0..frames {
                        let l = ((frame * 23 % 127) as i32 - 63) as f32 / 256.0;
                        let r = ((frame * 17 % 63) as i32 - 31) as f32 / 256.0;
                        input[frame * channels] = l;
                        input[frame * channels + 1] = r;
                        expected[(frame + delay) * 2] = l;
                        expected[(frame + delay) * 2 + 1] = r;
                    }
                    let mut actual = render(&mut plugin, rate, &input, chunks);
                    actual.extend(finish(&mut plugin, rate, &[1, 137, 4096]));
                    assert_close(&actual, &expected);
                }
            }
        }
    }
}

#[test]
fn nontrivial_spectral_tail_matches_ordinary_zero_continuation() {
    for mode in [Mode::Phase, Mode::LtRt] {
        for frames in [1, H - 1, H, H + 1, N - 1, N, N + 1, N * 4 + 73] {
            for capacities in [&[1][..], &[17, 257, 4096][..]] {
                let mut plugin = make("5.1", 6, mode, true);
                let mut reference = make("5.1", 6, mode, true);
                plugin.initialize(f64::from(RATE)).unwrap();
                reference.initialize(f64::from(RATE)).unwrap();
                let input: Vec<f32> = (0..frames * 6)
                    .map(|i| ((i * 37 % 127) as i32 - 63) as f32 / 512.0)
                    .collect();
                assert_close(
                    &render(&mut plugin, RATE, &input, &[137, 17, 1024]),
                    &render(&mut reference, RATE, &input, &[4096]),
                );
                // Keep a live non-LFE coefficient ramp through the final windows.
                for p in [&mut plugin, &mut reference] {
                    p.set_parameter("surround_gain_db".into(), ParameterValue::Float(-9.0))
                        .unwrap();
                }
                let actual = finish(&mut plugin, RATE, capacities);
                assert_eq!(actual.len(), tail_frames(frames) * 2);
                let expected = render(
                    &mut reference,
                    RATE,
                    &vec![0.0; (tail_frames(frames) + N * 2) * 6],
                    &[1, 479, 1024],
                );
                assert_close(&actual, &expected[..actual.len()]);
                assert!(expected[actual.len()..].iter().all(|&v| v == 0.0));
            }
        }
    }
}

#[test]
fn recursive_lfe_is_eligible_only_when_unobservable() {
    for mode in [Mode::Simple, Mode::Phase] {
        let mut plugin = make("5.1", 6, mode, false);
        plugin.initialize(f64::from(RATE)).unwrap();
        assert_eq!(plugin.tail_length(), TailLength::Infinite);
        let input = vec![0.125; H * 6];
        render(&mut plugin, RATE, &input, &[137]);
        assert!(
            plugin
                .drain(&mut [], &ProcessContext::new(RATE, 0))
                .unwrap()
                .complete
        );
        plugin
            .set_parameter("itu_mode".into(), ParameterValue::Bool(true))
            .unwrap();
        assert_eq!(
            plugin.tail_length(),
            TailLength::Infinite,
            "a target alone cannot silence the current LFE gain"
        );
        for _ in 0..256 {
            if plugin.tail_length() != TailLength::Infinite {
                break;
            }
            render(&mut plugin, RATE, &vec![0.0; H * 6], &[H]);
        }
        let support = if matches!(mode, Mode::Simple) {
            0
        } else {
            N * 2 - 1
        };
        assert_eq!(plugin.tail_length(), TailLength::Finite(support as u64));
        finish(&mut plugin, RATE, &[257]);
    }
    // Lt/Rt explicitly discards LFE even with a nonzero gain/filter state.
    let mut plugin = make("5.1", 6, Mode::LtRt, false);
    plugin.initialize(f64::from(RATE)).unwrap();
    let mut input = vec![0.0; (N + 73) * 6];
    input[3] = 1.0;
    let mut output = render(&mut plugin, RATE, &input, &[137]);
    assert_eq!(plugin.tail_length(), TailLength::Finite((N * 2 - 1) as u64));
    output.extend(finish(&mut plugin, RATE, &[17]));
    assert!(output.iter().all(|&v| v == 0.0));
}

#[test]
fn drain_validation_and_frozen_controls_preserve_the_audio() {
    let mut plugin = make("2.0", 2, Mode::Phase, false);
    let mut canary = [987.0; 8];
    assert!(
        plugin
            .drain(&mut canary, &ProcessContext::new(RATE, 0))
            .is_err()
    );
    assert_eq!(canary, [987.0; 8]);
    plugin.initialize(f64::from(RATE)).unwrap();
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .unwrap()
            .complete
    );
    let mut reference = make("2.0", 2, Mode::Phase, false);
    reference.initialize(f64::from(RATE)).unwrap();
    let input: Vec<f32> = (0..(N + 73) * 2)
        .map(|i| ((i % 31) as f32 - 15.0) / 64.0)
        .collect();
    render(&mut plugin, RATE, &input, &[137]);
    render(&mut reference, RATE, &input, &[137]);
    assert!(
        plugin
            .drain(&mut [], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary[..3], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut canary, &ProcessContext::new(44_100, 0))
            .is_err()
    );
    assert!(plugin.initialize(0.0).is_err());
    assert_eq!(canary, [987.0; 8]);
    let first = plugin
        .drain(&mut canary, &ProcessContext::new(RATE, 0))
        .unwrap();
    let mut expected = [0.0; 8];
    assert_eq!(
        reference
            .drain(&mut expected, &ProcessContext::new(RATE, 0))
            .unwrap(),
        first
    );
    assert_eq!(canary, expected);
    let current = plugin.get_parameter(&"center_gain_db".into()).unwrap();
    plugin
        .set_parameter("center_gain_db".into(), current)
        .unwrap();
    assert!(
        plugin
            .set_parameter("center_gain_db".into(), ParameterValue::Float(-9.0))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter("unknown".into(), ParameterValue::Bool(false))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter("center_gain_db".into(), ParameterValue::Bool(false))
            .is_err()
    );
    let mut rejected = [987.0; 2];
    assert!(
        plugin
            .process(&[0.25, 0.5], &mut rejected, &ProcessContext::new(RATE, 1))
            .is_err()
    );
    assert_eq!(rejected, [987.0; 2]);
    assert!(
        plugin
            .drain(&mut rejected[..1], &ProcessContext::new(RATE, 0))
            .is_err()
    );
    assert_close(
        &finish(&mut plugin, RATE, &[257]),
        &finish(&mut reference, RATE, &[1, 4096]),
    );
    assert!(
        plugin
            .process(&[], &mut [], &ProcessContext::new(RATE, 0))
            .is_ok()
    );
}

#[test]
fn newly_settled_itu_keeps_prior_analysis_and_overlap_output() {
    let mut plugin = make("5.1", 6, Mode::Phase, false);
    let mut reference = make("5.1", 6, Mode::Phase, false);
    plugin.initialize(f64::from(RATE)).unwrap();
    reference.initialize(f64::from(RATE)).unwrap();
    let frames = N * 2 + 13;
    let input: Vec<f32> = (0..frames * 6)
        .map(|i| ((i * 31 % 127) as i32 - 63) as f32 / 256.0)
        .collect();
    for p in [&mut plugin, &mut reference] {
        render(p, RATE, &input, &[137]);
        p.set_parameter("itu_mode".into(), ParameterValue::Bool(true))
            .unwrap();
    }
    let mut total = frames;
    for _ in 0..256 {
        if plugin.tail_length() != TailLength::Infinite {
            break;
        }
        // Keep exciting the recursive LFE while its gain moves toward zero.
        let mut block = vec![0.0; H * 6];
        block[3] = 0.25;
        assert_eq!(
            render(&mut plugin, RATE, &block, &[H]),
            render(&mut reference, RATE, &block, &[H])
        );
        total += H;
    }
    assert_eq!(plugin.tail_length(), TailLength::Finite(4095));
    let actual = finish(&mut plugin, RATE, &[17, 257]);
    assert_eq!(actual.len(), tail_frames(total) * 2);
    let expected = render(
        &mut reference,
        RATE,
        &vec![0.0; (tail_frames(total) + N * 2) * 6],
        &[137],
    );
    assert_close(&actual, &expected[..actual.len()]);
    assert!(expected[actual.len()..].iter().all(|&v| v == 0.0));
}

#[test]
fn reset_and_reinitialize_rearm_finite_streams() {
    for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
        for reinitialize in [false, true] {
            let mut plugin = make("5.1", 6, mode, true);
            plugin.initialize(f64::from(RATE)).unwrap();
            render(&mut plugin, RATE, &vec![0.25; (N + 73) * 6], &[137]);
            finish(&mut plugin, RATE, &[257]);
            assert!(
                plugin
                    .process(&[0.0; 6], &mut [987.0; 2], &ProcessContext::new(RATE, 1))
                    .is_err()
            );
            if reinitialize {
                plugin.initialize(f64::from(RATE)).unwrap();
            } else {
                plugin.reset();
            }
            let mut fresh = make("5.1", 6, mode, true);
            fresh.initialize(f64::from(RATE)).unwrap();
            let input: Vec<f32> = (0..(N + 11) * 6)
                .map(|i| ((i % 63) as f32 - 31.0) / 128.0)
                .collect();
            assert_eq!(
                render(&mut plugin, RATE, &input, &[17, 137]),
                render(&mut fresh, RATE, &input, &[17, 137])
            );
            assert_eq!(
                finish(&mut plugin, RATE, &[257]),
                finish(&mut fresh, RATE, &[257])
            );
        }
    }
}

#[test]
fn declared_call_bound_covers_full_capacity_and_partially_served_tails() {
    for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
        for source in [1, H - 1, H, H + 1, N - 1, N, N + 1] {
            for partial in [false, true] {
                let mut plugin = make("5.1", 6, mode, true);
                plugin.initialize(f64::from(RATE)).unwrap();
                render(&mut plugin, RATE, &vec![0.125; source * 6], &[137]);
                if partial {
                    plugin
                        .drain(&mut [0.0; 34], &ProcessContext::new(RATE, 0))
                        .unwrap();
                }
                let bound = plugin.drain_call_bound().unwrap().get();
                let mut calls = 0;
                loop {
                    calls += 1;
                    assert!(calls <= bound);
                    let maximum = plugin.drain_output_frames_max();
                    let result = plugin
                        .drain(&mut vec![0.0; maximum * 2], &ProcessContext::new(RATE, 0))
                        .unwrap();
                    if result.complete {
                        break;
                    }
                }
                assert_eq!(calls, bound, "native full-capacity advance is exact");
            }
        }
    }
}

mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    thread_local! {
        static TRACK: Cell<bool> = const { Cell::new(false) };
        static ALLOC: Cell<usize> = const { Cell::new(0) };
        static FREE: Cell<usize> = const { Cell::new(0) };
    }
    struct Allocator;
    // SAFETY: This observer forwards allocation ownership unchanged to System.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                ALLOC.with(|count| count.set(count.get() + 1));
            }
            // SAFETY: The caller's valid layout is forwarded unchanged.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                FREE.with(|count| count.set(count.get() + 1));
            }
            // SAFETY: The caller's original pointer and allocation layout are preserved.
            unsafe { System.dealloc(pointer, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Allocator = Allocator;

    pub fn measure(run: impl FnOnce()) -> (usize, usize) {
        ALLOC.set(0);
        FREE.set(0);
        TRACK.set(true);
        run();
        TRACK.set(false);
        (ALLOC.get(), FREE.get())
    }
}

#[test]
fn cold_first_final_drain_metadata_and_reset_do_not_allocate_or_free() {
    for (layout, channels) in [("1.0", 1), ("2.0", 2), ("5.1", 6), ("9.1.6", 16)] {
        for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
            for rate in [44_100, 192_000] {
                let mut plugin = make(layout, channels, mode, true);
                plugin.initialize(f64::from(rate)).unwrap();
                let input = vec![0.125; (N + 73) * channels];
                render(&mut plugin, rate, &input, &[137]);
                let mut output = vec![0.0; (N + 73) * 2];
                let counts = std::thread::spawn(move || {
                    heap::measure(|| {
                        for _ in 0..2 {
                            let mut complete = false;
                            for _ in 0..N {
                                let _ = plugin.tail_length();
                                let _ = plugin.drain_output_frames_max();
                                assert!(plugin.drain_call_bound().is_some());
                                let result = plugin
                                    .drain(&mut output[..34], &ProcessContext::new(rate, 0))
                                    .unwrap();
                                if result.complete {
                                    complete = true;
                                    break;
                                }
                            }
                            assert!(complete);
                            plugin.reset();
                            plugin
                                .process(&input, &mut output, &ProcessContext::new(rate, N + 73))
                                .unwrap();
                        }
                    })
                })
                .join()
                .unwrap();
                assert_eq!(
                    counts,
                    (0, 0),
                    "layout={layout}, mode={mode:?}, rate={rate}"
                );
            }
        }
    }
}

#[test]
fn invalid_ordinary_callbacks_preserve_output_and_prepared_audio() {
    for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
        let mut plugin = make("5.1", 6, mode, true);
        let mut sentinel = [987.0; 2];
        // Constructor-time processing uses the documented default 44.1 kHz.
        // A different context must not silently retime its configured filters.
        assert!(
            plugin
                .process(&[0.125; 6], &mut sentinel, &ProcessContext::new(RATE, 1))
                .is_err()
        );
        assert_eq!(sentinel, [987.0; 2]);
        plugin.initialize(f64::from(RATE)).unwrap();
        let mut reference = make("5.1", 6, mode, true);
        reference.initialize(f64::from(RATE)).unwrap();
        let input: Vec<f32> = (0..(N + 73) * 6)
            .map(|i| ((i % 31) as f32 - 15.0) / 64.0)
            .collect();
        assert_eq!(
            render(&mut plugin, RATE, &input, &[137]),
            render(&mut reference, RATE, &input, &[137])
        );
        assert!(
            plugin
                .process(&[0.125; 6], &mut sentinel, &ProcessContext::new(44_100, 1))
                .is_err()
        );
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut block = [0.125; 6];
            block[3] = invalid; // Also reject nonfinite LFE discarded by this mode.
            assert!(
                plugin
                    .process(&block, &mut sentinel, &ProcessContext::new(RATE, 1))
                    .is_err()
            );
        }
        assert_eq!(sentinel, [987.0; 2]);
        assert_eq!(
            render(&mut plugin, RATE, &input, &[17]),
            render(&mut reference, RATE, &input, &[17])
        );
        assert_eq!(
            finish(&mut plugin, RATE, &[257]),
            finish(&mut reference, RATE, &[257])
        );
    }
}

#[test]
fn minimal_oversampled_setup_prepares_the_future_spectral_drain_capacity() {
    for factor in [2, 4] {
        let inner = make("2.0", 2, Mode::Phase, false);
        let mut plugin =
            sotf_host::AutoOversampledPlugin::new_with_max_frames(Box::new(inner), factor, 1)
                .unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin
            .process(
                &[0.25, -0.125],
                &mut [0.0; 2],
                &ProcessContext::new(RATE, 1),
            )
            .unwrap();
        let mut output = vec![0.0; plugin.drain_output_frames_max() * 2];
        let counts = std::thread::spawn(move || {
            heap::measure(|| {
                plugin.begin_drain(&ProcessContext::new(RATE, 0)).unwrap();
                let bound = plugin.drain_call_bound().unwrap().get();
                let mut completed = false;
                let mut audible = false;
                for _ in 0..bound {
                    let result = plugin
                        .drain(&mut output, &ProcessContext::new(RATE, 0))
                        .unwrap();
                    audible |= output[..result.frames * 2].iter().any(|v| v.abs() > 1e-6);
                    if result.complete {
                        completed = true;
                        break;
                    }
                }
                assert!(completed);
                assert!(
                    audible,
                    "the retained impulse must leave the wrapped plugin"
                );
            })
        })
        .join()
        .unwrap();
        assert_eq!(counts, (0, 0), "factor={factor}");
    }
}
