//! Independent negative-origin window and stream-timing regressions.
// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_downmix::{DownmixPlugin, DownmixPluginParams};

const N: usize = 2048;
const H: usize = N / 2;
const LAYOUTS: [(&str, usize); 12] = [
    ("1.0", 1),
    ("2.0", 2),
    ("2.1", 3),
    ("5.0", 5),
    ("5.1", 6),
    ("7.1", 8),
    ("5.1.2", 8),
    ("5.1.4", 10),
    ("7.1.2", 10),
    ("7.1.4", 12),
    ("9.1.4", 14),
    ("9.1.6", 16),
];

#[derive(Clone, Copy, Debug)]
enum Mode {
    Simple,
    Phase,
    LtRt,
}
fn make(layout: &str, channels: usize, mode: Mode) -> DownmixPlugin {
    let params: DownmixPluginParams = serde_json::from_value(serde_json::json!({
        "input_channels": channels,
        "input_layout": layout,
        "center_gain_db": 0.0,
        "phase_coherence": matches!(mode, Mode::Phase),
        "matrix_ltrt": matches!(mode, Mode::LtRt),
    }))
    .unwrap();
    DownmixPlugin::try_from_params(params).unwrap()
}

fn render(plugin: &mut DownmixPlugin, rate: u32, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    let channels = plugin.input_channels();
    let frames = input.len() / channels;
    let mut output = vec![987.0; frames * 2 + 4];
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
                    &mut output[2 + position * 2..2 + (position + n) * 2],
                    &ProcessContext::new(rate, n),
                )
                .unwrap(),
            n
        );
        position += n;
    }
    assert_eq!(&output[..2], &[987.0; 2]);
    assert_eq!(&output[output.len() - 2..], &[987.0; 2]);
    output[2..output.len() - 2].to_vec()
}

fn programme(channels: usize, frames: usize, mode: Mode) -> (Vec<f32>, Vec<f32>) {
    let mut input = vec![0.0; frames * channels];
    let mut expected = vec![0.0; frames * 2];
    for frame in 0..frames {
        let left = ((frame * 23 % 127) as i32 - 63) as f32 / 256.0;
        let right = ((frame * 17 % 63) as i32 - 31) as f32 / 256.0;
        input[frame * channels] = left;
        if channels == 1 {
            // Explicit configured center fold-down, independent of plugin state.
            let gain = if matches!(mode, Mode::LtRt) {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                0.707
            };
            expected[frame * 2] = left * gain;
            expected[frame * 2 + 1] = left * gain;
        } else {
            // Every named layout exposes its direct front L/R as channels0/1.
            input[frame * channels + 1] = right;
            expected[frame * 2] = left;
            expected[frame * 2 + 1] = right;
        }
    }
    (input, expected)
}

fn assert_delayed(actual: &[f32], expected: &[f32], delay: usize) {
    assert_eq!(actual.len(), expected.len() + delay * 2);
    assert!(
        actual[..delay * 2].iter().all(|v| *v == 0.0),
        "exact declared startup silence"
    );
    for (i, (a, e)) in actual[delay * 2..].iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() < 1.5e-6,
            "frame {} channel {}: actual {a}, expected {e}",
            i / 2,
            i % 2
        );
    }
}

#[test]
fn dense_front_programme_preserves_startup_and_ring_wrap_for_every_layout() {
    for (layout, channels) in LAYOUTS {
        for mode in [Mode::Phase, Mode::LtRt] {
            for rate in [44_100, 48_000, 96_000, 192_000] {
                let mut plugin = make(layout, channels, mode);
                plugin.initialize(f64::from(rate)).unwrap();
                assert_eq!(plugin.latency_samples(), N);
                let (mut input, expected) = programme(channels, N * 5 + 17, mode);
                input.resize(input.len() + N * channels, 0.0);
                for chunks in [
                    &[1][..],
                    &[17, 137][..],
                    &[N][..],
                    &[4096, 137][..],
                    &[8193][..],
                ] {
                    plugin.reset();
                    assert_delayed(&render(&mut plugin, rate, &input, chunks), &expected, N);
                }
            }
        }
    }
}

#[test]
fn each_initial_hop_phase_and_final_marker_survives() {
    for mode in [Mode::Phase, Mode::LtRt] {
        let mut plugin = make("2.0", 2, mode);
        plugin.initialize(48_000.0).unwrap();
        for marker in 0..H {
            plugin.reset();
            // This marker is also the final programme sample, with only zero
            // continuation supplied afterward to expose the full delayed output.
            let mut source = vec![0.0; (marker + 1) * 2];
            source[marker * 2] = 0.25;
            source[marker * 2 + 1] = -0.125;
            let mut input = source.clone();
            input.resize(source.len() + N * 2, 0.0);
            assert_delayed(
                &render(&mut plugin, 48_000, &input, &[17, 137, 8193]),
                &source,
                N,
            );
        }
    }
}

#[test]
fn constructors_mode_setup_and_reset_establish_the_same_clock() {
    for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
        let mut constructed = make("2.0", 2, mode);
        let mut initialized = make("2.0", 2, mode);
        initialized.initialize(44_100.0).unwrap();
        let delay = if matches!(mode, Mode::Simple) { 0 } else { N };
        let (mut input, expected) = programme(2, N + 17, mode);
        input.resize(input.len() + delay * 2, 0.0);
        // Constructor processing is already a supported Downmix behavior.
        let output = render(&mut constructed, 44_100, &input, &[137]);
        assert_delayed(&output, &expected, delay);
        assert_eq!(output, render(&mut initialized, 44_100, &input, &[137]));
        initialized.reset();
        assert_eq!(output, render(&mut initialized, 44_100, &input, &[8193]));
        initialized.initialize(96_000.0).unwrap();
        let mut fresh = make("2.0", 2, mode);
        fresh.initialize(96_000.0).unwrap();
        assert_eq!(
            render(&mut initialized, 96_000, &input, &[1]),
            render(&mut fresh, 96_000, &input, &[8193])
        );
    }
    let mut defaults = DownmixPlugin::new(2);
    let (mut input, expected) = programme(2, N + 17, Mode::Phase);
    input.resize(input.len() + N * 2, 0.0);
    assert_delayed(&render(&mut defaults, 44_100, &input, &[137]), &expected, N);
}

#[test]
fn setup_mode_changes_clear_the_correct_spectral_prefix() {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    let mut plugin = DownmixPlugin::new(2);
    for mode in [Mode::LtRt, Mode::Simple, Mode::Phase, Mode::LtRt] {
        plugin
            .set_parameter(
                ParameterId::from("phase_coherence"),
                ParameterValue::Bool(false),
            )
            .unwrap();
        plugin
            .set_parameter(
                ParameterId::from("matrix_ltrt"),
                ParameterValue::Bool(false),
            )
            .unwrap();
        if matches!(mode, Mode::Phase) {
            plugin
                .set_parameter(
                    ParameterId::from("phase_coherence"),
                    ParameterValue::Bool(true),
                )
                .unwrap();
        } else if matches!(mode, Mode::LtRt) {
            plugin
                .set_parameter(ParameterId::from("matrix_ltrt"), ParameterValue::Bool(true))
                .unwrap();
        }
        let delay = if matches!(mode, Mode::Simple) { 0 } else { N };
        let (mut input, expected) = programme(2, N + 137, mode);
        input.resize(input.len() + delay * 2, 0.0);
        assert_delayed(
            &render(&mut plugin, 44_100, &input, &[137]),
            &expected,
            delay,
        );
    }
}

#[test]
fn simple_named_layouts_keep_immediate_front_routing() {
    for (layout, channels) in LAYOUTS {
        let mut plugin = make(layout, channels, Mode::Simple);
        plugin.initialize(48_000.0).unwrap();
        let (input, expected) = programme(channels, N + 17, Mode::Simple);
        assert_eq!(plugin.latency_samples(), 0);
        assert_eq!(
            render(&mut plugin, 48_000, &input, &[1, 137, 8193]),
            expected
        );
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
    // SAFETY: This observer preserves System's allocation and ownership contracts.
    unsafe impl GlobalAlloc for Allocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                ALLOC.with(|count| count.set(count.get() + 1));
            }
            // SAFETY: Forwarding the allocator caller's valid layout unchanged.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if TRACK.try_with(Cell::get).unwrap_or(false) {
                FREE.with(|count| count.set(count.get() + 1));
            }
            // SAFETY: Forwarding the original allocation pointer and layout.
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
fn cold_spectral_and_simple_process_reset_do_not_allocate_or_free() {
    for (layout, channels) in LAYOUTS {
        for mode in [Mode::Simple, Mode::Phase, Mode::LtRt] {
            for rate in [44_100, 192_000] {
                let mut plugin = make(layout, channels, mode);
                plugin.initialize(f64::from(rate)).unwrap();
                let frames = N * 5 + 731;
                let input = vec![0.125; frames * channels];
                let mut output = vec![0.0; frames * 2];
                let counts = std::thread::spawn(move || {
                    heap::measure(|| {
                        assert_eq!(
                            plugin
                                .process(&input, &mut output, &ProcessContext::new(rate, frames))
                                .unwrap(),
                            frames
                        );
                        plugin.reset();
                        for start in (0..frames).step_by(137) {
                            let n = 137.min(frames - start);
                            assert_eq!(
                                plugin
                                    .process(
                                        &input[start * channels..(start + n) * channels],
                                        &mut output[start * 2..(start + n) * 2],
                                        &ProcessContext::new(rate, n)
                                    )
                                    .unwrap(),
                                n
                            );
                        }
                        assert!(output.iter().all(|v| v.is_finite()));
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
