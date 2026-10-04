//! Independent startup and accepted-input timeline regressions.
// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

fn make(fft_size: usize, rate: u32) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size,
            bypass_xtc_filters: true,
            auto_gain_enabled: false,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn render(plugin: &mut XtcPlugin, rate: u32, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    let mut output = vec![987.0; input.len() + 4];
    let frames = input.len() / 2;
    let mut position = 0;
    for &chunk in chunks.iter().cycle() {
        let n = chunk.min(frames - position);
        if n == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[position * 2..(position + n) * 2],
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

fn assert_delayed(actual: &[f32], source: &[f32], delay: usize) {
    assert_eq!(actual.len(), (source.len() / 2 + delay) * 2);
    assert!(
        actual[..delay * 2].iter().all(|value| *value == 0.0),
        "startup must contain exactly declared zero padding"
    );
    for (index, (&value, &expected)) in actual[delay * 2..].iter().zip(source).enumerate() {
        assert!(
            (value - expected).abs() < 1.5e-6,
            "frame {} channel {}: actual {value}, expected {expected}",
            index / 2,
            index % 2
        );
    }
}

#[test]
fn neutral_dense_waveform_has_fixed_delay_across_all_sizes_rates_and_partitions() {
    for fft_size in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let mut plugin = make(fft_size, rate);
            assert_eq!(plugin.latency_samples(), fft_size);
            // Cover every startup/hop phase and more than one complete ring wrap.
            let source: Vec<f32> = (0..(fft_size * 5 + 731) * 2)
                .map(|i| (((i * 23 + i / 2 * 7) % 127) as i32 - 63) as f32 / 256.0)
                .collect();
            let mut input = source.clone();
            input.resize(source.len() + fft_size * 2, 0.0);
            for chunks in [
                &[1][..],
                &[17, 137, 512][..],
                &[fft_size][..],
                &[4096, 137][..],
                &[8193][..],
            ] {
                plugin.reset();
                let actual = render(&mut plugin, rate, &input, chunks);
                assert_delayed(&actual, &source, fft_size);
            }
        }
    }
}

#[test]
fn mixed_callback_schedule_preserves_the_previously_discarded_interval() {
    let mut plugin = make(2048, 48_000);
    let mut source = vec![0.0; 7000 * 2];
    for frame in 4607..=4644 {
        source[frame * 2] = 0.25;
        source[frame * 2 + 1] = -0.125;
    }
    let mut input = source.clone();
    input.resize(source.len() + 2048 * 2, 0.0);
    let mut output = vec![987.0; input.len()];
    plugin
        .process(
            &input[..4096 * 2],
            &mut output[..4096 * 2],
            &ProcessContext::new(48_000, 4096),
        )
        .unwrap();
    output[4096 * 2..].copy_from_slice(&render(&mut plugin, 48_000, &input[4096 * 2..], &[137]));
    assert_delayed(&output, &source, 2048);
}

#[test]
fn invalid_callbacks_are_transactional_and_initialize_restores_the_clock() {
    let rate = 48_000;
    let mut actual = make(512, rate);
    let mut reference = make(512, rate);
    let prefix = vec![0.125; 137 * 2];
    assert_eq!(
        render(&mut actual, rate, &prefix, &[17]),
        render(&mut reference, rate, &prefix, &[17])
    );
    let input = [0.25; 32];
    for (input_len, output_len, context) in [
        (32, 32, ProcessContext::new(44_100, 16)),
        (31, 32, ProcessContext::new(rate, 16)),
        (32, 31, ProcessContext::new(rate, 16)),
        (0, 0, ProcessContext::new(rate, usize::MAX)),
    ] {
        let mut output = [999.0; 35];
        assert!(
            actual
                .process(&input[..input_len], &mut output[..output_len], &context)
                .is_err()
        );
        assert_eq!(output, [999.0; 35]);
    }
    let suffix = vec![-0.0625; 7000 * 2];
    assert_eq!(
        render(&mut actual, rate, &suffix, &[137]),
        render(&mut reference, rate, &suffix, &[137])
    );
    actual.initialize(96_000.0).unwrap();
    let mut fresh = make(512, 96_000);
    assert_eq!(
        render(&mut actual, 96_000, &suffix, &[17]),
        render(&mut fresh, 96_000, &suffix, &[17])
    );
    actual.reset();
    assert!(actual.initialize(0.0).is_err());
    fresh.reset();
    assert_eq!(
        render(&mut actual, 96_000, &suffix, &[137]),
        render(&mut fresh, 96_000, &suffix, &[137])
    );
}

#[test]
fn startup_impulses_keep_every_small_hop_phase_and_large_window_boundaries() {
    for fft_size in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        let mut plugin = make(fft_size, 48_000);
        let hop = fft_size / 4;
        let positions: Vec<usize> = if fft_size <= 512 {
            (0..hop).collect()
        } else {
            vec![0, 1, hop - 1, hop, fft_size - 1, fft_size, fft_size + 1]
        };
        for marker in positions {
            plugin.reset();
            let mut source = vec![0.0; (fft_size + 17) * 2];
            source[marker * 2] = 0.25;
            source[marker * 2 + 1] = -0.125;
            let mut input = source.clone();
            input.resize(source.len() + fft_size * 2, 0.0);
            assert_delayed(
                &render(&mut plugin, 48_000, &input, &[17, 137]),
                &source,
                fft_size,
            );
        }
    }
}

#[test]
fn disabled_direct_routing_preserves_exact_values_at_the_declared_delay() {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            enabled: false,
            ..Default::default()
        },
        48_000,
    )
    .unwrap();
    plugin.initialize(48_000.0).unwrap();
    let input: Vec<f32> = (0..20_000).map(|i| (i % 31) as f32 / 64.0).collect();
    let delay = plugin.latency_samples();
    let mut padded = input.clone();
    padded.resize(input.len() + delay * 2, 0.0);
    let output = render(&mut plugin, 48_000, &padded, &[1, 137, 8193]);
    assert_eq!(&output[..delay * 2], vec![0.0; delay * 2]);
    assert_eq!(&output[delay * 2..], &input);
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
fn cold_default_and_neutral_clocks_reset_without_allocating_or_freeing() {
    for fft_size in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        for neutral in [false, true] {
            for rate in [44_100, 192_000] {
                let mut plugin = XtcPlugin::new(
                    XtcPluginParams {
                        fft_size,
                        bypass_xtc_filters: neutral,
                        auto_gain_enabled: !neutral,
                        ..Default::default()
                    },
                    rate,
                )
                .unwrap();
                plugin.initialize(f64::from(rate)).unwrap();
                // Exceed prepared staging and cover all initial windows/ring wrap.
                let frames = (fft_size * 5 + 731).max(20_003);
                let input = vec![0.125; frames * 2];
                let mut output = vec![0.0; input.len()];
                let counts = std::thread::spawn(move || {
                    heap::measure(|| {
                        plugin
                            .process(&input, &mut output, &ProcessContext::new(rate, frames))
                            .unwrap();
                        plugin.reset();
                        for start in (0..frames).step_by(137) {
                            let n = 137.min(frames - start);
                            plugin
                                .process(
                                    &input[start * 2..(start + n) * 2],
                                    &mut output[start * 2..(start + n) * 2],
                                    &ProcessContext::new(rate, n),
                                )
                                .unwrap();
                        }
                        assert!(output.iter().all(|v| v.is_finite()));
                    })
                })
                .join()
                .unwrap();
                assert_eq!(
                    counts,
                    (0, 0),
                    "N={fft_size}, rate={rate}, neutral={neutral}"
                );
            }
        }
    }
}
