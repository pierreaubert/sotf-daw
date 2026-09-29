//! Finite FIR output agrees with direct convolution through the last input frame.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use sotf_host::fir_crossover::FirCrossover;
use sotf_host::plugin::{PluginDrainResult, TailLength};
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_crossover::{CrossoverPlugin, CrossoverPluginParams};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static COUNTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

struct CountAlloc;

// SAFETY: Every request is forwarded unchanged to the system allocator. The
// const thread-local counters do not allocate or expose the returned pointers.
unsafe impl GlobalAlloc for CountAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a + 1, d));
            });
        }
        // SAFETY: Forward the caller's valid layout without modification.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNTING.try_with(Cell::get).unwrap_or(false) {
            let _ = COUNTS.try_with(|c| {
                let (a, d) = c.get();
                c.set((a, d + 1));
            });
        }
        // SAFETY: Pointer and layout belong to the matching system allocation.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountAlloc = CountAlloc;

fn make(channels: usize, rate: u32, taps: usize, splits: usize, mode: &str) -> CrossoverPlugin {
    let params = CrossoverPluginParams {
        crossover_type: "LinearPhase".into(),
        frequency: 700.0,
        extra_frequencies: [2500.0, 6500.0][..splits - 1].to_vec(),
        output: mode.into(),
        fir_taps: Some(taps),
        channel_frequencies_hz: Vec::new(),
        channel_modes: Vec::new(),
    };
    let mut plugin = CrossoverPlugin::from_params(channels, &params).unwrap();
    plugin.initialize(rate).unwrap();
    plugin
}

fn convolution(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut result = vec![0.0; a.len() + b.len() - 1];
    for (i, &x) in a.iter().enumerate() {
        for (j, &y) in b.iter().enumerate() {
            result[i + j] += x * y;
        }
    }
    result
}

// Coefficients are data from the published filter design. The oracle uses an
// offline f64 linear convolution, with no production ring or streaming calls.
fn band_impulses(rate: u32, taps: usize, splits: usize) -> Vec<Vec<f64>> {
    let mut remaining = vec![1.0];
    let mut bands = Vec::new();
    for (split, frequency) in [700.0, 2500.0, 6500.0].into_iter().take(splits).enumerate() {
        let design = FirCrossover::<f32>::new(frequency, rate as f32, 1, taps);
        let low: Vec<f64> = design
            .lowpass_coefficients()
            .iter()
            .map(|&x| f64::from(x))
            .collect();
        let mut high: Vec<f64> = low.iter().map(|x| -x).collect();
        high[(taps - 1) / 2] += 1.0;
        let delay = (splits - split - 1) * ((taps - 1) / 2);
        let mut band = vec![0.0; delay];
        band.extend(convolution(&remaining, &low));
        bands.push(band);
        remaining = convolution(&remaining, &high);
    }
    bands.push(remaining);
    bands
}

fn process_partitioned(plugin: &mut CrossoverPlugin, input: &[f32], rate: u32) -> Vec<f32> {
    let channels = plugin.input_channels();
    let output_channels = plugin.output_channels();
    let frames = input.len() / channels;
    let mut output = vec![0.0; frames * output_channels];
    let mut offset = 0;
    for block in [1, 7, 53, 2, 257].into_iter().cycle() {
        let count = block.min(frames - offset);
        if count == 0 {
            break;
        }
        assert_eq!(
            plugin
                .process(
                    &input[offset * channels..(offset + count) * channels],
                    &mut output[offset * output_channels..(offset + count) * output_channels],
                    &ProcessContext::new(rate, count),
                )
                .unwrap(),
            count
        );
        offset += count;
    }
    output
}

fn drain_all(plugin: &mut CrossoverPlugin, rate: u32, capacity: usize) -> Vec<f32> {
    let channels = plugin.output_channels();
    let mut output = Vec::new();
    for _ in 0..4096 {
        let mut buffer = vec![17.0; capacity * channels + 3];
        let result = plugin
            .drain(
                &mut buffer[..capacity * channels],
                &ProcessContext::new(rate, 0),
            )
            .unwrap();
        assert!(result.frames <= capacity && result.frames <= plugin.drain_output_frames_max());
        assert!(
            buffer[result.frames * channels..]
                .iter()
                .all(|&x| x == 17.0)
        );
        output.extend_from_slice(&buffer[..result.frames * channels]);
        if result.complete {
            return output;
        }
    }
    panic!("finite FIR drain did not complete");
}

#[test]
fn finite_fir_matches_direct_convolution_and_delayed_band_sum() {
    // 324 distinct rate/channel/tap/split/mode/stream configurations.
    for rate in [32000, 96000] {
        for channels in [1, 2, 6] {
            for taps in [31, 129] {
                for splits in 1..=3 {
                    let impulses = band_impulses(rate, taps, splits);
                    for mode in ["lowpass", "highpass", "both"] {
                        for frames in [1, 17, 301] {
                            let mut plugin = make(channels, rate, taps, splits, mode);
                            let support = splits * (taps - 1);
                            assert_eq!(plugin.tail_length(), TailLength::Finite(support as u64));
                            let input: Vec<f32> = (0..frames * channels)
                                .map(|i| {
                                    if i / channels == frames - 1 {
                                        0.25 * (i % channels + 1) as f32
                                    } else {
                                        ((i * 17 % 59) as f32 - 29.0) / 80.0
                                    }
                                })
                                .collect();
                            let mut actual = process_partitioned(&mut plugin, &input, rate);
                            actual.extend(drain_all(&mut plugin, rate, [1, 13, 1024][frames % 3]));
                            let out_channels = plugin.output_channels();
                            assert_eq!(actual.len(), (frames + support) * out_channels);
                            for channel in 0..channels {
                                let program: Vec<f64> = input
                                    .chunks_exact(channels)
                                    .map(|frame| f64::from(frame[channel]))
                                    .collect();
                                let selected: Vec<usize> = match mode {
                                    "lowpass" => vec![0],
                                    "highpass" => vec![splits],
                                    _ => (0..=splits).collect(),
                                };
                                for (output_band, &band) in selected.iter().enumerate() {
                                    let reference = convolution(&program, &impulses[band]);
                                    for frame in 0..frames + support {
                                        let expected = reference.get(frame).copied().unwrap_or(0.0);
                                        let observed = f64::from(
                                            actual[frame * out_channels
                                                + output_band * channels
                                                + channel],
                                        );
                                        assert!(
                                            (observed - expected).abs() < 2.0e-6,
                                            "{rate}/{channels}/{taps}/{splits}/{mode}/{frames} frame{frame} band{band}: {observed} vs {expected}"
                                        );
                                    }
                                }
                                if mode == "both" {
                                    let delay = splits * ((taps - 1) / 2);
                                    for frame in 0..frames + support {
                                        let sum: f64 = (0..=splits)
                                            .map(|band| {
                                                f64::from(
                                                    actual[frame * out_channels
                                                        + band * channels
                                                        + channel],
                                                )
                                            })
                                            .sum();
                                        let expected = frame
                                            .checked_sub(delay)
                                            .and_then(|i| program.get(i))
                                            .copied()
                                            .unwrap_or(0.0);
                                        assert!((sum - expected).abs() < 2.0e-6);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn errors_do_not_consume_tail_and_reset_replays_it() {
    let rate = 48000;
    let mut plugin = make(2, rate, 129, 3, "both");
    let mut reference = make(2, rate, 129, 3, "both");
    let input = [0.25, -0.5, 0.125, -0.25];
    let first = process_partitioned(&mut plugin, &input, rate);
    assert_eq!(first, process_partitioned(&mut reference, &input, rate));
    let mut sentinel = [123.0; 16];
    assert!(
        plugin
            .drain(&mut sentinel[..7], &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut sentinel[..9], &ProcessContext::new(rate, 0))
            .is_err()
    );
    assert!(
        plugin
            .drain(&mut sentinel, &ProcessContext::new(44100, 0))
            .is_err()
    );
    assert_eq!(sentinel, [123.0; 16]);
    let tail = drain_all(&mut plugin, rate, 1);
    assert_eq!(tail, drain_all(&mut reference, rate, 333));
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(rate, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert!(
        plugin
            .process(&input, &mut [0.0; 16], &ProcessContext::new(rate, 2))
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(
                ParameterId::from("mode"),
                ParameterValue::String("lowpass".into())
            )
            .is_err()
    );
    plugin.reset();
    assert_eq!(first, process_partitioned(&mut plugin, &input, rate));
    assert_eq!(tail, drain_all(&mut plugin, rate, 13));
    plugin.initialize(rate).unwrap();
    assert_eq!(first, process_partitioned(&mut plugin, &input, rate));
    assert_eq!(tail, drain_all(&mut plugin, rate, 257));
}

#[test]
fn empty_drain_is_noop_and_iir_does_not_claim_finite_support() {
    let mut plugin = make(1, 48000, 31, 1, "lowpass");
    assert_eq!(
        plugin
            .drain(&mut [], &ProcessContext::new(48000, 0))
            .unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert_eq!(process_partitioned(&mut plugin, &[1.0], 48000).len(), 1);
    assert_eq!(drain_all(&mut plugin, 48000, 3).len(), 30);
    let mut iir = CrossoverPlugin::new(1, "LR24", 1000.0, "lowpass").unwrap();
    iir.initialize(48000).unwrap();
    assert_eq!(iir.tail_length(), TailLength::Unknown);
    assert_eq!(iir.drain_output_frames_max(), 0);
}

#[test]
fn cold_process_drain_and_reset_allocate_and_free_nothing() {
    for channels in [1, 2, 6] {
        for splits in 1..=3 {
            for cold_drain in [false, true] {
                let mut plugin = make(channels, 48000, 129, splits, "both");
                let input = vec![0.25; 17003 * channels];
                let mut output = vec![0.0; 17003 * plugin.output_channels()];
                let mut tail = vec![123.0; 17 * plugin.output_channels()];
                if cold_drain {
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(48000, 17003))
                        .unwrap();
                }
                let counts = std::thread::spawn(move || {
                    COUNTING.set(true);
                    if !cold_drain {
                        plugin
                            .process(&input, &mut output, &ProcessContext::new(48000, 17003))
                            .unwrap();
                    }
                    loop {
                        if plugin
                            .drain(&mut tail, &ProcessContext::new(48000, 0))
                            .unwrap()
                            .complete
                        {
                            break;
                        }
                    }
                    plugin.reset();
                    COUNTING.set(false);
                    COUNTS.get()
                })
                .join()
                .unwrap();
                assert_eq!(
                    counts,
                    (0, 0),
                    "channels={channels}, splits={splits}, cold_drain={cold_drain}"
                );
            }
        }
    }
}
