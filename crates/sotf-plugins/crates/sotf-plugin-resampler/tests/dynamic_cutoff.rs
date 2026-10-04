//! Independent tone and streaming-history checks for prepared dynamic cutoffs.

// Rust guideline compliant 2026-02-21
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;

const INPUT_RATE: u32 = 48_000;
const CHUNK: usize = 256;
const CHANNELS: usize = 2;
const AMPLITUDE: f64 = 0.5;
const QUALITIES: [ResamplerQuality; 3] = [
    ResamplerQuality::Fast,
    ResamplerQuality::Medium,
    ResamplerQuality::High,
];

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct CallbackAllocator;

// SAFETY: System receives the original valid layout/pointer unchanged. The
// counters use constant-initialized TLS cells without allocating or borrowing.
unsafe impl GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                ALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Delegate the caller's allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACKING.try_with(|tracking| {
            if tracking.get() {
                DEALLOCATIONS.with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Delegate the original allocation's pointer and layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CallbackAllocator = CallbackAllocator;

fn parameters(quality: ResamplerQuality, cutoff_scale: f32) -> SincInterpolationParameters {
    let (sinc_len, oversampling_factor) = match quality {
        ResamplerQuality::Fast => (64, 128),
        ResamplerQuality::Medium => (128, 256),
        ResamplerQuality::High => (256, 256),
    };
    SincInterpolationParameters {
        sinc_len,
        f_cutoff: Some(
            rubato::calculate_cutoff::<f32>(sinc_len, WindowFunction::BlackmanHarris2)
                * cutoff_scale,
        ),
        oversampling_factor,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    }
}

fn make_plugin(output_rate: u32, quality: ResamplerQuality) -> ResamplerPlugin {
    let mut plugin =
        ResamplerPlugin::with_quality(CHANNELS, INPUT_RATE, output_rate, CHUNK, quality).unwrap();
    plugin.initialize(f64::from(INPUT_RATE)).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
}

fn unbanked(nominal: f64, quality: ResamplerQuality, cutoff_scale: f32) -> Async<f32> {
    Async::new_sinc(
        nominal,
        2.0,
        &parameters(quality, cutoff_scale),
        CHUNK,
        CHANNELS,
        FixedAsync::Input,
    )
    .unwrap()
}

fn reference_block(reference: &mut Async<f32>, input: &[f32]) -> Vec<f32> {
    let planar: Vec<Vec<f32>> = (0..CHANNELS)
        .map(|channel| {
            input
                .iter()
                .skip(channel)
                .step_by(CHANNELS)
                .copied()
                .collect()
        })
        .collect();
    let capacity = reference.output_frames_max();
    let mut output = vec![vec![f32::NAN; capacity]; CHANNELS];
    let (_, frames) = reference
        .process_into_buffer(
            &SequentialSliceOfVecs::new(&planar, CHANNELS, CHUNK).unwrap(),
            &mut SequentialSliceOfVecs::new_mut(&mut output, CHANNELS, capacity).unwrap(),
            None,
        )
        .unwrap();
    (0..frames)
        .flat_map(|frame| [output[0][frame], output[1][frame]])
        .collect()
}

fn plugin_block(plugin: &mut ResamplerPlugin, input: &[f32], partitions: &[usize]) -> Vec<f32> {
    let mut output = Vec::new();
    let mut offset = 0;
    for &frames in partitions {
        let mut block = vec![f32::NAN; plugin.output_frames_for_input(frames) * CHANNELS + 4];
        let written = plugin
            .process(
                &input[offset * CHANNELS..(offset + frames) * CHANNELS],
                &mut block,
                &ProcessContext::new(INPUT_RATE, frames),
            )
            .unwrap();
        assert!(
            block[written * CHANNELS..]
                .iter()
                .all(|sample| sample.is_nan())
        );
        output.extend_from_slice(&block[..written * CHANNELS]);
        offset += frames;
    }
    assert_eq!(offset, input.len() / CHANNELS);
    output
}

fn stereo_tones(block: usize, frequencies: [f64; CHANNELS]) -> Vec<f32> {
    (0..CHUNK)
        .flat_map(|frame| {
            frequencies.map(|frequency| {
                (AMPLITUDE
                    * (TAU * frequency * (block * CHUNK + frame) as f64 / f64::from(INPUT_RATE))
                        .sin()) as f32
            })
        })
        .collect()
}

fn rms_gain_db(output: &[f32], channel: usize) -> f64 {
    // Both measured frequencies complete whole periods in 2048 output frames.
    // The stream has already passed startup; no EOS padding is included.
    let frames = 2048;
    let start = output.len() / CHANNELS - frames;
    let energy = (start..start + frames)
        .map(|frame| f64::from(output[frame * CHANNELS + channel]).powi(2))
        .sum::<f64>()
        / frames as f64;
    10.0 * (energy / (AMPLITUDE * AMPLITUDE / 2.0)).log10()
}

fn analytic_integer_phase_gain(quality: ResamplerQuality, ratio: f64, frequency: f64) -> f64 {
    // Independently evaluate the declared periodic squared four-term windowed
    // sinc in f64, then take its DTFT. No production taps/window/FFT are used.
    // Four-term window definition: scipy.signal.windows.blackmanharris,
    // https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.windows.blackmanharris.html
    let specs = parameters(quality, 1.0);
    let length = specs.sinc_len;
    let phases = specs.oversampling_factor;
    let cutoff = f64::from(specs.f_cutoff.expect("test builds explicit cutoffs") * ratio as f32);
    let prototype = |position: f64| {
        let angle = TAU * position / length as f64;
        let window = 0.35875 - 0.48829 * angle.cos() + 0.14128 * (2.0 * angle).cos()
            - 0.01168 * (3.0 * angle).cos();
        let argument = std::f64::consts::PI * cutoff * (position - length as f64 / 2.0);
        let sinc = if argument == 0.0 {
            1.0
        } else {
            argument.sin() / argument
        };
        window * window * sinc
    };
    // Rubato's table layout reverses phase indices and normalizes
    // the sum over all prepared phases. Integer source anchors select phase0,
    // whose prototype samples have fractional displacement (P-1)/P.
    let normalization = (0..length * phases)
        .map(|index| prototype(index as f64 / phases as f64))
        .sum::<f64>()
        / phases as f64;
    let (real, imaginary) = (0..length).fold((0.0, 0.0), |(real, imaginary), tap| {
        let coefficient =
            prototype(tap as f64 + (phases - 1) as f64 / phases as f64) / normalization;
        let angle = TAU * frequency * tap as f64 / f64::from(INPUT_RATE);
        (
            real + coefficient * angle.cos(),
            imaginary - coefficient * angle.sin(),
        )
    });
    real.hypot(imaginary)
}

#[test]
fn prepared_cutoffs_reject_specific_stop_tones_and_match_analytic_response() {
    // Targets are 1.5 times the *new* Nyquist, not its transition boundary.
    // Acceptance is deliberately quality/ratio dependent. These are regression
    // checkpoints, not a uniform stopband or near-Nyquist rejection claim.
    for (output_rate, stop_hz, bounds) in [
        (48_000, 18_000.0, [-70.0, -90.0, -90.0]),
        (24_000, 9_000.0, [-45.0, -70.0, -90.0]),
        (12_000, 4_500.0, [-20.0, -45.0, -70.0]),
    ] {
        for (quality, bound) in QUALITIES.into_iter().zip(bounds) {
            let nominal = f64::from(output_rate) / f64::from(INPUT_RATE);
            let target = nominal * 0.5;
            let mut plugin = make_plugin(output_rate, quality);
            plugin.set_ratio(target, false).unwrap();
            let mut control = unbanked(nominal, quality, 1.0);
            control.set_resample_ratio(target, false).unwrap();
            let mut actual = Vec::new();
            let mut old_cutoff = Vec::new();
            for block in 0..96 {
                let input = stereo_tones(block, [stop_hz, f64::from(output_rate) * 0.5 / 16.0]);
                actual.extend(plugin_block(&mut plugin, &input, &[CHUNK]));
                old_cutoff.extend(reference_block(&mut control, &input));
            }
            let alias_db = rms_gain_db(&actual, 0);
            let control_db = rms_gain_db(&old_cutoff, 0);
            let passband_db = rms_gain_db(&actual, 1);
            let expected_passband = 20.0
                * analytic_integer_phase_gain(quality, target, f64::from(output_rate) * 0.5 / 16.0)
                    .log10();
            let expected_alias = analytic_integer_phase_gain(quality, target, stop_hz);
            eprintln!(
                "{quality:?}, ratio={target}: alias={alias_db:.3} dB, old={control_db:.3} dB, passband={passband_db:.6} dB"
            );
            assert!(
                alias_db < bound,
                "{quality:?} ratio={target}: alias {alias_db} dB exceeds {bound} dB"
            );
            assert!(
                alias_db < control_db - 15.0,
                "cutoff selection must materially reduce the old alias"
            );
            assert!(
                (passband_db - expected_passband).abs() < 0.001,
                "{quality:?} ratio={target}: passband {passband_db} dB != analytic {expected_passband} dB"
            );
            assert!(
                (10.0_f64.powf(alias_db / 20.0) - expected_alias).abs() < 2e-6,
                "{quality:?} ratio={target}: alias amplitude differs from analytic FIR response"
            );
        }
    }
}

#[test]
fn prepared_nominal_filter_matches_the_unbanked_response_exactly() {
    for quality in QUALITIES {
        for output_rate in [24_000, 44_100, 48_000, 96_000] {
            let nominal = f64::from(output_rate) / f64::from(INPUT_RATE);
            let mut plugin = make_plugin(output_rate, quality);
            let mut reference = unbanked(nominal, quality, 1.0);
            let nyquist = f64::from(output_rate.min(INPUT_RATE)) / 2.0;
            for block in 0..16 {
                // The upper tone would expose a permanently narrowed bank.
                let input = stereo_tones(block, [nyquist * 0.8, nyquist * 0.13]);
                assert_eq!(
                    plugin_block(&mut plugin, &input, &[CHUNK]),
                    reference_block(&mut reference, &input),
                    "nominal {quality:?}, rate={output_rate}, block={block}"
                );
            }
        }
    }
}

#[test]
fn repeated_steps_and_ramps_retain_the_exact_reference_history() {
    for quality in QUALITIES {
        for ramp in [false, true] {
            for partitions in [&[CHUNK][..], &[17, 61, 178][..]] {
                let mut plugin = make_plugin(INPUT_RATE, quality);
                // Both references have the entire input history and follow the
                // same clock; their filters never switch. This is independent
                // of the production bank's table ownership and slot selection.
                let mut low = unbanked(1.0, quality, 0.5);
                let mut nominal = unbanked(1.0, quality, 1.0);
                let mut previous = 1.0_f64;
                for block in 0..48 {
                    let target = [1.0_f64, 0.5, 2.0, 0.5, 1.0, 2.0][block / 8];
                    if block % 8 == 0 {
                        plugin.set_ratio(target, ramp).unwrap();
                        low.set_resample_ratio(target, ramp).unwrap();
                        nominal.set_resample_ratio(target, ramp).unwrap();
                    }
                    let mut input = stereo_tones(block, [997.0, 18_000.0]);
                    for frame in input.as_chunks_mut::<CHANNELS>().0 {
                        frame[0] += 0.3 * frame[1];
                        frame[1] -= 0.37 * frame[0];
                    }
                    let low_output = reference_block(&mut low, &input);
                    let nominal_output = reference_block(&mut nominal, &input);
                    // Instant changes immediately use the new ratio; ramps
                    // must protect the narrower endpoint for their whole block.
                    let ceiling = if ramp { previous.min(target) } else { target };
                    let expected = if ceiling < 1.0 {
                        low_output
                    } else {
                        nominal_output
                    };
                    assert_eq!(
                        plugin_block(&mut plugin, &input, partitions),
                        expected,
                        "{quality:?}, ramp={ramp}, block={block}, partitions={partitions:?}"
                    );
                    previous = target;
                }
            }
        }
    }
}

#[test]
fn completed_ramp_restores_wider_cutoff_inside_one_large_callback() {
    for quality in QUALITIES {
        let mut plugin = make_plugin(INPUT_RATE, quality);
        let mut low = unbanked(1.0, quality, 0.5);
        let mut nominal = unbanked(1.0, quality, 1.0);
        plugin.set_ratio(0.5, false).unwrap();
        low.set_resample_ratio(0.5, false).unwrap();
        nominal.set_resample_ratio(0.5, false).unwrap();
        for block in 0..8 {
            let input = stereo_tones(block, [997.0, 18_000.0]);
            assert_eq!(
                plugin_block(&mut plugin, &input, &[CHUNK]),
                reference_block(&mut low, &input)
            );
            reference_block(&mut nominal, &input);
        }
        plugin.set_ratio(2.0, true).unwrap();
        low.set_resample_ratio(2.0, true).unwrap();
        nominal.set_resample_ratio(2.0, true).unwrap();
        let mut input = Vec::new();
        let mut expected = Vec::new();
        for block in 8..12 {
            let next = stereo_tones(block, [997.0, 18_000.0]);
            let low_output = reference_block(&mut low, &next);
            let nominal_output = reference_block(&mut nominal, &next);
            expected.extend(if block == 8 {
                low_output
            } else {
                nominal_output
            });
            input.extend(next);
        }
        assert_eq!(
            plugin_block(&mut plugin, &input, &[4 * CHUNK]),
            expected,
            "{quality:?}: ramp cutoff must be reevaluated between backend chunks"
        );
    }
}

#[test]
fn cold_prepared_changes_and_processing_do_not_allocate_or_free() {
    for quality in QUALITIES {
        let mut plugin =
            ResamplerPlugin::with_quality(CHANNELS, INPUT_RATE, 44_100, CHUNK, quality).unwrap();
        plugin.initialize(f64::from(INPUT_RATE)).unwrap();
        let nominal = plugin.ratio();
        let dynamic_id = ParameterId::from("dynamic_ratio");
        let input = vec![0.125; CHUNK * CHANNELS];
        let mut output = vec![0.0; plugin.output_frames_for_input(CHUNK) * CHANNELS];
        let counts = std::thread::spawn(move || {
            ALLOCATIONS.set(0);
            DEALLOCATIONS.set(0);
            TRACKING.set(true);
            plugin
                .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                .unwrap();
            for reset in [false, true] {
                if reset {
                    plugin.reset();
                }
                for block in 0..24 {
                    let relative = [0.5, 0.9999, 1.0, 1.7, 2.0, 0.75][block % 6];
                    plugin
                        .set_ratio(nominal * relative, block % 2 == 0)
                        .unwrap();
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(INPUT_RATE, CHUNK))
                        .unwrap();
                }
                let context = ProcessContext::new(INPUT_RATE, 0);
                plugin.begin_drain(&context).unwrap();
                let bound = plugin.drain_call_bound().unwrap().get();
                let mut complete = false;
                for _ in 0..bound {
                    assert!(plugin.drain_call_bound().is_some());
                    if plugin.drain(&mut output, &context).unwrap().complete {
                        complete = true;
                        break;
                    }
                }
                assert!(complete);
            }
            TRACKING.set(false);
            (ALLOCATIONS.get(), DEALLOCATIONS.get())
        })
        .join()
        .unwrap();
        assert_eq!(
            counts,
            (0, 0),
            "{quality:?}: callback allocation/free counts"
        );
    }
}
