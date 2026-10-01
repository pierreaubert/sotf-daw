//! Derived slew artifact bounds and full-trajectory checks.
//!
//! ADDITIVE coverage for review P1-1 and P2-8; the existing
//! `smooth_cutoff.rs` upward test (3 dB HF release, 0.5 dB LF) is untouched.
//! Bounds here are derived a-priori from the independent analytic table
//! model plus the published contract, and every magic number carries its
//! provenance per ms-rust M-DOCUMENTED-MAGIC.
//!
//! Error budgets:
//! - HF 3 dB: the analytic steady-state delta between the least-attenuated
//!   transition table (rank 4, cutoff 0.7071) and the wide table (1.0) at
//!   18 kHz exceeds 4 dB for every preset (asserted below); the 3 dB bound
//!   keeps a 1 dB margin for FIR history mixing across the table switch.
//!   History is the same input window for both modes, so the mixed output
//!   stays within 1 dB of the steady-state delta.
//! - LF 0.1 dB (5x tighter than the existing 0.5 dB): 0.02 dB steady-state
//!   table delta (twice the 0.01 dB flat-band ripple, triangle inequality
//!   at 997 Hz, which sits below the tested flat band and closer to the
//!   DC-normalized flat point) + 0.05 dB transient allowance for the
//!   one-table-per-chunk coefficient switch + 0.03 dB RMS measurement
//!   margin over the 4-block window.
//! - Residual -50 dB: the 0.02 dB table delta is a 0.23% amplitude step
//!   (-52.7 dB); the -50 dB bound allows 2.7 dB for short-window
//!   least-squares fit uncertainty over ~2k samples (vs the -90 dB
//!   steady-state bound over a 200 ms window). Fix-r7 keeps the -50 dB
//!   bound unchanged; the fit itself is corrected from the coherent-only
//!   `2/N` projection to the true 2x2 least-squares solution (see
//!   `fit_tone`), because the transition window is non-coherent
//!   (997 Hz at 96 kHz over ~2048 samples = ~21.27 cycles). The old
//!   projection's normal-equation perturbation is bounded by
//!   `1/(2 sin w)` ~7.7 counts vs `N/2 = 1024` (~0.75% = -42.5 dB),
//!   exactly the observed false residual; the true solution removes this
//!   estimator bias and isolates real spurious.
//! - Per-block P2-8: smoothed HF at or below instant within 1.0 dB
//!   (single-block RMS noise plus transient), and nondecreasing within
//!   1.0 dB per step when the current block is measurable (at or above the
//!   derived linear floor; fix-r9 F1 passes legitimate below-to-above
//!   release crossing while large above-to-below drops still fail via the
//!   linear bound). Below the floor the check is linear-energy bounded, not
//!   a dB ratio, because dB below the f32/filter floor measures noise, not
//!   filter monotonicity. The floor is derived from the established 2e-6
//!   alias-amplitude bound plus IEEE-754 precision (see `LINEAR_AMP_FLOOR`);
//!   no constant comes from the failed probes.
//!
//! Tags: `SLEW-BOUNDS` (analytic + measured + residual), `SLEW-TRAJECTORY`
//! (per-block HF), `SLEW-CLOCK` (cumulative-clock uniformity),
//! `SLEW-REGRESSION` (oracle sensitivity vs chirp separation). The canonical
//! transition window is blocks 9..13 (ranks 1..4); the full slew trajectory
//! is blocks 9..=17 (ranks 1..9).

// Rust guideline compliant 2026-02-21
use rubato::{WindowFunction, calculate_cutoff};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_resampler::{ResamplerPlugin, ResamplerQuality};
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const CHUNK: usize = 256;
const AMPLITUDE: f64 = 0.5;
const QUALITIES: [ResamplerQuality; 3] = [
    ResamplerQuality::Fast,
    ResamplerQuality::Medium,
    ResamplerQuality::High,
];

/// Linear peak-amplitude floor for measurable audio differences.
///
/// Why 2e-6: this is the established alias-amplitude bound from the
/// `dynamic_cutoff` suite (analytic-vs-measured alias amplitude agrees
/// within 2e-6), preserved here, not fitted to any slew probe. It covers
/// worst-case f32 FIR rounding (256 taps * (eps/2) * 0.5 peak * ~0.2 peak
/// coefficient ~= 1.5e-6, with eps = 2^-23) plus input/output f32
/// quantization (<= 3e-8 at 0.5 peak). Any linear difference below this
/// floor is measurement uncertainty, not filter response. Lowering the
/// floor would claim sub-LSB FIR accuracy the f32 path cannot provide;
/// raising it would hide real 0.02 dB (1150e-6 amplitude) table steps,
/// which sit 575x above it. The corresponding power floor is -108 dB
/// (see `floor_power_db`); the failed -141/-145 dB probes lie 33 dB below
/// it and are therefore correctly classified as below-floor noise.
const LINEAR_AMP_FLOOR: f64 = 2e-6;

/// RMS floor derived from the peak floor for sine-like signals.
fn floor_rms() -> f64 {
    LINEAR_AMP_FLOOR / 2.0_f64.sqrt()
}

/// Power floor in dB relative to the 0.5-peak test tone.
///
/// 20*log10(2e-6/0.5) = -108 dB amplitude; power uses the same number
/// because 10*log10(power ratio) = 20*log10(RMS ratio).
fn floor_power_db() -> f64 {
    20.0 * (LINEAR_AMP_FLOOR / AMPLITUDE).log10()
}

/// Linear RMS (not dB) for floor-aware comparisons.
fn linear_rms(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|x| f64::from(*x).powi(2))
        .sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt()
}

fn make(quality: ResamplerQuality, channels: usize, smoothing: bool) -> ResamplerPlugin {
    let mut plugin = ResamplerPlugin::with_quality(channels, RATE, RATE, CHUNK, quality).unwrap();
    plugin.initialize(RATE).unwrap();
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

fn specs(quality: ResamplerQuality) -> (usize, usize) {
    match quality {
        ResamplerQuality::Fast => (64, 128),
        ResamplerQuality::Medium => (128, 256),
        ResamplerQuality::High => (256, 256),
    }
}

/// Analytic linear magnitude of one prepared table (independent DTFT).
fn analytic_gain(
    taps: usize,
    phases: usize,
    ratio_scale: f64,
    freq_hz: f64,
    input_rate: u32,
) -> f64 {
    let cutoff = f64::from(
        calculate_cutoff::<f32>(taps, WindowFunction::BlackmanHarris2) * ratio_scale as f32,
    );
    let prototype = |position: f64| {
        let angle = TAU * position / taps as f64;
        let window = 0.35875 - 0.48829 * angle.cos() + 0.14128 * (2.0 * angle).cos()
            - 0.01168 * (3.0 * angle).cos();
        let argument = std::f64::consts::PI * cutoff * (position - taps as f64 / 2.0);
        let sinc = if argument == 0.0 {
            1.0
        } else {
            argument.sin() / argument
        };
        window * window * sinc
    };
    let normalization = (0..taps * phases)
        .map(|index| prototype(index as f64 / phases as f64))
        .sum::<f64>()
        / phases as f64;
    let (real, imaginary) = (0..taps).fold((0.0, 0.0), |(real, imaginary), tap| {
        let coefficient =
            prototype(tap as f64 + (phases - 1) as f64 / phases as f64) / normalization;
        let angle = TAU * freq_hz * tap as f64 / f64::from(input_rate);
        (
            real + coefficient * angle.cos(),
            imaginary - coefficient * angle.sin(),
        )
    });
    real.hypot(imaginary)
}

fn analytic_db(
    taps: usize,
    phases: usize,
    ratio_scale: f64,
    freq_hz: f64,
    input_rate: u32,
) -> f64 {
    20.0 * analytic_gain(taps, phases, ratio_scale, freq_hz, input_rate).log10()
}

/// True least-squares tone fit over the given mono samples.
///
/// Model `y[n] ~= a*sin(w n) + b*cos(w n)` with `w = 2 pi f/Fs`.
/// Normal equations `[Sss Ssc; Ssc Scc][a;b] = [Sys;Syc]` where `Sss =
/// sum sin^2`, `Scc = sum cos^2`, `Ssc = sum sin cos`, `Sys = sum y sin`,
/// `Syc = sum y cos`. Solution `det = Sss*Scc - Ssc^2`, `a =
/// (Sys*Scc - Syc*Ssc)/det`, `b = (Syc*Sss - Sys*Ssc)/det`. For coherent
/// windows (integer cycles) `Sss = Scc = N/2`, `Ssc = 0`, so this reduces
/// to the `2/N` projection used by the steady-state suites; for the short
/// non-coherent transition window (~21.27 cycles) the full solution
/// removes the `1/(2 sin w)` perturbation (~0.75% = -42.5 dB) that the
/// coherent-only projection leaves as false residual. Returns
/// (gain dB vs 0.5 peak, residual dB vs input RMS).
fn fit_tone(samples: &[f32], rate: u32, frequency: f64) -> (f64, f64) {
    let count = samples.len().max(1);
    let mut sum_ss = 0.0;
    let mut sum_cc = 0.0;
    let mut sum_sc = 0.0;
    let mut sum_ys = 0.0;
    let mut sum_yc = 0.0;
    for (frame, &value) in samples.iter().enumerate() {
        let phase = TAU * frequency * frame as f64 / f64::from(rate);
        let sine = phase.sin();
        let cosine = phase.cos();
        let sample = f64::from(value);
        sum_ss += sine * sine;
        sum_cc += cosine * cosine;
        sum_sc += sine * cosine;
        sum_ys += sample * sine;
        sum_yc += sample * cosine;
    }
    let determinant = sum_ss * sum_cc - sum_sc * sum_sc;
    // Determinant is ~N^2/4 for any window with N >> 1/sin w (here N~2048,
    // w=0.065 rad, det ~1e6); zero would mean a degenerate single-sample
    // window, which the transition (4 blocks) never produces.
    assert!(
        determinant > 0.0,
        "least-squares fit is degenerate for {} samples",
        count
    );
    let sin_gain = (sum_ys * sum_cc - sum_yc * sum_sc) / determinant;
    let cos_gain = (sum_yc * sum_ss - sum_ys * sum_sc) / determinant;
    let fitted = sin_gain.hypot(cos_gain);
    let residual = samples
        .iter()
        .enumerate()
        .map(|(frame, &value)| {
            let phase = TAU * frequency * frame as f64 / f64::from(rate);
            let error = f64::from(value) - sin_gain * phase.sin() - cos_gain * phase.cos();
            error * error
        })
        .sum::<f64>()
        / count as f64;
    let input_rms = AMPLITUDE / 2.0_f64.sqrt();
    (
        20.0 * (fitted / AMPLITUDE).log10(),
        20.0 * (residual.sqrt() / input_rms).log10(),
    )
}

/// Coherent-only `2/N` projection kept to prove the old oracle's bias.
///
/// Valid only for integer-cycle windows (steady-state 200 ms suites). Used
/// solely by the regression test to show it reports ~-42 dB false residual
/// on the same non-coherent transition data where the true solution reports
/// the real spurious. Never used for acceptance.
fn fit_tone_coherent_only(samples: &[f32], rate: u32, frequency: f64) -> (f64, f64) {
    let count = samples.len().max(1);
    let (sin_sum, cos_sum) = samples.iter().enumerate().fold(
        (0.0, 0.0),
        |(s, c), (frame, &value)| {
            let phase = TAU * frequency * frame as f64 / f64::from(rate);
            (
                s + f64::from(value) * phase.sin(),
                c + f64::from(value) * phase.cos(),
            )
        },
    );
    let sin_gain = 2.0 * sin_sum / count as f64;
    let cos_gain = 2.0 * cos_sum / count as f64;
    let fitted = sin_gain.hypot(cos_gain);
    let residual = samples
        .iter()
        .enumerate()
        .map(|(frame, &value)| {
            let phase = TAU * frequency * frame as f64 / f64::from(rate);
            let error = f64::from(value) - sin_gain * phase.sin() - cos_gain * phase.cos();
            error * error
        })
        .sum::<f64>()
        / count as f64;
    let input_rms = AMPLITUDE / 2.0_f64.sqrt();
    (
        20.0 * (fitted / AMPLITUDE).log10(),
        20.0 * (residual.sqrt() / input_rms).log10(),
    )
}

/// Least-squares tone fit with arbitrary sample times (clock-derived ideal).
///
/// Model `y[n] ~= a*sin(2 pi f t[n]) + b*cos(2 pi f t[n])` where `t[n]` are
/// absolute input-clock seconds (e.g. `(origin+anchor)/Fs_in` from the
/// cumulative-clock trace). Solves the same 2x2 normal equations as
/// `fit_tone` but with clock times instead of uniform `n/Fs`. A single
/// constant gain/phase fit over the whole window absorbs the documented
/// signal-delay (constant group delay) and flat-band gain, not timing drift:
/// drift or chirp cannot be absorbed by constant `a,b` and appears as
/// residual. Returns (gain dB vs 0.5 peak, residual dB vs input RMS).
fn fit_tone_with_times(samples: &[f32], times: &[f64], frequency: f64) -> (f64, f64) {
    assert_eq!(
        samples.len(),
        times.len(),
        "clock-ideal fit requires one time per sample"
    );
    let count = samples.len().max(1);
    let mut sum_ss = 0.0;
    let mut sum_cc = 0.0;
    let mut sum_sc = 0.0;
    let mut sum_ys = 0.0;
    let mut sum_yc = 0.0;
    for (&value, &time) in samples.iter().zip(times.iter()) {
        let phase = TAU * frequency * time;
        let sine = phase.sin();
        let cosine = phase.cos();
        let sample = f64::from(value);
        sum_ss += sine * sine;
        sum_cc += cosine * cosine;
        sum_sc += sine * cosine;
        sum_ys += sample * sine;
        sum_yc += sample * cosine;
    }
    let determinant = sum_ss * sum_cc - sum_sc * sum_sc;
    assert!(
        determinant > 0.0,
        "clock-ideal fit is degenerate for {} samples",
        count
    );
    let sin_gain = (sum_ys * sum_cc - sum_yc * sum_sc) / determinant;
    let cos_gain = (sum_yc * sum_ss - sum_ys * sum_sc) / determinant;
    let fitted = sin_gain.hypot(cos_gain);
    let residual = samples
        .iter()
        .zip(times.iter())
        .map(|(&value, &time)| {
            let phase = TAU * frequency * time;
            let error = f64::from(value) - sin_gain * phase.sin() - cos_gain * phase.cos();
            error * error
        })
        .sum::<f64>()
        / count as f64;
    let input_rms = AMPLITUDE / 2.0_f64.sqrt();
    (
        20.0 * (fitted / AMPLITUDE).log10(),
        20.0 * (residual.sqrt() / input_rms).log10(),
    )
}

/// Cumulative-clock oracle for the 0.5 -> 2.0 ramp schedule.
///
/// Never calls Rubato iterators, sizing helpers, or the plugin. Non-independent
/// disclosure: it replays the same scalar recurrence as
/// `dynamic_endpoint::Clock` (same `-(taps-1)` init, increment-first order,
/// bisection boundary `chunk - (taps+1)`, `current = target` after each
/// block), so a recurrence bug would repeat in both. Independence comes from
/// measurement, not recurrence: per-block output lengths are asserted equal
/// to genuine `render_upward` production counts for all 29 blocks (including
/// the ramp), and moving-leg audio is fitted against both this clock-derived
/// ideal and an independent uniform 1/96 kHz ideal (no recurrence). The trace
/// records `(origin, anchor, step)` per emitted output, from which the
/// coherent ideal at actual output samples is defined as
/// `A*sin(2 pi f*(origin+anchor)/Fs_in)`.
struct SlewClock {
    chunk: usize,
    taps: usize,
    index: f64,
    origin: i128,
    current: f64,
    target: f64,
    trace: Vec<(i128, f64, f64)>,
}

impl SlewClock {
    fn new(chunk: usize, taps: usize, nominal: f64) -> Self {
        Self {
            chunk,
            taps,
            index: -(taps as f64 - 1.0),
            origin: 0,
            current: nominal,
            target: nominal,
            trace: Vec::new(),
        }
    }

    fn set_ratio(&mut self, target: f64, ramp: bool) {
        self.target = target;
        if !ramp {
            self.current = target;
        }
    }

    fn last_anchor(&self, frames: usize) -> f64 {
        let mut position = self.index;
        let mut step = self.current.recip();
        let increment = (self.target.recip() - step) / frames as f64;
        for _ in 0..frames {
            step += increment;
            position += step;
        }
        position
    }

    /// Processes one backend chunk, returning per-output steps for this block.
    fn block(&mut self) -> Vec<f64> {
        let boundary = self.chunk as f64 - (self.taps + 1) as f64;
        let mut upper = 1;
        while self.last_anchor(upper) <= boundary {
            upper *= 2;
        }
        let mut lower = 0;
        while upper - lower > 1 {
            let middle = (upper + lower) / 2;
            if self.last_anchor(middle) <= boundary {
                lower = middle;
            } else {
                upper = middle;
            }
        }
        let frames = lower;
        let mut step = self.current.recip();
        let increment = if frames == 0 {
            0.0
        } else {
            (self.target.recip() - step) / frames as f64
        };
        let mut steps = Vec::with_capacity(frames);
        for _ in 0..frames {
            step += increment;
            self.index += step;
            self.trace.push((self.origin, self.index, step));
            steps.push(step);
        }
        self.index -= self.chunk as f64;
        self.origin += self.chunk as i128;
        self.current = self.target;
        steps
    }

    /// Feeds input frames, processing whole chunks; returns per-block steps.
    fn feed(&mut self, frames: usize, pending: &mut usize) -> Vec<Vec<f64>> {
        *pending += frames;
        let mut blocks = Vec::new();
        while *pending >= self.chunk {
            blocks.push(self.block());
            *pending -= self.chunk;
        }
        blocks
    }
}

/// Renders the fixed-2.0 unchanged-ratio control leg (no ramp, no slew).
///
/// Same tones, chunking, and windowing as the transition, but the ratio is
/// set to 2.0 before any input and never changes. Both modes use the wide
/// table throughout, so any difference between this leg and the transition
/// leg isolates the ramp/chirp plus cutoff-switch contribution. Returns
/// per-block outputs for the smoothed instance (identical to instant here).
fn render_fixed_two(quality: ResamplerQuality) -> Vec<Vec<f32>> {
    let mut plugin = make(quality, 2, true);
    plugin.set_ratio(2.0, false).unwrap();
    let mut blocks = Vec::new();
    let mut position = 0;
    for _ in 0..29 {
        let input = tones(CHUNK, 2, &[997.0, 18_000.0], position);
        position += CHUNK;
        let mut out = Vec::new();
        feed(&mut plugin, &input, CHUNK, &mut out);
        blocks.push(out);
    }
    blocks
}

fn render_upward(
    quality: ResamplerQuality,
) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let mut smoothed = make(quality, 2, true);
    let mut instant = make(quality, 2, false);
    smoothed.set_ratio(0.5, false).unwrap();
    instant.set_ratio(0.5, false).unwrap();
    let mut smooth_blocks = Vec::new();
    let mut instant_blocks = Vec::new();
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
    (smooth_blocks, instant_blocks)
}

#[test]
fn upward_transition_has_derived_hf_release_and_tight_lf() {
    // Nominal-1.0 bank cutoffs ascending: 0.5, 0.5453, 0.5946, 0.6484,
    // 0.7071, 0.7711, 0.8409, 0.9170, 0.999, 1.0 (8 grid intervals per
    // octave plus the drift anchor; pinned by the unit table-count test).
    // Blocks 9..13 use ranks 1..4; rank 4 (0.7071 = 2^(-4/8)) is the least
    // attenuated, so its delta to wide lower-bounds the window release.
    let rank4 = 2.0_f64.powf(-4.0 / 8.0);
    for quality in QUALITIES {
        let (taps, phases) = specs(quality);
        let wide_db = analytic_db(taps, phases, 1.0, 18_000.0, RATE);
        let rank4_db = analytic_db(taps, phases, rank4, 18_000.0, RATE);
        let analytic_delta = wide_db - rank4_db;
        eprintln!(
            "SLEW-BOUNDS {quality:?} analytic 18kHz wide={wide_db:.2}dB rank4={rank4_db:.2}dB delta={analytic_delta:.2}dB"
        );
        assert!(
            analytic_delta > 4.0,
            "{quality:?}: analytic rank4-to-wide delta {analytic_delta:.2} dB must exceed \
             the 3 dB bound with a 1 dB history-mixing margin"
        );
        let (smooth_blocks, instant_blocks) = render_upward(quality);
        let transition_smooth: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let transition_instant: Vec<f32> =
            instant_blocks[9..13].iter().flatten().copied().collect();
        let hf_smooth = rms_db(&channel_samples(&transition_smooth, 2, 1));
        let hf_instant = rms_db(&channel_samples(&transition_instant, 2, 1));
        let lf_smooth = rms_db(&channel_samples(&transition_smooth, 2, 0));
        let lf_instant = rms_db(&channel_samples(&transition_instant, 2, 0));
        eprintln!(
            "SLEW-BOUNDS {quality:?} measured HF smooth={hf_smooth:.2}dB instant={hf_instant:.2}dB; \
             LF smooth={lf_smooth:.3}dB instant={lf_instant:.3}dB"
        );
        assert!(
            hf_smooth < hf_instant - 3.0,
            "{quality:?}: smoothed widening must release HF energy gradually"
        );
        assert!(
            (lf_smooth - lf_instant).abs() < 0.1,
            "{quality:?}: smoothing must preserve the passband within 0.1 dB"
        );
    }
}

#[test]
fn upward_transition_coherent_residual_bounds_spurious() {
    // Transition blocks run at the stable target ratio 2.0, so the effective
    // output clock is 96 kHz and the 997 Hz LF fit uses rate 96 kHz. The
    // -50 dB bound isolates spurious (images, switching transients) from the
    // fundamental, which total-energy RMS cannot do. Fix-r7 uses the true
    // 2x2 least-squares fit (same -50 dB bound); the coherent-only
    // projection is proven biased on this non-coherent window by the
    // regression test below and is never used for acceptance.
    for quality in QUALITIES {
        let (smooth_blocks, instant_blocks) = render_upward(quality);
        let transition_smooth: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let transition_instant: Vec<f32> =
            instant_blocks[9..13].iter().flatten().copied().collect();
        let lf_smooth = channel_samples(&transition_smooth, 2, 0);
        let lf_instant = channel_samples(&transition_instant, 2, 0);
        let (gain_smooth, residual_smooth) = fit_tone(&lf_smooth, 96_000, 997.0);
        let (gain_instant, residual_instant) = fit_tone(&lf_instant, 96_000, 997.0);
        eprintln!(
            "SLEW-BOUNDS {quality:?} LF residual smooth={residual_smooth:.1}dB \
             (gain {gain_smooth:.3}dB) instant={residual_instant:.1}dB (gain {gain_instant:.3}dB)"
        );
        assert!(
            residual_smooth < -50.0,
            "{quality:?}: transition spurious {residual_smooth:.1} dB exceeds -50 dB"
        );
    }
}

#[test]
fn slew_full_trajectory_uses_unified_window() {
    // Canonical window 9..13 (ranks 1..4) in both integration and QA; the
    // full trajectory 9..=17 (ranks 1..9) is checked per block. Smoothed HF
    // holds narrower tables longer, so it sits at or below instant and rises
    // toward it as the slew converges. Fix-r7 is floor-aware: dB ratios are
    // meaningful only at or above the derived -108 dB power floor (2e-6
    // peak); below it the check is a bounded linear-energy difference, and
    // the final converged block must sit 20 dB above the floor to prove the
    // trajectory is measurable. Per-block 9..17 coverage is kept.
    let meas_floor = floor_power_db();
    let rms_floor = floor_rms();
    for quality in QUALITIES {
        let (smooth_blocks, instant_blocks) = render_upward(quality);
        let early_smooth: Vec<f32> =
            smooth_blocks[9..11].iter().flatten().copied().collect();
        let early_instant: Vec<f32> =
            instant_blocks[9..11].iter().flatten().copied().collect();
        let hf_early_smooth = rms_db(&channel_samples(&early_smooth, 2, 1));
        let hf_early_instant = rms_db(&channel_samples(&early_instant, 2, 1));
        assert!(
            hf_early_smooth < hf_early_instant - 3.0,
            "{quality:?}: legacy QA window 9..11 must also meet 3 dB"
        );
        let mut previous_db = f64::NEG_INFINITY;
        let mut previous_rms = 0.0;
        let mut have_previous = false;
        let mut final_db = f64::NEG_INFINITY;
        for (index, (smooth, instant)) in smooth_blocks
            .iter()
            .zip(instant_blocks.iter())
            .enumerate()
            .skip(9)
            .take(9)
        {
            let smooth_hf = channel_samples(smooth, 2, 1);
            let instant_hf = channel_samples(instant, 2, 1);
            let hf_smooth = rms_db(&smooth_hf);
            let hf_instant = rms_db(&instant_hf);
            let rms_smooth = linear_rms(&smooth_hf);
            eprintln!(
                "SLEW-TRAJECTORY {quality:?} block={index} smooth={hf_smooth:.2}dB instant={hf_instant:.2}dB floor={meas_floor:.0}dB"
            );
            assert!(
                hf_smooth <= hf_instant + 1.0,
                "{quality:?} block {index}: smoothed HF must not exceed instant"
            );
            if have_previous {
                // Fix-r9 F1: use the dB monotonic guard whenever the current
                // block is measurable, and the linear bound only when current
                // is below the floor. The previous `both_measurable` form
                // conflated legitimate upward floor-crossing release (block 12
                // -108.07 dB to block 13 -59.81 dB, +48 dB) with noise,
                // requiring a 3.6e-4 linear step to sit below the 1.4e-6 RMS
                // floor. Upward crossing passes here because current (+48 dB)
                // exceeds previous - 1 dB. Large above-to-below drops still
                // fail: e.g. previous -59 dB (rms ~4e-4) to current below
                // -108 dB (rms <1.4e-6) gives a linear step ~4e-4, 280x above
                // the floor, tripping the else branch. Small drops within ~6 dB
                // of the floor may pass via the linear bound, matching
                // measurement uncertainty near the floor.
                let current_measurable = hf_smooth >= meas_floor;
                if current_measurable {
                    assert!(
                        hf_smooth >= previous_db - 1.0,
                        "{quality:?} block {index}: measurable HF must rise within 1 dB"
                    );
                } else {
                    // Current below floor: bound the linear RMS step by the
                    // floor itself (measurement uncertainty), not a dB ratio.
                    // A 4 dB dip at -143 dB is ~2e-8 linear, far below the
                    // 1.4e-6 RMS floor, so it passes here while a large
                    // above-to-below drop (step >> floor) still fails.
                    let step = (rms_smooth - previous_rms).abs();
                    assert!(
                        step < rms_floor,
                        "{quality:?} block {index}: below-floor linear step {step:.3e} \
                         exceeds the RMS floor {rms_floor:.3e}"
                    );
                }
            }
            previous_db = hf_smooth;
            previous_rms = rms_smooth;
            have_previous = true;
            if index == 17 {
                final_db = hf_smooth;
            }
        }
        // The converged wide table must release HF measurably: 20 dB (10x
        // amplitude) above the floor keeps the dB reading accurate to
        // 20*log10(1 +/- 0.1) ~= +/-0.8 dB, inside the 1 dB monotonic guard.
        // Why 20 dB: 10x the uncertainty bounds the floor-induced dB error
        // below the 1 dB trajectory tolerance; a smaller margin would let
        // floor noise masquerade as filter steps.
        assert!(
            final_db > meas_floor + 20.0,
            "{quality:?}: converged HF {final_db:.1} dB must exceed the floor \
             {meas_floor:.0} dB by 20 dB to prove a measurable trajectory"
        );
    }
    // Synthetic sensitivity proof for the F1 guard (no production data).
    // RMS from dB via rms = (0.5/sqrt2)*10^(dB/20), the inverse of `rms_db`;
    // floor rms is 2e-6/sqrt2. A large above-to-below drop must fail the
    // linear bound, while below-below noise must pass. Why these points:
    // -59 dB to -109 dB mirrors the observed +48 dB release reversed (a
    // 50 dB drop must fail); -141 dB to -145 dB mirrors the validation-r6
    // below-floor probes (noise must pass).
    {
        let reference_rms = AMPLITUDE / 2.0_f64.sqrt();
        let rms_above = reference_rms * 10_f64.powf(-59.0 / 20.0);
        let rms_below = reference_rms * 10_f64.powf(-109.0 / 20.0);
        let large_step = (rms_above - rms_below).abs();
        assert!(
            large_step > rms_floor,
            "F1 guard must fail a large above-to-below drop (step {large_step:.3e} vs floor {rms_floor:.3e})"
        );
        let rms_low_a = reference_rms * 10_f64.powf(-141.0 / 20.0);
        let rms_low_b = reference_rms * 10_f64.powf(-145.0 / 20.0);
        let noise_step = (rms_low_a - rms_low_b).abs();
        assert!(
            noise_step < rms_floor,
            "F1 guard must pass below-below noise (step {noise_step:.3e} vs floor {rms_floor:.3e})"
        );
    }
}

#[test]
fn cumulative_clock_proves_stable_window_and_quantifies_ramp_chirp() {
    // Ground truth for the 0.5 -> 2.0 ramp schedule (chunk 256, nominal 1.0),
    // connected to genuine production output (fix-r9 F4). The ramp block (8)
    // must show varying steps from 2.0 to 0.5 input frames per output
    // (intended chirp); blocks 9..13 must show exactly uniform steps at 0.5
    // (stable 96 kHz clock). Production lengths for all 29 blocks must equal
    // the clock's bisection sizing, and moving-leg LF audio (blocks 9..13)
    // must meet -50 dB against both the clock-derived ideal
    // `A*sin(2 pi f*(origin+anchor)/Fs_in)` and the independent uniform
    // 1/96 kHz ideal, with gains agreeing within 0.02 dB. A 1% wrong-rate
    // ideal must fail -50 dB (fault sensitivity). On the stable leg the clock
    // slope is exactly 1/96 kHz, so both ideals coincide up to the constant
    // phase the single gain/phase fit absorbs (documented signal-delay).
    for quality in QUALITIES {
        let (taps, _) = specs(quality);
        let mut clock = SlewClock::new(CHUNK, taps, 1.0);
        let mut pending = 0;
        clock.set_ratio(0.5, false);
        let mut all_steps: Vec<Vec<f64>> = Vec::new();
        for block in 0..29 {
            if block == 8 {
                clock.set_ratio(2.0, true);
            }
            let mut produced = clock.feed(CHUNK, &mut pending);
            // One input chunk of 256 yields exactly one backend block here
            // (pending starts empty and chunk divides the feed).
            assert_eq!(
                produced.len(),
                1,
                "{quality:?} block {block}: clock must emit one backend block"
            );
            all_steps.push(produced.pop().expect("one block"));
        }
        // Ramp block 8: steps must span the 2.0 -> 0.5 chirp (intended
        // modulation), strictly decreasing as the inverse ratio ramps.
        let ramp = &all_steps[8];
        let first = ramp.first().copied().unwrap_or(0.0);
        let last = ramp.last().copied().unwrap_or(0.0);
        eprintln!(
            "SLEW-CLOCK {quality:?} ramp block 8 outputs={} first_step={first:.6} last_step={last:.6}",
            ramp.len()
        );
        assert!(
            first > 1.0 && last < 1.0,
            "{quality:?}: ramp must chirp from ~2.0 to ~0.5 input frames per output"
        );
        assert!(
            first > last,
            "{quality:?}: ramp steps must decrease monotonically"
        );
        // Stable window 9..13: every step must be exactly 0.5 (1/2.0 is
        // exactly representable; increment is 0.0, so no rounding drift).
        for (index, steps) in all_steps.iter().enumerate().skip(9).take(4) {
            let uniform = steps.iter().all(|s| s.to_bits() == 0.5_f64.to_bits());
            let mean = steps.iter().sum::<f64>() / steps.len().max(1) as f64;
            eprintln!(
                "SLEW-CLOCK {quality:?} block={index} outputs={} mean_step={mean:.6} uniform={uniform}",
                steps.len()
            );
            assert!(
                uniform,
                "{quality:?} block {index}: stable-window steps must be exactly 0.5"
            );
        }
        // Coherent ideal slope on the stable leg: consecutive anchors differ
        // by exactly 0.5 input frames, i.e. 1/96 kHz in seconds, matching the
        // fixed 96 kHz model. Quantify by linear fit over the window trace.
        let window_start = all_steps[..9].iter().map(Vec::len).sum::<usize>();
        let window_end = window_start + all_steps[9..13].iter().map(Vec::len).sum::<usize>();
        let window = &clock.trace[window_start..window_end];
        let mut max_step_error = 0.0_f64;
        for pair in window.windows(2) {
            let time_a = (pair[0].0 as f64 + pair[0].1) / f64::from(RATE);
            let time_b = (pair[1].0 as f64 + pair[1].1) / f64::from(RATE);
            let delta = time_b - time_a;
            let expected = 1.0 / 96_000.0;
            max_step_error = max_step_error.max((delta - expected).abs());
        }
        eprintln!(
            "SLEW-CLOCK {quality:?} stable-window max step-time error vs 1/96kHz: {max_step_error:.3e} s"
        );
        // Why 1e-12 s: f64 anchor arithmetic over ~2k steps accumulates at
        // most N*eps*|anchor| ~= 2048*2.2e-16*256 ~= 1.2e-10 input frames
        // ~= 2.4e-15 s at 48 kHz; 1e-12 s keeps 400x margin while still
        // proving microsecond-level chirp would be caught (a 1% chirp over
        // the window would err by ~2e-7 s, 200000x above this bound).
        assert!(
            max_step_error < 1e-12,
            "{quality:?}: stable-window clock must be uniform to 1 ps"
        );
        // Fix-r9 F4: connect every block to genuine production output.
        // `render_upward` runs the real plugin through the same 0.5 -> 2.0
        // ramp schedule (29 input chunks of 256). Each production block holds
        // interleaved stereo, so frames = len/2. Length equality for all 29
        // blocks (including ramp block 8) proves the clock's bisection block
        // sizing matches the backend that actually emits. Counts alone cannot
        // prove sample-time uniformity (a shared recurrence bug would preserve
        // counts), so the audio-vs-ideal fits below use genuine emitted
        // samples against both the clock-derived ideal and an independent
        // uniform ideal with no recurrence.
        let (smooth_blocks, instant_blocks) = render_upward(quality);
        assert_eq!(
            smooth_blocks.len(),
            29,
            "{quality:?}: production must emit 29 blocks"
        );
        assert_eq!(
            instant_blocks.len(),
            29,
            "{quality:?}: production must emit 29 blocks"
        );
        assert_eq!(
            all_steps.len(),
            29,
            "{quality:?}: clock must emit 29 blocks"
        );
        for (index, ((smooth, instant), steps)) in smooth_blocks
            .iter()
            .zip(instant_blocks.iter())
            .zip(all_steps.iter())
            .enumerate()
        {
            let smooth_frames = smooth.len() / 2;
            let instant_frames = instant.len() / 2;
            assert_eq!(
                smooth_frames,
                steps.len(),
                "{quality:?} block {index}: smoothed production frames {smooth_frames} must equal clock {}",
                steps.len()
            );
            assert_eq!(
                instant_frames,
                steps.len(),
                "{quality:?} block {index}: instant production frames {instant_frames} must equal clock {}",
                steps.len()
            );
        }
        eprintln!(
            "SLEW-CLOCK {quality:?} production lengths match clock for 29 blocks (ramp block 8 outputs={})",
            all_steps[8].len()
        );
        // Quantitative clock-ideal fit on the moving-leg stable window
        // (blocks 9..13 genuine LF samples) against
        // `A*sin(2 pi f*(origin+anchor)/Fs_in)`. A single constant gain/phase
        // fit absorbs the documented signal-delay (constant group delay) and
        // flat-band gain, not drift: 1% chirp over the window would err by
        // ~2e-7 s (200000x above the 1 ps uniformity bound) and cannot be
        // absorbed. Requiring both the clock-derived ideal and the independent
        // uniform 1/96 kHz ideal (no recurrence, `fit_tone` at 96 kHz) to meet
        // -50 dB discriminates real DSP timing regressions from a recurrence
        // bug copied in both clocks: a shared chirp would pass the
        // clock-derived fit vacuously but fail the uniform fit.
        let transition_smooth: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let lf_moving = channel_samples(&transition_smooth, 2, 0);
        let clock_times: Vec<f64> = window
            .iter()
            .map(|(origin, anchor, _)| (*origin as f64 + *anchor) / f64::from(RATE))
            .collect();
        assert_eq!(
            lf_moving.len(),
            clock_times.len(),
            "{quality:?}: moving-leg window samples must match clock trace"
        );
        let (gain_clock, residual_clock) = fit_tone_with_times(&lf_moving, &clock_times, 997.0);
        let (gain_fixed, residual_fixed) = fit_tone(&lf_moving, 96_000, 997.0);
        eprintln!(
            "SLEW-CLOCK {quality:?} moving-leg LF clock-ideal gain={gain_clock:.3}dB residual={residual_clock:.1}dB; \
             fixed-96kHz gain={gain_fixed:.3}dB residual={residual_fixed:.1}dB"
        );
        assert!(
            residual_clock < -50.0,
            "{quality:?}: moving-leg clock-ideal residual {residual_clock:.1} dB exceeds -50 dB"
        );
        assert!(
            residual_fixed < -50.0,
            "{quality:?}: moving-leg fixed-96kHz residual {residual_fixed:.1} dB exceeds -50 dB"
        );
        // Why 0.02 dB: on the stable leg both ideals differ only by a constant
        // time origin (uniform 0.5 steps vs uniform 1/96 kHz), which the
        // constant fit absorbs, so fitted gains must agree to within the
        // 0.02 dB steady-state table delta (triangle inequality at 997 Hz).
        // A larger divergence would indicate clock non-uniformity or a fit bug.
        assert!(
            (gain_clock - gain_fixed).abs() < 0.02,
            "{quality:?}: clock-ideal gain {gain_clock:.3} dB must agree with fixed {gain_fixed:.3} dB within 0.02 dB"
        );
        // Injected clock/ratio fault sensitivity (negative control): a wrong-rate
        // uniform ideal (1% fast, 96960 Hz vs 96000 Hz) drifts ~76deg over ~2k
        // samples (2 pi*997*0.213ms), which no constant gain/phase fit can
        // absorb. It must fail -50 dB, proving the -50 dB audio-vs-ideal check
        // discriminates real timing drift. Why 1%: 100x the 0.01 dB flat-band
        // gain tolerance in time (0.213 ms vs 2.4e-15 s f64 floor), far above
        // any legitimate jitter, yet small enough to keep counts plausible.
        {
            let faulty_rate = 96_000.0 * 1.01;
            let start = clock_times.first().copied().unwrap_or(0.0);
            let faulty_times: Vec<f64> = (0..clock_times.len())
                .map(|n| start + n as f64 / faulty_rate)
                .collect();
            let (_, residual_faulty) = fit_tone_with_times(&lf_moving, &faulty_times, 997.0);
            eprintln!(
                "SLEW-CLOCK {quality:?} injected 1pct-fast ideal residual={residual_faulty:.1}dB (must fail -50 dB)"
            );
            assert!(
                residual_faulty > -50.0,
                "{quality:?}: injected 1pct-fast ideal residual {residual_faulty:.1} dB unexpectedly passes -50 dB (insensitive check)"
            );
        }
    }
}

#[test]
fn unchanged_ratio_control_isolates_ramp_from_estimator() {
    // Same tones, chunking, and 4-block windowing as the transition, but at
    // fixed ratio 2.0 with no ramp and no cutoff slew (wide table
    // throughout). The corrected fit must report a clean tone here
    // (residual well below -50 dB, gain within the 0.01 dB flat-band
    // bound), proving the oracle measures real audio. Comparing this leg to
    // the transition leg isolates any ramp/chirp contribution: both legs
    // share the estimator, so a transition-only excess would be real
    // switching/ramp spurious, not estimator bias.
    for quality in QUALITIES {
        let fixed = render_fixed_two(quality);
        let window: Vec<f32> = fixed[9..13].iter().flatten().copied().collect();
        let lf = channel_samples(&window, 2, 0);
        let (gain_fixed, residual_fixed) = fit_tone(&lf, 96_000, 997.0);
        let (smooth_blocks, _) = render_upward(quality);
        let transition: Vec<f32> =
            smooth_blocks[9..13].iter().flatten().copied().collect();
        let lf_transition = channel_samples(&transition, 2, 0);
        let (gain_transition, residual_transition) =
            fit_tone(&lf_transition, 96_000, 997.0);
        eprintln!(
            "SLEW-REGRESSION {quality:?} control gain={gain_fixed:.3}dB residual={residual_fixed:.1}dB; \
             transition gain={gain_transition:.3}dB residual={residual_transition:.1}dB"
        );
        // Control leg is steady-state wide-table audio: it must meet the
        // strict flat-band contract (0.01 dB gain, -75 dB residual for
        // dynamic-ratio paths per `spectral_accuracy` cumulative test).
        // Why -75 dB here vs -50 dB on the transition: the control has no
        // switching steps, so it must meet the tighter steady-state dynamic
        // bound; the transition keeps the looser -50 dB switching budget.
        assert!(
            gain_fixed.abs() < 0.01,
            "{quality:?}: control gain {gain_fixed:.3} dB exceeds the flat-band bound"
        );
        assert!(
            residual_fixed < -75.0,
            "{quality:?}: control residual {residual_fixed:.1} dB exceeds -75 dB"
        );
        // Transition must meet its own -50 dB switching budget (same assert
        // as the main test, repeated here for the isolation comparison) and
        // stay within 0.1 dB of the control gain (the P1-1 LF budget).
        assert!(
            residual_transition < -50.0,
            "{quality:?}: transition spurious {residual_transition:.1} dB exceeds -50 dB"
        );
        assert!(
            (gain_transition - gain_fixed).abs() < 0.1,
            "{quality:?}: transition gain must track the control within 0.1 dB"
        );
    }
}

#[test]
fn corrected_fit_detects_true_spur_and_rejects_old_bias() {
    // Synthetic regression at the same rate/length as the transition window
    // (~2048 samples at 96 kHz, f32 quantized like production output).
    // Part 1: a pure 997 Hz tone must fit cleanly with the true solution
    // (residual below -90 dB, near the f32 quantization floor) while the old
    // coherent-only projection reports a false ~-42 dB residual on the
    // identical samples, proving the old oracle's bias. Part 2: the same
    // tone plus a known -60 dB spur at 3 kHz (64 integer cycles, coherent)
    // must report ~-60 dB with the true solution, proving sensitivity to
    // real artifacts. Together they distinguish estimator bias (fixed) from
    // true spurious (detected) without touching production.
    const SYNTH_RATE: u32 = 96_000;
    const SYNTH_LEN: usize = 2048;
    const SPUR_FREQ: f64 = 3_000.0;
    // Why -60 dB spur: 10 dB below the -50 dB transition budget (must be
    // flagged) and 15 dB above the -75 dB control bound (cleanly separated
    // from steady-state noise); amplitude 0.5*10^(-60/20) = 5e-4 peak.
    const SPUR_PEAK: f64 = 0.5 * 0.001;
    let pure: Vec<f32> = (0..SYNTH_LEN)
        .map(|n| {
            (AMPLITUDE * (TAU * 997.0 * n as f64 / f64::from(SYNTH_RATE)).sin()) as f32
        })
        .collect();
    let (gain_true, residual_true) = fit_tone(&pure, SYNTH_RATE, 997.0);
    let (gain_old, residual_old) = fit_tone_coherent_only(&pure, SYNTH_RATE, 997.0);
    eprintln!(
        "SLEW-REGRESSION pure997 true gain={gain_true:.4}dB residual={residual_true:.1}dB; \
         coherent-only gain={gain_old:.4}dB residual={residual_old:.1}dB"
    );
    // True solution on a pure f32 tone: gain within half the 0.01 dB
    // flat-band bound (no filter involved, only f32 quantization ~-150 dB)
    // and residual below the -90 dB steady-state bound.
    assert!(
        gain_true.abs() < 0.005,
        "true fit gain {gain_true:.4} dB on pure tone exceeds 0.005 dB"
    );
    assert!(
        residual_true < -90.0,
        "true fit residual {residual_true:.1} dB on pure tone exceeds -90 dB"
    );
    // Old projection on the identical samples: gain bias is phase-dependent
    // (worst-case 1/(2 sin w) ~7.7 counts vs N/2 = 1024 is an upper bound,
    // ~0.75% = 0.065 dB, not a per-phase lower bound), so only the residual
    // lower bound reproduces the validation-r6 failure mode. At phase 0 the
    // measured coherent-only gain is 0.0039 dB (small) while residual is
    // -42.6 dB (failing -50 dB). The phase sweep below proves gain bias
    // varies with starting phase while the true solution stays clean and the
    // coherent-only residual robustly fails, replacing the false single-phase
    // gain lower bound with an independent phase-derived oracle check.
    assert!(
        residual_old > -50.0,
        "coherent-only residual {residual_old:.1} dB unexpectedly passes -50 dB on non-coherent data"
    );
    // Phase-derived bias check (independent, no fitted constant): the
    // coherent-only fractional amplitude error to first order is
    // 2/N*(cos^2*ds + sin^2*dc + sin(2phi)*s) where ds,dc,s are the
    // Sss,Scc,Ssc deviations (ds+dc=0, |s|<=7.7). At phi=0 error is 2*ds/N
    // (small, ds~0.46 from the observed 0.0039 dB); at 45deg it is 2*s/N
    // (large, ~0.065 dB worst case). Sweeping 0/45/90/135deg must show gain
    // spread >0.02 dB (phase dependence) while true-LS gains stay <0.005 dB
    // and coherent-only residuals stay >-50 dB (robust discriminator).
    {
        const PHASES_DEG: [f64; 4] = [0.0, 45.0, 90.0, 135.0];
        let mut coherent_gains = Vec::with_capacity(4);
        for phase_deg in PHASES_DEG {
            let phase_rad = phase_deg * std::f64::consts::PI / 180.0;
            let tone: Vec<f32> = (0..SYNTH_LEN)
                .map(|n| {
                    (AMPLITUDE
                        * (TAU * 997.0 * n as f64 / f64::from(SYNTH_RATE) + phase_rad).sin())
                        as f32
                })
                .collect();
            let (gain_true_phase, residual_true_phase) = fit_tone(&tone, SYNTH_RATE, 997.0);
            let (gain_old_phase, residual_old_phase) =
                fit_tone_coherent_only(&tone, SYNTH_RATE, 997.0);
            eprintln!(
                "SLEW-REGRESSION phase={phase_deg:.0}deg true gain={gain_true_phase:.4}dB residual={residual_true_phase:.1}dB; \
                 coherent-only gain={gain_old_phase:.4}dB residual={residual_old_phase:.1}dB"
            );
            assert!(
                gain_true_phase.abs() < 0.005,
                "true fit gain {gain_true_phase:.4} dB at {phase_deg:.0}deg exceeds 0.005 dB"
            );
            assert!(
                residual_true_phase < -90.0,
                "true fit residual {residual_true_phase:.1} dB at {phase_deg:.0}deg exceeds -90 dB"
            );
            assert!(
                residual_old_phase > -50.0,
                "coherent-only residual {residual_old_phase:.1} dB at {phase_deg:.0}deg unexpectedly passes -50 dB"
            );
            coherent_gains.push(gain_old_phase);
        }
        let max_gain = coherent_gains
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let min_gain = coherent_gains
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let spread = max_gain - min_gain;
        eprintln!(
            "SLEW-REGRESSION coherent-only gain spread over 4 phases: {spread:.4}dB (min {min_gain:.4}dB, max {max_gain:.4}dB)"
        );
        // Why 0.02 dB: same magnitude as the removed false lower bound, now
        // used as a spread (phase dependence) rather than a per-phase floor.
        // Predicted spread from s~7.7 is ~0.13 dB peak-to-peak (45deg vs
        // 135deg differ by 4*s/N), so 0.02 dB keeps 6x margin while still
        // proving phase dependence.
        assert!(
            spread > 0.02,
            "coherent-only gain spread {spread:.4} dB over 4 phases must exceed 0.02 dB to prove phase dependence"
        );
        // At least one phase must show a large bias (>0.02) and at least one
        // must show a small bias (<0.02), proving the single-phase lower bound
        // is invalid (phase 0 at 0.0039 dB is the small case).
        assert!(
            coherent_gains.iter().any(|g| g.abs() > 0.02),
            "coherent-only gains {coherent_gains:?} must include a large-bias phase (>0.02 dB)"
        );
        assert!(
            coherent_gains.iter().any(|g| g.abs() < 0.02),
            "coherent-only gains {coherent_gains:?} must include a small-bias phase (<0.02 dB)"
        );
    }
    // Injected spur: true solution must report it quantitatively.
    let with_spur: Vec<f32> = pure
        .iter()
        .enumerate()
        .map(|(n, &base)| {
            let spur = SPUR_PEAK * (TAU * SPUR_FREQ * n as f64 / f64::from(SYNTH_RATE)).sin();
            (f64::from(base) + spur) as f32
        })
        .collect();
    let (_, residual_spur) = fit_tone(&with_spur, SYNTH_RATE, 997.0);
    eprintln!("SLEW-REGRESSION injected -60dB spur at 3kHz reports {residual_spur:.1}dB");
    // Why +/-1.5 dB: the 3 kHz spur is coherent (64 cycles) so the fit
    // separates it cleanly; 1.5 dB covers f32 requantization of the sum
    // (<= 3e-8 vs 5e-4 spur = 0.005 dB) plus the pure-tone floor (-90 dB,
    // 30 dB below the spur, contributing <0.01 dB). A wider window would
    // let a deaf oracle pass by averaging the spur away.
    assert!(
        (residual_spur + 60.0).abs() < 1.5,
        "injected -60 dB spur must report within 1.5 dB, got {residual_spur:.1} dB"
    );
}
