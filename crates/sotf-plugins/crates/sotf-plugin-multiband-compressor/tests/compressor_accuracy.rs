//! Independent accuracy oracles for the compressor family.
//!
//! Covers COMPRESSOR-A1/A2/A3 and MULTIBAND-COMPRESSOR-A1/A2/A3 with references
//! that never reuse production fast math, coefficients, or filter state:
//! f64 knee/ratio/range law, f64 one-pole attack/hold/release trajectories,
//! windowed-RMS energy, Butterworth magnitude checks, an independent LR4
//! prototype cascade for crossover phase/sum, and sample-exact drain accounting.
//!
//! Predeclared numerical bounds (fixed before measuring):
//! - settled static gain law: 0.08 dB (production fast math ≈ 0.02 dB plus
//!   f32 envelope accumulation margin; Gate precedent uses 0.02 dB)
//! - attack/release trajectory: 0.12 dB per sample
//! - hold freeze level: 0.15 dB; hold duration is an exact sample counter
//! - RMS/HF functional separation: 0.5 dB; RMS-vs-Peak DC calibration: 0.2 dB
//! - HPF LF rejection: on < 1.5 dB GR, off > 10 dB GR (large-effect functional)
//! - lookahead/dry alignment: exact peak index, 1e-6 peak and floor
//! - LR4 complex transfer error: 2e-3 (AUD141 gate, magnitude and phase)
//! - stereo image: 0.15 dB per channel
//! - automation partition invariance: 2e-5 (existing bound, not weakened)
//! - realtime: 0 allocations and 0 frees on the cold measured path
//!
//! NOTE: the ms-rust compliance footer is intentionally omitted until the
//! coordinator executes the focused gates (shell was disabled here).

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::{ProcessContext, TailLength};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
struct TrackingAllocator;
fn record() {
    let _ = TRACKING.try_with(|tracking| {
        if tracking.get() {
            let _ = OPERATIONS.try_with(|count| count.set(count.get() + 1));
        }
    });
}
// SAFETY: all memory operations delegate unchanged to System. The thread-local
// counters neither allocate nor inspect the allocated memory.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record();
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

// ---------------------------------------------------------------------------
// Independent f64 oracles (no production helpers)
// ---------------------------------------------------------------------------

/// Static compressor gain-reduction law in dB, derived from the threshold /
/// ratio / knee contract independently of `calculate_gain_reduction`.
fn oracle_gain_reduction(idb: f64, threshold: f64, ratio: f64, knee: f64) -> f64 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee < 0.1 {
        if idb <= threshold {
            0.0
        } else {
            (idb - threshold) * slope
        }
    } else if idb < threshold - knee / 2.0 {
        0.0
    } else if idb > threshold + knee / 2.0 {
        (idb - threshold) * slope
    } else {
        let over = idb - threshold + knee / 2.0;
        let f = over / knee;
        f * f * (knee / 2.0) * slope
    }
}

fn oracle_range_limit(range_db: f64) -> f64 {
    if range_db >= 120.0 {
        f64::INFINITY
    } else {
        range_db
    }
}

/// One-pole envelope coefficient matching the documented time-constant
/// convention `exp(-1 / (time_ms * sr / 1000))`.
fn oracle_envelope_coeff(time_ms: f64, sample_rate: u32) -> f64 {
    (-1.0 / (time_ms.max(0.01) * 0.001 * sample_rate.max(1) as f64)).exp()
}

fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

fn linear_to_db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

// Complex helpers for the LR4 prototype oracle: (re, im) pairs.
fn c_mul(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0)
}
fn c_div(a: (f64, f64), b: (f64, f64)) -> (f64, f64) {
    let denom = b.0 * b.0 + b.1 * b.1;
    ((a.0 * b.0 + a.1 * b.1) / denom, (a.1 * b.0 - a.0 * b.1) / denom)
}
fn c_error(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Independent LR4 low/high complex transfers from the analog prototype with
/// prewarped bilinear substitution (AUD141 convention), not production code.
fn oracle_lr4_pair(freq: f64, cutoff: f64, sample_rate: f64) -> ((f64, f64), (f64, f64)) {
    let w = (std::f64::consts::PI * freq / sample_rate).tan()
        / (std::f64::consts::PI * cutoff / sample_rate).tan();
    // d = s^2 + sqrt(2) s + 1 with s = j w.
    let sqrt2 = std::f64::consts::SQRT_2;
    let d = (1.0 - w * w, sqrt2 * w);
    let d2 = c_mul(d, d);
    let low = c_div((1.0, 0.0), d2);
    // s^4 = (j w)^4 = w^4 (real).
    let w4 = w * w * w * w;
    let high = (low.0 * w4, low.1 * w4);
    (low, high)
}

/// Band transfers for the actual cascaded crossover topology used by this
/// crate: B0 = L0, Bk = H0..H(k-1) * Lk, Blast = H0..H(last).
fn oracle_cascade_bands(freq: f64, cuts: &[f64], sample_rate: f64) -> Vec<(f64, f64)> {
    let pairs: Vec<((f64, f64), (f64, f64))> = cuts
        .iter()
        .map(|cutoff| oracle_lr4_pair(freq, *cutoff, sample_rate))
        .collect();
    let bands = cuts.len() + 1;
    let mut out = Vec::with_capacity(bands);
    for band in 0..bands {
        let mut transfer = (1.0, 0.0);
        for (split, (low, high)) in pairs.iter().enumerate() {
            if band == split {
                transfer = c_mul(transfer, *low);
                break;
            }
            transfer = c_mul(transfer, *high);
        }
        out.push(transfer);
    }
    out
}

// ---------------------------------------------------------------------------
// Small processing helpers
// ---------------------------------------------------------------------------

fn one_band_plugin(
    channels: usize,
    sample_rate: u32,
    configure: impl FnOnce(&mut MultibandCompressorPluginParams),
) -> MultibandCompressorPlugin {
    let mut params = MultibandCompressorPluginParams {
        num_bands: 1,
        ..Default::default()
    };
    configure(&mut params);
    let mut plugin = MultibandCompressorPlugin::try_from_params(channels, params, sample_rate)
        .expect("valid one-band configuration");
    plugin.initialize(sample_rate).unwrap();
    plugin
}

fn process_all(
    plugin: &mut MultibandCompressorPlugin,
    sample_rate: u32,
    channels: usize,
    input: &[f32],
    block_frames: usize,
) -> Vec<f32> {
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut offset = 0;
    while offset < frames {
        let count = block_frames.min(frames - offset);
        let span = offset * channels..(offset + count) * channels;
        plugin
            .process_in_place(&mut output[span], &ProcessContext::new(sample_rate, count))
            .unwrap();
        offset += count;
    }
    output
}

fn peak_db(samples: &[f32]) -> f64 {
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max) as f64;
    linear_to_db(peak)
}

// ---------------------------------------------------------------------------
// A1: static knee/ratio/range law and absolute timing
// ---------------------------------------------------------------------------


#[test]
fn static_knee_ratio_range_law_matches_f64_oracle() {
    const BOUND_DB: f64 = 0.08;
    let mut worst = 0.0f64;
    let mut worst_case = String::new();
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for ratio in [1.0f32, 4.0, 20.0] {
            for knee in [0.0f32, 6.0] {
                for range in [120.0f32, 6.0] {
                    let mut plugin = one_band_plugin(1, sample_rate, |params| {
                        params.threshold_db = -20.0;
                        params.ratio = ratio;
                        params.attack_ms = 0.1;
                        params.release_ms = 20.0;
                        params.knee_db = knee;
                        params.range_db = range;
                        params.hold_ms = 0.0;
                        params.mix = 1.0;
                    });
                    for input_db in [-40.0, -30.0, -25.0, -20.0, -15.0, -10.0, -5.0, 0.0] {
                        // 150 ms settles the 20 ms release by e^-7.5.
                        // Fresh envelope per level so each case is independent.
                        plugin.reset();
                        let frames = sample_rate as usize * 150 / 1000;
                        let input =
                            vec![db_to_linear(input_db) as f32; frames];
                        let output =
                            process_all(&mut plugin, sample_rate, 1, &input, 1024);
                        let measured = linear_to_db(f64::from(output[frames - 1]));
                        let target = oracle_gain_reduction(
                            input_db,
                            -20.0,
                            f64::from(ratio),
                            f64::from(knee),
                        )
                        .min(oracle_range_limit(f64::from(range)));
                        let expected = input_db - target;
                        let error = (measured - expected).abs();
                        if error > worst {
                            worst = error;
                            worst_case = format!(
                                "sr={sample_rate} ratio={ratio} knee={knee} \
                                 range={range} in={input_db}"
                            );
                        }
                        assert!(
                            error < BOUND_DB,
                            "static law error {error:.4} dB exceeds {BOUND_DB} dB \
                             at {worst_case} (measured {measured:.4}, expected {expected:.4})"
                        );
                    }
                }
            }
        }
    }
    assert!(
        worst < BOUND_DB,
        "worst static-law error {worst:.4} dB at {worst_case}"
    );
}

#[test]
fn attack_hold_release_follow_absolute_sample_clock() {
    const TRAJECTORY_DB: f64 = 0.12;
    const HOLD_DB: f64 = 0.15;
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        let attack_ms = 5.0f64;
        let release_ms = 50.0f64;
        let hold_ms = 50.0f64;
        let hold_samples = (hold_ms * sample_rate as f64 / 1000.0).round() as usize;
        let mut plugin = one_band_plugin(1, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 4.0;
            params.attack_ms = attack_ms as f32;
            params.release_ms = release_ms as f32;
            params.knee_db = 0.0;
            params.range_db = 120.0;
            params.hold_ms = hold_ms as f32;
            params.mix = 1.0;
        });
        // Settle below threshold at -40 dB so the envelope starts at zero.
        let quiet = vec![db_to_linear(-40.0) as f32; sample_rate as usize / 5];
        process_all(&mut plugin, sample_rate, 1, &quiet, 1024);

        // Attack: step to 0 dB; target reduction is 15 dB.
        let attack_frames = sample_rate as usize / 4;
        let loud = vec![1.0f32; attack_frames];
        let attack_out = process_all(&mut plugin, sample_rate, 1, &loud, 1024);
        let attack_coeff = oracle_envelope_coeff(attack_ms, sample_rate);
        let mut envelope = 0.0f64;
        let mut worst_attack = 0.0f64;
        for sample in attack_out.iter() {
            envelope = 15.0 + attack_coeff * (envelope - 15.0);
            let expected = -envelope;
            let measured = linear_to_db(f64::from(*sample));
            worst_attack = worst_attack.max((measured - expected).abs());
        }
        assert!(
            worst_attack < TRAJECTORY_DB,
            "sr={sample_rate}: attack trajectory error {worst_attack:.4} dB"
        );

        // Release with hold: step down to -40 dB (still observable).
        // The envelope must freeze for exactly `hold_samples`, then release.
        let release_frames = hold_samples + sample_rate as usize / 2;
        let down = vec![db_to_linear(-40.0) as f32; release_frames];
        let down_out = process_all(&mut plugin, sample_rate, 1, &down, 1024);
        let held_level = linear_to_db(f64::from(down_out[4])) + 40.0;
        assert!(
            (held_level + 15.0).abs() < HOLD_DB,
            "sr={sample_rate}: envelope entering hold is {held_level:.3} dB, want -15"
        );
        for (offset, sample) in down_out.iter().enumerate().take(hold_samples).skip(4) {
            let level = linear_to_db(f64::from(*sample)) + 40.0;
            assert!(
                (level - held_level).abs() < HOLD_DB,
                "sr={sample_rate}: envelope moved during hold at +{offset}: {level:.3}"
            );
        }
        let release_coeff = oracle_envelope_coeff(release_ms, sample_rate);
        let mut envelope = -held_level;
        let mut worst_release = 0.0f64;
        for sample in down_out.iter().skip(hold_samples) {
            envelope *= release_coeff;
            let expected = -40.0 - envelope;
            let measured = linear_to_db(f64::from(*sample));
            worst_release = worst_release.max((measured - expected).abs());
        }
        assert!(
            worst_release < TRAJECTORY_DB,
            "sr={sample_rate}: release trajectory error {worst_release:.4} dB"
        );
    }
}

// ---------------------------------------------------------------------------
// Detector stages: RMS and sidechain HPF
// ---------------------------------------------------------------------------

#[test]
fn rms_detector_matches_windowed_energy_oracle() {
    const FUNCTIONAL_DB: f64 = 0.5;
    const CALIBRATION_DB: f64 = 0.2;
    let sample_rate = 48_000u32;
    // Sine peak -17 dB: Peak sees -17 dB, RMS sees -20.01 dB (below threshold).
    let peak_db_in = -17.0f64;
    let amplitude = db_to_linear(peak_db_in);
    let frames = sample_rate as usize / 5; // 200 ms, 20 RMS windows
    let sine: Vec<f32> = (0..frames)
        .map(|n| {
            (amplitude
                * (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / sample_rate as f64).sin())
                as f32
        })
        .collect();
    let mut gr = [0.0f64; 2];
    for (slot, mode) in ["Peak", "RMS"].iter().enumerate() {
        let mut plugin = one_band_plugin(1, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 4.0;
            params.attack_ms = 0.1;
            params.release_ms = 20.0;
            params.knee_db = 0.0;
            params.mix = 1.0;
            params.detection_mode = Some(mode.to_string());
        });
        let output = process_all(&mut plugin, sample_rate, 1, &sine, 1024);
        let tail = &output[output.len() - sample_rate as usize / 20..];
        gr[slot] = peak_db_in - peak_db(tail);
    }
    assert!(
        (gr[0] - 2.25).abs() < FUNCTIONAL_DB,
        "Peak GR on -17 dB sine is {:.3} dB, want 2.25",
        gr[0]
    );
    assert!(
        gr[1].abs() < FUNCTIONAL_DB,
        "RMS GR on -17 dB sine is {:.3} dB, want ~0",
        gr[1]
    );
    assert!(
        gr[0] - gr[1] > 1.0,
        "Peak/RMS separation too small: {:.3} dB",
        gr[0] - gr[1]
    );

    // DC calibration: RMS(DC) == Peak(DC), so both modes must agree.
    let dc = vec![amplitude as f32; frames];
    let mut dc_gr = [0.0f64; 2];
    for (slot, mode) in ["Peak", "RMS"].iter().enumerate() {
        let mut plugin = one_band_plugin(1, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 4.0;
            params.attack_ms = 0.1;
            params.release_ms = 20.0;
            params.knee_db = 0.0;
            params.mix = 1.0;
            params.detection_mode = Some(mode.to_string());
        });
        let output = process_all(&mut plugin, sample_rate, 1, &dc, 1024);
        dc_gr[slot] = peak_db_in - linear_to_db(f64::from(output[frames - 1]));
    }
    assert!(
        (dc_gr[0] - 2.25).abs() < 0.3 && (dc_gr[1] - 2.25).abs() < 0.3,
        "DC GR peak={:.3} rms={:.3}, want 2.25",
        dc_gr[0],
        dc_gr[1]
    );
    assert!(
        (dc_gr[0] - dc_gr[1]).abs() < CALIBRATION_DB,
        "Peak/RMS DC disagreement {:.4} dB",
        (dc_gr[0] - dc_gr[1]).abs()
    );
}

#[test]
fn sidechain_hpf_filters_detector_not_audio() {
    let sample_rate = 48_000u32;
    let tone = |freq: f64, db: f64, frames: usize| -> Vec<f32> {
        let amplitude = db_to_linear(db);
        (0..frames)
            .map(|n| {
                (amplitude
                    * (2.0 * std::f64::consts::PI * freq * n as f64 / sample_rate as f64).sin())
                    as f32
            })
            .collect()
    };
    let compressed_peak = |freq: f64, hpf: Option<f32>, order: &str| -> f64 {
        let mut plugin = one_band_plugin(1, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 20.0;
            params.attack_ms = 0.1;
            params.release_ms = 20.0;
            params.knee_db = 0.0;
            params.mix = 1.0;
            params.sidechain_hpf_hz = hpf;
            params.sidechain_hpf_order = Some(order.to_string());
            params.sidechain_hpf_enabled = Some(hpf.is_some());
        });
        let input = tone(freq, -6.0, sample_rate as usize / 2);
        let output = process_all(&mut plugin, sample_rate, 1, &input, 1024);
        peak_db(&output[output.len() - sample_rate as usize / 10..])
    };

    // 50 Hz loud tone: unfiltered detection compresses hard; a 150 Hz HPF
    // removes ~19 dB from the detector so the tone passes nearly untouched.
    let lf_open = compressed_peak(50.0, None, "2nd");
    let lf_hpf2 = compressed_peak(50.0, Some(150.0), "2nd");
    let lf_gr_open = -6.0 - lf_open;
    let lf_gr_hpf = -6.0 - lf_hpf2;
    assert!(
        lf_gr_open > 10.0,
        "50 Hz without HPF compressed only {lf_gr_open:.2} dB"
    );
    assert!(
        lf_gr_hpf < 1.5,
        "50 Hz with 150 Hz HPF still compressed {lf_gr_hpf:.2} dB"
    );

    // 100 Hz sits on the skirt: 4th order must reject more than 2nd order.
    let mid_hpf2 = compressed_peak(100.0, Some(150.0), "2nd");
    let mid_hpf4 = compressed_peak(100.0, Some(150.0), "4th");
    let mid_gr2 = -6.0 - mid_hpf2;
    let mid_gr4 = -6.0 - mid_hpf4;
    assert!(
        (mid_gr2 - 5.9).abs() < 1.5,
        "100 Hz 2nd-order GR is {mid_gr2:.2} dB, want ~5.9"
    );
    assert!(
        mid_gr4 < 1.5,
        "100 Hz 4th-order GR is {mid_gr4:.2} dB, want ~0"
    );

    // 5 kHz passes the sidechain filter: HPF on/off must agree, and the audio
    // path itself is never filtered (no LF loss on the tone).
    let hf_open = compressed_peak(5000.0, None, "2nd");
    let hf_hpf = compressed_peak(5000.0, Some(150.0), "2nd");
    assert!(
        (hf_open - hf_hpf).abs() < 0.7,
        "5 kHz HPF on/off mismatch: {hf_open:.2} vs {hf_hpf:.2}"
    );
    assert!(
        -6.0 - hf_open > 10.0,
        "5 kHz tone escaped compression: {hf_open:.2} dB peak"
    );
}

// ---------------------------------------------------------------------------
// A3: lookahead/dry alignment
// ---------------------------------------------------------------------------

#[test]
fn lookahead_dry_alignment_is_sample_exact() {
    let sample_rate = 48_000u32;
    for lookahead_ms in [0.0f32, 0.01, 5.0] {
        let expected_delay = if lookahead_ms == 0.0 {
            0
        } else {
            ((f64::from(lookahead_ms) * f64::from(sample_rate) / 1000.0).round() as usize).max(1)
        };
        for mix in [0.0f32, 0.5, 1.0] {
            let mut plugin = one_band_plugin(1, sample_rate, |params| {
                params.ratio = 1.0;
                params.mix = mix;
                params.per_band_lookahead_ms = lookahead_ms;
            });
            assert_eq!(
                plugin.latency_samples(),
                expected_delay,
                "reported latency must equal the active ring delay"
            );
            let frames = 1024usize;
            let mut input = vec![0.0f32; frames];
            input[0] = 1.0;
            let output = process_all(&mut plugin, sample_rate, 1, &input, 1024);
            for (index, sample) in output.iter().enumerate() {
                if index == expected_delay {
                    assert!(
                        (f64::from(*sample) - 1.0).abs() < 1e-6,
                        "lookahead={lookahead_ms} mix={mix}: peak at {index} is {sample}"
                    );
                } else {
                    assert!(
                        sample.abs() < 1e-6,
                        "lookahead={lookahead_ms} mix={mix}: leakage at {index}: {sample}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A2: adjacent-band multitone with independent crossover phase/sum oracle
// ---------------------------------------------------------------------------

/// Project the measured window onto one tone; returns the complex transfer
/// normalized by the tone amplitude. `start` is the absolute start frame.
fn measure_tone_transfer(
    samples: &[f32],
    start: usize,
    freq: f64,
    sample_rate: f64,
    amplitude: f64,
) -> (f64, f64) {
    let (mut sin_sum, mut cos_sum) = (0.0f64, 0.0f64);
    for (index, sample) in samples.iter().enumerate() {
        let phase =
            2.0 * std::f64::consts::PI * freq * (start + index) as f64 / sample_rate;
        sin_sum += f64::from(*sample) * phase.sin();
        cos_sum += f64::from(*sample) * phase.cos();
    }
    let scale = 2.0 / samples.len() as f64 / amplitude;
    (sin_sum * scale, cos_sum * scale)
}

#[test]
fn adjacent_multitone_matches_lr4_prototype_phase_and_sum() {
    const COMPLEX_BOUND: f64 = 2e-3;
    let sample_rate = 48_000u32;
    let sr = sample_rate as f64;
    let cuts = [500.0f64, 2000.0];
    // Coherent adjacent pairs around each cut (N = 4096, df = 11.71875 Hz).
    let tones = [42.0 * sr / 4096.0, 43.0 * sr / 4096.0, 170.0 * sr / 4096.0, 171.0 * sr / 4096.0];
    let amplitude = 0.2f64;
    let settle = 8192usize;
    let measure = 4096usize;
    let total = settle + measure;
    let input: Vec<f32> = (0..total)
        .map(|n| {
            tones
                .iter()
                .map(|freq| {
                    amplitude * (2.0 * std::f64::consts::PI * freq * n as f64 / sr).sin()
                })
                .sum::<f64>() as f32
        })
        .collect();

    let render = |solo_band: Option<usize>| -> Vec<f32> {
        let params = MultibandCompressorPluginParams {
            num_bands: 3,
            crossover_frequencies: vec![cuts[0] as f32, cuts[1] as f32],
            ratio: 1.0,
            mix: 1.0,
            ..Default::default()
        };
        let mut plugin =
            MultibandCompressorPlugin::try_from_params(1, params, sample_rate).unwrap();
        plugin.initialize(sample_rate).unwrap();
        if let Some(band) = solo_band {
            plugin
                .set_parameter(
                    ParameterId::from(format!("band_{band}_solo")),
                    ParameterValue::Bool(true),
                )
                .unwrap();
        }
        process_all(&mut plugin, sample_rate, 1, &input, 1024)
    };

    // Per-band transfers via solo, plus the recombined sum.
    let mut band_measured = Vec::new();
    for band in 0..3 {
        let output = render(Some(band));
        let window = &output[settle..];
        let transfers: Vec<(f64, f64)> = tones
            .iter()
            .map(|freq| measure_tone_transfer(window, settle, *freq, sr, amplitude))
            .collect();
        band_measured.push(transfers);
    }
    let sum_output = render(None);
    let sum_window = &sum_output[settle..];
    let sum_measured: Vec<(f64, f64)> = tones
        .iter()
        .map(|freq| measure_tone_transfer(sum_window, settle, *freq, sr, amplitude))
        .collect();

    for (tone_index, freq) in tones.iter().enumerate() {
        let reference = oracle_cascade_bands(*freq, &cuts, sr);
        let mut worst = 0.0f64;
        for band in 0..3 {
            let error = c_error(band_measured[band][tone_index], reference[band]);
            worst = worst.max(error);
            assert!(
                error < COMPLEX_BOUND,
                "band {band} tone {freq:.2} Hz complex error {error:.6} \
                 (measured {:?}, prototype {:?})",
                band_measured[band][tone_index],
                reference[band]
            );
        }
        let reference_sum = reference
            .iter()
            .fold((0.0, 0.0), |acc, band| (acc.0 + band.0, acc.1 + band.1));
        let sum_error = c_error(sum_measured[tone_index], reference_sum);
        worst = worst.max(sum_error);
        assert!(
            sum_error < COMPLEX_BOUND,
            "sum tone {freq:.2} Hz complex error {sum_error:.6} \
             (measured {:?}, prototype {reference_sum:?})",
            sum_measured[tone_index]
        );
        // The measured solo bands must add up to the measured sum: this
        // checks the recombination accounting independently of the prototype.
        let measured_sum = band_measured
            .iter()
            .map(|band| band[tone_index])
            .fold((0.0, 0.0), |acc, band| (acc.0 + band.0, acc.1 + band.1));
        let closure = c_error(measured_sum, sum_measured[tone_index]);
        assert!(
            closure < COMPLEX_BOUND,
            "tone {freq:.2} Hz solo/sum closure error {closure:.6}"
        );
        assert!(worst < COMPLEX_BOUND);
    }
}

// ---------------------------------------------------------------------------
// A2/A3: stereo image under linked/unlinked detection
// ---------------------------------------------------------------------------

#[test]
fn linked_and_unlinked_stereo_image() {
    const IMAGE_DB: f64 = 0.15;
    let sample_rate = 48_000u32;
    // Asymmetric DC program: left is 15 dB into compression, right is silent.
    let frames = sample_rate as usize / 4;
    let input: Vec<f32> = (0..frames).flat_map(|_| [1.0f32, 0.03162f32]).collect();
    for link in [1.0f32, 0.0] {
        let mut plugin = one_band_plugin(2, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 4.0;
            params.attack_ms = 0.1;
            params.release_ms = 10.0;
            params.knee_db = 0.0;
            params.mix = 1.0;
            params.link_amount = link;
        });
        let output = process_all(&mut plugin, sample_rate, 2, &input, 1024);
        let tail = &output[output.len() - 2000..];
        let (mut left, mut right) = (0.0f64, 0.0f64);
        for pair in tail.as_chunks::<2>().0 {
            left += f64::from(pair[0]);
            right += f64::from(pair[1]);
        }
        let count = (tail.len() / 2) as f64;
        let (left_db, right_db) = (linear_to_db(left / count), linear_to_db(right / count));
        let (expected_left, expected_right) = if link >= 1.0 {
            (-15.0, -45.0)
        } else {
            (-15.0, -30.0)
        };
        assert!(
            (left_db - expected_left).abs() < IMAGE_DB,
            "link={link}: left {left_db:.3} dB, want {expected_left}"
        );
        assert!(
            (right_db - expected_right).abs() < IMAGE_DB,
            "link={link}: right {right_db:.3} dB, want {expected_right}"
        );
    }

    // RMS agrees with Peak on DC program in both link modes.
    for link in [1.0f32, 0.0] {
        let mut plugin = one_band_plugin(2, sample_rate, |params| {
            params.threshold_db = -20.0;
            params.ratio = 4.0;
            params.attack_ms = 0.1;
            params.release_ms = 10.0;
            params.knee_db = 0.0;
            params.mix = 1.0;
            params.link_amount = link;
            params.detection_mode = Some("RMS".to_string());
        });
        let output = process_all(&mut plugin, sample_rate, 2, &input, 1024);
        let tail = &output[output.len() - 2000..];
        let (mut left, mut right) = (0.0f64, 0.0f64);
        for pair in tail.as_chunks::<2>().0 {
            left += f64::from(pair[0]);
            right += f64::from(pair[1]);
        }
        let count = (tail.len() / 2) as f64;
        let (left_db, right_db) = (linear_to_db(left / count), linear_to_db(right / count));
        let (expected_left, expected_right) = if link >= 1.0 {
            (-15.0, -45.0)
        } else {
            (-15.0, -30.0)
        };
        assert!(
            (left_db - expected_left).abs() < 0.2,
            "RMS link={link}: left {left_db:.3} dB, want {expected_left}"
        );
        assert!(
            (right_db - expected_right).abs() < 0.2,
            "RMS link={link}: right {right_db:.3} dB, want {expected_right}"
        );
    }
}

// ---------------------------------------------------------------------------
// A2/A3: broadband preset equivalence, legacy rejection, ID compatibility
// ---------------------------------------------------------------------------

#[test]
fn broadband_preset_matches_explicit_core_config() {
    let sample_rate = 48_000u32;
    // Explicit full broadband construction, including the newly implemented
    // detector controls and the single-band aliases.
    let explicit = MultibandCompressorPluginParams {
        num_bands: 1,
        threshold_db: -24.0,
        ratio: 4.0,
        attack_ms: 1.0,
        release_ms: 40.0,
        knee_db: 3.0,
        link_channels: true,
        mix: 0.8,
        per_band_lookahead_ms: 0.0,
        ms_mode: false,
        sidechain_tilt_db: 1.5,
        link_amount: 0.75,
        makeup_gain: Some(2.0),
        auto_makeup: Some(false),
        measured_auto_makeup: Some(false),
        sidechain_hpf_hz: Some(90.0),
        sidechain_hpf_order: Some("2nd".to_string()),
        sidechain_hpf_enabled: Some(true),
        detection_mode: Some("RMS".to_string()),
        lookahead_ms: None,
        program_dependent_release: None,
        sidechain_external: None,
        range_db: 12.0,
        hold_ms: 5.0,
        ..Default::default()
    };
    let mut reference =
        MultibandCompressorPlugin::try_from_params(2, explicit, sample_rate).unwrap();
    reference.initialize(sample_rate).unwrap();

    // Same configuration reached through the runtime setters instead.
    let mut configured = MultibandCompressorPlugin::try_from_params(
        2,
        MultibandCompressorPluginParams {
            num_bands: 1,
            ..Default::default()
        },
        sample_rate,
    )
    .unwrap();
    configured.initialize(sample_rate).unwrap();
    for (id, value) in [
        ("threshold", ParameterValue::Float(-24.0)),
        ("ratio", ParameterValue::Float(4.0)),
        ("attack", ParameterValue::Float(1.0)),
        ("release", ParameterValue::Float(40.0)),
        ("knee", ParameterValue::Float(3.0)),
        ("mix", ParameterValue::Float(0.8)),
        ("sidechain_tilt_db", ParameterValue::Float(1.5)),
        ("link_amount", ParameterValue::Float(0.75)),
        ("makeup_gain", ParameterValue::Float(2.0)),
        ("range_db", ParameterValue::Float(12.0)),
        ("hold_ms", ParameterValue::Float(5.0)),
        ("sidechain_hpf_hz", ParameterValue::Float(90.0)),
        ("sidechain_hpf_order", ParameterValue::Int(0)),
        ("sidechain_hpf_enabled", ParameterValue::Bool(true)),
        ("detection_mode", ParameterValue::Int(1)),
    ] {
        configured
            .set_parameter(ParameterId::from(id), value)
            .unwrap_or_else(|error| panic!("setter for {id} failed: {error}"));
    }
    assert_eq!(reference.current_values(), configured.current_values());
    // Setters ramp through smoothers; reset aligns both to identical state.
    configured.reset();
    let frames = 4096usize;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let time = frame as f32 / sample_rate as f32;
            let left = 0.3 * (2.0 * std::f32::consts::PI * 110.0 * time).sin()
                + 0.2 * (2.0 * std::f32::consts::PI * 4000.0 * time).sin();
            [left, left * 0.7]
        })
        .collect();
    let expected = process_all(&mut reference, sample_rate, 2, &input, 1024);
    let actual = process_all(&mut configured, sample_rate, 2, &input, 1024);
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-7,
        "explicit preset vs setter-built config differ by {worst}"
    );

    // The remaining unsupported legacy controls still fail loudly.
    for params in [
        MultibandCompressorPluginParams {
            program_dependent_release: Some(true),
            ..Default::default()
        },
        MultibandCompressorPluginParams {
            sidechain_external: Some(true),
            ..Default::default()
        },
    ] {
        assert!(
            MultibandCompressorPlugin::try_from_params(2, params, sample_rate).is_err()
        );
    }
    for (id, value) in [
        ("program_dependent_release", ParameterValue::Bool(true)),
        ("sidechain_external", ParameterValue::Bool(true)),
    ] {
        assert!(
            reference
                .set_parameter(ParameterId::from(id), value)
                .is_err()
        );
    }
}

#[test]
fn legacy_parameter_ids_and_defaults_are_preserved() {
    use sotf_plugin_multiband_compressor::params::{
        GLOBAL_PARAMS, PARAMS, default_link_amount, default_mix, default_ratio,
        default_threshold_db,
    };

    // Old global indices 0..19 keep their engine keys in order; the detector
    // controls are appended after them.
    let keys: Vec<&str> = GLOBAL_PARAMS.iter().map(|spec| spec.engine_key).collect();
    assert_eq!(
        &keys[..19],
        [
            "num_bands",
            "crossover_preset",
            "crossover_freq_1",
            "crossover_freq_2",
            "crossover_freq_3",
            "crossover_freq_4",
            "threshold",
            "ratio",
            "attack",
            "release",
            "knee",
            "mix",
            "link_channels",
            "per_band_lookahead_ms",
            "ms_mode",
            "sidechain_tilt_db",
            "link_amount",
            "range_db",
            "hold_ms",
        ]
    );
    assert_eq!(
        &keys[19..],
        [
            "sidechain_hpf_hz",
            "sidechain_hpf_order",
            "detection_mode",
            "sidechain_hpf_enabled",
        ]
    );

    // Core dynamics defaults are untouched by the detector work.
    assert_eq!(default_threshold_db(), -20.0);
    assert_eq!(default_ratio(), 4.0);
    assert_eq!(default_mix(), 1.0);
    assert_eq!(default_link_amount(), 1.0);

    // Old saved state without the new keys deserializes with benign defaults:
    // HPF frequency restores the legacy 80 Hz default but stays inactive
    // (enabled defaults false), Peak detection, unlimited range, no hold.
    let old: MultibandCompressorPluginParams =
        serde_json::from_str(r#"{"num_bands":3,"threshold_db":-12.0,"ratio":2.0}"#).unwrap();
    assert_eq!(old.sidechain_hpf_hz, None);
    assert_eq!(old.sidechain_hpf_order, None);
    assert_eq!(old.sidechain_hpf_enabled, None);
    assert_eq!(old.detection_mode, None);
    let plugin = MultibandCompressorPlugin::try_from_params(2, old, 48_000).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("sidechain_hpf_hz")),
        Some(ParameterValue::Float(80.0))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("sidechain_hpf_enabled")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("detection_mode")),
        Some(ParameterValue::Int(0))
    );
    // Old engine-style JSON carries explicit legacy detector values but no
    // enabled key: accepted, frequency stored, filter inactive.
    let engine_old: MultibandCompressorPluginParams = serde_json::from_str(
        r#"{"num_bands":1,"sidechain_hpf_hz":80.0,"sidechain_hpf_order":"2nd","detection_mode":"Peak"}"#,
    )
    .unwrap();
    assert_eq!(engine_old.sidechain_hpf_enabled, None);
    let engine_plugin =
        MultibandCompressorPlugin::try_from_params(2, engine_old, 48_000).unwrap();
    assert_eq!(
        engine_plugin.get_parameter(&ParameterId::from("sidechain_hpf_hz")),
        Some(ParameterValue::Float(80.0))
    );
    assert_eq!(
        engine_plugin.get_parameter(&ParameterId::from("sidechain_hpf_enabled")),
        Some(ParameterValue::Bool(false))
    );
    // Single-band compat array keeps its historical index layout; the new
    // enabled flag is appended after the original 18 entries.
    assert_eq!(PARAMS[0].engine_key, "threshold");
    assert_eq!(PARAMS[9].engine_key, "sidechain_hpf_hz");
    assert_eq!(PARAMS[17].engine_key, "hold_ms");
    assert_eq!(PARAMS[18].engine_key, "sidechain_hpf_enabled");
    assert_eq!(PARAMS.len(), 19);
}

// ---------------------------------------------------------------------------
// A3: dense automation partition invariance with new stages active
// ---------------------------------------------------------------------------

#[test]
fn dense_automation_with_new_stages_is_partition_invariant() {
    const PARTITION_BOUND: f32 = 2e-5;
    let sample_rate = 48_000u32;
    let render = |chunks: &[usize]| -> Vec<f32> {
        let params = MultibandCompressorPluginParams {
            num_bands: 3,
            threshold_db: -12.0,
            ratio: 2.0,
            attack_ms: 10.0,
            release_ms: 80.0,
            ..Default::default()
        };
        let mut plugin =
            MultibandCompressorPlugin::try_from_params(2, params, sample_rate).unwrap();
        plugin.initialize(sample_rate).unwrap();
        let warmup_frames = 1024usize;
        let mut warmup: Vec<f32> = (0..warmup_frames)
            .flat_map(|frame| {
                let time = frame as f32 / sample_rate as f32;
                let sample = 0.35 * (2.0 * std::f32::consts::PI * 997.0 * time).sin();
                [sample, sample * 0.37]
            })
            .collect();
        plugin
            .process_in_place(&mut warmup, &ProcessContext::new(sample_rate, warmup_frames))
            .unwrap();
        for (id, value) in [
            ("crossover_freq_1", ParameterValue::Float(350.0)),
            ("crossover_freq_2", ParameterValue::Float(3500.0)),
            ("threshold", ParameterValue::Float(-30.0)),
            ("ratio", ParameterValue::Float(12.0)),
            ("attack", ParameterValue::Float(0.5)),
            ("release", ParameterValue::Float(300.0)),
            ("knee", ParameterValue::Float(12.0)),
            ("link_amount", ParameterValue::Float(0.25)),
            ("sidechain_tilt_db", ParameterValue::Float(6.0)),
            ("sidechain_hpf_hz", ParameterValue::Float(120.0)),
            ("sidechain_hpf_order", ParameterValue::Int(1)),
            ("sidechain_hpf_enabled", ParameterValue::Bool(true)),
            ("detection_mode", ParameterValue::Int(1)),
            ("band_0_makeup", ParameterValue::Float(9.0)),
            ("band_1_threshold", ParameterValue::Float(-18.0)),
            ("band_2_ratio", ParameterValue::Float(8.0)),
        ] {
            plugin.set_parameter(ParameterId::from(id), value).unwrap();
        }
        let mut rendered = Vec::new();
        let mut offset = warmup_frames;
        for &frames in chunks {
            let mut block: Vec<f32> = (0..frames)
                .flat_map(|frame| {
                    let time = (offset + frame) as f32 / sample_rate as f32;
                    let sample = 0.35 * (2.0 * std::f32::consts::PI * 997.0 * time).sin();
                    [sample, sample * 0.37]
                })
                .collect();
            plugin
                .process_in_place(&mut block, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            rendered.extend(block);
            offset += frames;
        }
        rendered
    };

    let whole = render(&[4096]);
    let partitioned = render(&[17, 63, 256, 1024, 2736]);
    assert_eq!(whole.len(), partitioned.len());
    assert!(whole.iter().all(|sample| sample.is_finite()));
    assert!(whole.iter().all(|sample| sample.abs() < 2.0));
    let worst = whole
        .iter()
        .zip(&partitioned)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < PARTITION_BOUND,
        "partition-dependent automation with RMS+HPF: {worst}"
    );
}

// ---------------------------------------------------------------------------
// A3: end-of-stream accounting and cold realtime path with new stages
// ---------------------------------------------------------------------------

#[test]
fn eos_drain_accounts_with_new_stages_active() {
    let sample_rate = 48_000u32;
    let mut plugin = one_band_plugin(2, sample_rate, |params| {
        params.threshold_db = -27.0;
        params.ratio = 3.0;
        params.per_band_lookahead_ms = 5.0;
        params.mix = 1.0;
        params.sidechain_hpf_hz = Some(100.0);
        params.sidechain_hpf_enabled = Some(true);
        params.detection_mode = Some("RMS".to_string());
    });
    let delay = plugin.latency_samples();
    assert_eq!(delay, 240);
    assert_eq!(
        plugin.tail_length(),
        TailLength::Finite(delay as u64)
    );
    assert_eq!(
        plugin.drain_call_bound().map(|bound| bound.get()),
        Some(1)
    );

    let input: Vec<f32> = (0..2048 * 2)
        .map(|i| ((i * 7 % 23) as f32 - 11.0) / 64.0)
        .collect();
    let processed = process_all(&mut plugin, sample_rate, 2, &input, 1024);
    assert!(processed.iter().all(|sample| sample.is_finite()));

    // Drain in undersized calls; the total must equal the ring delay.
    let mut drained = Vec::new();
    for _ in 0..16 {
        let mut buffer = vec![0.0f32; 100 * 2];
        let step = plugin
            .drain(&mut buffer, &ProcessContext::new(sample_rate, 0))
            .unwrap();
        drained.extend_from_slice(&buffer[..step.frames * 2]);
        if step.complete {
            break;
        }
    }
    assert_eq!(drained.len() / 2, delay);
    assert!(drained.iter().all(|sample| sample.is_finite()));

    // Post-drain controls stay frozen until reset.
    assert!(
        plugin
            .process_in_place(
                &mut vec![0.0f32; 64 * 2],
                &ProcessContext::new(sample_rate, 64)
            )
            .is_err()
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-10.0))
            .is_err()
    );
    // Identical snapshots remain accepted no-ops.
    let current = plugin.current_values();
    plugin.apply_values(current).unwrap();
    plugin.reset();
    let mut probe = vec![0.1f32; 64 * 2];
    plugin
        .process_in_place(&mut probe, &ProcessContext::new(sample_rate, 64))
        .unwrap();
}

#[test]
fn new_detector_controls_allocate_nothing_on_cold_thread() {
    let mut plugin = one_band_plugin(2, 48_000, |params| {
        params.num_bands = 3;
        params.range_db = 6.0;
        params.hold_ms = 20.0;
    });
    let ids = [
        "threshold",
        "ratio",
        "attack",
        "release",
        "knee",
        "mix",
        "link_amount",
        "sidechain_tilt_db",
        "sidechain_hpf_hz",
        "sidechain_hpf_order",
        "sidechain_hpf_enabled",
        "detection_mode",
        "band_1_threshold",
    ]
    .map(ParameterId::from);
    let values = [
        ParameterValue::Float(-31.0),
        ParameterValue::Float(7.0),
        ParameterValue::Float(3.0),
        ParameterValue::Float(87.0),
        ParameterValue::Float(4.0),
        ParameterValue::Float(0.63),
        ParameterValue::Float(0.4),
        ParameterValue::Float(2.0),
        ParameterValue::Float(140.0),
        ParameterValue::Int(1),
        ParameterValue::Bool(true),
        ParameterValue::Int(1),
        ParameterValue::Float(-22.0),
    ];
    let detection_id = ParameterId::from("detection_mode");
    let enabled_id = ParameterId::from("sidechain_hpf_enabled");
    std::thread::spawn(move || {
        let mut buffer = [0.2f32; 514];
        OPERATIONS.set(0);
        TRACKING.set(true);
        for repeat in 0..32 {
            for (id, value) in ids.iter().zip(values.iter()) {
                ParametricInPlacePlugin::parametric_set_parameter(
                    &mut plugin,
                    id.clone(),
                    value.clone(),
                )
                .unwrap();
            }
            // Alternate detector modes so both switch directions are measured.
            ParametricInPlacePlugin::parametric_set_parameter(
                &mut plugin,
                detection_id.clone(),
                ParameterValue::Int((repeat % 2) as i32),
            )
            .unwrap();
            // Alternate the HPF enable flag so both toggle directions run on
            // the cold path (in-place coefficient refresh, no allocation).
            ParametricInPlacePlugin::parametric_set_parameter(
                &mut plugin,
                enabled_id.clone(),
                ParameterValue::Bool(repeat % 2 == 0),
            )
            .unwrap();
            let frames = [1, 127, 257, 3][repeat % 4];
            buffer.fill(0.2);
            plugin
                .process_in_place(
                    &mut buffer[..frames * 2],
                    &ProcessContext::new(48_000, frames),
                )
                .unwrap();
        }
        plugin.reset();
        TRACKING.set(false);
        assert_eq!(OPERATIONS.get(), 0, "cold callback heap operations");
    })
    .join()
    .unwrap();
}
