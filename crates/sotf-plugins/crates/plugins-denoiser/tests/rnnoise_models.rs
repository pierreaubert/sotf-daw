//! Backend model-registry, adoption, and per-model pipeline oracles.
//!
//! Covers the checked-asset selection surface of `RnnoiseBackend`: registry
//! identity/provenance, staged prepare/commit adoption with failure
//! continuation, per-model audio difference from the bundled default, and
//! per-model bypass/reset/partition/latency/stereo/allocation behavior. All
//! bundled expectations reproduce the pre-existing backend contract exactly.

// Rust guideline compliant 2026-02-21

use plugins_denoiser::rnnoise::{RnnoiseBackend, RnnoiseModelId, available_models};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const RATE: u32 = 48000;
const LATENCY: usize = 960;
const FRAME: usize = 480;

const ALL_MODELS: [RnnoiseModelId; 3] = [
    RnnoiseModelId::BundledFull,
    RnnoiseModelId::LegacyLq,
    RnnoiseModelId::LegacySh,
];

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static FREES: Cell<usize> = const { Cell::new(0) };
}

struct Allocator;
// SAFETY: The caller's allocation contracts are passed unchanged to System.
// Thread-local counters are plain cells that neither allocate nor retain.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK.try_with(|v| {
            if v.get() {
                ALLOCS.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: forwards the caller's layout to System unchanged, keeping
        // the `GlobalAlloc` contract; the counters above only touch
        // thread-local cells.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = TRACK.try_with(|v| {
            if v.get() {
                FREES.with(|n| n.set(n.get() + 1));
            }
        });
        // SAFETY: forwards the caller's pointer and layout to System
        // unchanged, keeping the `GlobalAlloc` contract; the counters
        // above only touch thread-local cells.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn counted(action: impl FnOnce()) -> (usize, usize) {
    ALLOCS.set(0);
    FREES.set(0);
    TRACK.set(true);
    action();
    TRACK.set(false);
    (ALLOCS.get(), FREES.get())
}

fn make_backend(channels: usize, id: RnnoiseModelId) -> RnnoiseBackend {
    let mut backend = RnnoiseBackend::new();
    backend.initialize_with_model(RATE, channels, id).unwrap();
    backend
}

fn process(
    backend: &mut RnnoiseBackend,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
    bypass: bool,
) -> Vec<f32> {
    let mut output = Vec::with_capacity(input.len());
    let mut offset = 0;
    let mut call = 0;
    while offset < input.len() / channels {
        let frames = blocks[call % blocks.len()].min(input.len() / channels - offset);
        let mut block = input[offset * channels..(offset + frames) * channels].to_vec();
        assert_eq!(
            backend.process(&mut block, frames, channels, bypass),
            frames
        );
        output.extend_from_slice(&block);
        offset += frames;
        call += 1;
    }
    output
}

fn signal(frames: usize, channels: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    let mut out = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        let voice = (frame as f32 * 0.061).sin() * 0.25 + (frame as f32 * 0.122).sin() * 0.12;
        for _ in 0..channels {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (state as f32 / u32::MAX as f32 - 0.5) * 0.3;
            out.push((voice + noise).clamp(-0.9, 0.9));
        }
    }
    out
}

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

#[test]
fn registry_identity_and_provenance_are_exact() {
    let models = available_models();
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, RnnoiseModelId::BundledFull);
    assert_eq!(models[0].label, "RNNoise Full");
    assert!(!models[0].origin.is_empty());
    assert_eq!(models[0].sha256, None);
    assert_eq!(RnnoiseModelId::BundledFull.embedded_bytes(), None);
    assert_eq!(models[1].id, RnnoiseModelId::LegacyLq);
    assert_eq!(models[1].label, "RNNoise Legacy LQ");
    assert_eq!(
        models[1].origin,
        "GregorR/rnnoise-models@3eee541 leavened-quisling-2018-08-31/lq.rnnn"
    );
    assert_eq!(
        models[1].sha256,
        Some("1957528b752799fddf06270bc5469af7cf54c3badc358544ae2abed730943ff9")
    );
    assert_eq!(models[2].id, RnnoiseModelId::LegacySh);
    assert_eq!(models[2].label, "RNNoise Legacy SH");
    assert_eq!(
        models[2].sha256,
        Some("70bb6685eb0c2a1d18e2918dca3fbfbd39317010b1802eb1b6ea73a92f3fdec0")
    );
    // Embedded byte lengths match the staged files and distinguish them; a
    // swapped include would fail here (297041 vs 297646).
    assert_eq!(
        RnnoiseModelId::LegacyLq.embedded_bytes().unwrap().len(),
        297_041
    );
    assert_eq!(
        RnnoiseModelId::LegacySh.embedded_bytes().unwrap().len(),
        297_646
    );
    for (index, id) in ALL_MODELS.iter().enumerate() {
        assert_eq!(id.index(), index);
        assert_eq!(id.label(), models[index].label);
        assert_eq!(RnnoiseModelId::from_index(index), Some(*id));
    }
    assert_eq!(RnnoiseModelId::from_index(3), None);
}

#[test]
fn bundled_initialize_matches_plain_initialize_bit_exactly() {
    for channels in [1, 2] {
        let input = signal(5 * FRAME + 73, channels, 0xB17);
        let mut plain = RnnoiseBackend::new();
        plain.initialize(RATE, channels).unwrap();
        assert_eq!(plain.active_model(), RnnoiseModelId::BundledFull);
        let mut explicit = make_backend(channels, RnnoiseModelId::BundledFull);
        assert_eq!(explicit.active_model(), RnnoiseModelId::BundledFull);
        assert_eq!(
            process(&mut plain, &input, channels, &[137, 1, 479], false),
            process(&mut explicit, &input, channels, &[137, 1, 479], false)
        );
        assert_eq!(plain.analyzer_data(), explicit.analyzer_data());
    }
}

#[test]
fn prepare_commit_matches_initialize_with_model() {
    for channels in [1, 2] {
        for id in ALL_MODELS {
            let input = signal(5 * FRAME + 73, channels, 0x9E9A);
            let mut adopted = make_backend(channels, RnnoiseModelId::BundledFull);
            let prepared = adopted.prepare_model(id).unwrap();
            assert_eq!(prepared.id(), id);
            assert_eq!(prepared.channels(), channels);
            adopted.commit_prepared(prepared).unwrap();
            assert_eq!(adopted.active_model(), id);
            let mut installed = make_backend(channels, id);
            assert_eq!(
                process(&mut adopted, &input, channels, &[137, 1], false),
                process(&mut installed, &input, channels, &[137, 1], false),
                "channels={channels} model={id:?}"
            );
            assert_eq!(adopted.analyzer_data(), installed.analyzer_data());
        }
    }
}

#[test]
fn prepare_requires_initialization_and_leaves_backend_uninitialized() {
    let backend = RnnoiseBackend::new();
    for id in ALL_MODELS {
        assert!(backend.prepare_model(id).is_err());
    }
    let mut backend = backend;
    let mut block = vec![0.0; FRAME];
    assert_eq!(backend.process(&mut block, FRAME, 1, false), 0);
}

#[test]
fn commit_rejects_channel_mismatch_and_preserves_history() {
    for channels in [1, 2] {
        for id in ALL_MODELS {
            let prefix = signal(1024, channels, 0xC011);
            let suffix = signal(1024, channels, 0xC0DE);
            let mut backend = make_backend(channels, RnnoiseModelId::BundledFull);
            let mut twin = make_backend(channels, RnnoiseModelId::BundledFull);
            let mut actual = process(&mut backend, &prefix, channels, &[137], false);
            let mut expected = process(&mut twin, &prefix, channels, &[137], false);
            let other = make_backend(3 - channels, id);
            let mismatched = other.prepare_model(id).unwrap();
            assert!(backend.commit_prepared(mismatched).is_err());
            assert_eq!(backend.active_model(), RnnoiseModelId::BundledFull);
            actual.extend(process(&mut backend, &suffix, channels, &[17], false));
            expected.extend(process(&mut twin, &suffix, channels, &[17], false));
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn initialize_with_model_rejects_bad_format_and_preserves_history() {
    for channels in [1, 2] {
        for id in ALL_MODELS {
            let prefix = signal(1024, channels, 0xF7CE);
            let suffix = signal(1024, channels, 0xE0F);
            let mut backend = make_backend(channels, id);
            let mut twin = make_backend(channels, id);
            let mut actual = process(&mut backend, &prefix, channels, &[137], false);
            let mut expected = process(&mut twin, &prefix, channels, &[137], false);
            assert!(backend.initialize_with_model(44100, channels, id).is_err());
            assert!(backend.initialize_with_model(RATE, 3, id).is_err());
            assert_eq!(backend.active_model(), id);
            actual.extend(process(&mut backend, &suffix, channels, &[17], false));
            expected.extend(process(&mut twin, &suffix, channels, &[17], false));
            assert_eq!(actual, expected);
        }
    }
}

#[test]
fn alternate_models_change_nonzero_audio() {
    for channels in [1, 2] {
        let input = signal(5 * FRAME, channels, 0xA17E);
        let mut bundled = make_backend(channels, RnnoiseModelId::BundledFull);
        let reference = process(&mut bundled, &input, channels, &[137, 1], false);
        let mut alternates = Vec::new();
        for id in [RnnoiseModelId::LegacyLq, RnnoiseModelId::LegacySh] {
            let mut backend = make_backend(channels, id);
            let output = process(&mut backend, &input, channels, &[137, 1], false);
            assert!(output.iter().all(|s| s.is_finite()), "{id:?}");
            let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
            println!("{id:?} {channels}ch: peak={peak:.4}");
            assert!(peak > 1e-4, "{id:?} produced near-silence");
            let diff = max_abs_diff(&output, &reference);
            println!("{id:?} {channels}ch: max diff vs bundled={diff:.4}");
            assert_ne!(output, reference, "{id:?} matches bundled inference");
            alternates.push(output);
        }
        let cross = max_abs_diff(&alternates[0], &alternates[1]);
        println!("lq-vs-sh {channels}ch: max diff={cross:.4}");
        assert_ne!(alternates[0], alternates[1], "alternates match each other");
    }
}

#[test]
fn bypass_is_exact_dry_per_model() {
    for channels in [1, 2] {
        let mut input = signal(4 * FRAME + 31, channels, 0xD9A1);
        input[13] = f32::NAN;
        input[channels * 479] = f32::INFINITY;
        input[channels * 480] = -17.0;
        let expected: Vec<f32> = input
            .iter()
            .enumerate()
            .map(|(i, _)| {
                i.checked_sub(LATENCY * channels)
                    .map_or(0.0, |j| sanitize(input[j]))
            })
            .collect();
        for id in ALL_MODELS {
            let mut backend = make_backend(channels, id);
            assert_eq!(
                process(&mut backend, &input, channels, &[137, 1], true),
                expected,
                "channels={channels} model={id:?}"
            );
        }
    }
}

#[test]
fn reset_partitions_latency_analyzer_hold_per_model() {
    for channels in [1, 2] {
        for id in ALL_MODELS {
            let input = signal(5 * FRAME + 17, channels, 0x9E5);
            let mut continuous = make_backend(channels, id);
            let mut partitioned = make_backend(channels, id);
            assert_eq!(continuous.latency_samples(), LATENCY);
            assert_eq!(
                process(
                    &mut partitioned,
                    &input,
                    channels,
                    &[1, 137, 479, 481],
                    false
                ),
                process(&mut continuous, &input, channels, &[8193], false),
                "channels={channels} model={id:?}"
            );
            let data = continuous.analyzer_data();
            assert!(data.model_frames > 0);
            assert!((0.0..=1.0).contains(&data.vad_probability));
            assert!(
                data.band_gains
                    .iter()
                    .all(|gain| gain.is_finite() && (0.0..=1.0).contains(gain))
            );
            continuous.reset();
            let mut fresh = make_backend(channels, id);
            assert_eq!(
                process(&mut continuous, &input, channels, &[137], false),
                process(&mut fresh, &input, channels, &[137], false),
                "channels={channels} model={id:?}"
            );
            assert_eq!(continuous.active_model(), id);
        }
    }
}

#[test]
fn stereo_swap_invariant_holds_per_model() {
    for id in ALL_MODELS {
        let mut input = vec![0.0; FRAME * 2];
        for frame in 0..FRAME {
            let sample = (frame as f32 * 0.071).sin() * 0.35;
            input[2 * frame] = sample;
            input[2 * frame + 1] = -sample;
        }
        let mut swapped_input = input.clone();
        for frame in swapped_input.as_chunks_mut::<2>().0 {
            frame.swap(0, 1);
        }
        let mut backend = make_backend(2, id);
        let mut first = input.clone();
        backend.process(&mut first, FRAME, 2, false);
        let mut output = vec![0.0; FRAME * 2];
        backend.process(&mut output, FRAME, 2, false);
        let mut swapped_backend = make_backend(2, id);
        swapped_backend.process(&mut swapped_input, FRAME, 2, false);
        let mut swapped_output = vec![0.0; FRAME * 2];
        swapped_backend.process(&mut swapped_output, FRAME, 2, false);
        for frame in swapped_output.as_chunks_mut::<2>().0 {
            frame.swap(0, 1);
        }
        let max_swap_error = max_abs_diff(&output, &swapped_output);
        assert!(
            max_swap_error < 1.0e-6,
            "model={id:?} is channel-biased: swap error={max_swap_error}"
        );
        assert_eq!(backend.analyzer_data(), swapped_backend.analyzer_data());
    }
}

#[test]
fn realtime_lifecycle_allocates_nothing_per_model() {
    for id in ALL_MODELS {
        for channels in [1, 2] {
            let mut backend = make_backend(channels, id);
            let mut buffer = vec![0.125; FRAME * channels];
            let (allocs, frees) = std::thread::spawn(move || {
                counted(|| {
                    backend.process(&mut buffer, FRAME, channels, false);
                    backend.process(&mut buffer, FRAME, channels, false);
                    backend.reset();
                    backend.process(&mut buffer, FRAME, channels, false);
                    let _ = backend.analyzer_data();
                    let _ = backend.latency_samples();
                })
            })
            .join()
            .unwrap();
            assert_eq!((allocs, frees), (0, 0), "model={id:?} channels={channels}");
        }
        // Adoption itself allocates nothing; retired states free here.
        let mut backend = make_backend(1, RnnoiseModelId::BundledFull);
        let prepared = backend.prepare_model(id).unwrap();
        let (allocs, frees) = std::thread::spawn(move || {
            counted(|| {
                backend.commit_prepared(prepared).unwrap();
            })
        })
        .join()
        .unwrap();
        assert_eq!(allocs, 0, "model={id:?} commit allocated");
        assert!(frees > 0, "model={id:?} commit freed nothing");
        // Dropping an uncommitted bundle retires its states without allocating.
        let backend = make_backend(1, RnnoiseModelId::BundledFull);
        let prepared = backend.prepare_model(id).unwrap();
        let (allocs, frees) = counted(|| drop(prepared));
        assert_eq!(allocs, 0, "model={id:?} bundle drop allocated");
        assert!(frees > 0, "model={id:?} bundle drop freed nothing");
    }
}
