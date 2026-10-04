//! Exercises populated four-band PhaseCompensated lifecycle transitions.

// Rust guideline compliant 2026-02-21

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_band_split::{BandSplitPlugin, BandSplitRecombinationMode};

const CHANNELS: usize = 2;
const BANDS: usize = 4;
const INITIAL_SAMPLE_RATE: u32 = 48_000;
const RETRY_SAMPLE_RATE: u32 = 96_000;
const INITIAL_CUTOFFS: [f32; BANDS - 1] = [430.0, 1_720.0, 6_880.0];
const FIRST_TARGET_CUTOFFS: [f32; BANDS - 1] = [540.0, 2_160.0, 8_640.0];
const SECOND_TARGET_CUTOFFS: [f32; BANDS - 1] = [500.0, 2_000.0, 8_000.0];
const RETRY_CUTOFFS: [f32; BANDS - 1] = [540.0, 2_250.0, 8_640.0];
const INITIAL_GAINS_DB: [f32; BANDS] = [0.0, 0.0, 0.0, 0.0];
const FIRST_TARGET_GAINS_DB: [f32; BANDS] = [-4.0, 1.5, -2.0, 3.0];
const SECOND_TARGET_GAINS_DB: [f32; BANDS] = [-1.0, -3.0, 2.0, -4.0];
const RAMP_FRAMES: usize = 8_192;
const RESUME_FRAMES: usize = 8_192;
const BAND_GAIN_IDS: [&str; BANDS] = [
    "band_0_gain_db",
    "band_1_gain_db",
    "band_2_gain_db",
    "band_3_gain_db",
];
const CUTOFF_IDS: [&str; BANDS - 1] = ["frequency", "frequency_2", "frequency_3"];

struct LifecycleCountingAllocator;

static COUNT_AUDIO_MEMORY: AtomicBool = AtomicBool::new(false);
static AUDIO_ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static AUDIO_DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: Every operation delegates unchanged to `System`; the atomics only
// observe calls during a single test's explicitly armed reset/process window.
unsafe impl GlobalAlloc for LifecycleCountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if COUNT_AUDIO_MEMORY.load(Ordering::Relaxed) {
            AUDIO_ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        if COUNT_AUDIO_MEMORY.load(Ordering::Relaxed) {
            AUDIO_DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: LifecycleCountingAllocator = LifecycleCountingAllocator;

fn create_configured_plugin(
    crossover_type: &str,
    sample_rate: u32,
    cutoffs: [f32; BANDS - 1],
    gains_db: [f32; BANDS],
) -> BandSplitPlugin {
    let cutoffs_f64 = cutoffs.map(f64::from);
    let mut plugin = BandSplitPlugin::new_multiband_with_mode(
        CHANNELS,
        &cutoffs_f64,
        crossover_type,
        BandSplitRecombinationMode::PhaseCompensated,
    )
    .unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    set_gains(&mut plugin, gains_db);
    plugin.reset();
    assert_eq!(plugin.output_channels(), CHANNELS * BANDS);
    plugin
}

fn set_cutoffs(plugin: &mut BandSplitPlugin, cutoffs: [f32; BANDS - 1]) {
    for (id, value) in CUTOFF_IDS.into_iter().zip(cutoffs) {
        plugin
            .set_parameter(ParameterId::from(id), ParameterValue::Float(value))
            .unwrap();
    }
}

fn set_gains(plugin: &mut BandSplitPlugin, gains_db: [f32; BANDS]) {
    for (id, value) in BAND_GAIN_IDS.into_iter().zip(gains_db) {
        plugin
            .set_parameter(ParameterId::from(id), ParameterValue::Float(value))
            .unwrap();
    }
}

fn make_noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut left_state = seed.max(1);
    let mut right_state = seed.wrapping_add(0x9e37_79b9).max(1);
    let mut input = Vec::with_capacity(frames * CHANNELS);
    for _ in 0..frames {
        input.push(next_noise(&mut left_state) * 0.32);
        input.push(next_noise(&mut right_state) * 0.27);
    }
    input
}

fn next_noise(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    let unit = (*state >> 8) as f32 / 16_777_215.0;
    2.0 * unit - 1.0
}

fn process_audio(plugin: &mut BandSplitPlugin, input: &[f32], sample_rate: u32) -> Vec<f32> {
    assert_eq!(input.len() % CHANNELS, 0);
    let frames = input.len() / CHANNELS;
    let expected_len = frames * CHANNELS * BANDS;
    let mut output = vec![0.0; expected_len];
    let processed = plugin
        .process(
            input,
            &mut output,
            &ProcessContext::new(sample_rate, frames),
        )
        .unwrap();
    assert_eq!(processed, frames);
    assert_eq!(output.len(), expected_len);
    assert_valid_audio(input, &output, frames);
    output
}

fn assert_valid_audio(input: &[f32], output: &[f32], frames: usize) {
    assert_eq!(input.len(), frames * CHANNELS);
    assert_eq!(output.len(), frames * CHANNELS * BANDS);
    assert!(input.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!((0..frames).any(|frame| {
        input[frame * CHANNELS].to_bits() != input[frame * CHANNELS + 1].to_bits()
    }));
    for band in 0..BANDS {
        assert!(
            (0..frames).any(|frame| {
                (0..CHANNELS).any(|channel| {
                    output[frame * CHANNELS * BANDS + band * CHANNELS + channel].abs() > 1e-6
                })
            }),
            "band {band} did not produce nontrivial audio"
        );
    }
}

fn assert_same_audio(actual: &[f32], expected: &[f32], description: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{description}: length changed"
    );
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(expected.iter().all(|sample| sample.is_finite()));
    let mismatch = actual
        .iter()
        .zip(expected)
        .position(|(actual, expected)| actual.to_bits() != expected.to_bits());
    assert!(
        mismatch.is_none(),
        "{description}: first mismatch {mismatch:?}"
    );
}

fn verify_repeated_populated_resets(
    plugin: &mut BandSplitPlugin,
    crossover_type: &str,
    sample_rate: u32,
    cutoffs: [f32; BANDS - 1],
    gains_db: [f32; BANDS],
    seed: u32,
    epoch: usize,
) {
    let first_input = make_noise(RESUME_FRAMES, seed ^ 0x06e5_128b);
    let second_input = make_noise(RESUME_FRAMES, seed ^ 0x92c4_d731);
    let first_frames = first_input.len() / CHANNELS;
    let second_frames = second_input.len() / CHANNELS;
    let first_context = ProcessContext::new(sample_rate, first_frames);
    let second_context = ProcessContext::new(sample_rate, second_frames);
    let mut first_output = vec![0.0; first_frames * CHANNELS * BANDS];
    let mut second_output = vec![0.0; second_frames * CHANNELS * BANDS];
    let mut fresh_first = create_configured_plugin(crossover_type, sample_rate, cutoffs, gains_db);
    let mut fresh_second = create_configured_plugin(crossover_type, sample_rate, cutoffs, gains_db);

    AUDIO_ALLOCATIONS.store(0, Ordering::Relaxed);
    AUDIO_DEALLOCATIONS.store(0, Ordering::Relaxed);
    COUNT_AUDIO_MEMORY.store(true, Ordering::SeqCst);
    plugin.reset();
    let first_process = plugin.process(&first_input, &mut first_output, &first_context);
    plugin.reset();
    let second_process = plugin.process(&second_input, &mut second_output, &second_context);
    COUNT_AUDIO_MEMORY.store(false, Ordering::SeqCst);

    let allocations = AUDIO_ALLOCATIONS.load(Ordering::Relaxed);
    let deallocations = AUDIO_DEALLOCATIONS.load(Ordering::Relaxed);
    assert_eq!(
        allocations, 0,
        "{crossover_type} epoch {epoch} repeated reset/process allocated"
    );
    assert_eq!(
        deallocations, 0,
        "{crossover_type} epoch {epoch} repeated reset/process deallocated"
    );
    assert_eq!(first_process.unwrap(), first_frames);
    assert_eq!(second_process.unwrap(), second_frames);

    assert_valid_audio(&first_input, &first_output, first_frames);
    assert_valid_audio(&second_input, &second_output, second_frames);
    let fresh_first_output = process_audio(&mut fresh_first, &first_input, sample_rate);
    let fresh_second_output = process_audio(&mut fresh_second, &second_input, sample_rate);
    assert_same_audio(
        &first_output,
        &fresh_first_output,
        &format!("{crossover_type} epoch {epoch} first reset"),
    );
    assert_same_audio(
        &second_output,
        &fresh_second_output,
        &format!("{crossover_type} epoch {epoch} repeated reset"),
    );
}

fn four_band_compensated_reset_matches_fresh_targets_after_repeated_audio_epochs() {
    for crossover_type in ["LR24", "LR48"] {
        let mut populated = create_configured_plugin(
            crossover_type,
            INITIAL_SAMPLE_RATE,
            INITIAL_CUTOFFS,
            INITIAL_GAINS_DB,
        );

        for (epoch, (cutoffs, gains_db, seed)) in [
            (FIRST_TARGET_CUTOFFS, FIRST_TARGET_GAINS_DB, 0x14a2_7c51),
            (SECOND_TARGET_CUTOFFS, SECOND_TARGET_GAINS_DB, 0x8bd1_03e7),
        ]
        .into_iter()
        .enumerate()
        {
            set_cutoffs(&mut populated, cutoffs);
            set_gains(&mut populated, gains_db);
            let ramp_input = make_noise(RAMP_FRAMES, seed);
            let ramp_output = process_audio(&mut populated, &ramp_input, INITIAL_SAMPLE_RATE);
            assert_valid_audio(&ramp_input, &ramp_output, RAMP_FRAMES);
            verify_repeated_populated_resets(
                &mut populated,
                crossover_type,
                INITIAL_SAMPLE_RATE,
                cutoffs,
                gains_db,
                seed,
                epoch,
            );
        }
    }
}

fn populated_refusals_preserve_audio_and_changed_rate_retry_matches_fresh() {
    for crossover_type in ["LR24", "LR48"] {
        let mut candidate = create_configured_plugin(
            crossover_type,
            INITIAL_SAMPLE_RATE,
            INITIAL_CUTOFFS,
            INITIAL_GAINS_DB,
        );
        let mut untouched_twin = create_configured_plugin(
            crossover_type,
            INITIAL_SAMPLE_RATE,
            INITIAL_CUTOFFS,
            INITIAL_GAINS_DB,
        );

        for plugin in [&mut candidate, &mut untouched_twin] {
            set_cutoffs(plugin, FIRST_TARGET_CUTOFFS);
            set_gains(plugin, FIRST_TARGET_GAINS_DB);
        }

        let warm_input = make_noise(RAMP_FRAMES, 0x3a1d_67c9);
        let candidate_warm = process_audio(&mut candidate, &warm_input, INITIAL_SAMPLE_RATE);
        let twin_warm = process_audio(&mut untouched_twin, &warm_input, INITIAL_SAMPLE_RATE);
        assert_same_audio(&candidate_warm, &twin_warm, "paired warmup");

        assert!(candidate.initialize(0.0).is_err());
        let after_bad_rate_input = make_noise(RESUME_FRAMES, 0xc18f_204b);
        let candidate_after_bad_rate =
            process_audio(&mut candidate, &after_bad_rate_input, INITIAL_SAMPLE_RATE);
        let twin_after_bad_rate = process_audio(
            &mut untouched_twin,
            &after_bad_rate_input,
            INITIAL_SAMPLE_RATE,
        );
        assert_same_audio(
            &candidate_after_bad_rate,
            &twin_after_bad_rate,
            "rejected sample-rate preparation",
        );

        assert!(candidate.initialize(8_000.0).is_err());
        let after_inadmissible_rate_input = make_noise(RESUME_FRAMES, 0x4b16_f839);
        let candidate_after_inadmissible_rate = process_audio(
            &mut candidate,
            &after_inadmissible_rate_input,
            INITIAL_SAMPLE_RATE,
        );
        let twin_after_inadmissible_rate = process_audio(
            &mut untouched_twin,
            &after_inadmissible_rate_input,
            INITIAL_SAMPLE_RATE,
        );
        assert_same_audio(
            &candidate_after_inadmissible_rate,
            &twin_after_inadmissible_rate,
            "rejected rate below selected crossover cutoffs",
        );

        let rejected_cutoff = FIRST_TARGET_CUTOFFS[0] - 10.0;
        assert!(
            candidate
                .set_parameter(
                    ParameterId::from("frequency_2"),
                    ParameterValue::Float(rejected_cutoff),
                )
                .is_err()
        );
        assert_eq!(
            candidate.get_parameter(&ParameterId::from("frequency_2")),
            Some(ParameterValue::Float(FIRST_TARGET_CUTOFFS[1]))
        );
        let after_bad_control_input = make_noise(RESUME_FRAMES, 0x16df_2a83);
        let candidate_after_bad_control = process_audio(
            &mut candidate,
            &after_bad_control_input,
            INITIAL_SAMPLE_RATE,
        );
        let twin_after_bad_control = process_audio(
            &mut untouched_twin,
            &after_bad_control_input,
            INITIAL_SAMPLE_RATE,
        );
        assert_same_audio(
            &candidate_after_bad_control,
            &twin_after_bad_control,
            "rejected crossing update",
        );

        for plugin in [&mut candidate, &mut untouched_twin] {
            set_cutoffs(plugin, RETRY_CUTOFFS);
        }
        let valid_retry_input = make_noise(RESUME_FRAMES, 0x76c2_11ad);
        let candidate_after_valid_retry =
            process_audio(&mut candidate, &valid_retry_input, INITIAL_SAMPLE_RATE);
        let twin_after_valid_retry =
            process_audio(&mut untouched_twin, &valid_retry_input, INITIAL_SAMPLE_RATE);
        assert_same_audio(
            &candidate_after_valid_retry,
            &twin_after_valid_retry,
            "valid crossing retry",
        );

        candidate.initialize(f64::from(RETRY_SAMPLE_RATE)).unwrap();
        let mut fresh_retry = create_configured_plugin(
            crossover_type,
            RETRY_SAMPLE_RATE,
            RETRY_CUTOFFS,
            FIRST_TARGET_GAINS_DB,
        );
        let retry_input = make_noise(RESUME_FRAMES, 0xf013_98a5);
        let retried_audio = process_audio(&mut candidate, &retry_input, RETRY_SAMPLE_RATE);
        let fresh_audio = process_audio(&mut fresh_retry, &retry_input, RETRY_SAMPLE_RATE);
        assert_same_audio(
            &retried_audio,
            &fresh_audio,
            &format!("{crossover_type} changed-rate retry"),
        );
    }
}

#[test]
fn four_band_compensated_lifecycle_preserves_audio_and_heap_contracts() {
    four_band_compensated_reset_matches_fresh_targets_after_repeated_audio_epochs();
    populated_refusals_preserve_audio_and_changed_rate_retry_matches_fresh();
}
