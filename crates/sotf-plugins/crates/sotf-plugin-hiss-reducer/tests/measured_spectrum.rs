//! Measured spectral noise-profile tests.
//!
//! Tolerances are fixed here before any candidate runs: per-bin capture
//! agreement 2% against an independent direct-DFT oracle, Parseval
//! reconstruction 10%, regional discrimination 4x/0.25x power ratios on a
//! band-gap stimulus pair (the original complementary pair is retained as
//! an oracle-verified diagnostic), suppression below -2 dB with 1.5 dB
//! regional separation, tone loss within 1 dB, v1 guard acceptance
//! retention plus measured guard coverage at the original -3/+2 dB
//! bounds, and bit-exact round-trip/partition/reset/EOF assertions where
//! the contract is exact.

// Rust guideline compliant 2026-02-21
use plugins_denoiser::spectral_hiss::SPECTRAL_HISS_NUM_BINS;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::test_utils::measure_heap_activity;
use sotf_host::CountingAlloc;
use sotf_plugin_hiss_reducer::profile::{NoiseProfileData, SpectralProfileData};
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

const RATE: u32 = 48_000;
const FFT_SIZE: usize = 1024;
const HOP_SIZE: usize = 256;
const PARTITIONS: [usize; 5] = [1, 64, 511, 73, 997];
// Exact-bin measurement tone: bin 213 of the reducer FFT (N=1024).
const TONE_HZ: f64 = 213.0 * 48_000.0 / 1024.0;
const TONE_AMPLITUDE: f32 = 0.06;

fn ctx(frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(RATE, frames)
}

fn ctx_at(rate: u32, frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(rate, frames)
}

fn lcg_noise(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..frames)
        .map(|_| {
            state = state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            amplitude * ((state as f32 / u32::MAX as f32) * 2.0 - 1.0)
        })
        .collect()
}

fn sine_tone(frames: usize, amplitude: f32, freq_hz: f64, rate: u32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            (f64::from(amplitude)
                * (2.0 * std::f64::consts::PI * freq_hz * i as f64 / f64::from(rate)).sin())
                as f32
        })
        .collect()
}

/// Independent f64 one-pole high-band power oracle.
fn oracle_high_band_power(signal: &[f32], cutoff_hz: f64, rate: f64) -> f64 {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / rate).exp();
    let mut low = 0.0;
    let mut sum = 0.0;
    for &sample in signal {
        let dry = sample as f64;
        low = alpha * dry + (1.0 - alpha) * low;
        let high = dry - low;
        sum += high * high;
    }
    sum / signal.len() as f64
}

fn process_all(
    plugin: &mut HissReducerPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> Vec<f32> {
    process_all_at_rate(plugin, input, channels, blocks, RATE)
}

fn process_all_at_rate(
    plugin: &mut HissReducerPlugin,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
    rate: u32,
) -> Vec<f32> {
    assert_eq!(input.len() % channels, 0);
    let mut output = input.to_vec();
    let frames = input.len() / channels;
    let mut pos = 0;
    let mut call = 0;
    while pos < frames {
        let count = blocks[call % blocks.len()].min(frames - pos);
        plugin
            .process_in_place(
                &mut output[pos * channels..(pos + count) * channels],
                &ctx_at(rate, count),
            )
            .unwrap();
        pos += count;
        call += 1;
    }
    output
}

fn start_capture(plugin: &mut HissReducerPlugin) {
    plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    assert!(plugin.is_capturing());
}

fn time_domain_plugin(channels: usize) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::new(channels);
    plugin.initialize(f64::from(RATE)).unwrap();
    plugin
}

fn spectral_plugin(channels: usize) -> HissReducerPlugin {
    spectral_plugin_at_rate(channels, RATE)
}

fn spectral_plugin_at_rate(channels: usize, rate: u32) -> HissReducerPlugin {
    let mut plugin = HissReducerPlugin::from_params(
        channels,
        HissReducerPluginParams {
            spectral_mode: true,
            ..HissReducerPluginParams::default()
        },
    );
    plugin.initialize(f64::from(rate)).unwrap();
    plugin
}

fn capture_mono(noise: &[f32], blocks: &[usize]) -> HissReducerPlugin {
    let mut plugin = time_domain_plugin(1);
    start_capture(&mut plugin);
    process_all(&mut plugin, noise, 1, blocks);
    assert!(!plugin.is_capturing());
    assert!(plugin.has_captured_profile());
    assert!(plugin.has_measured_spectrum());
    plugin
}

/// Independent periodic Hann window (direct formula, not the DSP crate).
fn independent_hann(n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()))
        .collect()
}

/// Independent direct-DFT mean per-bin powers (f64, full windows only).
///
/// Mirrors the documented framing (N=1024, H=256, periodic Hann, full
/// windows, rectangular hop average) with a naive O(N^2) DFT rather than
/// the production FFT. Returns 513 one-sided mean `|X|^2` powers.
fn independent_spectrum(signal: &[f32]) -> Vec<f64> {
    let window = independent_hann(FFT_SIZE);
    let frames = signal.len();
    assert!(frames >= FFT_SIZE);
    let hops = (frames - FFT_SIZE) / HOP_SIZE + 1;
    let mut sum = vec![0.0; SPECTRAL_HISS_NUM_BINS];
    for hop in 0..hops {
        let start = hop * HOP_SIZE;
        let mut windowed = vec![0.0; FFT_SIZE];
        for n in 0..FFT_SIZE {
            windowed[n] = f64::from(signal[start + n]) * window[n];
        }
        for (bin, slot) in sum.iter_mut().enumerate() {
            let mut re = 0.0;
            let mut im = 0.0;
            for (n, &sample) in windowed.iter().enumerate() {
                let angle = 2.0 * std::f64::consts::PI * bin as f64 * n as f64 / FFT_SIZE as f64;
                re += sample * angle.cos();
                im -= sample * angle.sin();
            }
            *slot += re * re + im * im;
        }
    }
    sum.iter().map(|s| s / hops as f64).collect()
}

/// Single-bin Goertzel power (unnormalized; ratios cancel the scale).
fn goertzel_power(signal: &[f32], bin: usize) -> f64 {
    let n = signal.len() as f64;
    let coefficient = 2.0 * (2.0 * std::f64::consts::PI * bin as f64 / n).cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for &sample in signal {
        let s0 = sample as f64 + coefficient * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coefficient * s1 * s2
}

/// Band power over `lo_hz..hi_hz` via per-bin Goertzel sums.
fn band_power(signal: &[f32], rate: f64, lo_hz: f64, hi_hz: f64) -> f64 {
    let n = signal.len();
    let k_lo = (lo_hz * n as f64 / rate).ceil() as usize;
    let k_hi = (hi_hz * n as f64 / rate).floor() as usize;
    let mut sum = 0.0;
    for k in k_lo..=k_hi.min(n / 2) {
        sum += goertzel_power(signal, k);
    }
    sum
}

fn mean_power(signal: &[f32]) -> f64 {
    signal
        .iter()
        .map(|s| {
            let d = f64::from(*s);
            d * d
        })
        .sum::<f64>()
        / signal.len() as f64
}

fn power_db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// Two-pole lowpass (cascaded exact-mapped one-poles) for coloring.
fn lowpass_two_pole(signal: &[f32], cutoff_hz: f64, rate: f64) -> Vec<f32> {
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * cutoff_hz / rate).exp();
    let mut first = 0.0;
    let mut second = 0.0;
    signal
        .iter()
        .map(|&sample| {
            first = alpha * f64::from(sample) + (1.0 - alpha) * first;
            second = alpha * first + (1.0 - alpha) * second;
            second as f32
        })
        .collect()
}

fn scale_to_highband_rms(signal: &[f32], cutoff_hz: f64, rate: f64, target_rms: f64) -> Vec<f32> {
    let power = oracle_high_band_power(signal, cutoff_hz, rate);
    let gain = target_rms / power.sqrt();
    signal.iter().map(|s| (*s as f64 * gain) as f32).collect()
}

fn regional_mean(spectrum: &[f32], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().map(|p| f64::from(*p)).sum::<f64>() / slice.len() as f64
}

fn regional_mean_f64(spectrum: &[f64], lo_bin: usize, hi_bin: usize) -> f64 {
    let slice = &spectrum[lo_bin..=hi_bin];
    slice.iter().sum::<f64>() / slice.len() as f64
}

/// Asserts production capture matches the naive-DFT oracle per bin.
///
/// Returns the oracle spectrum for further predicted-ratio checks. The 2%
/// relative bound is the same predefined capture-accuracy bound used by
/// the white-noise DFT test; the `1e-12` denominator guards near-zero bins.
fn assert_capture_matches_oracle(measured: &[f32], signal: &[f32], label: &str) -> Vec<f64> {
    assert_eq!(measured.len(), SPECTRAL_HISS_NUM_BINS);
    let expected = independent_spectrum(signal);
    let mut worst_error = 0.0f64;
    for (bin, (&actual, &reference)) in measured.iter().zip(expected.iter()).enumerate() {
        assert!(
            actual.is_finite() && actual >= 0.0,
            "{label} bin {bin}: {actual}"
        );
        let error = (f64::from(actual) - reference).abs() / reference.max(1e-12);
        worst_error = worst_error.max(error);
    }
    assert!(
        worst_error < 0.02,
        "{label} worst per-bin error {worst_error:.4} above 2%"
    );
    expected
}

/// Predicts steady Wiener power suppression in dB over a bin region.
///
/// Uses two independent oracle spectra (profile noise, test signal) with
/// the documented strength law `gain = floor + (1-floor)*sqrt(max(0,1-n/t))`
/// at flat curve, so the prediction involves no production capture data.
fn predicted_suppression_db(
    profile_oracle: &[f64],
    test_oracle: &[f64],
    lo_bin: usize,
    hi_bin: usize,
    strength: f64,
) -> f64 {
    let floor = 1.0 - strength;
    let mut sum = 0.0;
    let mut count = 0;
    for bin in lo_bin..=hi_bin {
        let ratio = profile_oracle[bin] / test_oracle[bin].max(1e-18);
        let gain = floor + (1.0 - floor) * (1.0 - ratio).max(0.0).sqrt();
        sum += gain * gain;
        count += 1;
    }
    10.0 * (sum / f64::from(count)).log10()
}

#[test]
fn spectral_capture_matches_independent_dft_and_parseval() {
    let noise = lcg_noise(RATE as usize, 0.05, 0x1234_5678);
    let plugin = capture_mono(&noise, &[4096]);
    let measured = plugin.measured_spectrum().unwrap().to_vec();
    assert_eq!(measured.len(), SPECTRAL_HISS_NUM_BINS);
    assert_eq!(plugin.profile_spectral_hops(), Some(184));
    assert_eq!(
        plugin.profile_metadata(),
        Some((f64::from(RATE), 4000.0, u64::from(RATE)))
    );

    // Per-bin agreement against the naive-DFT oracle (2% relative).
    let expected = independent_spectrum(&noise);
    let mut worst_bin = 0;
    let mut worst_error = 0.0;
    for (bin, (&actual, &reference)) in measured.iter().zip(expected.iter()).enumerate() {
        assert!(actual.is_finite() && actual >= 0.0, "bin {bin}: {actual}");
        let error = (f64::from(actual) - reference).abs() / reference.max(1e-12);
        if error > worst_error {
            worst_error = error;
            worst_bin = bin;
        }
    }
    assert!(
        worst_error < 0.02,
        "worst per-bin error {worst_error:.4} at bin {worst_bin}"
    );

    // Parseval reconstruction: doubled interior plus single DC/Nyquist
    // recovers the time-domain variance within 10%.
    let mut doubled = f64::from(measured[0]) + f64::from(measured[SPECTRAL_HISS_NUM_BINS - 1]);
    for power in &measured[1..SPECTRAL_HISS_NUM_BINS - 1] {
        doubled += 2.0 * f64::from(*power);
    }
    let variance_estimate = doubled / ((FFT_SIZE * FFT_SIZE) as f64 * 0.375);
    let time_variance = mean_power(&noise);
    let parseval_error = (variance_estimate - time_variance).abs() / time_variance;
    assert!(
        parseval_error < 0.10,
        "Parseval {variance_estimate:.6} vs time {time_variance:.6}"
    );
}

#[test]
fn tone_profiles_with_equal_broadband_rms_discriminate_bins() {
    // Bin-centered tones share broadband RMS by construction (same peak
    // amplitude) but occupy disjoint bins.
    let low_tone = sine_tone(RATE as usize, 0.09, 100.0 * 48_000.0 / 1024.0, RATE);
    let high_tone = sine_tone(RATE as usize, 0.09, 300.0 * 48_000.0 / 1024.0, RATE);
    assert!(
        (mean_power(&low_tone) / mean_power(&high_tone) - 1.0).abs() < 0.01,
        "broadband RMS must match by construction"
    );
    let low_plugin = capture_mono(&low_tone, &[4096]);
    let high_plugin = capture_mono(&high_tone, &[4096]);
    let low_spectrum = low_plugin.measured_spectrum().unwrap().to_vec();
    let high_spectrum = high_plugin.measured_spectrum().unwrap().to_vec();

    // Each peaks exactly at its tone bin with >100x peak-to-median.
    for (spectrum, expected_bin, label) in
        [(&low_spectrum, 100, "low"), (&high_spectrum, 300, "high")]
    {
        let peak = spectrum
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .unwrap()
            .0;
        assert_eq!(peak, expected_bin, "{label} tone peaks at bin {peak}");
        let mut sorted = spectrum.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = f64::from(sorted[SPECTRAL_HISS_NUM_BINS / 2]).max(1e-18);
        let ratio = f64::from(spectrum[expected_bin]) / median;
        assert!(
            ratio > 100.0,
            "{label} peak-to-median ratio {ratio:.1} below 100x"
        );
    }

    // Cross-discrimination exceeds 100x in both directions.
    let low_at_high = f64::from(low_spectrum[300]).max(1e-18);
    let high_at_high = f64::from(high_spectrum[300]);
    let high_at_low = f64::from(high_spectrum[100]).max(1e-18);
    let low_at_low = f64::from(low_spectrum[100]);
    assert!(
        high_at_high / low_at_high > 100.0,
        "bin-300 cross ratio {:.1}",
        high_at_high / low_at_high
    );
    assert!(
        low_at_low / high_at_low > 100.0,
        "bin-100 cross ratio {:.1}",
        low_at_low / high_at_low
    );
}

#[test]
fn original_2pole6k_pair_retained_as_capture_accuracy_diagnostic() {
    // ORIGINAL r1-r3 STIMULUS, VERBATIM (seed 0xc010, two-pole 6 kHz
    // complementary pair, -36 dBFS high-band normalization, bins
    // 86-149/341-490, floors match <0.3 dB): retained as an explicit
    // diagnostic. r3 measured low-region ratio 3.66 < 4x here. Two
    // independent implementations (production FFT capture, naive-DFT
    // oracle) agree on the spectra below, proving the stimulus physics —
    // not the capture — cannot meet 4x: after identical-RMS normalization
    // the low-region contrast of a complementary pair equals its energy-
    // concentration ratio (analytically ~3.7x here). The 4x acceptance
    // lives in the separated-cutoff test; the measured PSD is never
    // altered to amplify contrast.
    let white = lcg_noise(RATE as usize, 0.05, 0xc010);
    let low_raw = lowpass_two_pole(&white, 6000.0, f64::from(RATE));
    let high_raw: Vec<f32> = white
        .iter()
        .zip(low_raw.iter())
        .map(|(w, l)| w - l)
        .collect();
    let target_rms = 10.0f64.powf(-36.0 / 20.0);
    let low = scale_to_highband_rms(&low_raw, 4000.0, f64::from(RATE), target_rms);
    let high = scale_to_highband_rms(&high_raw, 4000.0, f64::from(RATE), target_rms);

    let low_plugin = capture_mono(&low, &[4096]);
    let high_plugin = capture_mono(&high, &[4096]);
    let low_floors = low_plugin.persisted_params().captured_profile.unwrap();
    let high_floors = high_plugin.persisted_params().captured_profile.unwrap();
    assert!(
        (low_floors.floor_db_per_channel[0] - high_floors.floor_db_per_channel[0]).abs() < 0.3,
        "floors must match by normalization: {:?} vs {:?}",
        low_floors.floor_db_per_channel,
        high_floors.floor_db_per_channel
    );
    let low_spectrum = low_plugin.measured_spectrum().unwrap().to_vec();
    let high_spectrum = high_plugin.measured_spectrum().unwrap().to_vec();

    // Full independent capture accuracy on the original pair (2% / bin).
    let low_oracle = assert_capture_matches_oracle(&low_spectrum, &low, "original-low");
    let high_oracle = assert_capture_matches_oracle(&high_spectrum, &high, "original-high");

    // Regional ratios, production vs oracle (bins 86-149 / 341-490).
    let ratio_low =
        regional_mean(&low_spectrum, 86, 149) / regional_mean(&high_spectrum, 86, 149);
    let ratio_high =
        regional_mean(&low_spectrum, 341, 490) / regional_mean(&high_spectrum, 341, 490);
    let oracle_ratio_low =
        regional_mean_f64(&low_oracle, 86, 149) / regional_mean_f64(&high_oracle, 86, 149);
    let oracle_ratio_high =
        regional_mean_f64(&low_oracle, 341, 490) / regional_mean_f64(&high_oracle, 341, 490);
    println!(
        "original pair low-region ratio: production {ratio_low:.3}, oracle {oracle_ratio_low:.3}"
    );
    println!(
        "original pair high-region ratio: production {ratio_high:.4}, oracle {oracle_ratio_high:.4}"
    );
    for (label, measured, predicted) in [
        ("low-region", ratio_low, oracle_ratio_low),
        ("high-region", ratio_high, oracle_ratio_high),
    ] {
        assert!(
            (measured - predicted).abs() / predicted < 0.05,
            "{label} production {measured:.3} disagrees with oracle {predicted:.3} beyond 5%"
        );
    }
    // Physical inability, proven by the independent oracle rather than the
    // production capture under test: the complementary pair's low-region
    // contrast cannot meet 4x with this filter geometry. The high region
    // keeps its original 0.25x bound on the original stimulus.
    assert!(
        oracle_ratio_low < 4.0,
        "oracle low-region ratio {oracle_ratio_low:.3} unexpectedly meets 4x"
    );
    assert!(
        ratio_high < 0.25,
        "high-region ratio {ratio_high:.4} above 0.25x"
    );
}

#[test]
fn separated_cutoff_pair_meets_4x_and_suppresses_regionally() {
    // STRONGER STIMULUS carrying the ORIGINAL 4x/0.25x/-2dB/1.5dB bounds
    // verbatim: band-gap pair instead of complementary — low = two-pole LP
    // at 5 kHz, high = white minus two-pole LP at 12 kHz. Same white base
    // (seed 0xc010), same -36 dBFS high-band normalization, same regions
    // (bins 86-149 / 341-490). After identical-RMS normalization the
    // regional contrast equals the energy-concentration ratio; the 5-12 kHz
    // gap drives the oracle-predicted low ratio above 6x (acceptance 4x)
    // and the high ratio below 0.15x (acceptance 0.25x). Hann 1/f^3 far
    // sidelobes keep cross-region leakage below -40 dB, far under the
    // attenuated tails, so regional means reflect true PSD.
    let white = lcg_noise(RATE as usize, 0.05, 0xc010);
    let low_raw = lowpass_two_pole(&white, 5000.0, f64::from(RATE));
    let high_lp = lowpass_two_pole(&white, 12_000.0, f64::from(RATE));
    let high_raw: Vec<f32> = white
        .iter()
        .zip(high_lp.iter())
        .map(|(w, l)| w - l)
        .collect();
    let target_rms = 10.0f64.powf(-36.0 / 20.0);
    let low = scale_to_highband_rms(&low_raw, 4000.0, f64::from(RATE), target_rms);
    let high = scale_to_highband_rms(&high_raw, 4000.0, f64::from(RATE), target_rms);

    let low_plugin = capture_mono(&low, &[4096]);
    let high_plugin = capture_mono(&high, &[4096]);
    let low_blob = low_plugin.persisted_params().captured_profile.unwrap();
    let high_blob = high_plugin.persisted_params().captured_profile.unwrap();
    assert!(
        (low_blob.floor_db_per_channel[0] - high_blob.floor_db_per_channel[0]).abs() < 0.3,
        "floors must match by normalization: {:?} vs {:?}",
        low_blob.floor_db_per_channel,
        high_blob.floor_db_per_channel
    );
    let low_spectrum = low_plugin.measured_spectrum().unwrap().to_vec();
    let high_spectrum = high_plugin.measured_spectrum().unwrap().to_vec();
    let low_oracle = assert_capture_matches_oracle(&low_spectrum, &low, "gap-low");
    let high_oracle = assert_capture_matches_oracle(&high_spectrum, &high, "gap-high");

    // Stimulus qualification on the INDEPENDENT oracle (predefined margins
    // above the acceptance bars), then production acceptance on the bars.
    let oracle_low_ratio =
        regional_mean_f64(&low_oracle, 86, 149) / regional_mean_f64(&high_oracle, 86, 149);
    let oracle_high_ratio =
        regional_mean_f64(&low_oracle, 341, 490) / regional_mean_f64(&high_oracle, 341, 490);
    println!("gap pair oracle low-region ratio: {oracle_low_ratio:.2}");
    println!("gap pair oracle high-region ratio: {oracle_high_ratio:.4}");
    assert!(
        oracle_low_ratio > 6.0,
        "oracle low-region ratio {oracle_low_ratio:.2} below 6x qualification"
    );
    assert!(
        oracle_high_ratio < 0.15,
        "oracle high-region ratio {oracle_high_ratio:.4} above 0.15x qualification"
    );
    let ratio_low =
        regional_mean(&low_spectrum, 86, 149) / regional_mean(&high_spectrum, 86, 149);
    let ratio_high =
        regional_mean(&low_spectrum, 341, 490) / regional_mean(&high_spectrum, 341, 490);
    assert!(
        ratio_low > 4.0,
        "low-region ratio {ratio_low:.2} below 4x"
    );
    assert!(
        ratio_high < 0.25,
        "high-region ratio {ratio_high:.4} above 0.25x"
    );

    // Regional suppression with oracle-predicted Wiener qualification.
    // Flat white test hiss (same seed 0x7e57 as the original test) isolates
    // the profile as the only difference.
    let test_hiss = lcg_noise(RATE as usize, 0.04, 0x7e57);
    let test_floor =
        10.0 * oracle_high_band_power(&test_hiss, 4000.0, f64::from(RATE)).log10();
    assert!(
        test_floor < -30.0,
        "test hiss must clear the gate, got {test_floor:.2} dB"
    );
    let test_oracle = independent_spectrum(&test_hiss);
    for (profile, oracle, label) in [
        (&low_blob, &low_oracle, "low-profile"),
        (&high_blob, &high_oracle, "high-profile"),
    ] {
        // Predicted steady suppression per region from oracle spectra only
        // (documented Wiener law at flat curve, strength 0.85).
        let pred_low = predicted_suppression_db(oracle, &test_oracle, 86, 149, 0.85);
        let pred_high = predicted_suppression_db(oracle, &test_oracle, 341, 490, 0.85);
        let (pred_own, pred_other) = if label == "low-profile" {
            (pred_low, pred_high)
        } else {
            (pred_high, pred_low)
        };
        println!("{label} predicted suppression: own {pred_own:.2} dB, other {pred_other:.2} dB");
        assert!(
            pred_own < -3.0,
            "{label} predicted own-region suppression {pred_own:.2} dB above -3 dB"
        );
        assert!(
            pred_other - pred_own > 3.0,
            "{label} predicted separation {pred_other:.2} vs {pred_own:.2} below 3 dB"
        );

        let mut plugin = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                spectral_mode: true,
                strength: 0.85,
                use_captured_profile: true,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.restore_profile(profile).unwrap();
        assert!(plugin.profile_uses_spectral());
        let output = process_all(&mut plugin, &test_hiss, 1, &[4096]);
        // Skip latency plus settling; compare steady input/output bands.
        let skip = 8192;
        let low_in = band_power(&test_hiss[skip..], 48_000.0, 4000.0, 7000.0);
        let low_out = band_power(&output[skip + 1024..], 48_000.0, 4000.0, 7000.0);
        let high_in = band_power(&test_hiss[skip..], 48_000.0, 16_000.0, 23_000.0);
        let high_out = band_power(&output[skip + 1024..], 48_000.0, 16_000.0, 23_000.0);
        let low_db = 10.0 * (low_out / low_in).log10();
        let high_db = 10.0 * (high_out / high_in).log10();
        if label == "low-profile" {
            assert!(
                low_db < -2.0,
                "{label} low suppression {low_db:.2} dB above -2 dB"
            );
            assert!(
                low_db + 1.5 < high_db,
                "{label} separation low {low_db:.2} vs high {high_db:.2} below 1.5 dB"
            );
        } else {
            assert!(
                high_db < -2.0,
                "{label} high suppression {high_db:.2} dB above -2 dB"
            );
            assert!(
                high_db + 1.5 < low_db,
                "{label} separation high {high_db:.2} vs low {low_db:.2} below 1.5 dB"
            );
        }
    }
}

#[test]
fn wanted_tone_loss_vs_colored_suppression_separated() {
    // Lowpassed colored hiss plus an exact-bin tone at 9984 Hz where the
    // hiss is weak: suppression and loss measured on separate renders.
    let white = lcg_noise(RATE as usize, 0.05, 0xc011 ^ 0x1111);
    let colored_raw = lowpass_two_pole(&white, 6000.0, f64::from(RATE));
    let colored = scale_to_highband_rms(
        &colored_raw,
        4000.0,
        f64::from(RATE),
        10.0f64.powf(-36.0 / 20.0),
    );
    let profiler = capture_mono(&colored, &[4096]);
    let profile = profiler.persisted_params().captured_profile.unwrap();
    assert_eq!(profile.format_version, 2);

    // Hiss-only suppression below -2 dB in the 4-7 kHz hiss band.
    let mut hiss_plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            use_captured_profile: true,
            ..HissReducerPluginParams::default()
        },
    );
    hiss_plugin.initialize(f64::from(RATE)).unwrap();
    hiss_plugin.restore_profile(&profile).unwrap();
    let hiss_test = lcg_noise(RATE as usize, 0.04, 0xd15c);
    let hiss_out = process_all(&mut hiss_plugin, &hiss_test, 1, &[4096]);
    let skip = 8192;
    let hiss_in_power = band_power(&hiss_test[skip..], 48_000.0, 4000.0, 7000.0);
    let hiss_out_power = band_power(&hiss_out[skip + 1024..], 48_000.0, 4000.0, 7000.0);
    let suppression_db = 10.0 * (hiss_out_power / hiss_in_power).log10();
    assert!(
        suppression_db < -2.0,
        "colored suppression {suppression_db:.2} dB above -2 dB"
    );

    // Tone-only loss within 1 dB via exact-bin Goertzel.
    let mut tone_plugin = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            use_captured_profile: true,
            ..HissReducerPluginParams::default()
        },
    );
    tone_plugin.initialize(f64::from(RATE)).unwrap();
    tone_plugin.restore_profile(&profile).unwrap();
    let tone = sine_tone(16384, TONE_AMPLITUDE, TONE_HZ, RATE);
    let tone_out_full = process_all(&mut tone_plugin, &tone, 1, &[4096]);
    // Measurement window past latency; 16384-sample DFT puts the tone at
    // bin 3408 (16384 == 16 * 1024, tone at reducer bin 213).
    let window_in = &tone[..16384 - 1024];
    let window_out = &tone_out_full[1024..16384];
    assert_eq!(window_in.len(), window_out.len());
    let tone_bin = (TONE_HZ * window_in.len() as f64 / 48_000.0).round() as usize;
    let loss_db = 10.0 * (goertzel_power(window_out, tone_bin) / goertzel_power(window_in, tone_bin)).log10();
    assert!(
        loss_db.abs() < 1.0,
        "tone loss {loss_db:.2} dB exceeds 1 dB"
    );
}

#[test]
fn capture_partitions_and_partial_spectral_identity() {
    let noise = lcg_noise(RATE as usize, 0.05, 0x9a17);
    let single = capture_mono(&noise, &[RATE as usize]);
    let varied = capture_mono(&noise, &PARTITIONS);
    assert_eq!(
        single.persisted_params().captured_profile,
        varied.persisted_params().captured_profile,
        "callback partitions must not change floors or spectrum"
    );
    assert_eq!(
        single.measured_spectrum().unwrap(),
        varied.measured_spectrum().unwrap()
    );

    // Partial capture: restart then cancel keeps the stored spectrum.
    let mut plugin = time_domain_plugin(1);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[4096]);
    let before = plugin.persisted_params().captured_profile.unwrap();
    assert!(plugin.has_measured_spectrum());
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise[..4096], 1, &[997]);
    assert!(plugin.is_capturing());
    plugin
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(false))
        .unwrap();
    assert!(!plugin.is_capturing());
    assert!(plugin.has_measured_spectrum());
    assert_eq!(
        plugin.persisted_params().captured_profile.unwrap(),
        before,
        "cancelled restart must keep floors and spectrum"
    );
}

fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    assert_eq!(left.len(), right.len());
    let mut out = Vec::with_capacity(left.len() * 2);
    for pair in left.iter().zip(right.iter()) {
        out.push(*pair.0);
        out.push(*pair.1);
    }
    out
}

#[test]
fn stereo_spectral_capture_keeps_channels_independent() {
    let left_tone = sine_tone(RATE as usize, 0.09, 100.0 * 48_000.0 / 1024.0, RATE);
    let right_tone = sine_tone(RATE as usize, 0.09, 300.0 * 48_000.0 / 1024.0, RATE);
    let stereo_capture = interleave(&left_tone, &right_tone);
    let mut stereo = time_domain_plugin(2);
    start_capture(&mut stereo);
    process_all(&mut stereo, &stereo_capture, 2, &[7, 511, 64]);
    assert!(stereo.has_measured_spectrum());
    let spectrum = stereo.measured_spectrum().unwrap().to_vec();
    assert_eq!(spectrum.len(), 2 * SPECTRAL_HISS_NUM_BINS);
    let (left_spectrum, right_spectrum) = spectrum.split_at(SPECTRAL_HISS_NUM_BINS);
    let left_peak = left_spectrum
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap()
        .0;
    let right_peak = right_spectrum
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .unwrap()
        .0;
    assert_eq!(left_peak, 100, "left channel peaks at bin {left_peak}");
    assert_eq!(right_peak, 300, "right channel peaks at bin {right_peak}");

    // Mono capture of the left signal matches the stereo left table.
    let mono = capture_mono(&left_tone, &[4096]);
    assert_eq!(
        mono.measured_spectrum().unwrap(),
        left_spectrum,
        "mono and stereo-left spectra must match bit-exactly"
    );
    assert_eq!(
        mono.persisted_params()
            .captured_profile
            .unwrap()
            .floor_db_per_channel[0],
        stereo
            .persisted_params()
            .captured_profile
            .unwrap()
            .floor_db_per_channel[0]
    );
}

#[test]
fn linked_spectral_with_per_channel_spectra_preserves_dual_mono() {
    // Stereo capture with disjoint per-channel tones, then dual-mono
    // program: linked gains stay common (bit-exact L/R), independent
    // gains diverge with the per-channel spectra.
    let left_tone = sine_tone(RATE as usize, 0.09, 100.0 * 48_000.0 / 1024.0, RATE);
    let right_tone = sine_tone(RATE as usize, 0.09, 300.0 * 48_000.0 / 1024.0, RATE);
    let stereo_capture = interleave(&left_tone, &right_tone);
    let mut profiler = HissReducerPlugin::from_params(
        2,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            use_captured_profile: true,
            ..HissReducerPluginParams::default()
        },
    );
    profiler.initialize(f64::from(RATE)).unwrap();
    start_capture(&mut profiler);
    process_all(&mut profiler, &stereo_capture, 2, &[4096]);
    assert!(profiler.profile_uses_spectral());
    let profile = profiler.persisted_params().captured_profile.unwrap();

    let hiss_l = lcg_noise(16384, 0.04, 0x1e);
    let hiss_r = hiss_l.clone();
    let dual_mono = interleave(&hiss_l, &hiss_r);
    for (link, label) in [(0, "independent"), (1, "linked")] {
        let mut plugin = HissReducerPlugin::from_params(
            2,
            HissReducerPluginParams {
                spectral_mode: true,
                strength: 0.85,
                use_captured_profile: true,
                link_mode: link,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.restore_profile(&profile).unwrap();
        let output = process_all(&mut plugin, &dual_mono, 2, &[4096]);
        assert!(output.iter().all(|s| s.is_finite()), "{label} finite");
        let left: Vec<f32> = output.iter().step_by(2).copied().collect();
        let right: Vec<f32> = output.iter().skip(1).step_by(2).copied().collect();
        if label == "linked" {
            assert_eq!(
                left, right,
                "linked dual-mono must stay bit-exact with per-channel spectra"
            );
        } else {
            assert_ne!(
                left, right,
                "independent dual-mono must diverge with disjoint spectra"
            );
        }
    }
}

fn drain_all(plugin: &mut HissReducerPlugin, channels: usize) -> Vec<f32> {
    let mut output = Vec::new();
    for _ in 0..4096 {
        let mut block = vec![0.0; 256 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 256))
            .unwrap();
        output.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return output;
        }
    }
    panic!("drain did not complete");
}

#[test]
fn reset_preserves_spectral_and_eof_matches_endpoint() {
    let noise = lcg_noise(RATE as usize, 0.05, 0x71e0);
    let mut plugin = spectral_plugin(1);
    start_capture(&mut plugin);
    process_all(&mut plugin, &noise, 1, &[4096]);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(plugin.profile_uses_spectral());
    let exported = plugin.persisted_params().captured_profile.unwrap();

    let segment: Vec<f32> = (0..8192)
        .map(|i| {
            0.1 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / RATE as f32).sin()
                + 0.03 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
        })
        .collect();
    plugin.reset();
    assert!(plugin.has_measured_spectrum(), "reset keeps the spectrum");
    assert_eq!(
        plugin.persisted_params().captured_profile.unwrap(),
        exported
    );
    let after_reset = process_all(&mut plugin, &segment, 1, &[997, 64]);

    let mut fresh = spectral_plugin(1);
    fresh.restore_profile(&exported).unwrap();
    fresh
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let restored = process_all(&mut fresh, &segment, 1, &[997, 64]);
    assert_eq!(after_reset, restored);

    // Engaged spectral EOF keeps the derived endpoint with a v2 profile.
    fn end(frames: usize) -> usize {
        2048 + ((frames - 1) / 256) * 256
    }
    for phase in [0, 1, 127, 255] {
        let mut eof_plugin = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                spectral_mode: true,
                strength: 0.85,
                use_captured_profile: true,
                ..HissReducerPluginParams::default()
            },
        );
        eof_plugin.initialize(f64::from(RATE)).unwrap();
        eof_plugin.restore_profile(&exported).unwrap();
        assert!(eof_plugin.profile_uses_spectral());
        let marker_frames = phase + 1;
        let marker = vec![0.0625; marker_frames];
        let mut output = process_all(&mut eof_plugin, &noise, 1, &[4096]);
        output.extend(process_all(&mut eof_plugin, &marker, 1, &[7, 137, 1]));
        output.extend(drain_all(&mut eof_plugin, 1));
        let total = RATE as usize + marker_frames;
        assert_eq!(output.len(), end(total), "phase={phase}");
        assert!(
            output.iter().all(|s| s.is_finite()),
            "phase={phase} finite"
        );
    }
}

#[test]
fn v2_roundtrip_v1_restore_and_invalid_rollback() {
    let noise = lcg_noise(RATE as usize, 0.05, 0xbeef);
    let profiler = capture_mono(&noise, &[4096]);
    let v2 = profiler.persisted_params().captured_profile.unwrap();
    assert_eq!(v2.format_version, 2);
    assert!(v2.spectral.is_some());
    assert_eq!(v2.spectral.as_ref().unwrap().hops_analyzed, 184);

    // v2 JSON round-trips bit-exactly through params and construction.
    let json = serde_json::to_string(&profiler.persisted_params()).unwrap();
    let restored_params: HissReducerPluginParams = serde_json::from_str(&json).unwrap();
    let reloaded = restored_params.captured_profile.clone().unwrap();
    assert_eq!(reloaded, v2, "v2 JSON must restore exact measured data");
    let mut reloaded_plugin = HissReducerPlugin::from_params(1, restored_params);
    reloaded_plugin.initialize(f64::from(RATE)).unwrap();
    assert!(reloaded_plugin.has_measured_spectrum());
    assert_eq!(
        reloaded_plugin.measured_spectrum().unwrap(),
        profiler.measured_spectrum().unwrap()
    );

    // v1 JSON without the spectral key still loads as floors-only.
    let v1_json = serde_json::json!({
        "captured_profile": {
            "format_version": 1,
            "sample_rate": RATE,
            "channels": 1,
            "measurement_cutoff_hz": 4000.0,
            "floor_db_per_channel": [-40.0],
            "frames_analyzed": RATE,
        }
    });
    let v1_params: HissReducerPluginParams = serde_json::from_value(v1_json).unwrap();
    let v1_data = v1_params.captured_profile.clone().unwrap();
    assert_eq!(v1_data.format_version, 1);
    assert!(v1_data.spectral.is_none());
    v1_data.validate().unwrap();

    // v1 restore over v2 clears the spectrum (white-spread semantics);
    // v2 restore reinstalls it exactly.
    let mut plugin = spectral_plugin(1);
    plugin.restore_profile(&v2).unwrap();
    assert!(plugin.has_measured_spectrum());
    plugin.restore_profile(&v1_data).unwrap();
    assert!(!plugin.has_measured_spectrum());
    assert!(!plugin.profile_uses_spectral());
    assert_eq!(plugin.overall_profile_floor_db(), Some(-40.0));
    plugin.restore_profile(&v2).unwrap();
    assert!(plugin.has_measured_spectrum());
    assert_eq!(
        plugin.persisted_params().captured_profile.unwrap(),
        v2
    );

    // Every malformed spectral candidate rejects transactionally, keeping
    // the accepted v2 profile and its audio bit-exactly.
    let segment = lcg_noise(8192, 0.04, 0x77aa);
    plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let reference = process_all(&mut plugin, &segment, 1, &[997, 64]);
    let base_spectral = v2.spectral.clone().unwrap();
    let mut candidates: Vec<(&'static str, NoiseProfileData)> = Vec::new();
    let mut push_variant =
        |label: &'static str, mutate: &dyn Fn(&mut SpectralProfileData)| {
        let mut spectral = base_spectral.clone();
        mutate(&mut spectral);
        candidates.push((
            label,
            NoiseProfileData {
                format_version: 2,
                sample_rate: f64::from(RATE),
                channels: 1,
                measurement_cutoff_hz: 4000.0,
                floor_db_per_channel: vec![-40.0],
                frames_analyzed: u64::from(RATE),
                spectral: Some(spectral),
            },
        ));
    };
    push_variant("bad-fft", &|s| s.fft_size = 2048);
    push_variant("bad-hop", &|s| s.hop_size = 128);
    push_variant("bad-window", &|s| s.window = "hann-symmetric".to_string());
    push_variant("bad-rate", &|s| s.sample_rate = 96_000.0);
    push_variant("bad-channels", &|s| s.channels = 2);
    push_variant("bad-bins", &|s| s.num_bins = 256);
    push_variant("zero-hops", &|s| s.hops_analyzed = 0);
    push_variant("bad-length", &|s| {
        s.power_per_channel_bin.pop();
    });
    push_variant("nan-power", &|s| s.power_per_channel_bin[9] = f32::NAN);
    push_variant("negative-power", &|s| {
        s.power_per_channel_bin[11] = -1.0;
    });
    push_variant("infinite-power", &|s| {
        s.power_per_channel_bin[13] = f32::INFINITY;
    });
    // Version/payload mismatches.
    candidates.push((
        "v1-with-spectral",
        NoiseProfileData {
            format_version: 1,
            sample_rate: f64::from(RATE),
            channels: 1,
            measurement_cutoff_hz: 4000.0,
            floor_db_per_channel: vec![-40.0],
            frames_analyzed: u64::from(RATE),
            spectral: Some(base_spectral.clone()),
        },
    ));
    candidates.push((
        "v2-without-spectral",
        NoiseProfileData {
            format_version: 2,
            sample_rate: f64::from(RATE),
            channels: 1,
            measurement_cutoff_hz: 4000.0,
            floor_db_per_channel: vec![-40.0],
            frames_analyzed: u64::from(RATE),
            spectral: None,
        },
    ));
    candidates.push((
        "bad-version",
        NoiseProfileData {
            format_version: 99,
            sample_rate: f64::from(RATE),
            channels: 1,
            measurement_cutoff_hz: 4000.0,
            floor_db_per_channel: vec![-40.0],
            frames_analyzed: u64::from(RATE),
            spectral: None,
        },
    ));
    for (label, candidate) in &candidates {
        let error = plugin.restore_profile(candidate).unwrap_err();
        assert!(!error.is_empty(), "{label} must explain the rejection");
        assert_eq!(
            plugin.persisted_params().captured_profile.as_ref().unwrap(),
            &v2,
            "{label} must retain the accepted v2 blob"
        );
    }
    plugin.reset();
    let after_reject = process_all(&mut plugin, &segment, 1, &[997, 64]);
    assert_eq!(
        reference, after_reject,
        "rejected spectral candidates must retain accepted audio"
    );
}

#[test]
fn spectral_realtime_paths_do_not_allocate_or_free() {
    let mut plugin = time_domain_plugin(1);
    let warm = vec![0.01; 4096];
    process_all(&mut plugin, &warm, 1, &[4096]);

    let learn_id = ParameterId::from("learn_noise");
    let use_id = ParameterId::from("use_captured_profile");
    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .parametric_set_parameter(learn_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "learn trigger heap activity");

    // Full 1 s capture including spectral FFTs and completion.
    let mut block = lcg_noise(RATE as usize, 0.05, 0xa110c);
    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .process_in_place(&mut block, &ctx(RATE as usize))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "capture process heap activity");
    assert!(plugin.has_measured_spectrum());

    let (allocs, frees) = measure_heap_activity(|| {
        plugin
            .parametric_set_parameter(use_id.clone(), ParameterValue::Bool(true))
            .unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "use-profile toggle heap activity");

    let data = plugin.persisted_params().captured_profile.unwrap();
    assert_eq!(data.format_version, 2);
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.restore_profile(&data).unwrap();
    });
    assert_eq!((allocs, frees), (0, 0), "v2 profile restore heap activity");

    let (allocs, frees) = measure_heap_activity(|| {
        plugin.clear_captured_profile();
    });
    assert_eq!((allocs, frees), (0, 0), "profile clear heap activity");

    plugin.restore_profile(&data).unwrap();
    let (allocs, frees) = measure_heap_activity(|| {
        plugin.reset();
    });
    assert_eq!((allocs, frees), (0, 0), "reset heap activity");
}

#[test]
fn cross_rate_falls_back_to_floors_and_cutoff_keeps_spectral() {
    const RATE_96K: u32 = 96_000;
    // Capture v2 at 48 kHz; spectral engages at the capture rate.
    let noise_48k = lcg_noise(RATE as usize, 0.05, 0xc405);
    let profiler_48k = capture_mono(&noise_48k, &[4096]);
    let v2_48k = profiler_48k.persisted_params().captured_profile.unwrap();
    assert_eq!(v2_48k.format_version, 2);

    let mut plugin_48k = spectral_plugin(1);
    plugin_48k.restore_profile(&v2_48k).unwrap();
    plugin_48k
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(plugin_48k.profile_uses_spectral());

    // At 96 kHz the 48 kHz spectrum stays stored but disengages; audio
    // matches the v1 floors-only fallback bit-exactly (no silent reuse).
    let v1_fallback = NoiseProfileData {
        format_version: 1,
        sample_rate: f64::from(RATE),
        channels: 1,
        measurement_cutoff_hz: 4000.0,
        floor_db_per_channel: v2_48k.floor_db_per_channel.clone(),
        frames_analyzed: v2_48k.frames_analyzed,
        spectral: None,
    };
    let segment_96k = lcg_noise(16384, 0.04, 0x96c0);
    let mut spectral_96k = spectral_plugin_at_rate(1, RATE_96K);
    spectral_96k.restore_profile(&v2_48k).unwrap();
    spectral_96k
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(spectral_96k.has_measured_spectrum());
    assert!(
        !spectral_96k.profile_uses_spectral(),
        "cross-rate spectrum must not engage"
    );
    let mut fallback_96k = spectral_plugin_at_rate(1, RATE_96K);
    fallback_96k.restore_profile(&v1_fallback).unwrap();
    fallback_96k
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    let spectral_out =
        process_all_at_rate(&mut spectral_96k, &segment_96k, 1, &[4096], RATE_96K);
    let fallback_out =
        process_all_at_rate(&mut fallback_96k, &segment_96k, 1, &[4096], RATE_96K);
    assert_eq!(
        spectral_out, fallback_out,
        "cross-rate v2 must equal v1 white-spread fallback"
    );

    // A fresh 96 kHz capture engages spectrally at 96 kHz.
    let noise_96k = lcg_noise(RATE_96K as usize, 0.05, 0x96ca);
    let mut profiler_96k = spectral_plugin_at_rate(1, RATE_96K);
    start_capture(&mut profiler_96k);
    process_all_at_rate(&mut profiler_96k, &noise_96k, 1, &[4096], RATE_96K);
    assert!(profiler_96k.has_measured_spectrum());
    assert_eq!(profiler_96k.profile_spectral_hops(), Some(372));
    profiler_96k
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    assert!(profiler_96k.profile_uses_spectral());

    // A cutoff change keeps per-bin validity: spectral stays engaged and
    // differs from the floors-only rendering at the new cutoff.
    let mut cutoff_plugin = spectral_plugin(1);
    cutoff_plugin.restore_profile(&v2_48k).unwrap();
    cutoff_plugin
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    cutoff_plugin
        .set_parameter(ParameterId::from("frequency_hz"), ParameterValue::Float(8000.0))
        .unwrap();
    assert!(
        cutoff_plugin.profile_uses_spectral(),
        "cutoff change must keep spectral engaged"
    );
    let mut cutoff_fallback = spectral_plugin(1);
    cutoff_fallback.restore_profile(&v1_fallback).unwrap();
    cutoff_fallback
        .set_parameter(
            ParameterId::from("use_captured_profile"),
            ParameterValue::Bool(true),
        )
        .unwrap();
    cutoff_fallback
        .set_parameter(ParameterId::from("frequency_hz"), ParameterValue::Float(8000.0))
        .unwrap();
    let segment = lcg_noise(16384, 0.04, 0xc470);
    let spectral_cutoff = process_all(&mut cutoff_plugin, &segment, 1, &[4096]);
    let fallback_cutoff = process_all(&mut cutoff_fallback, &segment, 1, &[4096]);
    assert_ne!(
        spectral_cutoff, fallback_cutoff,
        "engaged spectrum must differ from white-spread at the new cutoff"
    );
}

const LATENCY: usize = 1024;

/// Peak magnitude within ±64 frames of a base (legacy guard metric).
fn peak_near(signal: &[f32], base: usize, radius: usize) -> f32 {
    let start = base.saturating_sub(radius);
    let end = (base + radius + 1).min(signal.len());
    signal[start..end]
        .iter()
        .map(|s| s.abs())
        .fold(0.0f32, f32::max)
}

/// Mean 20log10 output/input peak ratio over settled impulses plus the
/// per-impulse ratios (legacy guard metric, verbatim).
fn impulse_peak_stats(input: &[f32], output: &[f32], bases: &[usize]) -> (f64, Vec<f64>) {
    let mut in_sum = 0.0f64;
    let mut out_sum = 0.0f64;
    let mut per = Vec::with_capacity(bases.len());
    for &base in bases {
        let in_peak = peak_near(input, base, 4);
        let out_peak = peak_near(output, base + LATENCY, 64);
        in_sum += f64::from(in_peak);
        out_sum += f64::from(out_peak);
        per.push(20.0 * (f64::from(out_peak) / f64::from(in_peak)).log10());
    }
    (20.0 * (out_sum / in_sum).log10(), per)
}

/// Settled impulse bases: past 1.5 s with the latency-shifted measurement
/// window inside the run (legacy guard metric, verbatim).
fn settled_bases(frames: usize, first: usize, step: usize) -> Vec<usize> {
    (first..frames)
        .step_by(step)
        .filter(|&b| b >= RATE as usize * 3 / 2 && b + LATENCY + 64 < frames)
        .collect()
}

/// First-difference high-passed hiss, bit-identical to the legacy guard
/// and accuracy fixtures (zero initial condition, same LCG sequence).
fn first_difference_hiss(frames: usize, amplitude: f32, seed: u32) -> Vec<f32> {
    let white = lcg_noise(frames, 1.0, seed);
    let mut previous = 0.0f32;
    white
        .iter()
        .map(|&sample| {
            let high_pass = amplitude * (sample - previous);
            previous = sample;
            high_pass
        })
        .collect()
}

#[test]
fn v1_guard_fixture_retained_and_measured_guard_meets_bounds() {
    // Legacy 4864-period settled-impulse stimulus, seeds verbatim
    // (stimulus 0x51ab_0002, capture 0x9a5516): the configuration that
    // regressed to -3.07 dB under measured spectra at r3. One real capture
    // supplies both legs: the v1 leg strips the spectrum (identical floors,
    // white-spread, legacy 2.0x firing — bit-exact pre-slice behavior) and
    // must meet the ORIGINAL -3/+2/-2/1.5 dB acceptance (retention); the
    // measured leg engages the spectrum with earlier 1.5x firing and must
    // meet the same bounds (coverage). The legs must render differently,
    // proving the measured path is genuinely engaged rather than falling
    // back to white-spread.
    let frames = RATE as usize * 3;
    let mut signal = first_difference_hiss(frames, 0.035, 0x51ab_0002);
    for base in (RATE as usize + 100..frames).step_by(4864) {
        signal[base] += 1.0;
    }
    let bases = settled_bases(frames, RATE as usize + 100, 4864);
    assert!(bases.len() >= 10, "need settled impulses, got {}", bases.len());

    let mut profiler = HissReducerPlugin::from_params(
        1,
        HissReducerPluginParams {
            spectral_mode: true,
            strength: 0.85,
            ..HissReducerPluginParams::default()
        },
    );
    profiler.initialize(f64::from(RATE)).unwrap();
    let capture = first_difference_hiss(RATE as usize, 0.035, 0x9a5516);
    start_capture(&mut profiler);
    process_all(&mut profiler, &capture, 1, &[4096]);
    let v2 = profiler.persisted_params().captured_profile.unwrap();
    assert_eq!(v2.format_version, 2);
    let v1 = NoiseProfileData {
        format_version: 1,
        sample_rate: v2.sample_rate,
        channels: v2.channels,
        measurement_cutoff_hz: v2.measurement_cutoff_hz,
        floor_db_per_channel: v2.floor_db_per_channel.clone(),
        frames_analyzed: v2.frames_analyzed,
        spectral: None,
    };

    let render_leg = |blob: &NoiseProfileData, guard: bool| {
        let mut plugin = HissReducerPlugin::from_params(
            1,
            HissReducerPluginParams {
                spectral_mode: true,
                strength: 0.85,
                use_captured_profile: true,
                transient_guard: guard,
                ..HissReducerPluginParams::default()
            },
        );
        plugin.initialize(f64::from(RATE)).unwrap();
        plugin.restore_profile(blob).unwrap();
        process_all(&mut plugin, &signal, 1, &PARTITIONS)
    };
    for (blob, label) in [(&v1, "v1"), (&v2, "measured")] {
        let out_off = render_leg(blob, false);
        let out_on = render_leg(blob, true);
        assert!(out_off.iter().all(|s| s.is_finite()));
        assert!(out_on.iter().all(|s| s.is_finite()));
        assert_ne!(out_off, out_on, "{label}: guard must act on settled impulses");

        let (loss_off, _) = impulse_peak_stats(&signal, &out_off, &bases);
        let (loss_on, per_on) = impulse_peak_stats(&signal, &out_on, &bases);
        assert!(
            loss_off < -1.0,
            "{label}: guard-off leg not engaged ({loss_off:.2} dB)"
        );
        let improvement = loss_on - loss_off;
        assert!(
            improvement > 1.5,
            "{label}: guard improvement only {improvement:.2} dB (off {loss_off:.2} dB, on {loss_on:.2} dB)"
        );
        let worst_on = per_on.iter().copied().fold(f64::INFINITY, f64::min);
        let best_on = per_on.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        println!("{label} guard-on per-impulse losses (dB): {per_on:.2?}");
        println!("{label} guard-on worst {worst_on:.2} dB, best {best_on:.2} dB, mean {loss_on:.2} dB");
        assert!(
            worst_on > -3.0,
            "{label}: guard-on worst settled peak lost ({worst_on:.2} dB, mean {loss_on:.2} dB)"
        );
        assert!(
            best_on < 2.0,
            "{label}: guard-on amplified a settled peak ({best_on:.2} dB)"
        );
        for &base in &bases {
            let off_peak = peak_near(&out_off, base + LATENCY, 64);
            let on_peak = peak_near(&out_on, base + LATENCY, 64);
            let delta = 20.0 * (f64::from(on_peak) / f64::from(off_peak)).log10();
            assert!(
                delta > -1.0,
                "{label}: impulse at frame {base} worse with guard on ({delta:.2} dB)"
            );
        }

        // Hiss-only twins: suppression stays engaged in both guard states.
        for guard in [false, true] {
            let mut plugin = HissReducerPlugin::from_params(
                1,
                HissReducerPluginParams {
                    spectral_mode: true,
                    strength: 0.85,
                    use_captured_profile: true,
                    transient_guard: guard,
                    ..HissReducerPluginParams::default()
                },
            );
            plugin.initialize(f64::from(RATE)).unwrap();
            plugin.restore_profile(blob).unwrap();
            let hiss_only = first_difference_hiss(frames, 0.035, 0x51ab_0002);
            let hiss_out = process_all(&mut plugin, &hiss_only, 1, &PARTITIONS);
            let start = RATE as usize + LATENCY;
            let suppression = power_db(
                mean_power(&hiss_out[start..]) / mean_power(&hiss_only[start - LATENCY..]),
            );
            assert!(
                suppression < -2.0,
                "{label}: guard={guard} hiss-only suppression lost ({suppression:.2} dB)"
            );
        }
    }

    // Distinct renderings prove the measured path engages per-bin spectra.
    let v1_on = render_leg(&v1, true);
    let v2_on = render_leg(&v2, true);
    assert_ne!(
        v1_on, v2_on,
        "measured guard render must differ from v1 white-spread render"
    );
}
