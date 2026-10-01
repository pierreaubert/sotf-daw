//! Independent delay accuracy audits (DELAY-R1, DELAY-A1/A2/A3).
//!
//! These tests verify rendered audio against independently derived references:
//! four-point Lagrange weights through the defining product formula (not the
//! plugin's expanded coefficients), closed-loop comb-filter gains from the
//! feedback difference equation, and exact partition/reset equalities. All
//! tolerances below are fixed a priori from f32 rounding plus the existing
//! `static_taps` bound (2e-6 at 0.25 amplitude, scaled here); see each test.
//!
//! The high-frequency transfer sweep uses integer parts of at least 2 samples
//! (the standard four-tap stencil). Sub-two-sample fractional delays use the
//! documented causal stencil — ring-unavailable taps are solved implicitly
//! through the current frame — and are covered against their own independent
//! oracle plus DC-gain checks below.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_delay::{DelayPlugin, DelayPluginParams};
use std::f64::consts::PI;

/// Render `input` through `plugin` in mono, cycling `blocks` (frame counts).
fn render_blocks(
    plugin: &mut DelayPlugin,
    rate: u32,
    input: &[f32],
    blocks: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut offset = 0;
    let mut step = 0;
    while offset < output.len() {
        let frames = blocks[step % blocks.len()].min(output.len() - offset);
        plugin
            .process_in_place(
                &mut output[offset..offset + frames],
                &ProcessContext::new(rate, frames),
            )
            .unwrap();
        offset += frames;
        step += 1;
    }
    output
}

/// Four-point Lagrange weights from the defining product formula.
///
/// Same nodes (`-1, 0, 1, 2` relative to the integer delay) and evaluation
/// point as production, but derived through the textbook product instead of
/// the expanded coefficient expressions, so a transcription error in either
/// form fails the comparison.
fn lagrange_weights(frac: f64) -> [f64; 4] {
    const NODES: [f64; 4] = [-1.0, 0.0, 1.0, 2.0];
    let mut weights = [0.0; 4];
    for (k, &node) in NODES.iter().enumerate() {
        let mut weight = 1.0;
        for other in NODES {
            if other != node {
                weight *= (frac - other) / (node - other);
            }
        }
        weights[k] = weight;
    }
    weights
}

/// Analytic fractional-delay transfer at one angular frequency.
///
/// Returns (real, imag) of `sum_k w_k z^-(int + node_k)` at `z = e^(j*omega)`.
fn analytic_transfer(weights: [f64; 4], int_delay: isize, omega: f64) -> (f64, f64) {
    const NODES: [isize; 4] = [-1, 0, 1, 2];
    let mut real = 0.0;
    let mut imag = 0.0;
    for (k, &node) in NODES.iter().enumerate() {
        let phase = omega * (int_delay + node) as f64;
        real += weights[k] * phase.cos();
        imag -= weights[k] * phase.sin();
    }
    (real, imag)
}

/// Least-squares complex gain of `output` against a unit sine at `omega`.
///
/// Fits `y[n] = P sin(w n) + Q cos(w n)` in f64 over `len` samples from
/// `start` (absolute sample indices, matching the tone phase reference) and
/// returns `(P + jQ) / amplitude`. Exact for a pure tone even with a
/// non-integer cycle count, so no coherent-sampling assumption leaks into the
/// magnitude/phase check.
fn fit_complex_gain(
    output: &[f32],
    start: usize,
    len: usize,
    omega: f64,
    amplitude: f64,
) -> (f64, f64) {
    let mut ss = 0.0;
    let mut sc = 0.0;
    let mut cc = 0.0;
    let mut ys = 0.0;
    let mut yc = 0.0;
    for i in 0..len {
        let n = (start + i) as f64;
        let s = (omega * n).sin();
        let c = (omega * n).cos();
        let y = f64::from(output[start + i]);
        ss += s * s;
        sc += s * c;
        cc += c * c;
        ys += y * s;
        yc += y * c;
    }
    let det = ss * cc - sc * sc;
    assert!(det > 0.0, "tone fit is singular");
    let p = (ys * cc - yc * sc) / det;
    let q = (ss * yc - sc * ys) / det;
    (p / amplitude, q / amplitude)
}

/// Deterministic pseudo-random samples in `[-scale, scale]` (seeded LCG).
fn pseudo_random(len: usize, seed: u64, scale: f32) -> Vec<f32> {
    // Numerical Recipes constants; wrapping arithmetic keeps the stream fixed.
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let unit = (state >> 32) as u32 as f32 / u32::MAX as f32;
            (unit - 0.5) * 2.0 * scale
        })
        .collect()
}

fn sine_tone(total: usize, freq: f64, rate: u32, amplitude: f64) -> Vec<f32> {
    (0..total)
        .map(|n| (amplitude * (2.0 * PI * freq * n as f64 / f64::from(rate)).sin()) as f32)
        .collect()
}

/// DELAY-A1: rendered high-frequency tones match the analytic Lagrange transfer.
///
/// Covers an exact tier (1 kHz rate, where milliseconds convert to exact
/// sample delays) and real rates (44.1/48/96/192 kHz, where the oracle derives
/// int/frac from the same f32 millisecond value the plugin receives, keeping
/// only the interpolation formula independent). Frequencies reach 0.8+ pi,
/// where four-point Lagrange loses decibels relative to an ideal delay.
#[test]
fn lagrange_transfer_matches_rendered_output_at_high_frequencies() {
    // A priori bounds for 0.5-amplitude tones through f32 DSP: ~2.5x the
    // scaled `static_taps` bound (2e-6 at 0.25 amplitude), with margin for HF
    // phase sensitivity. Lagrange-vs-ideal deviations here are ~1e-1 (guard).
    const SAMPLE_TOL: f64 = 1e-5;
    const GAIN_TOL: f64 = 5e-5;
    const AMPLITUDE: f64 = 0.5;
    const SETTLE: usize = 2048;
    const MEASURE: usize = 4096;
    const BLOCKS: &[usize] = &[1, 64, 511, 73, 997];

    struct Case {
        rate: u32,
        delay_ms: f32,
        int_delay: isize,
        frac: f64,
        freq: f64,
    }
    let mut cases = Vec::new();
    // Exact tier: delay_ms equals the sample count exactly in f32.
    for delay in [10.25f64, 32.5, 63.75] {
        for freq in [10.0, 50.0, 100.0, 200.0, 300.0, 400.0] {
            cases.push(Case {
                rate: 1000,
                delay_ms: delay as f32,
                int_delay: delay.floor() as isize,
                frac: delay.fract(),
                freq,
            });
        }
    }
    // Real-rate tier: oracle mirrors the f32 ms->samples conversion exactly
    // (`ms * rate / 1000` in f32, as in production) and derives the
    // interpolation weights independently from the converted value.
    let real: &[(u32, &[f64])] = &[
        (44_100, &[100.0, 1000.0, 5000.0, 10000.0, 15000.0, 20000.0]),
        (48_000, &[100.0, 1000.0, 5000.0, 10000.0, 15000.0, 20000.0]),
        (
            96_000,
            &[100.0, 1000.0, 5000.0, 10000.0, 20000.0, 30000.0, 40000.0],
        ),
        (
            192_000,
            &[100.0, 1000.0, 5000.0, 10000.0, 20000.0, 40000.0, 80000.0],
        ),
    ];
    for (rate, freqs) in real {
        for delay in [10.5f64, 33.25, 64.75] {
            let delay_ms = (delay * 1000.0 / f64::from(*rate)) as f32;
            let converted = delay_ms * *rate as f32 / 1000.0;
            let int_delay = converted.floor() as isize;
            assert!(
                int_delay >= 2,
                "case left the integer-part >= 2 precondition"
            );
            for freq in freqs.iter() {
                cases.push(Case {
                    rate: *rate,
                    delay_ms,
                    int_delay,
                    frac: f64::from(converted - int_delay as f32),
                    freq: *freq,
                });
            }
        }
    }

    let mut worst_sample: f64 = 0.0;
    let mut worst_case = String::new();
    let mut worst_gain: f64 = 0.0;
    for case in &cases {
        let total = SETTLE + MEASURE;
        let input = sine_tone(total, case.freq, case.rate, AMPLITUDE);
        let mut plugin =
            DelayPlugin::try_new_with_max_delay(1, case.delay_ms, 0.0, 1.0, 5000.0).unwrap();
        plugin.initialize(case.rate).unwrap();
        let output = render_blocks(&mut plugin, case.rate, &input, BLOCKS);
        let weights = lagrange_weights(case.frac);
        for (i, actual) in output[SETTLE..].iter().enumerate() {
            // The measurement window starts far past the delay, so every tap
            // reads rendered input history (no zero padding involved).
            let n = (SETTLE + i) as isize;
            let mut expected = 0.0;
            for (k, &node) in [-1isize, 0, 1, 2].iter().enumerate() {
                expected += weights[k] * f64::from(input[(n - case.int_delay - node) as usize]);
            }
            let error = (f64::from(*actual) - expected).abs();
            if error > worst_sample {
                worst_sample = error;
                worst_case = format!(
                    "rate={} delay_ms={} freq={} frame={}",
                    case.rate, case.delay_ms, case.freq, n
                );
            }
            assert!(
                error < SAMPLE_TOL,
                "rate={} delay_ms={} freq={} frame={n}: {actual} vs {expected} (err {error})",
                case.rate,
                case.delay_ms,
                case.freq
            );
        }
        // Explicit magnitude/phase cross-check in complex-gain coordinates.
        let omega = 2.0 * PI * case.freq / f64::from(case.rate);
        let (gm_r, gm_i) = fit_complex_gain(&output, SETTLE, MEASURE, omega, AMPLITUDE);
        let (he_r, he_i) = analytic_transfer(weights, case.int_delay, omega);
        let gain_error = ((gm_r - he_r).powi(2) + (gm_i - he_i).powi(2)).sqrt();
        worst_gain = worst_gain.max(gain_error);
        assert!(
            gain_error < GAIN_TOL,
            "rate={} delay_ms={} freq={}: measured ({gm_r:.6},{gm_i:.6}) vs analytic ({he_r:.6},{he_i:.6})",
            case.rate,
            case.delay_ms,
            case.freq
        );
    }
    println!("delay audit: worst HF sample error {worst_sample:.3e} at {worst_case}");
    println!("delay audit: worst HF complex-gain error {worst_gain:.3e}");

    // Regime guard: the analytic response itself must deviate from the ideal
    // delay (|H| = 1) at each top frequency, proving the sweep reaches beyond
    // interpolation-polynomial exactness instead of re-testing the flat band.
    let guards: &[(u32, f64, f64)] = &[
        (1000, 0.5, 400.0),
        (44_100, 0.5, 20000.0),
        (48_000, 0.5, 20000.0),
        (96_000, 0.5, 40000.0),
        (192_000, 0.5, 80000.0),
    ];
    for (rate, frac, freq) in guards {
        let weights = lagrange_weights(*frac);
        let omega = 2.0 * PI * freq / f64::from(*rate);
        let (re, im) = analytic_transfer(weights, 10, omega);
        let deviation = (1.0 - (re * re + im * im).sqrt()).abs();
        assert!(
            deviation > 0.05,
            "rate={rate} freq={freq}: analytic Lagrange is within 5% of ideal; sweep is vacuous"
        );
    }
}

/// DELAY-A1: integer-delay echoes decay as exact powers of the feedback gain.
#[test]
fn closed_loop_echoes_decay_geometrically_at_integer_delays() {
    const RATE: u32 = 48_000;
    const DELAY_SAMPLES: usize = 480; // 10 ms, exactly integer
    const ECHOES: usize = 8;
    // A priori: f32 multiply rounding over 8 recirculations is ~1e-6.
    const ECHO_TOL: f64 = 1e-5;
    const SILENCE_TOL: f32 = 1e-6;
    let mut worst: f64 = 0.0;
    for feedback in [0.5f32, 0.9, -0.5, -0.9] {
        let mut plugin = DelayPlugin::try_new(1, 10.0, feedback, 1.0).unwrap();
        plugin.initialize(RATE).unwrap();
        let total = DELAY_SAMPLES * ECHOES + 64;
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;
        let output = render_blocks(&mut plugin, RATE, &input, &[1024, 17, 3000]);
        for k in 0..ECHOES {
            let expected = f64::from(feedback).powi(k as i32);
            let actual = f64::from(output[DELAY_SAMPLES * (k + 1)]);
            let error = (actual - expected).abs();
            worst = worst.max(error);
            assert!(
                error < ECHO_TOL,
                "feedback={feedback} echo k={k}: {actual} vs {expected}"
            );
        }
        for (n, sample) in output.iter().enumerate() {
            let is_echo =
                n % DELAY_SAMPLES == 0 && (1..=ECHOES).contains(&(n / DELAY_SAMPLES));
            if !is_echo {
                assert!(
                    sample.abs() < SILENCE_TOL,
                    "feedback={feedback} non-echo frame {n}: {sample}"
                );
            }
        }
        // A contractive loop (|g| <= 0.95) never amplifies a unit impulse.
        let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak <= 1.0 + 1e-5, "feedback={feedback} peak {peak}");
        assert!(output.iter().all(|s| s.is_finite()));
    }
    println!("delay audit: worst echo-decay error {worst:.3e}");
}

/// DELAY-A1: closed-loop DC gain equals `1 / (1 - g)`, including boundaries.
#[test]
fn closed_loop_dc_gain_matches_one_over_one_minus_feedback() {
    const RATE: u32 = 48_000;
    // A priori: contractive f32 loop error is ~1e-6 relative; 1e-3 still
    // catches any structural feedback-path error (which moves gain by ~1%).
    const REL_TOL: f64 = 1e-3;
    let mut worst: f64 = 0.0;
    for feedback in [0.5f32, 0.9, 0.95, -0.5, -0.95] {
        let mut plugin = DelayPlugin::try_new(1, 1.0, feedback, 1.0).unwrap();
        plugin.initialize(RATE).unwrap();
        let total = 96_000;
        let input = vec![1.0f32; total];
        let output = render_blocks(&mut plugin, RATE, &input, &[4096, 1000, 63]);
        let tail = &output[total - 8192..];
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        let expected = 1.0 / (1.0 - f64::from(feedback));
        let rel = ((mean - expected) / expected).abs();
        worst = worst.max(rel);
        assert!(
            rel < REL_TOL,
            "feedback={feedback}: mean {mean} vs {expected} (rel {rel:.3e})"
        );
    }
    println!("delay audit: worst closed-loop DC-gain deviation {worst:.3e}");
}

/// DELAY-A1: comb peak/null gains follow the feedback sign (pole angle check).
#[test]
fn closed_loop_comb_peak_and_null_follow_feedback_sign() {
    const RATE: u32 = 48_000;
    const AMPLITUDE: f64 = 0.25;
    const TOTAL: usize = 49_152;
    const MEASURE: usize = 9_600;
    const REL_TOL: f64 = 1e-3;
    // D = 48 samples (1 ms): 1000 Hz completes one period per delay, so the
    // recirculating wave adds in phase (|H| = 1/|1-g|); 500 Hz completes half
    // a period (|H| = 1/|1+g|). Negative feedback swaps peak and null.
    let mut worst: f64 = 0.0;
    for feedback in [0.5f32, -0.5f32] {
        for (freq, expected) in [
            (1000.0, 1.0 / (1.0 - f64::from(feedback))),
            (500.0, 1.0 / (1.0 + f64::from(feedback))),
        ] {
            let mut plugin = DelayPlugin::try_new(1, 1.0, feedback, 1.0).unwrap();
            plugin.initialize(RATE).unwrap();
            let input = sine_tone(TOTAL, freq, RATE, AMPLITUDE);
            let output = render_blocks(&mut plugin, RATE, &input, &[2048, 777]);
            let omega = 2.0 * PI * freq / f64::from(RATE);
            let (gm_r, gm_i) =
                fit_complex_gain(&output, TOTAL - MEASURE, MEASURE, omega, AMPLITUDE);
            let measured = (gm_r * gm_r + gm_i * gm_i).sqrt();
            let rel = ((measured - expected) / expected).abs();
            worst = worst.max(rel);
            assert!(
                rel < REL_TOL,
                "feedback={feedback} freq={freq}: |H| {measured} vs {expected}"
            );
        }
    }
    println!("delay audit: worst comb-gain deviation {worst:.3e}");
}

/// DELAY-A1: allpass loop coloring preserves DC gain and the energy bound.
#[test]
fn allpass_feedback_preserves_dc_gain_and_loop_energy_bound() {
    const RATE: u32 = 48_000;
    // A first-order allpass has unit DC response, so closed-loop DC gain
    // stays 1/(1-g) with allpass coloring in the loop.
    let dc_params = DelayPluginParams {
        delay_ms: 1.0,
        feedback: 0.5,
        mix: 1.0,
        lfo_rate_hz: 0.0,
        lfo_depth_ms: 0.0,
        pitch_preserving: false,
        allpass_feedback: true,
        allpass_coeff: 0.7,
        channel_delays_ms: Vec::new(),
    };
    let mut plugin = DelayPlugin::from_params(1, dc_params).unwrap();
    plugin.initialize(RATE).unwrap();
    let total = 48_000;
    let output = render_blocks(&mut plugin, RATE, &vec![1.0f32; total], &[2048, 500]);
    let tail = &output[total - 8192..];
    let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
    assert!(
        ((mean - 2.0) / 2.0).abs() < 2e-3,
        "allpass DC gain: mean {mean} vs 2.0"
    );

    // Unitary coloring: each echo carries g^2 times the previous echo's
    // energy, so echo energies decay geometrically and the impulse peak is
    // bounded by sqrt(total energy) = 1/sqrt(1-g^2).
    let echo_params = DelayPluginParams {
        delay_ms: 10.0,
        feedback: 0.9,
        mix: 1.0,
        lfo_rate_hz: 0.0,
        lfo_depth_ms: 0.0,
        pitch_preserving: false,
        allpass_feedback: true,
        allpass_coeff: 0.7,
        channel_delays_ms: Vec::new(),
    };
    let mut plugin = DelayPlugin::from_params(1, echo_params.clone()).unwrap();
    plugin.initialize(RATE).unwrap();
    let mut input = vec![0.0f32; 480 * 5 + 240];
    input[0] = 1.0;
    let output = render_blocks(&mut plugin, RATE, &input, &[1024, 65]);
    // Echo k occupies [480k, 480k + 240): the 0.7 allpass tail is ~1e-37 by
    // tap 240, so windows neither truncate nor overlap.
    let mut energies = [0.0f64; 4];
    for (k, energy) in energies.iter_mut().enumerate() {
        let start = 480 * (k + 1);
        *energy = output[start..start + 240]
            .iter()
            .map(|s| f64::from(*s).powi(2))
            .sum();
    }
    let mut worst_ratio: f64 = 0.0;
    for k in 0..3 {
        let ratio = energies[k + 1] / energies[k];
        let rel = ((ratio - 0.81) / 0.81).abs();
        worst_ratio = worst_ratio.max(rel);
        assert!(
            rel < 0.02,
            "echo energy ratio E{} / E{} = {ratio}, expected 0.81",
            k + 2,
            k + 1
        );
    }
    let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let bound = 1.0 / (1.0 - 0.81f64).sqrt();
    assert!(
        f64::from(peak) <= bound + 0.01,
        "allpass impulse peak {peak} exceeds 1/sqrt(1-g^2) = {bound:.4}"
    );
    assert!(output.iter().all(|s| s.is_finite()));
    println!("delay audit: worst allpass echo-energy deviation {worst_ratio:.3e}");

    // Allpass-specific shape: the DC gain, echo-energy ratio, and peak bound
    // above also hold for direct feedback, so they cannot detect a bypassed
    // allpass on their own. The first echo (frame 480) bypasses the loop
    // filter, but the recirculated echo is the analytic first-order allpass
    // impulse response scaled by g: h[0] = a*g = 0.63, h[n] =
    // g*(1-a^2)*(-a)^(n-1), i.e. h[1] = 0.459, h[2] = -0.3213 for a = 0.7,
    // g = 0.9. A priori tap tolerance 1e-5 (f32 recirculation, echo-decay
    // family); the allpass state has decayed to exact zero over the
    // 480-sample gap (0.7^479 underflows), so the shape is exact.
    const SHAPE_TOL: f64 = 1e-5;
    assert!(
        (f64::from(output[480]) - 1.0).abs() < 1e-6,
        "first echo bypasses the loop filter: {}",
        output[480]
    );
    assert!(
        output[481].abs() < 1e-6,
        "first echo is a single impulse: {}",
        output[481]
    );
    for (index, expected) in [(960usize, 0.63), (961, 0.459), (962, -0.3213)] {
        let error = (f64::from(output[index]) - expected).abs();
        assert!(
            error < SHAPE_TOL,
            "allpass echo-1 tap {index}: {} vs {expected} (err {error:.3e})",
            output[index]
        );
    }
    // Diffusion spread: no single-sample impulse remains in the echo-1
    // window, and the second tap is substantially nonzero.
    let echo1_peak = output[960..1200]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max);
    assert!(
        echo1_peak < 0.9,
        "allpass echo-1 window must spread below the direct 0.9 impulse: {echo1_peak}"
    );
    assert!(
        output[961].abs() > 0.1,
        "allpass echo-1 second tap must be nonzero: {}",
        output[961]
    );

    // Discrimination control: with direct feedback the recirculated echo is
    // a single 0.9 impulse, which violates the allpass shape bounds above.
    // A build that ignored `allpass_feedback` would render this shape and
    // fail the 0.63 tap assertion.
    let direct_params = DelayPluginParams {
        allpass_feedback: false,
        ..echo_params
    };
    let mut direct = DelayPlugin::from_params(1, direct_params).unwrap();
    direct.initialize(RATE).unwrap();
    let mut direct_input = vec![0.0f32; 480 * 5 + 240];
    direct_input[0] = 1.0;
    let direct_output = render_blocks(&mut direct, RATE, &direct_input, &[1024, 65]);
    assert!(
        (f64::from(direct_output[960]) - 0.9).abs() < 1e-6,
        "direct echo-1 must be a single 0.9 impulse: {}",
        direct_output[960]
    );
    assert!(
        direct_output[961].abs() < 1e-6,
        "direct echo-1 has no spread: {}",
        direct_output[961]
    );
    assert!(
        (f64::from(direct_output[960]) - 0.63).abs() > SHAPE_TOL,
        "discrimination control is vacuous: direct feedback satisfies the allpass tap bound"
    );
    assert!(
        (f64::from(output[960]) - f64::from(direct_output[960])).abs() > 0.1,
        "allpass coloring must move echo-1 off the direct 0.9 impulse"
    );
}

/// DELAY-A2: dry/wet mix produces comb construction, cancellation, and unity.
#[test]
fn dry_wet_mix_produces_comb_construction_and_cancellation() {
    const RATE: u32 = 48_000;
    const TOTAL: usize = 8192;
    const SETTLE: usize = 1024;
    // 1000 Hz through a 48-sample integer delay returns in phase, so mix 0.5
    // reconstructs the input sample-by-sample past the line fill.
    let mut plugin = DelayPlugin::try_new(1, 1.0, 0.0, 0.5).unwrap();
    plugin.initialize(RATE).unwrap();
    let input = sine_tone(TOTAL, 1000.0, RATE, 0.4);
    let output = render_blocks(&mut plugin, RATE, &input, &[1024, 513]);
    let mut worst_constructive: f64 = 0.0;
    for (i, actual) in output[SETTLE..].iter().enumerate() {
        let error = (f64::from(*actual) - f64::from(input[SETTLE + i])).abs();
        worst_constructive = worst_constructive.max(error);
        assert!(error < 1e-6, "constructive frame {i}: {actual} vs input");
    }
    // 500 Hz returns inverted and cancels the dry leg.
    let mut plugin = DelayPlugin::try_new(1, 1.0, 0.0, 0.5).unwrap();
    plugin.initialize(RATE).unwrap();
    let input = sine_tone(TOTAL, 500.0, RATE, 0.4);
    let output = render_blocks(&mut plugin, RATE, &input, &[513, 1024]);
    let null_rms = (output[SETTLE..]
        .iter()
        .map(|s| f64::from(*s).powi(2))
        .sum::<f64>()
        / (TOTAL - SETTLE) as f64)
        .sqrt();
    assert!(null_rms < 1e-5, "cancellation residual RMS {null_rms:.3e}");
    // Zero delay: wet equals dry from the first sample, so mix 0.5 is unity.
    let mut plugin = DelayPlugin::try_new(1, 0.0, 0.0, 0.5).unwrap();
    plugin.initialize(RATE).unwrap();
    let input = pseudo_random(TOTAL, 0x5EED, 0.8);
    let output = render_blocks(&mut plugin, RATE, &input, &[777, 31]);
    for (i, actual) in output.iter().enumerate() {
        assert!(
            (f64::from(*actual) - f64::from(input[i])).abs() < 1e-6,
            "zero-delay unity frame {i}"
        );
    }
    println!("delay audit: worst constructive dry/wet error {worst_constructive:.3e}");
    println!("delay audit: destructive dry/wet null RMS {null_rms:.3e}");

    // Stereo: identical inputs stay bit-identical across channels (shared
    // smoothers advance once per frame; per-channel allpass states match).
    let mut plugin = DelayPlugin::try_new(2, 1.0, 0.0, 0.5).unwrap();
    plugin.initialize(RATE).unwrap();
    let mono = sine_tone(TOTAL, 1000.0, RATE, 0.4);
    let mut stereo = Vec::with_capacity(TOTAL * 2);
    for sample in &mono {
        stereo.extend_from_slice(&[*sample, *sample]);
    }
    let mut output = stereo;
    let mut offset = 0;
    let mut step = 0;
    let blocks = [900, 17, 2048];
    while offset < TOTAL {
        let frames = blocks[step % blocks.len()].min(TOTAL - offset);
        plugin
            .process_in_place(
                &mut output[offset * 2..(offset + frames) * 2],
                &ProcessContext::new(RATE, frames),
            )
            .unwrap();
        offset += frames;
        step += 1;
    }
    for frame in 0..TOTAL {
        assert_eq!(
            output[frame * 2],
            output[frame * 2 + 1],
            "stereo drift at frame {frame}"
        );
    }
}

/// DELAY-A2/R2: bounded instances reject automation past the declared range.
#[test]
fn bounded_instances_reject_automation_beyond_declared_range() {
    let mut plugin = DelayPlugin::try_new_with_max_delay(1, 5.0, 0.0, 0.5, 10.0).unwrap();
    plugin.initialize(48_000).unwrap();
    plugin
        .set_parameter(ParameterId::from("delay_ms"), ParameterValue::Float(10.0))
        .unwrap();
    let error = plugin
        .set_parameter(ParameterId::from("delay_ms"), ParameterValue::Float(10.5))
        .unwrap_err();
    assert!(error.contains("delay_ms"), "unexpected error: {error}");
    // Rejected writes retain the accepted configuration.
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("delay_ms")),
        Some(ParameterValue::Float(10.0))
    );

    let mut routing =
        DelayPlugin::new_per_channel_with_max_delay(vec![5.0, 8.0], 10.0).unwrap();
    routing.initialize(48_000).unwrap();
    let error = routing
        .set_parameter(
            ParameterId::from("delay_ms_1"),
            ParameterValue::Float(12.0),
        )
        .unwrap_err();
    assert!(error.contains("delay_ms_1"), "unexpected error: {error}");
    assert_eq!(
        routing.get_parameter(&ParameterId::from("delay_ms_1")),
        Some(ParameterValue::Float(8.0))
    );
}

/// DELAY-R2: per-channel routing mode rejects every effect control.
///
/// Per-channel mode is a pure routing delay (wet-only, no feedback, LFO, or
/// diffusion); each incompatible setting is a hard factory error, and the
/// clean combination constructs.
#[test]
fn per_channel_mode_rejects_every_effect_control() {
    let base = || DelayPluginParams {
        delay_ms: 0.0,
        feedback: 0.0,
        mix: 1.0,
        lfo_rate_hz: 0.0,
        lfo_depth_ms: 0.0,
        pitch_preserving: false,
        allpass_feedback: false,
        allpass_coeff: 0.5,
        channel_delays_ms: vec![1.0, 2.0],
    };
    DelayPlugin::from_params(2, base()).unwrap();
    let violations = [
        ("feedback", DelayPluginParams { feedback: 0.1, ..base() }),
        ("mix", DelayPluginParams { mix: 0.5, ..base() }),
        ("lfo_rate", DelayPluginParams { lfo_rate_hz: 1.0, ..base() }),
        ("lfo_depth", DelayPluginParams { lfo_depth_ms: 1.0, ..base() }),
        ("pitch", DelayPluginParams { pitch_preserving: true, ..base() }),
        ("allpass", DelayPluginParams { allpass_feedback: true, ..base() }),
    ];
    for (name, params) in violations {
        let error = match DelayPlugin::from_params(2, params) {
            Ok(_) => panic!("per-channel mode must reject {name}"),
            Err(error) => error,
        };
        assert!(
            error.contains("pure routing delay"),
            "{name}: unexpected error: {error}"
        );
    }
}

/// DELAY-A3: feedback/mix/delay automation is callback-partition invariant.
///
/// Automation lands on identical sample clocks under both partitionings; the
/// per-frame smoothers and Doppler read head make the renders bit-identical.
#[test]
fn feedback_and_mix_automation_is_callback_partition_invariant() {
    const RATE: u32 = 48_000;
    let input = pseudo_random(6000, 0xA1, 0.5);
    let mut renders = Vec::new();
    const CONTIGUOUS: &[usize] = &[4096];
    const IRREGULAR: &[usize] = &[1, 64, 511, 73, 997];
    for blocks in [CONTIGUOUS, IRREGULAR] {
        let mut plugin = DelayPlugin::try_new(1, 10.0, 0.0, 1.0).unwrap();
        plugin.initialize(RATE).unwrap();
        let mut output = Vec::with_capacity(6000);
        output.extend(render_blocks(&mut plugin, RATE, &input[0..2000], blocks));
        plugin
            .set_parameter(ParameterId::from("feedback"), ParameterValue::Float(0.5))
            .unwrap();
        output.extend(render_blocks(&mut plugin, RATE, &input[2000..4000], blocks));
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.3))
            .unwrap();
        plugin
            .set_parameter(ParameterId::from("delay_ms"), ParameterValue::Float(15.0))
            .unwrap();
        output.extend(render_blocks(&mut plugin, RATE, &input[4000..6000], blocks));
        renders.push(output);
    }
    assert_eq!(renders[0], renders[1]);
}

/// DELAY-A3: reset after mid-ramp automation matches a fresh instance exactly.
#[test]
fn reset_after_mid_ramp_automation_matches_fresh_instance() {
    const RATE: u32 = 48_000;
    let target = DelayPluginParams {
        delay_ms: 25.0,
        feedback: 0.4,
        mix: 0.7,
        lfo_rate_hz: 3.0,
        lfo_depth_ms: 2.0,
        pitch_preserving: false,
        allpass_feedback: true,
        allpass_coeff: 0.6,
        channel_delays_ms: Vec::new(),
    };
    let mut cycled = DelayPlugin::try_new(1, 10.0, 0.0, 1.0).unwrap();
    cycled.initialize(RATE).unwrap();
    render_blocks(&mut cycled, RATE, &vec![0.1f32; 1000], &[1000]);
    for (id, value) in [
        ("delay_ms", ParameterValue::Float(25.0)),
        ("feedback", ParameterValue::Float(0.4)),
        ("mix", ParameterValue::Float(0.7)),
        ("lfo_rate_hz", ParameterValue::Float(3.0)),
        ("lfo_depth_ms", ParameterValue::Float(2.0)),
        ("allpass_feedback", ParameterValue::Bool(true)),
        ("allpass_coeff", ParameterValue::Float(0.6)),
    ] {
        cycled.set_parameter(ParameterId::from(id), value).unwrap();
    }
    // Process mid-ramp (50 ms delay and 20 ms allpass smoothers unsettled).
    render_blocks(&mut cycled, RATE, &vec![0.1f32; 500], &[500]);
    cycled.reset();

    let mut fresh = DelayPlugin::from_params(1, target).unwrap();
    fresh.initialize(RATE).unwrap();
    let input = pseudo_random(4000, 0xBE5E, 0.6);
    let a = render_blocks(&mut cycled, RATE, &input, &[512, 100]);
    let b = render_blocks(&mut fresh, RATE, &input, &[512, 100]);
    assert_eq!(a, b);
}

/// DELAY-A3: seeded randomized partitioning matches continuous processing.
///
/// LFO, feedback, and allpass advance per frame with no block-rate logic, so
/// a fixed pseudo-random block sequence renders bit-identically to one block.
#[test]
fn randomized_block_partitioning_matches_continuous_processing() {
    const RATE: u32 = 48_000;
    const TOTAL: usize = 20_000;
    // Fixed LCG block sizes in 1..=2048; the sequence is deterministic.
    let mut state = 0x243F_6A88_85A3_0BDDu64;
    let mut blocks = Vec::new();
    let mut remaining = TOTAL;
    while remaining > 0 {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let size = ((state >> 32) as usize % 2048 + 1).min(remaining);
        blocks.push(size);
        remaining -= size;
    }
    let params = || DelayPluginParams {
        delay_ms: 12.5,
        feedback: 0.6,
        mix: 0.8,
        lfo_rate_hz: 5.0,
        lfo_depth_ms: 4.0,
        pitch_preserving: false,
        allpass_feedback: true,
        allpass_coeff: 0.65,
        channel_delays_ms: Vec::new(),
    };
    let input = pseudo_random(TOTAL, 0x1F3D, 0.6);
    let mut scattered = DelayPlugin::from_params(1, params()).unwrap();
    scattered.initialize(RATE).unwrap();
    let mut continuous = DelayPlugin::from_params(1, params()).unwrap();
    continuous.initialize(RATE).unwrap();
    let a = render_blocks(&mut scattered, RATE, &input, &blocks);
    let b = render_blocks(&mut continuous, RATE, &input, &[TOTAL]);
    assert_eq!(a, b);
}

/// Independent reference for a sub-two-sample fractional delay.
///
/// The ring cannot supply the `y_m1` tap (`int_delay == 1`) or the `y_m1`
/// and `y_0` taps (`int_delay == 0`, where `y_m1` would need the non-causal
/// future sample); the documented contract evaluates those taps at the
/// current frame value `v`, which closes the feedback loop implicitly:
/// `v = x + g * (sub * v + ring)`. This reference solves the same linear
/// equation in f64 with product-form Lagrange weights, so only the stencil
/// contract is shared with production, never its expanded coefficients.
fn causal_stencil_reference(
    weights: [f64; 4],
    int_delay: usize,
    input: &[f32],
    feedback: f64,
) -> Vec<f64> {
    let mut frames = Vec::with_capacity(input.len());
    let mut delayed = Vec::with_capacity(input.len());
    for (n, &sample) in input.iter().enumerate() {
        let past = |lag: usize| -> f64 {
            if lag > n {
                0.0
            } else {
                frames[n - lag]
            }
        };
        let (sub, ring) = if int_delay == 0 {
            (
                weights[0] + weights[1],
                weights[2] * past(1) + weights[3] * past(2),
            )
        } else {
            (
                weights[0],
                weights[1] * past(1) + weights[2] * past(2) + weights[3] * past(3),
            )
        };
        let current = (f64::from(sample) + feedback * ring) / (1.0 - feedback * sub);
        frames.push(current);
        delayed.push(sub * current + ring);
    }
    delayed
}

/// DELAY-A1/A2: sub-two-sample fractional delays match the causal stencil.
///
/// Covers both integer parts (0.5, 1.25, and 1.5 samples at the exact 1 kHz
/// tier) with zero feedback — where the solve is exact — and with 0.5
/// feedback, where the implicit solve preserves the true closed-loop gain
/// that a plain `input` substitution would scale by `(1 - w_m1)`.
#[test]
fn sub_two_sample_fractional_delays_match_causal_stencil_oracle() {
    const RATE: u32 = 1000;
    // A priori: f32 tap arithmetic plus one implicit division per frame is
    // ~1e-6 at unit amplitude; the 0.5-feedback loop roughly doubles f32
    // rounding. Same bound family as the high-frequency per-sample check.
    const SAMPLE_TOL: f64 = 1e-5;
    const BLOCKS: &[usize] = &[1, 64, 511, 73, 997];
    let mut worst: f64 = 0.0;
    for delay in [0.5f32, 1.25, 1.5] {
        let int_delay = delay.floor() as usize;
        let frac = f64::from(delay.fract());
        let weights = lagrange_weights(frac);
        let mut impulse = vec![0.0f32; 64];
        impulse[0] = 1.0;
        let inputs = [
            ("impulse", impulse),
            ("random", pseudo_random(4096, 0x005E_EDF1, 0.8)),
        ];
        for feedback in [0.0f32, 0.5] {
            for (name, input) in &inputs {
                // The feedback loop needs the longer input to exercise
                // recirculation; the impulse pins the exact tap shape.
                if feedback != 0.0 && *name == "impulse" {
                    continue;
                }
                let mut plugin =
                    DelayPlugin::try_new_with_max_delay(1, delay, feedback, 1.0, 64.0).unwrap();
                plugin.initialize(RATE).unwrap();
                let output = render_blocks(&mut plugin, RATE, input, BLOCKS);
                let expected =
                    causal_stencil_reference(weights, int_delay, input, f64::from(feedback));
                assert!(output.iter().all(|s| s.is_finite()));
                for (n, (actual, want)) in output.iter().zip(expected.iter()).enumerate() {
                    let error = (f64::from(*actual) - *want).abs();
                    worst = worst.max(error);
                    assert!(
                        error < SAMPLE_TOL,
                        "delay={delay} feedback={feedback} {name} frame={n}: {actual} vs {want} (err {error:.3e})"
                    );
                }
            }
        }
    }
    println!("delay audit: worst sub-two-sample stencil error {worst:.3e}");
}

/// DELAY-A1: sub-two-sample fractional delays preserve the DC loop gain.
///
/// The causal stencil weights sum to one, so the implicit solve converges to
/// the ideal closed-loop DC gain `1 / (1 - g)` exactly as the standard
/// stencil does — including the +/-0.95 feedback boundaries. A plain
/// `input` substitution would instead converge to `1 / (1 - g * (1 - w_m1))`
/// (22.9 instead of 10.0 at 1.5 samples, g = 0.9) or diverge.
#[test]
fn sub_two_sample_fractional_delay_preserves_dc_gain_with_feedback() {
    const RATE: u32 = 1000;
    const TOTAL: usize = 16_384;
    const MEASURE: usize = 2048;
    // A priori: same 1e-3 relative bound as the integer-delay DC check; the
    // 0.95 loop settles in ~300 frames, far inside the measurement window.
    const REL_TOL: f64 = 1e-3;
    let mut worst: f64 = 0.0;
    for delay in [0.5f32, 1.5] {
        for feedback in [0.5f32, 0.9, 0.95, -0.5, -0.95] {
            let mut plugin =
                DelayPlugin::try_new_with_max_delay(1, delay, feedback, 1.0, 64.0).unwrap();
            plugin.initialize(RATE).unwrap();
            let output = render_blocks(&mut plugin, RATE, &vec![1.0f32; TOTAL], &[4096, 63]);
            assert!(output.iter().all(|s| s.is_finite()));
            let tail = &output[TOTAL - MEASURE..];
            let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / MEASURE as f64;
            let expected = 1.0 / (1.0 - f64::from(feedback));
            let rel = ((mean - expected) / expected).abs();
            worst = worst.max(rel);
            assert!(
                rel < REL_TOL,
                "delay={delay} feedback={feedback}: mean {mean} vs {expected} (rel {rel:.3e})"
            );
        }
    }
    println!("delay audit: worst sub-two-sample DC-gain deviation {worst:.3e}");
}

/// DELAY-R2: per-channel routing mode rejects runtime effect writes.
///
/// Factory construction already rejects effect settings; the runtime single
/// and batch paths must too, retaining the accepted configuration and
/// populated history (COMMON §2). Pure values themselves stay accepted so
/// hosts can re-apply current state.
#[test]
fn per_channel_mode_rejects_runtime_effect_writes() {
    const RATE: u32 = 48_000;
    let mut dut = DelayPlugin::new_per_channel_with_max_delay(vec![5.0], 10.0).unwrap();
    let mut twin = DelayPlugin::new_per_channel_with_max_delay(vec![5.0], 10.0).unwrap();
    dut.initialize(RATE).unwrap();
    twin.initialize(RATE).unwrap();
    // Populate identical history on both instances.
    let warmup = pseudo_random(2000, 0xD1E7, 0.6);
    render_blocks(&mut dut, RATE, &warmup, &[511, 73]);
    render_blocks(&mut twin, RATE, &warmup, &[511, 73]);
    let snapshot = dut.current_values();

    // Single-write path: every deviating effect write is rejected.
    let violations: Vec<(ParameterId, ParameterValue)> = vec![
        (ParameterId::from("feedback"), ParameterValue::Float(0.5)),
        (ParameterId::from("mix"), ParameterValue::Float(0.5)),
        (
            ParameterId::from("lfo_rate_hz"),
            ParameterValue::Float(1.0),
        ),
        (
            ParameterId::from("lfo_depth_ms"),
            ParameterValue::Float(1.0),
        ),
        (
            ParameterId::from("allpass_feedback"),
            ParameterValue::Bool(true),
        ),
    ];
    for (id, value) in &violations {
        let error = dut.set_parameter(id.clone(), value.clone()).unwrap_err();
        assert!(
            error.contains("pure routing delay"),
            "{id}: unexpected error: {error}"
        );
        assert_eq!(dut.current_values(), snapshot, "{id}: config changed");
    }
    // `pitch_preserving` is structural after initialization (existing
    // precedence), so its purity rejection is pinned pre-initialization.
    let mut cold = DelayPlugin::new_per_channel(vec![5.0]).unwrap();
    let error = cold
        .set_parameter(
            ParameterId::from("pitch_preserving"),
            ParameterValue::Bool(true),
        )
        .unwrap_err();
    assert!(
        error.contains("pure routing delay"),
        "pitch_preserving: unexpected error: {error}"
    );
    let error = dut
        .set_parameter(
            ParameterId::from("pitch_preserving"),
            ParameterValue::Bool(true),
        )
        .unwrap_err();
    assert!(
        error.contains("structural"),
        "pitch_preserving post-init: unexpected error: {error}"
    );
    assert_eq!(dut.current_values(), snapshot);

    // Batch path: a batch mixing a valid delay change with a deviating
    // effect value is rejected as a whole (transactional).
    let mut poisoned = ParameterSet::new();
    poisoned.insert(
        ParameterId::from("delay_ms_0"),
        ParameterValue::Float(6.0),
    );
    poisoned.insert(
        ParameterId::from("feedback"),
        ParameterValue::Float(0.5),
    );
    let error = dut.apply_values(poisoned).unwrap_err();
    assert!(
        error.contains("pure routing delay"),
        "batch: unexpected error: {error}"
    );
    assert_eq!(
        dut.get_parameter(&ParameterId::from("delay_ms_0")),
        Some(ParameterValue::Float(5.0)),
        "batch rejection must not apply the valid delay change"
    );
    assert_eq!(dut.current_values(), snapshot);

    // No-op pure values stay accepted on both paths (host re-application).
    for (id, value) in [
        ("feedback", ParameterValue::Float(0.0)),
        ("mix", ParameterValue::Float(1.0)),
        ("lfo_rate_hz", ParameterValue::Float(0.0)),
        ("lfo_depth_ms", ParameterValue::Float(0.0)),
        ("allpass_feedback", ParameterValue::Bool(false)),
    ] {
        dut.set_parameter(ParameterId::from(id), value).unwrap();
    }
    // Batch no-op of the pure subset (the batch path rejects any batch
    // containing the structural `pitch_preserving` key after initialization,
    // a pre-existing scalar behavior orthogonal to per-channel purity).
    let mut reapplied = ParameterSet::new();
    for (id, value) in [
        ("feedback", ParameterValue::Float(0.0)),
        ("mix", ParameterValue::Float(1.0)),
        ("lfo_rate_hz", ParameterValue::Float(0.0)),
        ("lfo_depth_ms", ParameterValue::Float(0.0)),
        ("allpass_feedback", ParameterValue::Bool(false)),
        ("delay_ms_0", ParameterValue::Float(5.0)),
    ] {
        reapplied.insert(ParameterId::from(id), value);
    }
    dut.apply_values(reapplied).unwrap();
    // A pure delay change still applies.
    let mut retune = ParameterSet::new();
    retune.insert(
        ParameterId::from("delay_ms_0"),
        ParameterValue::Float(6.0),
    );
    dut.apply_values(retune).unwrap();
    twin
        .set_parameter(
            ParameterId::from("delay_ms_0"),
            ParameterValue::Float(6.0),
        )
        .unwrap();
    assert_eq!(
        dut.get_parameter(&ParameterId::from("delay_ms_0")),
        Some(ParameterValue::Float(6.0))
    );

    // Rejected writes left no trace: post-rejection renders match the twin
    // that never received them (history retained).
    let input = pseudo_random(3000, 0x9E7, 0.6);
    let a = render_blocks(&mut dut, RATE, &input, &[1024, 17]);
    let b = render_blocks(&mut twin, RATE, &input, &[1024, 17]);
    assert_eq!(a, b);

    // Schema marks the six effect controls unsupported in per-channel mode
    // while scalar instances carry no such mark.
    let schema = dut.parameter_schema();
    for id in [
        "feedback",
        "mix",
        "lfo_rate_hz",
        "lfo_depth_ms",
        "allpass_feedback",
        "pitch_preserving",
    ] {
        let param = schema
            .iter()
            .find(|p| p.id.as_str() == id)
            .unwrap_or_else(|| panic!("per-channel schema missing {id}"));
        assert!(
            param
                .description
                .as_deref()
                .is_some_and(|d| d.contains("per-channel")),
            "{id}: schema must mark the control unsupported in per-channel mode"
        );
    }
    let scalar = DelayPlugin::try_new(1, 10.0, 0.0, 1.0).unwrap();
    for param in scalar.parameter_schema() {
        if matches!(
            param.id.as_str(),
            "feedback"
                | "mix"
                | "lfo_rate_hz"
                | "lfo_depth_ms"
                | "allpass_feedback"
                | "pitch_preserving"
        ) {
            assert!(
                param.description.is_none(),
                "{}: scalar schema must not carry the per-channel mark",
                param.id.as_str()
            );
        }
    }

    // The DSP itself is untouched by the rejections: after reset the
    // instance renders an exact integer routing delay.
    dut.reset();
    let mut impulse = vec![0.0f32; 512];
    impulse[0] = 1.0;
    let output = render_blocks(&mut dut, RATE, &impulse, &[512]);
    // delay_ms_0 is 6.0 ms = 288 samples at 48 kHz.
    for (n, sample) in output.iter().enumerate() {
        let expected = if n == 288 { 1.0 } else { 0.0 };
        assert_eq!(*sample, expected, "routing impulse frame {n}");
    }
}
