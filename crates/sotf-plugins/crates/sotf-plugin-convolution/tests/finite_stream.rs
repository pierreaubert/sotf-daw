//! Native finite convolution tails and cold callback ownership regression tests.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_convolution::{ConvolutionPlugin, ConvolutionPluginParams};
use std::path::PathBuf;

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
fn heap_counts<T>(operation: impl FnOnce() -> T) -> (T, (usize, usize)) {
    COUNTS.with(|counts| counts.set((0, 0)));
    COUNTING.with(|active| active.set(true));
    let guard = StopCounting;
    let value = operation();
    drop(guard);
    (value, COUNTS.with(Cell::get))
}

struct ImpulseFile(PathBuf);

impl Drop for ImpulseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_impulse(channels: &[Vec<i16>], sample_rate: u32) -> ImpulseFile {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "sotf-convolution-oracle-{}-{unique}.wav",
        std::process::id()
    ));
    let channel_count = channels.len() as u16;
    let frames = channels[0].len();
    let data_bytes = (frames * channels.len() * 2) as u32;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channel_count.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * u32::from(channel_count) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channel_count * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    std::fs::write(&path, bytes).unwrap();
    ImpulseFile(path)
}

fn direct_convolution(input: &[f32], impulse: &[Vec<i16>]) -> Vec<f64> {
    let channels = 3;
    let mut output = vec![0.0; input.len() + (impulse[0].len() - 1) * channels];
    for (frame, samples) in input.as_chunks::<3>().0.iter().enumerate() {
        for (channel, &sample) in samples.iter().enumerate() {
            for (tap, &coefficient) in impulse[channel % impulse.len()].iter().enumerate() {
                output[(frame + tap) * channels + channel] +=
                    f64::from(sample) * f64::from(coefficient) / 32768.0;
            }
        }
    }
    output
}

fn feed(plugin: &mut ConvolutionPlugin, source: &[f32]) -> Vec<f32> {
    let mut output = source.to_vec();
    let channels = plugin.channels();
    for samples in output.chunks_mut(17 * channels) {
        plugin
            .process_in_place(
                samples,
                &ProcessContext::new(48_000, samples.len() / channels),
            )
            .unwrap();
    }
    output
}

fn finish(plugin: &mut ConvolutionPlugin) -> Vec<f32> {
    finish_with_capacity(plugin, 257)
}

fn finish_with_capacity(plugin: &mut ConvolutionPlugin, capacity: usize) -> Vec<f32> {
    let mut destination = vec![432.0; capacity * plugin.channels()];
    let mut output = Vec::new();
    for _ in 0..65536 {
        destination.fill(432.0);
        let result = plugin
            .drain(&mut destination, &ProcessContext::new(48_000, 0))
            .unwrap();
        assert!(result.frames <= capacity && result.frames <= plugin.drain_output_frames_max());
        assert!(
            destination[result.frames * plugin.channels()..]
                .iter()
                .all(|&v| v == 432.0)
        );
        output.extend_from_slice(&destination[..result.frames * plugin.channels()]);
        if result.complete {
            return output;
        }
        assert!(result.frames > 0);
    }
    panic!("finite convolution drain did not converge");
}

#[test]
fn native_drain_matches_direct_convolution_including_final_tap() {
    let mut failures = Vec::new();
    for length in [1, 97, 1023, 1024, 1025, 2053, 8195] {
        let mut ir = vec![vec![0_i16; length]; 2];
        for (channel, impulse) in ir.iter_mut().enumerate() {
            for (tap, sample) in impulse.iter_mut().enumerate() {
                *sample = ((tap * 17 + channel * 3) % 31) as i16 - 15;
            }
            impulse[0] = 8192;
            impulse[length - 1] = if channel == 0 { 16384 } else { -8192 };
        }
        let file = write_impulse(&ir, 48_000);
        for (use_nupc, zero_latency_head, head_taps) in [
            (false, false, 128),
            (true, false, 128),
            (true, true, 32),
            (true, true, 128),
        ] {
            for mix in [0.0_f32, 0.375, 1.0] {
                let gain_db = -3.0;
                let mut plugin = ConvolutionPlugin::from_params(
                    3,
                    48_000,
                    ConvolutionPluginParams {
                        ir_file: file.0.to_str().unwrap().into(),
                        use_nupc,
                        zero_latency_head,
                        head_taps,
                        mix,
                        gain_db,
                    },
                )
                .unwrap();
                plugin.reset();
                let latency = if zero_latency_head { 0 } else { 1024 };
                let mut input = vec![0.0; 259 * 3];
                input[..3].copy_from_slice(&[0.25, -0.5, 0.125]);
                input[258 * 3..].copy_from_slice(&[-0.75, 0.5, 0.25]);
                let direct = direct_convolution(&input, &ir);
                let mut output = feed(&mut plugin, &input);
                output.extend(finish(&mut plugin));
                let correct_length = output.len() == (259 + latency + length - 1) * 3;
                let correct_audio = output.iter().enumerate().all(|(i, &actual)| {
                    let expected = i.checked_sub(latency * 3).map_or(0.0, |index| {
                        let wet = direct.get(index).copied().unwrap_or(0.0);
                        let dry = input.get(index).copied().map(f64::from).unwrap_or(0.0);
                        wet * f64::from(mix) * 10.0_f64.powf(f64::from(gain_db) / 20.0)
                            + dry * f64::from(1.0 - mix)
                    });
                    (f64::from(actual) - expected).abs() < 1e-5
                });
                if !correct_length || !correct_audio {
                    failures.push((length, use_nupc, zero_latency_head, head_taps, output.len()));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "missing or incorrect complete responses: {failures:?}"
    );
}

#[test]
fn inactive_dry_delay_is_emitted_at_end_of_stream() {
    let mut plugin = ConvolutionPlugin::new(2, 48_000);
    let input = [0.75, -0.25];
    let mut output = feed(&mut plugin, &input);
    output.extend(finish(&mut plugin));
    assert_eq!(output.len(), 2 * 1025);
    assert_eq!(&output[2048..], &input);
}

#[test]
fn cold_process_and_continuation_do_not_allocate_or_free() {
    // Keep the producer thread alive while the new audio thread runs so its
    // ArcSwap reader slot cannot simply be recycled by that new thread.
    let mut plugin = ConvolutionPlugin::new(1, 48_000);
    plugin
        .process_in_place(&mut [0.75], &ProcessContext::new(48_000, 1))
        .unwrap();
    let mut input = [0.0; 17];
    let mut destination = [0.0; 257];
    let counts = std::thread::spawn(move || {
        let (_, counts) = heap_counts(|| {
            plugin
                .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
                .unwrap();
            loop {
                let result = plugin
                    .drain(&mut destination, &ProcessContext::new(48_000, 0))
                    .unwrap();
                if result.complete {
                    break;
                }
            }
        });
        counts
    })
    .join()
    .unwrap();
    assert_eq!(counts, (0, 0), "cold process/drain allocations and frees");
}

#[test]
fn capacity_partitions_and_midstream_controls_match_zero_padded_reference() {
    use sotf_host::ParameterValue;
    let file = write_impulse(&[vec![512; 2053]], 48_000);
    for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
        for capacity in [1, 17, 255, 1024, 4097] {
            let params = ConvolutionPluginParams {
                ir_file: file.0.to_str().unwrap().into(),
                use_nupc,
                zero_latency_head,
                ..ConvolutionPluginParams::default()
            };
            let mut p = ConvolutionPlugin::from_params(2, 48_000, params.clone()).unwrap();
            let mut reference = ConvolutionPlugin::from_params(2, 48_000, params).unwrap();
            p.reset();
            reference.reset();
            let input: Vec<f32> = (0..17003)
                .flat_map(|frame| {
                    [
                        ((frame * 17 % 61) as f32 - 30.0) / 256.0,
                        ((frame * 13 % 43) as f32 - 21.0) / 256.0,
                    ]
                })
                .collect();
            // An oversized callback and a split stream must have identical history.
            let mut actual = input[..input.len() - 34].to_vec();
            p.process_in_place(&mut actual, &ProcessContext::new(48_000, 17003 - 17))
                .unwrap();
            let mut expected = feed(&mut reference, &input[..input.len() - 34]);
            for plugin in [&mut p, &mut reference] {
                plugin
                    .parametric_set_parameter("mix".into(), ParameterValue::Float(0.375))
                    .unwrap();
                plugin
                    .parametric_set_parameter("gain_db".into(), ParameterValue::Float(-6.0))
                    .unwrap();
            }
            actual.extend(feed(&mut p, &input[input.len() - 34..]));
            expected.extend(feed(&mut reference, &input[input.len() - 34..]));
            let tail = p.latency_samples() + 2052;
            actual.extend(finish_with_capacity(&mut p, capacity));
            expected.extend(feed(&mut reference, &vec![0.0; tail * 2]));
            assert_eq!(
                actual, expected,
                "nupc={use_nupc}, head={zero_latency_head}, capacity={capacity}"
            );
            let after = feed(&mut reference, &[0.0; 66]);
            assert!(after.iter().all(|v| v.abs() < 1e-6));
            let mut sentinel = [77.0; 2];
            assert_eq!(
                p.drain(&mut sentinel, &ProcessContext::new(48_000, 0))
                    .unwrap(),
                sotf_host::plugin::PluginDrainResult::COMPLETE
            );
            assert_eq!(sentinel, [77.0; 2]);
        }
    }
}

#[test]
fn preflight_and_control_rejections_preserve_the_remaining_response() {
    use sotf_host::{ParameterId, ParameterValue, plugin::PluginDrainResult};
    let mut p = ConvolutionPlugin::new(2, 48_000);
    let mut reference = ConvolutionPlugin::new(2, 48_000);
    assert_eq!(
        p.drain(&mut [], &ProcessContext::new(48_000, 0)).unwrap(),
        PluginDrainResult::COMPLETE
    );
    let source = [0.25; 34];
    assert_eq!(feed(&mut p, &source), feed(&mut reference, &source));
    for (capacity, rate) in [(0, 48_000), (3, 48_000), (34, 44_100)] {
        let mut output = vec![91.0; capacity];
        assert!(p.drain(&mut output, &ProcessContext::new(rate, 0)).is_err());
        assert!(output.iter().all(|&v| v == 91.0));
    }
    let mut one = [0.0; 2];
    let mut other = [0.0; 2];
    p.drain(&mut one, &ProcessContext::new(48_000, 0)).unwrap();
    reference
        .drain(&mut other, &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(one, other);
    let values = p.current_values();
    p.apply_values(values.clone()).unwrap();
    for (id, value) in &values {
        p.parametric_set_parameter(id.clone(), value.clone())
            .unwrap();
    }
    let mut changed = values.clone();
    changed.insert(ParameterId::from("gain_db"), ParameterValue::Float(-3.0));
    assert!(p.apply_values(changed).is_err());
    assert!(
        p.parametric_set_parameter("mix".into(), ParameterValue::Float(0.0))
            .is_err()
    );
    assert!(
        p.load_ir("/missing/path.wav")
            .unwrap_err()
            .contains("reset")
    );
    assert_eq!(p.current_values(), values);
    let mut source = [0.25; 34];
    assert!(
        p.process_in_place(&mut source, &ProcessContext::new(48_000, 17))
            .is_err()
    );
    assert_eq!(source, [0.25; 34]);
    p.process_in_place(&mut [], &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(finish(&mut p), finish(&mut reference));
    for reinitialize in [false, true] {
        if reinitialize {
            p.initialize(48_000).unwrap();
        } else {
            p.reset();
        }
        let mut fresh = ConvolutionPlugin::new(2, 48_000);
        assert_eq!(feed(&mut p, &source), feed(&mut fresh, &source));
        assert_eq!(finish(&mut p), finish(&mut fresh));
    }
}

#[test]
fn active_backends_drain_and_reset_without_cold_heap_operations() {
    let file = write_impulse(&[vec![1024; 4097]], 48_000);
    for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
        for first_drain in [false, true] {
            let mut plugin = ConvolutionPlugin::from_params(
                2,
                48_000,
                ConvolutionPluginParams {
                    ir_file: file.0.to_str().unwrap().into(),
                    use_nupc,
                    zero_latency_head,
                    ..ConvolutionPluginParams::default()
                },
            )
            .unwrap();
            plugin.reset();
            let mut input = [0.25; 34];
            let mut destination = [0.0; 2050];
            if first_drain {
                plugin
                    .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
                    .unwrap();
            }
            let counts = std::thread::spawn(move || {
                let (_, counts) = heap_counts(|| {
                    if !first_drain {
                        plugin
                            .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
                            .unwrap();
                    }
                    loop {
                        if plugin
                            .drain(&mut destination, &ProcessContext::new(48_000, 0))
                            .unwrap()
                            .complete
                        {
                            break;
                        }
                    }
                    plugin.reset();
                    plugin
                        .process_in_place(&mut input, &ProcessContext::new(48_000, 17))
                        .unwrap();
                });
                counts
            })
            .join()
            .unwrap();
            assert_eq!(
                counts,
                (0, 0),
                "nupc={use_nupc}, head={zero_latency_head}, first_drain={first_drain}"
            );
        }
    }
}

#[test]
fn finite_response_keeps_the_prepared_clock_at_other_sample_rates() {
    for rate in [44_100, 96_000, 192_000] {
        let mut ir = vec![0_i16; 2053];
        ir[0] = 8192;
        ir[2052] = -16384;
        let file = write_impulse(&[ir], rate);
        for (use_nupc, zero_latency_head) in [(false, false), (true, false), (true, true)] {
            let mut p = ConvolutionPlugin::from_params(
                1,
                rate,
                ConvolutionPluginParams {
                    ir_file: file.0.to_str().unwrap().into(),
                    use_nupc,
                    zero_latency_head,
                    ..ConvolutionPluginParams::default()
                },
            )
            .unwrap();
            p.reset();
            let mut input = [0.0; 31];
            input[30] = 0.75;
            p.process_in_place(&mut input, &ProcessContext::new(rate, 31))
                .unwrap();
            let latency = if zero_latency_head { 0 } else { 1024 };
            let mut output = input.to_vec();
            let mut buffer = [0.0; 255];
            loop {
                let result = p.drain(&mut buffer, &ProcessContext::new(rate, 0)).unwrap();
                output.extend_from_slice(&buffer[..result.frames]);
                if result.complete {
                    break;
                }
            }
            assert_eq!(output.len(), 31 + latency + 2052);
            assert!((output[30 + latency] - 0.1875).abs() < 1e-6);
            assert!((output[30 + latency + 2052] + 0.375).abs() < 1e-6);
            assert!(p.initialize(0).is_err());
            assert!(
                p.process_in_place(&mut [0.0], &ProcessContext::new(rate, 1))
                    .is_err()
            );
            p.initialize(rate).unwrap();
            assert_eq!(
                p.drain(&mut [], &ProcessContext::new(rate, 0)).unwrap(),
                sotf_host::plugin::PluginDrainResult::COMPLETE
            );
        }
    }
}
