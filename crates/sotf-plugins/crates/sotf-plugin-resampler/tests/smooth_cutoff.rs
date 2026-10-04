//! Smoothed cutoff-table transitions: clock, count, and audio behavior.
//!
//! The `cutoff_smoothing` parameter (default off) slews upward cutoff
//! widening at most one prepared table per selection call while downward
//! narrowing still jumps immediately. These tests prove the slew preserves
//! sample clocks, exact frame counts, drain completion, and
//! callback-partition invariance; acts on audio only across upward
//! transitions; and keeps downward alias protection bit-exact. Valid
//! parameter changes, process, drain, and reset allocate and free nothing;
//! control rejections return heap-allocated `String` errors (as do all
//! existing control rejections) and belong on a control thread. Bounds are
//! pre-declared below; existing tolerances are untouched.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const CHUNK: usize = 256;
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

fn make(quality: ResamplerQuality, channels: usize, smoothing: bool) -> ResamplerPlugin {
    let mut plugin = ResamplerPlugin::with_quality(channels, RATE, RATE, CHUNK, quality).unwrap();
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
        .set_parameter(
            ParameterId::from("dynamic_ratio"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    plugin
        .set_parameter(
            ParameterId::from("cutoff_smoothing"),
            ParameterValue::Bool(smoothing),
        )
        .unwrap();
    plugin
}

/// Continuous-phase tones across the whole stream, one frequency per channel.
fn tones(frames: usize, channels: usize, frequencies: &[f64], start: usize) -> Vec<f32> {
    debug_assert_eq!(frequencies.len(), channels);
    (0..frames)
        .flat_map(|frame| {
            frequencies.iter().map(move |frequency| {
                (AMPLITUDE * (TAU * frequency * (start + frame) as f64 / f64::from(RATE)).sin())
                    as f32
            })
        })
        .collect()
}

fn feed(plugin: &mut ResamplerPlugin, input: &[f32], frames: usize, output: &mut Vec<f32>) {
    let channels = input.len() / frames.max(1);
    let mut block = vec![f32::NAN; plugin.output_frames_for_input(frames) * channels];
    let written = plugin
        .process(input, &mut block, &ProcessContext::new(RATE, frames))
        .unwrap();
    output.extend_from_slice(&block[..written * channels]);
    assert!(block[written * channels..].iter().all(|x| x.is_nan()));
}

fn finish(plugin: &mut ResamplerPlugin, channels: usize, output: &mut Vec<f32>) {
    let bound = plugin.drain_call_bound().unwrap().get() as usize;
    for _ in 0..bound {
        let mut block = vec![f32::NAN; plugin.drain_output_frames_max() * channels];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 0))
            .unwrap();
        output.extend_from_slice(&block[..result.frames * channels]);
        assert!(block[result.frames * channels..].iter().all(|x| x.is_nan()));
        if result.complete {
            return;
        }
    }
    panic!("drain did not complete within its advertised bound");
}

fn rms_db(samples: &[f32]) -> f64 {
    let energy = samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>()
        / samples.len().max(1) as f64;
    10.0 * (energy / (AMPLITUDE * AMPLITUDE / 2.0)).log10()
}

fn channel_samples(interleaved: &[f32], channels: usize, channel: usize) -> Vec<f32> {
    interleaved
        .iter()
        .skip(channel)
        .step_by(channels)
        .copied()
        .collect()
}

fn coherent_energy_db(output: &[f32], rate: u32, frequency: f64) -> f64 {
    // Same coherent least-squares method as spectral_accuracy.rs: a 200 ms
    // window well inside the stream, fitted against sin/cos references.
    let offset = rate as usize / 10;
    let count = rate as usize / 5;
    let (sin_sum, cos_sum) = (0..count).fold((0.0, 0.0), |(s, c), frame| {
        let value = f64::from(output[offset + frame]);
        let phase = TAU * frequency * frame as f64 / f64::from(rate);
        (s + value * phase.sin(), c + value * phase.cos())
    });
    let sin_gain = 2.0 * sin_sum / count as f64;
    let cos_gain = 2.0 * cos_sum / count as f64;
    let energy = (0..count)
        .map(|frame| {
            let phase = TAU * frequency * frame as f64 / f64::from(rate);
            let fitted = sin_gain * phase.sin() + cos_gain * phase.cos();
            let value = f64::from(output[offset + frame]);
            (value - fitted).powi(2)
        })
        .sum::<f64>()
        / count as f64;
    let signal = (sin_gain.powi(2) + cos_gain.powi(2)) / 2.0 + energy;
    10.0 * (signal / (AMPLITUDE * AMPLITUDE / 2.0)).log10()
}

#[test]
fn smoothing_preserves_counts_clocks_and_completion() {
    // Ratio schedules: fixed, upward, downward, and mixed. Smoothing must
    // never alter the emitted clock, so every smoothed stream matches its
    // instant twin sample-for-sample in length, and fixed trajectories keep
    // the exact legacy count ceil(S*r) + floor(L*r/2).
    let schedules: &[&[(f64, bool)]] = &[&[], &[(0.5, false), (2.0, true)], &[(2.0, false), (0.5, false)], &[(0.5, false), (2.0, true), (1.0, false)]];
    for quality in QUALITIES {
        let taps = match quality {
            ResamplerQuality::Fast => 64,
            ResamplerQuality::Medium => 128,
            ResamplerQuality::High => 256,
        };
        for schedule in schedules {
            let mut smoothed = make(quality, 1, true);
            let mut instant = make(quality, 1, false);
            let mut actual = Vec::new();
            let mut expected = Vec::new();
            let mut position = 0;
            for change in std::iter::once(None).chain(schedule.iter().map(Some)) {
                if let Some((ratio, ramp)) = change {
                    smoothed.set_ratio(*ratio, *ramp).unwrap();
                    instant.set_ratio(*ratio, *ramp).unwrap();
                }
                let frames = 1024;
                let input: Vec<f32> = (0..frames)
                    .map(|n| ((position + n) % 23) as f32 / 128.0)
                    .collect();
                position += frames;
                feed(&mut smoothed, &input, frames, &mut actual);
                feed(&mut instant, &input, frames, &mut expected);
            }
            finish(&mut smoothed, 1, &mut actual);
            finish(&mut instant, 1, &mut expected);
            assert_eq!(
                actual.len(),
                expected.len(),
                "{quality:?} schedule {schedule:?}: smoothing changed the frame count"
            );
            if schedule.is_empty() {
                let legacy = 1024 + (f64::from(taps) / 2.0).floor() as usize;
                assert_eq!(actual.len(), legacy, "{quality:?}: fixed count drifted");
                assert!(
                    actual == expected,
                    "{quality:?}: smoothing without ratio changes must be bit-exact"
                );
            }
        }
    }
}

#[test]
fn downward_narrowing_is_bit_exact_with_smoothing_enabled() {
    // Downward moves jump to the safe narrow table in both modes, so the
    // deep-stop alias protection is identical sample for sample.
    let frames = RATE as usize / 2 + 37;
    let input = tones(frames, 1, &[18_000.0], 0);
    let mut smoothed = make(ResamplerQuality::High, 1, true);
    let mut instant = make(ResamplerQuality::High, 1, false);
    smoothed.set_ratio(0.5, false).unwrap();
    instant.set_ratio(0.5, false).unwrap();
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    feed(&mut smoothed, &input, frames, &mut actual);
    feed(&mut instant, &input, frames, &mut expected);
    finish(&mut smoothed, 1, &mut actual);
    finish(&mut instant, 1, &mut expected);
    assert!(
        actual == expected,
        "downward narrowing must be bit-exact with smoothing enabled"
    );
    // The 18 kHz tone aliases to 6 kHz at the 24 kHz output clock; the
    // long-standing 60 dB deep-stop bound applies unchanged.
    let energy_db = coherent_energy_db(&actual, 24_000, 6_000.0);
    eprintln!("SMOOTH-CUTOFF downward 18kHz alias energy: {energy_db:.3} dB");
    assert!(
        energy_db < -60.0,
        "downward alias protection regressed: {energy_db}"
    );

    // A mid-stream downward change is likewise bit-exact.
    let music = tones(2048, 1, &[997.0], 0);
    let mut moved_smooth = make(ResamplerQuality::High, 1, true);
    let mut moved_instant = make(ResamplerQuality::High, 1, false);
    for plugin in [&mut moved_smooth, &mut moved_instant] {
        plugin.set_ratio(2.0, false).unwrap();
    }
    let mut moved_actual = Vec::new();
    let mut moved_expected = Vec::new();
    feed(&mut moved_smooth, &music[..1024], 1024, &mut moved_actual);
    feed(&mut moved_instant, &music[..1024], 1024, &mut moved_expected);
    moved_smooth.set_ratio(0.5, false).unwrap();
    moved_instant.set_ratio(0.5, false).unwrap();
    feed(&mut moved_smooth, &music[1024..], 1024, &mut moved_actual);
    feed(&mut moved_instant, &music[1024..], 1024, &mut moved_expected);
    finish(&mut moved_smooth, 1, &mut moved_actual);
    finish(&mut moved_instant, 1, &mut moved_expected);
    assert!(
        moved_actual == moved_expected,
        "mid-stream downward narrowing must be bit-exact"
    );
}

#[test]
fn upward_widening_acts_on_audio_then_converges() {
    // Settle at ratio 0.5, ramp to 2.0, then wash out. Pre-ramp and ramp
    // blocks are bit-exact (both modes hold the narrow table through the
    // ramp); post-ramp blocks differ materially on high-frequency content
    // while low frequencies stay within 0.5 dB; after washout plus drain
    // the streams reconverge bit-exactly with identical lengths.
    for quality in QUALITIES {
        let mut smoothed = make(quality, 2, true);
        let mut instant = make(quality, 2, false);
        let mut smooth_blocks: Vec<Vec<f32>> = Vec::new();
        let mut instant_blocks: Vec<Vec<f32>> = Vec::new();
        smoothed.set_ratio(0.5, false).unwrap();
        instant.set_ratio(0.5, false).unwrap();
        let mut position = 0;
        for block in 0..29 {
            if block == 8 {
                smoothed.set_ratio(2.0, true).unwrap();
                instant.set_ratio(2.0, true).unwrap();
            }
            let input = tones(CHUNK, 2, &[997.0, 18_000.0], position);
            position += CHUNK;
            let mut a = Vec::new();
            let mut b = Vec::new();
            feed(&mut smoothed, &input, CHUNK, &mut a);
            feed(&mut instant, &input, CHUNK, &mut b);
            smooth_blocks.push(a);
            instant_blocks.push(b);
        }
        // Settle blocks and the ramp block itself are bit-exact.
        for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
            if index <= 8 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: pre-widening output must be bit-exact"
                );
            }
        }
        // Post-ramp transition blocks differ materially on HF content.
        let transition_smooth: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let transition_instant: Vec<f32> =
            instant_blocks[9..13].iter().flatten().copied().collect();
        let peak = transition_smooth
            .iter()
            .zip(&transition_instant)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            peak > 1e-6,
            "{quality:?}: smoothing must act on transition audio, peak {peak}"
        );
        let hf_smooth = rms_db(&channel_samples(&transition_smooth, 2, 1));
        let hf_instant = rms_db(&channel_samples(&transition_instant, 2, 1));
        let lf_smooth = rms_db(&channel_samples(&transition_smooth, 2, 0));
        let lf_instant = rms_db(&channel_samples(&transition_instant, 2, 0));
        eprintln!(
            "SMOOTH-CUTOFF {quality:?} transition HF smooth={hf_smooth:.2} dB instant={hf_instant:.2} dB; LF smooth={lf_smooth:.3} dB instant={lf_instant:.3} dB"
        );
        assert!(
            hf_smooth < hf_instant - 3.0,
            "{quality:?}: smoothed widening must release HF energy gradually"
        );
        assert!(
            (lf_smooth - lf_instant).abs() < 0.5,
            "{quality:?}: smoothing must not disturb the passband"
        );
        // The nominal-1.0 bank holds 10 tables, so the 9-step slew from
        // the narrow table converges during block 16. Both instances then
        // share the wide table, identical input history, and the identical
        // interpolation clock, hence every later block — and the drain —
        // reconverges bit-exactly with identical total lengths.
        for (index, (a, b)) in smooth_blocks.iter().zip(&instant_blocks).enumerate() {
            if index >= 17 {
                assert!(
                    a == b,
                    "{quality:?} block {index}: converged output must be bit-exact"
                );
            }
        }
        let mut drain_smooth = Vec::new();
        let mut drain_instant = Vec::new();
        finish(&mut smoothed, 2, &mut drain_smooth);
        finish(&mut instant, 2, &mut drain_instant);
        assert!(
            drain_smooth == drain_instant,
            "{quality:?}: drain must reconverge bit-exactly"
        );
        let len_smooth: usize =
            smooth_blocks.iter().map(Vec::len).sum::<usize>() + drain_smooth.len();
        let len_instant: usize =
            instant_blocks.iter().map(Vec::len).sum::<usize>() + drain_instant.len();
        assert_eq!(len_smooth, len_instant, "{quality:?}: lengths diverged");
    }
}

#[test]
fn smoothed_transitions_are_partition_invariant() {
    // The slew advances once per backend chunk, so host callback
    // partitioning cannot change the smoothed complete stream.
    for smoothing in [false, true] {
        let mut narrow = make(ResamplerQuality::High, 1, smoothing);
        let mut wide = make(ResamplerQuality::High, 1, smoothing);
        let input = tones(4096, 1, &[997.0], 0);
        for plugin in [&mut narrow, &mut wide] {
            plugin.set_ratio(0.5, false).unwrap();
        }
        let mut a = Vec::new();
        let mut b = Vec::new();
        feed(&mut narrow, &input[..2048], 2048, &mut a);
        feed(&mut wide, &input[..2048], 2048, &mut b);
        narrow.set_ratio(2.0, true).unwrap();
        wide.set_ratio(2.0, true).unwrap();
        // Same stream position, different callback partitions.
        let mut offset = 2048;
        for frames in [1, 7, 131, 509, 2, 1023].into_iter().cycle() {
            let frames = frames.min(4096 - offset);
            if frames == 0 {
                break;
            }
            feed(
                &mut narrow,
                &input[offset..offset + frames],
                frames,
                &mut a,
            );
            offset += frames;
        }
        feed(&mut wide, &input[2048..], 2048, &mut b);
        finish(&mut narrow, 1, &mut a);
        finish(&mut wide, 1, &mut b);
        assert!(
            a == b,
            "smoothing={smoothing}: callback partitioning changed the transition stream"
        );
    }
}

#[test]
fn cold_smoothed_changes_process_drain_and_reset_do_not_allocate_or_free() {
    for quality in QUALITIES {
        let mut plugin =
            ResamplerPlugin::with_quality(2, RATE, 44_100, CHUNK, quality).unwrap();
        plugin.initialize(f64::from(RATE)).unwrap();
        let nominal = plugin.ratio();
        let dynamic_id = ParameterId::from("dynamic_ratio");
        let smooth_id = ParameterId::from("cutoff_smoothing");
        let input = vec![0.125; CHUNK * 2];
        let mut output = vec![0.0; plugin.output_frames_for_input(CHUNK) * 2];
        let counts = std::thread::spawn(move || {
            ALLOCATIONS.set(0);
            DEALLOCATIONS.set(0);
            TRACKING.set(true);
            plugin
                .set_parameter(dynamic_id.clone(), ParameterValue::Bool(true))
                .unwrap();
            plugin
                .set_parameter(smooth_id.clone(), ParameterValue::Bool(true))
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
                        .process(&input, &mut output, &ProcessContext::new(RATE, CHUNK))
                        .unwrap();
                }
                let context = ProcessContext::new(RATE, 0);
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
        assert_eq!(counts, (0, 0), "{quality:?}: callback allocation/free counts");
    }
}

#[test]
fn smoothing_rejection_preserves_audio_and_reset_restores_nominal() {
    // A rejected post-finalization change preserves the exact stream.
    let music = tones(512, 1, &[997.0], 0);
    let mut rejected = make(ResamplerQuality::High, 1, true);
    let mut control = make(ResamplerQuality::High, 1, true);
    let mut actual = Vec::new();
    let mut expected = Vec::new();
    feed(&mut rejected, &music, 512, &mut actual);
    feed(&mut control, &music, 512, &mut expected);
    for (plugin, output) in [(&mut rejected, &mut actual), (&mut control, &mut expected)] {
        let mut block = vec![f32::NAN; plugin.drain_output_frames_max()];
        let result = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 0))
            .unwrap();
        output.extend_from_slice(&block[..result.frames]);
        assert!(block[result.frames..].iter().all(|x| x.is_nan()));
    }
    assert!(
        rejected
            .set_parameter(
                ParameterId::from("cutoff_smoothing"),
                ParameterValue::Bool(false),
            )
            .is_err(),
        "post-finalization smoothing changes must reject"
    );
    finish(&mut rejected, 1, &mut actual);
    finish(&mut control, 1, &mut expected);
    assert!(
        actual == expected,
        "rejected smoothing change must preserve the exact stream"
    );

    // Reset keeps the smoothing flag and restores the nominal table: a
    // post-reset fixed stream equals a fresh smoothed instance exactly.
    let mut reset = make(ResamplerQuality::High, 1, true);
    reset.set_ratio(0.5, false).unwrap();
    let mut scratch = Vec::new();
    feed(&mut reset, &music, 512, &mut scratch);
    reset.set_ratio(2.0, true).unwrap();
    feed(&mut reset, &music, 512, &mut scratch);
    reset.reset();
    assert_eq!(
        reset.get_parameter(&ParameterId::from("cutoff_smoothing")),
        Some(ParameterValue::Bool(true))
    );
    let mut fresh = make(ResamplerQuality::High, 1, true);
    let mut after = Vec::new();
    let mut pristine = Vec::new();
    feed(&mut reset, &music, 512, &mut after);
    feed(&mut fresh, &music, 512, &mut pristine);
    finish(&mut reset, 1, &mut after);
    finish(&mut fresh, 1, &mut pristine);
    assert!(
        after == pristine,
        "reset must restore nominal smoothed behavior exactly"
    );

    // Unity passthrough ignores the bank entirely in both modes.
    let mut passthrough = ResamplerPlugin::new(1, RATE, RATE, CHUNK).unwrap();
    passthrough.initialize(f64::from(RATE)).unwrap();
    passthrough
        .set_parameter(
            ParameterId::from("cutoff_smoothing"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let input: Vec<f32> = (0..300).map(f32::from_bits).collect();
    let mut output = vec![0.0; 300];
    let produced = passthrough
        .process(&input, &mut output, &ProcessContext::new(RATE, 300))
        .unwrap();
    assert_eq!(produced, 300);
    assert_eq!(output, input);
}
