//! Wanted-signal preservation and noise suppression (DENOISER-R4/A2).
//!
//! Clean references and noise realizations are generated independently of
//! the plugin and stored separately; metrics compare denoised output
//! against the aligned clean reference, never against plugin-internal
//! oracles. Energy reduction alone is not acceptance: tone and transient
//! preservation carry their own predeclared bounds.
//!
//! Gate status (release stabilization): the frozen tests below use the
//! r1-predeclared stimuli, seeds, and bounds verbatim (COMMON freeze: no
//! tuning after failure). Four tests that expose pre-existing blind
//! default-mode limitations are `#[ignore]`d under the explicit user
//! stabilization deferral — stationary tones are absorbed by the
//! recursive noise floor (minima converge to tone+noise, presence
//! freezes open, floor tracks tone power; DD then locks in zero SNR)
//! and isolated impulses by decision-directed smoothing at alpha = 0.98
//! (2-frame events never ramp; gain hits the 0.1 floor). Mechanisms
//! re-derived in fix-r4-result.md from `src/mcra.rs` Step 5, the DD
//! update, and the G = xi/(xi+10) reduction law in
//! `src/wiener/consts.rs`. That failure is the exact remaining
//! requirement, deferred to the post-release backlog (see
//! release-stabilization/denoiser-result.md); no acceptance scope change
//! is adopted here and nothing is claimed fixed.
//!
//! Running coverage in this file: `changing_noise_improves_in_both_halves`
//! (green) and `stationary_tone_with_captured_noise_profile`, which
//! exercises the published profile workflow (R3/A3 machinery) on the
//! frozen stimuli. The `diagnostic_*` reference tests are `#[ignore]`d
//! mechanism evidence (r3 showed both fail too, refuting the r2
//! minima/DD-only theories).
//!
//! Predeclared bounds used below:
//! - SNR and SI-SDR improve directionally (output above input, in dB).
//! - Wanted-tone level (Goertzel at the tone frequency) holds within 3 dB.
//! - Transient impulse peaks hold within [-6 dB, +3.5 dB].

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::{DenoiserData, DenoiserPlugin, DenoiserPluginParams};

const RATE: u32 = 48_000;
const RATE_F64: f64 = 48_000.0;

/// Deterministic uniform noise in [-1, 1); no thread-local randomness.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / 2_147_483_648.0) * 2.0 - 1.0
    }
}

fn process_all(
    plugin: &mut DenoiserPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    let mut output = input.to_vec();
    let mut pos = 0;
    let mut call = 0;
    while pos < input.len() / channels {
        let n = blocks[call % blocks.len()].min(input.len() / channels - pos);
        assert_eq!(
            plugin
                .process_in_place(
                    &mut output[pos * channels..(pos + n) * channels],
                    &ProcessContext::new(RATE, n)
                )
                .unwrap(),
            n
        );
        pos += n;
        call += 1;
    }
    output
}

/// Clean programme (frozen r1 stimulus): two continuous tones plus sparse
/// unit impulses, mono. Used verbatim by the frozen gates.
fn clean_programme(frames: usize) -> Vec<f32> {
    let mut clean = vec![0.0; frames];
    for (i, sample) in clean.iter_mut().enumerate() {
        let t = i as f32 / RATE as f32;
        *sample = 0.1 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
            + 0.05 * (2.0 * std::f32::consts::PI * 3150.0 * t).sin();
    }
    let mut at = RATE as usize / 2;
    while at < frames {
        clean[at] += 1.0;
        at += RATE as usize / 4;
    }
    clean
}

/// Burst schedule for the diagnostic reference only: 1.2 s period, first
/// burst at 0.6 s, 300 ms on.
const BURST_PERIOD: usize = RATE as usize * 12 / 10;
const BURST_ONSET: usize = RATE as usize * 6 / 10;
const BURST_ON_LEN: usize = RATE as usize * 3 / 10;
/// 5 ms raised-cosine edges keep gating splatter out of the measurement.
const BURST_RAMP: usize = RATE as usize / 200;

/// Gated two-tone bursts for the diagnostic reference test.
///
/// Non-stationary by construction: 300 ms on-frames (~14 STFT frames) ramp
/// decision-directed SNR to about a quarter of the tone's
/// maximum-likelihood value, while 900 ms gaps keep every 50-frame minimum
/// window free of burst power. Not a gate; the frozen gate keeps
/// continuous tones (see the module note).
fn burst_programme(frames: usize) -> Vec<f32> {
    let mut clean = vec![0.0; frames];
    for (i, sample) in clean.iter_mut().enumerate() {
        let t = i as f32 / RATE as f32;
        let tone = 0.1 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
            + 0.05 * (2.0 * std::f32::consts::PI * 3150.0 * t).sin();
        *sample = tone * burst_envelope(i);
    }
    clean
}

fn burst_envelope(i: usize) -> f32 {
    if i < BURST_ONSET {
        return 0.0;
    }
    let phase = (i - BURST_ONSET) % BURST_PERIOD;
    if phase >= BURST_ON_LEN {
        return 0.0;
    }
    if phase < BURST_RAMP {
        let x = phase as f32 / BURST_RAMP as f32;
        return 0.5 - 0.5 * (std::f32::consts::PI * x).cos();
    }
    if phase >= BURST_ON_LEN - BURST_RAMP {
        let x = (BURST_ON_LEN - phase) as f32 / BURST_RAMP as f32;
        return 0.5 - 0.5 * (std::f32::consts::PI * x).cos();
    }
    1.0
}

fn snr_db(clean: &[f32], processed: &[f32]) -> f64 {
    assert_eq!(clean.len(), processed.len());
    let (mut signal, mut error) = (0.0, 0.0);
    for (&c, &p) in clean.iter().zip(processed.iter()) {
        signal += f64::from(c) * f64::from(c);
        error += f64::from(c - p) * f64::from(c - p);
    }
    10.0 * (signal / error.max(1e-30)).log10()
}

fn sisdr_db(clean: &[f32], processed: &[f32]) -> f64 {
    assert_eq!(clean.len(), processed.len());
    let (mut dot, mut target) = (0.0, 0.0);
    for (&c, &p) in clean.iter().zip(processed.iter()) {
        dot += f64::from(p) * f64::from(c);
        target += f64::from(c) * f64::from(c);
    }
    let alpha = dot / target.max(1e-30);
    let (mut num, mut den) = (0.0, 0.0);
    for (&c, &p) in clean.iter().zip(processed.iter()) {
        let scaled = alpha * f64::from(c);
        num += scaled * scaled;
        den += (f64::from(p) - scaled).powi(2);
    }
    10.0 * (num / den.max(1e-30)).log10()
}

/// Hann-windowed generalized Goertzel magnitude squared at `freq_hz`.
fn goertzel_mag2(samples: &[f32], freq_hz: f64) -> f64 {
    use std::f64::consts::PI;
    let n = samples.len();
    let omega = 2.0 * PI * freq_hz / RATE_F64;
    let coeff = 2.0 * omega.cos();
    let mut s1 = 0.0;
    let mut s2 = 0.0;
    for (i, &x) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        let s0 = window * f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(1e-30)
}

fn run_denoised(input: &[f32], channels: usize, params: DenoiserPluginParams) -> (Vec<f32>, usize) {
    let mut plugin = DenoiserPlugin::from_params(channels, params);
    plugin.initialize(f64::from(RATE)).unwrap();
    let latency = plugin.latency_samples();
    let output = process_all(&mut plugin, input, channels, &[1024, 63]);
    (output, latency)
}

// Deferred post-release under explicit user stabilization scope (NOT a
// regression: red since r1, never passed). Exposes the accepted blind
// estimator's stationary-tone absorption; stimuli/seeds/bounds preserved
// verbatim below. Backlog: audit/requirements/denoiser.md R4/A2 and
// release-stabilization/denoiser-result.md. Reproduce with --ignored.
#[test]
#[ignore]
fn stationary_noise_snr_improves_with_tone_preserved() {
    let frames = 4 * RATE as usize;
    let clean = clean_programme(frames);
    // Scale white noise to 0 dB broadband input SNR against the clean mix.
    let signal_power: f64 =
        clean.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / frames as f64;
    let mut rng = Lcg(0x90A115);
    let noise_gain = (3.0 * signal_power).sqrt() as f32;
    let noise: Vec<f32> = (0..frames).map(|_| rng.next() * noise_gain).collect();
    let input: Vec<f32> = clean
        .iter()
        .zip(noise.iter())
        .map(|(&c, &n)| c + n)
        .collect();
    let (output, latency) = run_denoised(&input, 1, DenoiserPluginParams::default());
    // Steady-state voiced region only: skip latency plus one convergence
    // second. Output frame o carries clean frame o - latency.
    let start = latency + RATE as usize;
    let clean_ref = &clean[start - latency..frames - latency];
    let noisy_ref = &input[start - latency..frames - latency];
    let denoised = &output[start..frames];
    let snr_in = snr_db(clean_ref, noisy_ref);
    let snr_out = snr_db(clean_ref, denoised);
    let sisdr_in = sisdr_db(clean_ref, noisy_ref);
    let sisdr_out = sisdr_db(clean_ref, denoised);
    println!("stationary: SNR {snr_in:.2} -> {snr_out:.2} dB");
    println!("stationary: SI-SDR {sisdr_in:.2} -> {sisdr_out:.2} dB");
    // All three metrics are computed and printed before asserting so a gate
    // failure still reports every measurement; assertion set and order are
    // unchanged from the frozen r1 gate.
    let tone_in = goertzel_mag2(clean_ref, 1000.0);
    let tone_out = goertzel_mag2(denoised, 1000.0);
    let drift = 10.0 * (tone_out / tone_in).log10();
    println!("stationary: 1 kHz tone drift {drift:.2} dB");
    assert!(snr_out > snr_in, "SNR regressed");
    assert!(sisdr_out > sisdr_in, "SI-SDR regressed");
    assert!(drift.abs() <= 3.0, "tone drift {drift:.2} dB");
}

#[test]
fn changing_noise_improves_in_both_halves() {
    let frames = 4 * RATE as usize;
    let clean = clean_programme(frames);
    let signal_power: f64 =
        clean.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / frames as f64;
    // Noise steps up 6 dB at the midpoint; MCRA must track the change.
    let mut rng = Lcg(0xCA46E);
    let noise_gain = (3.0 * signal_power).sqrt() as f32;
    let noise: Vec<f32> = (0..frames)
        .map(|i| {
            let gain = if i < frames / 2 { 1.0 } else { 2.0 };
            rng.next() * noise_gain * gain
        })
        .collect();
    let input: Vec<f32> = clean
        .iter()
        .zip(noise.iter())
        .map(|(&c, &n)| c + n)
        .collect();
    let (output, latency) = run_denoised(&input, 1, DenoiserPluginParams::default());
    for (half, range) in [("first", 0..frames / 2), ("second", frames / 2..frames)] {
        // Skip one convergence second after stream start and after the step.
        // The voiced output ends one latency before the input does.
        let from = range.start + RATE as usize;
        let end = range.end.min(frames - latency);
        let clean_ref = &clean[from..end];
        let noisy_ref = &input[from..end];
        let base = from + latency;
        let denoised = &output[base..base + clean_ref.len()];
        let snr_in = snr_db(clean_ref, noisy_ref);
        let snr_out = snr_db(clean_ref, denoised);
        println!("{half} half: SNR {snr_in:.2} -> {snr_out:.2} dB");
        assert!(snr_out > snr_in, "{half} half SNR regressed");
    }
}

// Deferred post-release under explicit user stabilization scope (NOT a
// regression: red since r1, never passed). Exposes DD/ML isolated-impulse
// capture by the recursive floor; stimuli/seeds/bounds preserved verbatim
// below. Backlog: audit/requirements/denoiser.md R4/A2 and
// release-stabilization/denoiser-result.md. Reproduce with --ignored.
#[test]
#[ignore]
fn transient_peaks_preserved_with_fast_attack() {
    let frames = 2 * RATE as usize;
    let mut clean = vec![0.0; frames];
    let mut impulses = Vec::new();
    let mut at = RATE as usize / 2;
    while at + RATE as usize / 8 < frames {
        clean[at] += 1.0;
        impulses.push(at);
        at += RATE as usize / 4;
    }
    // Quiet bed (unit impulses sit ~+16 dB per bin above it, so the
    // Wiener path preserves them instead of treating them as noise).
    let mut rng = Lcg(0x7A551E);
    let input: Vec<f32> = clean.iter().map(|&c| c + rng.next() * 0.005).collect();
    // Both HPSS arms are measured before asserting so the log reports both;
    // assertion set and order are unchanged from the frozen r1 gate.
    let mut peaks = Vec::new();
    for harmonic in [false, true] {
        let (output, latency) = run_denoised(
            &input,
            1,
            DenoiserPluginParams {
                attack_ms: 0.1,
                harmonic_percussive: harmonic,
                ..Default::default()
            },
        );
        for &impulse in &impulses {
            // Search one hop around the expected delayed peak.
            let center = impulse + latency;
            let span = 1024;
            let peak = output[center - span..center + span]
                .iter()
                .map(|s| s.abs())
                .fold(0.0f32, f32::max);
            println!("harmonic={harmonic} impulse@{impulse}: peak {peak:.3}");
            peaks.push((harmonic, peak));
        }
    }
    for (harmonic, peak) in peaks {
        assert!(
            (0.5..=1.5).contains(&peak),
            "harmonic={harmonic} peak {peak:.3} outside [-6 dB, +3.5 dB]"
        );
    }
}

// Deferred post-release under explicit user stabilization scope (NOT a
// regression: red since introduction in r2/r3). Burst-plateau mechanism
// evidence; stimulus/bounds preserved verbatim below. Backlog:
// audit/requirements/denoiser.md R4/A2 and
// release-stabilization/denoiser-result.md. Reproduce with --ignored.
#[test]
#[ignore]
fn diagnostic_burst_wanted_signal_reference() {
    // Non-gate reference: the same metrics on non-stationary tone bursts.
    // The r2 "minima see gaps" theory was wrong: the minima are fine, but
    // the RECURSIVE floor tracks burst-onset power for the ~5 frames speech
    // presence needs to rise (alpha_p = 0.7), captures ~10% of burst power,
    // and ML collapses; r3 red (SI-SDR -0.46 -> -0.82) is the evidence.
    // The plateau drift printed below separates plateau survival from
    // edge-dominated failure for root's audit.
    let frames = 4 * RATE as usize;
    let clean = burst_programme(frames);
    let signal_power: f64 =
        clean.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / frames as f64;
    let mut rng = Lcg(0x90A115);
    let noise_gain = (3.0 * signal_power).sqrt() as f32;
    let noise: Vec<f32> = (0..frames).map(|_| rng.next() * noise_gain).collect();
    let input: Vec<f32> = clean
        .iter()
        .zip(noise.iter())
        .map(|(&c, &n)| c + n)
        .collect();
    let (output, latency) = run_denoised(&input, 1, DenoiserPluginParams::default());
    let start = latency + RATE as usize;
    let clean_ref = &clean[start - latency..frames - latency];
    let noisy_ref = &input[start - latency..frames - latency];
    let denoised = &output[start..frames];
    let snr_in = snr_db(clean_ref, noisy_ref);
    let snr_out = snr_db(clean_ref, denoised);
    let sisdr_in = sisdr_db(clean_ref, noisy_ref);
    let sisdr_out = sisdr_db(clean_ref, denoised);
    println!("diagnostic bursts: SNR {snr_in:.2} -> {snr_out:.2} dB");
    println!("diagnostic bursts: SI-SDR {sisdr_in:.2} -> {sisdr_out:.2} dB");
    // Burst plateau: middle of the third burst (3.10-3.25 s), ramps out.
    // Computed and printed before asserting (same assertion set and order)
    // so the plateau component is visible even when SI-SDR fails: it
    // separates plateau survival (decision-directed ramp converges) from
    // edge-dominated failure (onset capture + transition mutilation).
    const TONE_FROM: usize = 148_800;
    const TONE_TO: usize = 156_000;
    let tone_in = goertzel_mag2(&clean[TONE_FROM..TONE_TO], 1000.0);
    let tone_out = goertzel_mag2(&output[TONE_FROM + latency..TONE_TO + latency], 1000.0);
    let drift = 10.0 * (tone_out / tone_in).log10();
    println!("diagnostic bursts: 1 kHz tone drift {drift:.2} dB");
    assert!(snr_out > snr_in, "SNR regressed");
    assert!(sisdr_out > sisdr_in, "SI-SDR regressed");
    assert!(drift.abs() <= 3.0, "tone drift {drift:.2} dB");
}

// Deferred post-release under explicit user stabilization scope (NOT a
// regression: red since introduction in r2/r3). ML-arm mechanism
// evidence; stimulus/bounds preserved verbatim below. Backlog:
// audit/requirements/denoiser.md R4/A2 and
// release-stabilization/denoiser-result.md. Reproduce with --ignored.
#[test]
#[ignore]
fn diagnostic_transient_ml_snr_reference() {
    // Non-gate reference: transient peaks with maximum-likelihood SNR
    // (`dd_enabled = false`). r3 showed early impulses survive (~0.5 via
    // the reduction law G = xi/(xi+10)) while the last dies (0.104): the
    // recursive floor captures 2-frame impulse power faster than gaps
    // recover it. Both arms print before asserting; the unit
    // characterization in src/tests.rs traces the floor trajectory.
    let frames = 2 * RATE as usize;
    let mut clean = vec![0.0; frames];
    let mut impulses = Vec::new();
    let mut at = RATE as usize / 2;
    while at + RATE as usize / 8 < frames {
        clean[at] += 1.0;
        impulses.push(at);
        at += RATE as usize / 4;
    }
    let mut rng = Lcg(0x7A551E);
    let input: Vec<f32> = clean.iter().map(|&c| c + rng.next() * 0.005).collect();
    // Both arms are measured before asserting (same eight peak bounds) so
    // a failure in the first arm cannot hide the second arm's evidence.
    let mut peaks = Vec::new();
    for harmonic in [false, true] {
        let (output, latency) = run_denoised(
            &input,
            1,
            DenoiserPluginParams {
                attack_ms: 0.1,
                dd_enabled: false,
                harmonic_percussive: harmonic,
                ..Default::default()
            },
        );
        for &impulse in &impulses {
            let center = impulse + latency;
            let peak = output[center - 1024..center + 1024]
                .iter()
                .map(|s| s.abs())
                .fold(0.0f32, f32::max);
            println!("diagnostic ml: harmonic={harmonic} impulse@{impulse}: peak {peak:.3}");
            peaks.push((harmonic, impulse, peak));
        }
    }
    for (harmonic, impulse, peak) in peaks {
        assert!(
            (0.5..=1.5).contains(&peak),
            "harmonic={harmonic} impulse@{impulse} peak {peak:.3} outside [-6 dB, +3.5 dB]"
        );
    }
}

#[test]
fn stationary_tone_with_captured_noise_profile() {
    // Published profile workflow (R3/A3 machinery) on the frozen stimuli:
    // learn a noise-only profile from the separately stored noise, reset
    // stream state (the captured profile survives reset by design), then
    // process the tone+noise mixture with the captured profile bypassing
    // minimum-statistics tracking. Same signals, seeds, and bounds as the
    // frozen stationary gate; only the workflow differs. A 6 s mixture lets
    // decision-directed SNR converge (~150 frames for 95% ramp); metrics
    // use the converged last 2 s. This test does not replace the frozen
    // default-mode gate and claims nothing about default-mode quality.
    let frames = 6 * RATE as usize;
    let clean = clean_programme(frames);
    let signal_power: f64 =
        clean.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / frames as f64;
    let mut rng = Lcg(0x90A115);
    let noise_gain = (3.0 * signal_power).sqrt() as f32;
    let noise: Vec<f32> = (0..frames).map(|_| rng.next() * noise_gain).collect();
    // Phase 1: noise-only profile capture (first 2 s of the same noise).
    let mut plugin = DenoiserPlugin::from_params(1, DenoiserPluginParams::default());
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
        .parametric_set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    let learn_len = 2 * RATE as usize;
    let _ = process_all(&mut plugin, &noise[..learn_len], 1, &[1024, 63]);
    let data = plugin
        .get_data()
        .unwrap()
        .downcast::<DenoiserData>()
        .unwrap();
    assert!(data.has_captured_profile, "profile capture must complete");
    assert!(data.using_captured_profile);
    drop(data);
    // Phase 2: mixture through the profiled plugin from clean stream state.
    plugin.reset();
    let input: Vec<f32> = clean
        .iter()
        .zip(noise.iter())
        .map(|(&c, &n)| c + n)
        .collect();
    let output = process_all(&mut plugin, &input, 1, &[1024, 63]);
    let latency = plugin.latency_samples();
    // Converged last 2 s: clean frames [4 s, 6 s - latency].
    let from = 4 * RATE as usize;
    let clean_ref = &clean[from..frames - latency];
    let noisy_ref = &input[from..frames - latency];
    let base = from + latency;
    let denoised = &output[base..base + clean_ref.len()];
    let snr_in = snr_db(clean_ref, noisy_ref);
    let snr_out = snr_db(clean_ref, denoised);
    let sisdr_in = sisdr_db(clean_ref, noisy_ref);
    let sisdr_out = sisdr_db(clean_ref, denoised);
    println!("profile: SNR {snr_in:.2} -> {snr_out:.2} dB");
    println!("profile: SI-SDR {sisdr_in:.2} -> {sisdr_out:.2} dB");
    assert!(snr_out > snr_in, "SNR regressed");
    assert!(sisdr_out > sisdr_in, "SI-SDR regressed");
    // Wanted tone on the converged last second.
    let tone_from = 5 * RATE as usize;
    let tone_to = frames - latency;
    let tone_in = goertzel_mag2(&clean[tone_from..tone_to], 1000.0);
    let tone_out = goertzel_mag2(&output[tone_from + latency..tone_to + latency], 1000.0);
    let drift = 10.0 * (tone_out / tone_in).log10();
    println!("profile: 1 kHz tone drift {drift:.2} dB");
    assert!(drift.abs() <= 3.0, "tone drift {drift:.2} dB");
}
