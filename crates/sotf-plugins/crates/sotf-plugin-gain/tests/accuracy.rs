// Rust guideline compliant 2026-02-21

//! Independent accuracy suite for `sotf-plugin-gain`.
//!
//! Covers GAIN-R2 and GAIN-A1/A2/A3 from `audit/requirements/gain.md`. Every
//! expectation below comes from an independent oracle implemented in this file
//! in f64; no test calls the production dB converter (`sotf_host::db_to_linear`,
//! `GainPlugin::db_to_linear`, or the identical f32 `powf` expression) to build
//! an expected value. Existing suites that do reuse the production converter
//! prove plumbing, not conversion accuracy; this file closes that gap.
//!
//! Oracles:
//!
//! - Settled gain: `(input as f64) * 10f64.powf(db as f64 / 20.0)`.
//! - Gain ramps: closed-form one-pole trajectory
//!   `target + coeff^k * (start - target)` with the f64 coefficient
//!   `exp(-1 / (time_ms * 0.001 * sample_rate))`, matching the documented
//!   smoother contract (`time_ms` is the ~63% time constant).
//! - Structural properties: bit-exact unity passthrough, bit-exact
//!   compiled/scalar parity, bit-exact callback-partition invariance, and
//!   bit-exact parameter/preset round-trip audio.
//!
//! Tolerances (fixed before evaluating any candidate; see `result.md`):
//!
//! - `CONVERSION_REL_TOL = 1e-5` relative: the f64 oracle error is ~1e-15
//!   while the f32 candidate accumulates powf rounding (a few ulp) plus one
//!   multiply rounding (0.5 ulp), i.e. well under 1e-6 typically. The bound
//!   holds ~84 ulp (~10x margin) yet still catches formula errors (dB/10 vs
//!   dB/20 differ by orders of magnitude), hidden clamps, and sign errors.
//! - `SUBNORMAL_ABS_FLOOR = 1e-38` absolute: covers subnormal-scale
//!   expectations and hosts that flush subnormal inputs/results to zero
//!   (f32 minimum normal is ~1.1754944e-38). It only dominates the bound
//!   below ~1e-33 expectations.
//! - `RAMP_ABS_TOL = 5e-4` absolute: per-step f32 rounding (~6e-8) through a
//!   one-pole accumulates to roughly `per_step * tau_fs`, which is ~1.2e-4 at
//!   the widest tested rate (192 kHz, 10 ms); f32-vs-f64 coefficient
//!   discrepancy adds ~6e-5 more. The bound holds ~3x margin and still
//!   catches time-constant errors above ~3% (a 10% tau error moves the
//!   mid-ramp by ~0.018 absolute for the tested 0.5 step).
//!
//! Deliberately out of scope here: non-finite input behavior (no documented
//! contract), save/reload through shared engine/FFI/native adapters (shared
//! ownership; coordinator gate), and native-binary loading (platform lane).
//!
//! Evidence capture: every oracle test prints its worst observed error with a
//! `GAIN-WORST` marker (visible with `--nocapture`). These diagnostics report
//! only; bounds and assertions are fixed and identical with or without them.

use sotf_host::host::DawHost;
use sotf_host::param_specs::find_by_key;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_plugin::{ParameterSet, ParametricPlugin, ParametricPluginAdapter};
use sotf_host::plugin::{PluginCompiledOp, ProcessContext};
use sotf_plugin_gain::params::PARAMS;
use sotf_plugin_gain::{GainPlugin, GainPluginParams, default_smoothing_ms};

/// Relative tolerance for settled gain against the f64 oracle (see docs).
const CONVERSION_REL_TOL: f64 = 1e-5;
/// Absolute floor for subnormal-scale expectations (see docs).
const SUBNORMAL_ABS_FLOOR: f64 = 1e-38;
/// Absolute tolerance for closed-form ramp trajectories (see docs).
const RAMP_ABS_TOL: f64 = 5e-4;

/// Interior sweep points; range endpoints come from the parameter spec.
const DB_SWEEP_INTERIOR: [f32; 18] = [
    -48.0, -36.0, -24.0, -18.0, -12.0, -9.0, -6.0206, -6.0, -3.0103, -3.0, -1.0, -0.5, 0.5, 1.0,
    3.0, 6.0, 12.0, 18.0,
];

/// Signed sweep stimulus: unity, fractions, zero, tiny normals, the minimum
/// normal, and a subnormal value.
const SWEEP_INPUTS: [f32; 14] = [
    1.0,
    -1.0,
    0.5,
    -0.25,
    0.125,
    -0.75,
    0.0,
    3.25,
    -2.75,
    1.0e-10,
    -1.0e-10,
    1.0e-30,
    f32::MIN_POSITIVE,
    1.0e-40,
];

/// Independent dB-to-linear oracle in f64. Never calls production converters.
fn db_to_linear_f64(db: f32) -> f64 {
    10f64.powf(f64::from(db) / 20.0)
}

/// Independent one-pole coefficient matching the documented smoother contract.
fn one_pole_coeff_f64(time_ms: f32, sample_rate: u32) -> f64 {
    (-1.0 / (f64::from(time_ms) * 0.001 * f64::from(sample_rate))).exp()
}

/// Combined relative/absolute comparison for oracle checks.
fn within_tolerance(actual: f32, expected: f64, tol_rel: f64, tol_abs: f64) -> bool {
    let diff = (f64::from(actual) - expected).abs();
    diff <= tol_rel * expected.abs() + tol_abs
}

/// Deterministic signed stimulus covering positive, negative, and zero samples.
fn deterministic_pattern(len: usize) -> Vec<f32> {
    const STEPS: [f32; 8] = [0.5, -0.25, 1.0, -1.0, 0.125, -0.75, 0.0, 0.9];
    (0..len).map(|i| STEPS[i % STEPS.len()]).collect()
}

/// Render `input` through `plugin` as a single block at `sample_rate`.
fn render_one_block(plugin: &mut GainPlugin, sample_rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = plugin.input_channels();
    assert_eq!(input.len() % channels, 0, "input must hold whole frames");
    let frames = input.len() / channels;
    let mut output = vec![0.0f32; input.len()];
    let processed = plugin
        .process(input, &mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert_eq!(processed, frames);
    output
}

/// GAIN-A1: settled global gain matches the independent f64 oracle across the
/// full contract range (spec endpoints included) and signed/tiny inputs.
#[test]
fn settled_global_gain_matches_f64_reference_sweep() {
    let spec = find_by_key(PARAMS, "gain_db");
    let spec_min = spec.min_f64() as f32;
    let spec_max = spec.max_f64() as f32;
    assert_eq!(spec_min, -60.0, "contract range minimum");
    assert_eq!(spec_max, 20.0, "contract range maximum");

    const CHANNELS: usize = 2;
    const FRAMES: usize = 1003; // Odd length: SIMD body plus scalar tail.
    const SAMPLE_RATE: u32 = 48_000;
    let mut dbs = Vec::with_capacity(DB_SWEEP_INTERIOR.len() + 2);
    dbs.push(spec_min);
    dbs.extend_from_slice(&DB_SWEEP_INTERIOR);
    dbs.push(spec_max);

    let mut worst_score = 0.0f64;
    let mut worst_case = (0.0f32, 0.0f32);
    let mut worst_diff = 0.0f64;
    for &db in &dbs {
        let mut plugin = GainPlugin::with_smoothing(CHANNELS, db, 0.0);
        plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        let input: Vec<f32> = (0..FRAMES * CHANNELS)
            .map(|i| SWEEP_INPUTS[i % SWEEP_INPUTS.len()])
            .collect();
        let output = render_one_block(&mut plugin, SAMPLE_RATE, &input);
        let gain = db_to_linear_f64(db);
        for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
            let expected = f64::from(sample) * gain;
            let diff = (f64::from(actual) - expected).abs();
            let allowed = CONVERSION_REL_TOL * expected.abs() + SUBNORMAL_ABS_FLOOR;
            let score = diff / allowed;
            if score > worst_score {
                worst_score = score;
                worst_case = (db, sample);
                worst_diff = diff;
            }
            assert!(
                score <= 1.0,
                "db={db} sample[{index}]={sample}: actual={actual} \
                 expected={expected} diff={diff} allowed={allowed}"
            );
        }
    }
    assert!(
        worst_score <= 1.0,
        "worst normalized conversion error {worst_score} at db={} input={} (abs diff {worst_diff})",
        worst_case.0,
        worst_case.1
    );
    eprintln!(
        "GAIN-WORST conversion-sweep: worst normalized error {worst_score:.6} \
         (bound 1.0) at db={} input={} (abs diff {worst_diff:.6e})",
        worst_case.0, worst_case.1
    );
}

/// GAIN-A1: settled per-channel gains match the f64 oracle per channel.
#[test]
fn settled_per_channel_gain_matches_f64_reference() {
    const CHANNEL_GAINS: [f32; 4] = [-60.0, -6.0, 0.0, 20.0];
    const FRAMES: usize = 259;
    const SAMPLE_RATE: u32 = 48_000;
    let mut plugin =
        GainPlugin::new_per_channel_with_smoothing(CHANNEL_GAINS.to_vec(), 0.0).unwrap();
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..FRAMES * CHANNEL_GAINS.len())
        .map(|i| SWEEP_INPUTS[(i * 7 + 3) % SWEEP_INPUTS.len()])
        .collect();
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &input);
    let mut worst_score = 0.0f64;
    let mut worst_case = (0usize, 0.0f32);
    for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
        let expected = f64::from(sample) * db_to_linear_f64(CHANNEL_GAINS[index % CHANNEL_GAINS.len()]);
        let diff = (f64::from(actual) - expected).abs();
        let allowed = CONVERSION_REL_TOL * expected.abs() + SUBNORMAL_ABS_FLOOR;
        let score = diff / allowed;
        if score > worst_score {
            worst_score = score;
            worst_case = (index, sample);
        }
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "sample[{index}]={sample} ch={}: actual={actual} expected={expected}",
            index % CHANNEL_GAINS.len()
        );
    }
    eprintln!(
        "GAIN-WORST per-channel-4ch: worst normalized error {worst_score:.6} \
         (bound 1.0) at sample {} input={} (ch {})",
        worst_case.0,
        worst_case.1,
        worst_case.0 % CHANNEL_GAINS.len()
    );
}

/// GAIN-A1: stereo per-channel gains match the oracle, covering the stereo
/// kernel (1000 SIMD frames) plus one scalar frame.
#[test]
fn settled_stereo_per_channel_gain_matches_f64_reference() {
    const CHANNEL_GAINS: [f32; 2] = [6.0, -6.0];
    const FRAMES: usize = 1001;
    const SAMPLE_RATE: u32 = 48_000;
    let mut plugin =
        GainPlugin::new_per_channel_with_smoothing(CHANNEL_GAINS.to_vec(), 0.0).unwrap();
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..FRAMES * CHANNEL_GAINS.len())
        .map(|i| SWEEP_INPUTS[(i * 5 + 1) % SWEEP_INPUTS.len()])
        .collect();
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &input);
    let mut worst_score = 0.0f64;
    let mut worst_case = (0usize, 0.0f32);
    for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
        let expected = f64::from(sample) * db_to_linear_f64(CHANNEL_GAINS[index % 2]);
        let diff = (f64::from(actual) - expected).abs();
        let allowed = CONVERSION_REL_TOL * expected.abs() + SUBNORMAL_ABS_FLOOR;
        let score = diff / allowed;
        if score > worst_score {
            worst_score = score;
            worst_case = (index, sample);
        }
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "sample[{index}]={sample}: actual={actual} expected={expected}"
        );
    }
    eprintln!(
        "GAIN-WORST stereo-per-channel: worst normalized error {worst_score:.6} \
         (bound 1.0) at sample {} input={} (ch {})",
        worst_case.0,
        worst_case.1,
        worst_case.0 % 2
    );
}

/// GAIN-A1: a large single block matches the oracle (no accumulation drift in
/// the settled kernel).
#[test]
fn large_block_matches_f64_reference() {
    const CHANNELS: usize = 2;
    const FRAMES: usize = 65536;
    const DB: f32 = 20.0;
    const SAMPLE_RATE: u32 = 48_000;
    let mut plugin = GainPlugin::with_smoothing(CHANNELS, DB, 0.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| if i % 2 == 0 { 0.5 } else { -0.5 })
        .collect();
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &input);
    let gain = db_to_linear_f64(DB);
    let mut worst_score = 0.0f64;
    let mut worst_case = (0usize, 0.0f32);
    for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
        let expected = f64::from(sample) * gain;
        let diff = (f64::from(actual) - expected).abs();
        let allowed = CONVERSION_REL_TOL * expected.abs() + SUBNORMAL_ABS_FLOOR;
        let score = diff / allowed;
        if score > worst_score {
            worst_score = score;
            worst_case = (index, sample);
        }
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "sample[{index}]: actual={actual} expected={expected}"
        );
    }
    eprintln!(
        "GAIN-WORST large-block: worst normalized error {worst_score:.6} \
         (bound 1.0) at sample {} input={}",
        worst_case.0, worst_case.1
    );
}

/// GAIN-A2: an automated gain ramp matches the closed-form one-pole oracle
/// sample by sample across representative rates, starting at the exact event
/// frame and decreasing monotonically while still ramping.
#[test]
fn gain_ramp_matches_closed_form_exponential() {
    const TARGET_DB: f32 = -6.0;
    const SMOOTHING_MS: f32 = 10.0;
    const RATES: [u32; 4] = [44_100, 48_000, 96_000, 192_000];
    // 0 dB start is exactly 1.0 by IEEE pow(x, 0) == 1; the production
    // constructor holds current == target there, so the event starts clean.
    const START_LINEAR: f64 = 1.0;
    for &rate in &RATES {
        let mut plugin = GainPlugin::with_smoothing(1, 0.0, SMOOTHING_MS);
        plugin.plugin_initialize(f64::from(rate)).unwrap();
        plugin.set_gain_db(TARGET_DB);
        // Four time constants: deep into the ramp yet far above snap.
        let frames = (4 * (rate as usize)) / 100;
        let input = vec![1.0f32; frames];
        let output = render_one_block(&mut plugin, rate, &input);
        let target = db_to_linear_f64(TARGET_DB);
        let coeff = one_pole_coeff_f64(SMOOTHING_MS, rate);
        // Frame k uses the smoother state after k+1 advances.
        let mut decay = coeff;
        let mut worst = 0.0f64;
        let mut worst_frame = 0usize;
        for (frame, &actual) in output.iter().enumerate() {
            let expected = target + decay * (START_LINEAR - target);
            let diff = (f64::from(actual) - expected).abs();
            if diff > worst {
                worst = diff;
                worst_frame = frame;
            }
            assert!(
                diff <= RAMP_ABS_TOL,
                "rate={rate} frame={frame}: actual={actual} expected={expected} diff={diff}"
            );
            if frame > 0 {
                assert!(
                    actual < output[frame - 1],
                    "rate={rate} frame={frame}: ramp must decrease monotonically"
                );
            }
            decay *= coeff;
        }
        assert!(
            output[0] < 1.0,
            "rate={rate}: first post-event frame must already move"
        );
        assert!(
            f64::from(output[frames - 1]) > target + 1e-3,
            "rate={rate}: ramp must still be converging at the block end"
        );
        assert!(
            worst <= RAMP_ABS_TOL,
            "rate={rate}: worst closed-form ramp error {worst}"
        );
        eprintln!(
            "GAIN-WORST ramp: rate={rate} worst abs error {worst:.6e} \
             (bound {RAMP_ABS_TOL}) at frame {worst_frame}"
        );
    }
}

/// GAIN-A2: a mid-stream per-channel event applies at the exact sample offset:
/// the untouched channel stays bit-exact while the automated channel follows
/// the closed-form ramp from the first post-event frame.
#[test]
fn per_channel_event_applies_at_exact_sample_offset() {
    const SAMPLE_RATE: u32 = 48_000;
    const SMOOTHING_MS: f32 = 10.0;
    const PRE_FRAMES: usize = 128;
    const POST_FRAMES: usize = 500;
    const EVENT_DB: f32 = -12.0;
    let mut plugin = GainPlugin::with_smoothing(2, 0.0, SMOOTHING_MS);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let pre = render_one_block(&mut plugin, SAMPLE_RATE, &vec![1.0f32; PRE_FRAMES * 2]);
    assert!(
        pre.iter().all(|&sample| sample == 1.0),
        "settled unity pre-roll must be exact"
    );
    plugin.set_channel_gain_db(1, EVENT_DB).unwrap();
    let post = render_one_block(&mut plugin, SAMPLE_RATE, &vec![1.0f32; POST_FRAMES * 2]);
    let target = db_to_linear_f64(EVENT_DB);
    let coeff = one_pole_coeff_f64(SMOOTHING_MS, SAMPLE_RATE);
    let mut decay = coeff;
    let mut worst = 0.0f64;
    let mut worst_frame = 0usize;
    for frame in 0..POST_FRAMES {
        let left = post[frame * 2];
        let right = post[frame * 2 + 1];
        assert_eq!(left, 1.0, "untouched channel stays bit-exact at frame {frame}");
        let expected = target + decay * (1.0 - target);
        let diff = (f64::from(right) - expected).abs();
        if diff > worst {
            worst = diff;
            worst_frame = frame;
        }
        assert!(
            diff <= RAMP_ABS_TOL,
            "frame={frame}: actual={right} expected={expected} diff={diff}"
        );
        decay *= coeff;
    }
    assert!(
        post[1] < 1.0,
        "automated channel moves on the first post-event frame"
    );
    eprintln!(
        "GAIN-WORST per-channel-event: worst abs error {worst:.6e} \
         (bound {RAMP_ABS_TOL}) at frame {worst_frame}"
    );
}

/// GAIN-A2: the compiled apply-gain op renders bit-identical audio to the
/// scalar path during an active ramp; other compiled ops are declined.
#[test]
fn compiled_path_matches_scalar_during_active_ramp() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 513;
    let input = deterministic_pattern(FRAMES * 2);
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let mut scalar = GainPlugin::with_smoothing(2, 0.0, 10.0);
    let mut compiled = GainPlugin::with_smoothing(2, 0.0, 10.0);
    scalar.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    compiled.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    scalar.set_gain_db(-6.0);
    compiled.set_gain_db(-6.0);
    let mut scalar_out = vec![0.0f32; input.len()];
    let mut compiled_out = vec![0.0f32; input.len()];
    let scalar_frames = scalar.process(&input, &mut scalar_out, &context).unwrap();
    let compiled_frames = compiled
        .process_compiled_f32(
            PluginCompiledOp::ApplyGain,
            &input,
            &mut compiled_out,
            &context,
        )
        .expect("gain accepts the compiled apply-gain op")
        .unwrap();
    assert_eq!(scalar_frames, FRAMES);
    assert_eq!(compiled_frames, FRAMES);
    assert_eq!(compiled_out, scalar_out);
    // The implied per-frame gain must move across the block, proving the
    // parity above holds on the moving ramp path rather than settled audio.
    // Pattern positions 0 and (FRAMES-1)*2 both hold nonzero 0.5 inputs.
    let first_gain = scalar_out[0] / input[0];
    let last_gain = scalar_out[(FRAMES - 1) * 2] / input[(FRAMES - 1) * 2];
    assert!(
        first_gain != last_gain,
        "ramp must move across the block: first={first_gain} last={last_gain}"
    );
    assert!(
        compiled
            .process_compiled_f32(
                PluginCompiledOp::EqBiquadBank,
                &input,
                &mut compiled_out,
                &context
            )
            .is_none(),
        "gain only implements the apply-gain compiled op"
    );
}

/// Render `input` with `set_gain_db(event_db)` applied exactly at frame
/// `event_at`, splitting callbacks by `partitions`.
fn render_with_event_at(
    input: &[f32],
    partitions: &[usize],
    event_at: usize,
    event_db: f32,
) -> Vec<f32> {
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: usize = 2;
    let mut plugin = GainPlugin::with_smoothing(2, 0.0, 10.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut rendered = Vec::with_capacity(input.len());
    let mut frame = 0;
    let mut event_fired = false;
    for &frames in partitions {
        if frame == event_at {
            plugin.set_gain_db(event_db);
            event_fired = true;
        }
        let start = frame * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![0.0f32; frames * CHANNELS];
        let processed = plugin
            .process(
                &input[start..end],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap();
        assert_eq!(processed, frames);
        rendered.extend_from_slice(&block);
        frame += frames;
    }
    assert_eq!(frame * CHANNELS, input.len());
    assert!(event_fired, "partitions must split at the event frame");
    rendered
}

/// GAIN-A2: callback partitioning is bit-identical when a mid-stream
/// automation event lands at the same absolute frame.
#[test]
fn callback_partitioning_is_bit_identical_with_midstream_event() {
    const CHANNELS: usize = 2;
    const TOTAL_FRAMES: usize = 256;
    const EVENT_AT_FRAME: usize = 100;
    const EVENT_DB: f32 = -12.0;
    let input = deterministic_pattern(TOTAL_FRAMES * CHANNELS);
    let wide = render_with_event_at(&input, &[100, 156], EVENT_AT_FRAME, EVENT_DB);
    let medium = render_with_event_at(
        &input,
        &[25, 25, 25, 25, 52, 52, 52],
        EVENT_AT_FRAME,
        EVENT_DB,
    );
    let mut fine_partitions = vec![1usize; 100];
    fine_partitions.resize(100 + 52, 3);
    let fine = render_with_event_at(&input, &fine_partitions, EVENT_AT_FRAME, EVENT_DB);
    assert_eq!(medium, wide);
    assert_eq!(fine, wide);
}

/// GAIN-A2/GAIN-A3: a settled fused static gain is the bit-exact multiplier
/// applied to every sample.
#[test]
fn settled_static_gain_is_bit_exact_multiplier() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 1025;
    let mut plugin = GainPlugin::with_smoothing(2, -6.0, 10.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let static_gain = plugin
        .compiled_static_gain()
        .expect("settled global gain fuses to a static scalar");
    let input = deterministic_pattern(FRAMES * 2);
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &input);
    for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
        assert_eq!(
            actual,
            sample * static_gain,
            "sample {index} must equal input times the fused gain"
        );
    }
}

/// GAIN-A3: 0 dB passes every finite sample through bit-exactly in both modes.
/// Subnormal inputs are verified with tolerance in the conversion sweep
/// instead: bit-exactness there would depend on host MXCSR flags outside the
/// plugin's control.
#[test]
fn unity_gain_is_bit_exact_passthrough() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 1001;
    const UNITY_INPUTS: [f32; 10] = [
        1.0,
        -1.0,
        0.5,
        -0.25,
        0.0,
        -0.0,
        0.125,
        1.0e-30,
        f32::MIN_POSITIVE,
        -3.25,
    ];
    let input: Vec<f32> = (0..FRAMES * 2)
        .map(|i| UNITY_INPUTS[i % UNITY_INPUTS.len()])
        .collect();
    let mut global = GainPlugin::with_smoothing(2, 0.0, 0.0);
    global.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(
        render_one_block(&mut global, SAMPLE_RATE, &input),
        input,
        "global unity must pass samples through bit-exactly"
    );
    let mut per_channel = GainPlugin::new_per_channel_with_smoothing(vec![0.0, 0.0], 0.0).unwrap();
    per_channel.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(
        render_one_block(&mut per_channel, SAMPLE_RATE, &input),
        input,
        "per-channel unity must pass samples through bit-exactly"
    );
}

/// GAIN-A3: the -60 dB floor attenuates (never gates to zero) per the oracle.
#[test]
fn minimum_gain_attenuates_without_hidden_gate() {
    const SAMPLE_RATE: u32 = 48_000;
    const DB: f32 = -60.0;
    const INPUTS: [f32; 4] = [1.0, 0.5, -0.25, 1.0e-30];
    let mut plugin = GainPlugin::with_smoothing(1, DB, 0.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &INPUTS);
    let gain = db_to_linear_f64(DB);
    for (index, (&actual, &sample)) in output.iter().zip(INPUTS.iter()).enumerate() {
        let expected = f64::from(sample) * gain;
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "sample[{index}]={sample}: actual={actual} expected={expected}"
        );
        assert_ne!(
            actual, 0.0,
            "sample[{index}]: -60 dB must attenuate, not gate to zero"
        );
    }
}

/// GAIN-A3: the +20 dB ceiling applies full boost with no hidden clamp.
#[test]
fn maximum_gain_applies_without_hidden_clamp() {
    const SAMPLE_RATE: u32 = 48_000;
    const DB: f32 = 20.0;
    const INPUTS: [f32; 3] = [0.5, -0.25, 1.0];
    let mut plugin = GainPlugin::with_smoothing(1, DB, 0.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let output = render_one_block(&mut plugin, SAMPLE_RATE, &INPUTS);
    let gain = db_to_linear_f64(DB);
    for (index, (&actual, &sample)) in output.iter().zip(INPUTS.iter()).enumerate() {
        let expected = f64::from(sample) * gain;
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "sample[{index}]={sample}: actual={actual} expected={expected}"
        );
    }
    assert!(
        output[0] > 1.0,
        "+20 dB on 0.5 must exceed 1.0; got {}",
        output[0]
    );
    assert!(
        output[2] > 1.0,
        "+20 dB on 1.0 must exceed 1.0; got {}",
        output[2]
    );
}

/// GAIN-A3: rejected updates leave the accepted configuration and rendered
/// audio untouched.
#[test]
fn rejected_parameter_updates_keep_accepted_state_and_audio() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 100;
    let mut plugin = GainPlugin::with_smoothing(2, 0.0, 10.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = deterministic_pattern(FRAMES * 2);
    let accepted_values = plugin.current_values();
    let accepted_audio = render_one_block(&mut plugin, SAMPLE_RATE, &input);
    const BAD_UPDATES: [(&str, f32); 6] = [
        ("gain_db", 21.0),
        ("gain_db", -60.1),
        ("gain_db", f32::NAN),
        ("gain_db", f32::INFINITY),
        ("smoothing_ms", 100.1),
        ("smoothing_ms", -1.0),
    ];
    for (id, value) in BAD_UPDATES {
        assert!(
            plugin
                .parametric_set_parameter(ParameterId::from(id), ParameterValue::Float(value))
                .is_err(),
            "{id}={value} must be rejected"
        );
    }
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("gain_db_5"), ParameterValue::Float(0.0))
            .is_err(),
        "out-of-range channel index must be rejected"
    );
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("nonexistent"), ParameterValue::Float(1.0))
            .is_err(),
        "unknown parameter must be rejected"
    );
    assert_eq!(plugin.gain_db(), 0.0);
    assert_eq!(plugin.current_values(), accepted_values);
    assert_eq!(
        render_one_block(&mut plugin, SAMPLE_RATE, &input),
        accepted_audio
    );
}

/// GAIN-A3: preset JSON round-trips preserve rendered audio; legacy partial
/// states load canonical defaults; malformed states are rejected before any
/// DSP state exists.
#[test]
fn preset_round_trip_preserves_rendered_audio() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 513;
    let input = deterministic_pattern(FRAMES * 2);
    for params in [
        GainPluginParams {
            gain_db: -6.0,
            smoothing_ms: 10.0,
            channel_gains: vec![],
        },
        GainPluginParams {
            gain_db: 0.0,
            smoothing_ms: 5.0,
            channel_gains: vec![3.0, -3.0],
        },
    ] {
        let json = serde_json::to_string(&params).unwrap();
        let restored: GainPluginParams = serde_json::from_str(&json).unwrap();
        let mut direct = GainPlugin::from_params(2, params).unwrap();
        let mut reloaded = GainPlugin::from_params(2, restored).unwrap();
        direct.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        reloaded.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        direct.plugin_reset();
        reloaded.plugin_reset();
        assert_eq!(
            render_one_block(&mut direct, SAMPLE_RATE, &input),
            render_one_block(&mut reloaded, SAMPLE_RATE, &input),
            "preset JSON round-trip must preserve rendered audio"
        );
    }
    let legacy: GainPluginParams = serde_json::from_str("{\"gain_db\": 6.0}").unwrap();
    assert_eq!(legacy.smoothing_ms, default_smoothing_ms());
    assert!(legacy.channel_gains.is_empty());
    let overrange: GainPluginParams =
        serde_json::from_str("{\"gain_db\": 999.0, \"smoothing_ms\": 10.0}").unwrap();
    assert!(GainPlugin::from_params(2, overrange).is_err());
    let mismatched = GainPluginParams {
        gain_db: 0.0,
        smoothing_ms: 10.0,
        channel_gains: vec![0.0],
    };
    assert!(GainPlugin::from_params(2, mismatched).is_err());
}

/// GAIN-A3: a `current_values` snapshot restores every parameter and renders
/// bit-identical audio after reset, in both automation modes.
#[test]
fn parameter_snapshot_round_trip_preserves_values_and_audio() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 513;
    let input = deterministic_pattern(FRAMES * 2);

    let mut global = GainPlugin::with_smoothing(2, -6.0, 25.0);
    global.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut per_channel =
        GainPlugin::new_per_channel_with_smoothing(vec![-6.0, 3.0], 5.0).unwrap();
    per_channel.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();

    for original in [&mut global, &mut per_channel] {
        let snapshot = original.current_values();
        let mut restored = GainPlugin::with_smoothing(2, 0.0, 10.0);
        restored.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        restored.apply_values(snapshot.clone()).unwrap();
        assert_eq!(
            restored.is_per_channel(),
            original.is_per_channel(),
            "snapshot restore must preserve the automation mode"
        );
        for (id, value) in &snapshot {
            assert_eq!(
                restored.parametric_get_parameter(id),
                Some(value.clone()),
                "parameter {id} must round-trip"
            );
        }
        original.plugin_reset();
        restored.plugin_reset();
        assert_eq!(
            restored.compile_metadata(),
            original.compile_metadata(),
            "snapshot restore must preserve compiled-plan fusion behavior"
        );
        assert_eq!(
            render_one_block(original, SAMPLE_RATE, &input),
            render_one_block(&mut restored, SAMPLE_RATE, &input),
            "snapshot-restored plugin must render bit-identical audio"
        );
    }
}

/// GAIN-A3 regression: restoring a global-mode snapshot keeps global mode and
/// its static-gain fusion. The mirrored `gain_db_{N}` entries in a global
/// snapshot must not convert the restored instance to per-channel state.
#[test]
fn apply_values_restores_global_mode_for_global_snapshots() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 513;
    let mut original = GainPlugin::with_smoothing(2, -6.0, 25.0);
    original.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let snapshot = original.current_values();
    // Restore into both a global and a per-channel instance: the snapshot
    // mode wins either way.
    for fresh in [
        GainPlugin::with_smoothing(2, 0.0, 10.0),
        GainPlugin::new_per_channel_with_smoothing(vec![3.0, 3.0], 5.0).unwrap(),
    ] {
        let mut restored = fresh;
        restored.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        restored.apply_values(snapshot.clone()).unwrap();
        assert!(
            !restored.is_per_channel(),
            "a global snapshot must restore as global mode"
        );
        original.plugin_reset();
        restored.plugin_reset();
        assert_eq!(
            restored.compile_metadata(),
            original.compile_metadata(),
            "restored global gain must keep static-gain fusion"
        );
        let input = deterministic_pattern(FRAMES * 2);
        assert_eq!(
            render_one_block(&mut restored, SAMPLE_RATE, &input),
            render_one_block(&mut original, SAMPLE_RATE, &input),
            "mode-faithful restore must render bit-identical audio"
        );
    }
}

/// GAIN-A3 regression: `apply_values` validates the whole set before mutating,
/// so a mixed valid/invalid set is rejected with the accepted configuration
/// and mid-ramp audio history fully retained.
#[test]
fn apply_values_rejects_mixed_validity_transactionally() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 256;
    let input = deterministic_pattern(FRAMES * 2);
    let float = ParameterValue::Float;
    let invalid_sets: Vec<Vec<(&str, ParameterValue)>> = vec![
        // Review check: a valid prefix must not stick past an invalid entry.
        vec![("gain_db", float(-6.0)), ("gain_db_1", float(999.0))],
        vec![("gain_db", float(999.0))],
        vec![("smoothing_ms", float(999.0))],
        vec![("gain_db_5", float(0.0))],
        vec![("nonexistent", float(1.0))],
        vec![("gain_db", float(f32::NAN))],
        vec![("gain_db", ParameterValue::Int(-6))],
    ];
    for (case, entries) in invalid_sets.iter().enumerate() {
        let mut plugin = GainPlugin::with_smoothing(2, -3.0, 100.0);
        let mut reference = GainPlugin::with_smoothing(2, -3.0, 100.0);
        plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        reference.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
        // Keep a ramp in flight so rejection must preserve live history.
        plugin.set_gain_db(-18.0);
        reference.set_gain_db(-18.0);
        let accepted_values = plugin.current_values();
        let mut rejected = ParameterSet::new();
        for (id, value) in entries {
            rejected.insert(ParameterId::from(*id), value.clone());
        }
        assert!(
            plugin.apply_values(rejected).is_err(),
            "case {case}: invalid set must be rejected"
        );
        assert_eq!(
            plugin.current_values(),
            accepted_values,
            "case {case}: rejected set must retain accepted values"
        );
        assert_eq!(
            render_one_block(&mut plugin, SAMPLE_RATE, &input),
            render_one_block(&mut reference, SAMPLE_RATE, &input),
            "case {case}: rejected set must retain mid-ramp audio history"
        );
    }
    // A fully valid set still applies.
    let mut plugin = GainPlugin::with_smoothing(2, 0.0, 10.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut valid = ParameterSet::new();
    valid.insert(ParameterId::from("gain_db"), float(-12.0));
    valid.insert(ParameterId::from("smoothing_ms"), float(5.0));
    plugin.apply_values(valid).unwrap();
    assert_eq!(plugin.gain_db(), -12.0);
}

/// GAIN-A3 pin: `set_channel_gains` is documented bulk-replacement snap
/// semantics (unlike the ramping `set_channel_gain_db`): the next block
/// already carries the full new gains, matching a freshly constructed
/// settled instance.
#[test]
fn set_channel_gains_snaps_smoothers_to_targets() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 257;
    const NEW_GAINS: [f32; 2] = [-12.0, 3.0];
    let input = deterministic_pattern(FRAMES * 2);
    // Start mid-ramp so a ramping implementation would visibly differ.
    let mut plugin = GainPlugin::with_smoothing(2, 0.0, 100.0);
    plugin.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin.set_gain_db(-20.0);
    plugin.set_channel_gains(NEW_GAINS.to_vec()).unwrap();
    assert!(plugin.is_per_channel());
    assert_eq!(plugin.channel_gain_db(0), Some(NEW_GAINS[0]));
    assert_eq!(plugin.channel_gain_db(1), Some(NEW_GAINS[1]));
    let mut settled =
        GainPlugin::new_per_channel_with_smoothing(NEW_GAINS.to_vec(), 100.0).unwrap();
    settled.plugin_initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(
        render_one_block(&mut plugin, SAMPLE_RATE, &input),
        render_one_block(&mut settled, SAMPLE_RATE, &input),
        "bulk channel set must snap: first post-call block equals settled targets"
    );
    // Length and range violations reject without touching state.
    assert!(plugin.set_channel_gains(vec![0.0]).is_err());
    assert!(plugin.set_channel_gains(vec![0.0, 999.0]).is_err());
    assert_eq!(plugin.channel_gain_db(0), Some(NEW_GAINS[0]));
    assert_eq!(plugin.channel_gain_db(1), Some(NEW_GAINS[1]));
}

/// Whole-chain (partial): per-channel automation through a real compiled
/// `DawHost` plan into output matches the f64 oracle. Save/reload through
/// shared engine/FFI/native adapters stays with the coordinator gate.
#[test]
fn host_automation_chain_matches_f64_reference() {
    const SAMPLE_RATE: u32 = 48_000;
    const FRAMES: usize = 512;
    const CHANNEL_GAINS: [f32; 2] = [6.0, -6.0];
    let mut host = DawHost::new(2, SAMPLE_RATE);
    host.add_plugin(Box::new(ParametricPluginAdapter::new(
        GainPlugin::with_smoothing(2, 0.0, 0.0),
    )))
    .unwrap();
    host.set_plugin_parameter(0, "gain_db_0", ParameterValue::Float(CHANNEL_GAINS[0]))
        .unwrap();
    host.set_plugin_parameter(0, "gain_db_1", ParameterValue::Float(CHANNEL_GAINS[1]))
        .unwrap();
    let input = deterministic_pattern(FRAMES * 2);
    let mut output = vec![0.0f32; input.len()];
    host.process(&input, &mut output).unwrap();
    let mut worst_score = 0.0f64;
    let mut worst_case = (0usize, 0.0f32);
    for (index, (&actual, &sample)) in output.iter().zip(input.iter()).enumerate() {
        let expected = f64::from(sample) * db_to_linear_f64(CHANNEL_GAINS[index % 2]);
        let diff = (f64::from(actual) - expected).abs();
        let allowed = CONVERSION_REL_TOL * expected.abs() + SUBNORMAL_ABS_FLOOR;
        let score = diff / allowed;
        if score > worst_score {
            worst_score = score;
            worst_case = (index, sample);
        }
        assert!(
            within_tolerance(
                actual,
                expected,
                CONVERSION_REL_TOL,
                SUBNORMAL_ABS_FLOOR
            ),
            "host sample[{index}]={sample}: actual={actual} expected={expected}"
        );
    }
    eprintln!(
        "GAIN-WORST host-chain: worst normalized error {worst_score:.6} \
         (bound 1.0) at sample {} input={} (ch {})",
        worst_case.0,
        worst_case.1,
        worst_case.0 % 2
    );
}
