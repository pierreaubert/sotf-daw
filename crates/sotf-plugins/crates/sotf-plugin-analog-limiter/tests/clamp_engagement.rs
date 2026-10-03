//! Clamp-engagement proof for the running Stop legs' limited stimulus.
//!
//! The running chain's limited Stop legs gate on the observed ceiling
//! (latch reads exactly C, then Stop, then terminal == C). That gate is
//! sound only if the stimulus actually drives post-color program above C
//! early and repeatedly: the fs/4 tone failure (R24 — post-color max
//! 0.24400914 < C, gate deadline) proved hot input alone is no such
//! guarantee. This test renders the limited-leg program class (the hot
//! 440/660 + 1000 Hz dyad, i16-quantized and production-decoded) through
//! the real [`AnalogLimiterPlugin`] (Tape, -12 dB, 5 ms lookahead — the
//! running chain's parameters, engine 512-frame blocks) and proves the
//! final clamp engages within the first second and recurs densely after.
//! Transfer to the FIFO legs: same DSP code, same params, same
//! rate/channels/chunking, stationary clamping program — and the online
//! gate re-verifies engagement on the actual bytes, so a stimulus failure
//! can only fail loud, never false-pass. A contrast leg pins the R24
//! mechanism: the same chain holds the maximum-slew fs/4 tone below C.

// Rust guideline compliant 2026-02-21
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};

/// Sample rate matching the running chain.
const RATE: u32 = 48_000;
/// Engine processing block size (frames).
const BLOCK_FRAMES: usize = 512;
/// Rendered blocks: 188 x 512 = 96256 frames (~2.005 s) — covers the 1 s
/// engagement window plus the 1 s density window with margin.
const BLOCKS: usize = 188;
/// Total rendered frames.
const TOTAL_FRAMES: usize = BLOCKS * BLOCK_FRAMES;
/// Engagement deadline: first clamped sample within 1 s of program
/// (predicted ~ms — core transient plus steady-state clamping every
/// carrier cycle — so 1 s carries two orders of magnitude).
const FIRST_CLAMP_FRAMES: usize = 48_000;
/// Density window after the first clamp (frames).
const DENSITY_WINDOW_FRAMES: usize = 48_000;
/// Minimum clamped frames in the density window: recurrence proof (every
/// carrier cycle re-clamps, so hundreds are expected; ten already kills
/// any single-alignment lottery).
const MIN_DENSITY_HITS: usize = 10;
/// Hot-program driver band, mirroring the running suite's
/// `HOT_PROGRAM_PEAK_*`: same program class, same proof that the bytes
/// are the intended hot driver. Not a clamp precondition — clamping
/// needs no rare alignment since every carrier cycle exceeds C
/// post-core.
const PEAK_FLOOR: f32 = 0.85;
/// Upper end of the driver band: the generator clamps to +-1.0 and i16
/// quantizes below it, so anything above is generator drift.
const PEAK_CEILING: f32 = 1.0;
/// Limited-leg threshold in dB, matching the running chain.
const THRESHOLD_DB: f32 = -12.0;
/// fs/4 contrast-tone amplitude, mirroring the running suite's
/// `TONE_AMPLITUDE` (same hot tone that missed the ceiling in R24).
const TONE_AMPLITUDE: f32 = 0.99;

/// Production decode: i16 fixture sample to f32, production divisor.
///
/// Mirrors the symphonia S16 arm (`i as f32 / 32768.0`) exactly — NOT the
/// 32767 of the fixture-writer quantization it inverts.
fn decode_i16(quantized: i16) -> f32 {
    f32::from(quantized) / 32768.0
}

/// Fixture quantization: f32 program sample to i16, writer formula.
///
/// Mirrors `write_hot_wav` / `write_tone_wav` op-for-op (clamp, scale by
/// `i16::MAX`, truncate toward zero).
fn quantize_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// Hot dyad program, interleaved stereo, production-decoded f32.
///
/// Same formula as the running suite's `write_hot_wav` (0.75 at 440/660
/// Hz plus 0.15 at 1 kHz per channel): same program class and statistics
/// as the FIFO'd bytes; the hotness-band assert couples the mirror, and
/// the online gate re-verifies engagement on the actual bytes.
fn hot_dyad(frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let time = frame as f32 / RATE as f32;
        let left = (time * 440.0 * std::f32::consts::TAU).sin() * 0.75
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15;
        let right = (time * 660.0 * std::f32::consts::TAU).sin() * 0.75
            + (time * 1000.0 * std::f32::consts::TAU).sin() * 0.15;
        out.push(decode_i16(quantize_i16(left)));
        out.push(decode_i16(quantize_i16(right)));
    }
    out
}

/// fs/4 contrast tone, interleaved stereo, production-decoded f32.
///
/// Same analytic program as the running suite's `write_tone_wav`
/// (sine phase left, cosine phase right): maximum slew, the case the
/// Tape ADAA stage holds below the ceiling.
fn fs4_tone(frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    for frame in 0..frames {
        let left = match frame % 4 {
            0 => 0.0,
            1 => TONE_AMPLITUDE,
            2 => 0.0,
            _ => -TONE_AMPLITUDE,
        };
        let right = match frame % 4 {
            0 => TONE_AMPLITUDE,
            1 => 0.0,
            2 => -TONE_AMPLITUDE,
            _ => 0.0,
        };
        out.push(decode_i16(quantize_i16(left)));
        out.push(decode_i16(quantize_i16(right)));
    }
    out
}

/// Render interleaved stereo through the running chain's plugin.
///
/// Fresh plugin, explicit running-chain parameters (any drift fails at
/// construction/validation, never silently), engine 512-frame blocks.
/// Returns the full output for sample-exact assertions.
fn render(input: &[f32]) -> Vec<f32> {
    let params = AnalogLimiterPluginParams {
        threshold: THRESHOLD_DB,
        release: 50.0,
        lookahead: 5.0,
        soft: false,
        true_peak: false,
        mix: 1.0,
        analog_model: "Tape".to_string(),
        analog_drive: 6.0,
        analog_color: 0.5,
        analog_character: 0.25,
        analog_trim: 0.0,
    };
    let mut plugin = AnalogLimiterPlugin::from_params(2, params).expect("plugin must construct");
    plugin.initialize(RATE).expect("plugin must initialize");
    let mut output = vec![0.0; input.len()];
    for (block_index, chunk) in input.chunks(BLOCK_FRAMES * 2).enumerate() {
        let mut block = chunk.to_vec();
        let done = plugin
            .process_in_place(&mut block, &ProcessContext::new(RATE, BLOCK_FRAMES))
            .expect("block must process");
        assert_eq!(done, BLOCK_FRAMES, "block {block_index} short");
        let start = block_index * BLOCK_FRAMES * 2;
        output[start..start + block.len()].copy_from_slice(&block);
    }
    output
}

/// Absolute peak of an interleaved buffer.
fn abs_peak(buffer: &[f32]) -> f32 {
    buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max)
}

/// Whether either channel of a stereo frame reads exactly `target` bits.
fn frame_hits(frame: &[f32], target: f32) -> bool {
    frame[0].abs().to_bits() == target.to_bits() || frame[1].abs().to_bits() == target.to_bits()
}

#[test]
fn hot_dyad_drives_final_clamp_early_and_repeatedly() {
    let ceiling = 10f32.powf(THRESHOLD_DB / 20.0);
    assert_eq!(
        ceiling.to_bits(),
        0.25118864f32.to_bits(),
        "ceiling formula drifted from the running suite literal"
    );
    let input = hot_dyad(TOTAL_FRAMES);
    let input_peak = abs_peak(&input);
    assert!(
        (PEAK_FLOOR..=PEAK_CEILING).contains(&input_peak),
        "dyad mirror outside the driver band: peak {input_peak} not in [{PEAK_FLOOR}, {PEAK_CEILING}]"
    );
    let output = render(&input);
    assert!(
        output.iter().all(|s| s.is_finite()),
        "non-finite output sample"
    );
    assert!(
        output.iter().all(|s| s.abs() <= ceiling),
        "output exceeds the ceiling: R1 contract violation"
    );
    let max = abs_peak(&output);
    assert_eq!(
        max.to_bits(),
        ceiling.to_bits(),
        "final clamp never engaged on the hot dyad — stimulus invalid"
    );
    let (frames, remainder) = output.as_chunks::<2>();
    assert!(
        remainder.is_empty(),
        "output length must be whole stereo frames"
    );
    let first = frames
        .iter()
        .position(|frame| frame_hits(frame, ceiling))
        .expect("no clamped frame in 2 s of hot dyad — stimulus invalid");
    assert!(
        first < FIRST_CLAMP_FRAMES,
        "first clamp at frame {first}, past the 1 s engagement deadline"
    );
    let hits = frames
        .iter()
        .skip(first)
        .take(DENSITY_WINDOW_FRAMES)
        .filter(|frame| frame_hits(*frame, ceiling))
        .count();
    assert!(
        hits >= MIN_DENSITY_HITS,
        "only {hits} clamped frames in the 1 s density window — recurrence unproven"
    );
    eprintln!(
        "dyad clamp proof: max_bits={:08x} first_clamp_frame={first} density_hits={hits}",
        max.to_bits()
    );
}

#[test]
fn fs4_tone_stays_below_ceiling_after_color() {
    // Mechanism pin for the R24 gate deadline: core-first architecture
    // plus Tape ADAA step-averaging holds maximum-slew program below the
    // dry (core-limited) level, so the final clamp never engages — hot
    // input is not a clamp guarantee. The 0.2 floor keeps this honest: a
    // degenerate (silent/broken) render would pass the upper pin
    // vacuously.
    let ceiling = 10f32.powf(THRESHOLD_DB / 20.0);
    let input = fs4_tone(TOTAL_FRAMES);
    let output = render(&input);
    assert!(
        output.iter().all(|s| s.is_finite()),
        "non-finite output sample"
    );
    let max = abs_peak(&output);
    assert!(
        max < ceiling,
        "fs/4 tone unexpectedly reaches the ceiling (max {max:.8}) — color response changed?"
    );
    assert!(
        max > 0.2,
        "fs/4 tone render degenerate (max {max:.8}) — expected hot colored output near 0.244"
    );
    eprintln!(
        "contrast fs/4 tone: max={max:.8} bits={:08x}",
        max.to_bits()
    );
}
