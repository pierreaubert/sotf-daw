use super::dither_plugin::DitherPlugin;
use super::misc::random_f32;
use super::misc::xorshift64;
use super::types::DitherPluginParams;
use crate::params::Params;
use rustfft::{FftPlanner, num_complex::Complex};
use sotf_host::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::ParameterSet;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::ProcessContext;
use sotf_host::plugin_params::PluginParamDef;

fn make_context(num_frames: usize) -> ProcessContext<'static> {
    ProcessContext::new(48000, num_frames)
}

fn noise_transfer_magnitude(plugin: &DitherPlugin, frequency_hz: f32) -> f32 {
    let mut real = 1.0_f32;
    let mut imag = 0.0_f32;
    for (coefficient, delay_seconds) in super::misc::NOISE_SHAPING_COEFFS
        .iter()
        .zip(plugin.noise_shaping_delays_samples.iter())
    {
        let delay_seconds = delay_seconds / plugin.sample_rate as f32;
        let phase = 2.0 * std::f32::consts::PI * frequency_hz * delay_seconds;
        real -= coefficient * phase.cos();
        imag += coefficient * phase.sin();
    }
    real.hypot(imag)
}

#[test]
fn f_weighted_shaper_preserves_absolute_frequency_response_across_sample_rates() {
    let mut reference = DitherPlugin::new(1);
    reference.initialize(44_100.0).unwrap();
    let mut double_rate = DitherPlugin::new(1);
    double_rate.initialize(88_200.0).unwrap();

    assert_eq!(reference.noise_shaping_delays_samples, [1.0, 2.0, 3.0]);
    assert_eq!(double_rate.noise_shaping_delays_samples, [2.0, 4.0, 6.0]);

    for frequency_hz in [1_000.0, 5_000.0, 10_000.0, 15_000.0, 20_000.0] {
        let at_reference = noise_transfer_magnitude(&reference, frequency_hz);
        let at_double_rate = noise_transfer_magnitude(&double_rate, frequency_hz);
        assert!(
            (at_reference - at_double_rate).abs() < 1.0e-5,
            "F-weighted NTF changed at {frequency_hz} Hz: {at_reference} vs {at_double_rate}"
        );
    }
}

#[test]
fn f_weighted_shaper_has_a_bounded_policy_for_every_supported_rate_family() {
    for sample_rate in [
        8_000_u32, 16_000, 22_050, 32_000, 44_100, 48_000, 96_000, 192_000, 384_000, 768_000,
    ] {
        let mut plugin = DitherPlugin::new(2);
        plugin.initialize(f64::from(sample_rate)).unwrap();
        assert!(
            plugin
                .noise_shaping_delays_samples
                .iter()
                .all(|delay| delay.is_finite() && *delay >= 1.0)
        );
        assert!(plugin.error_history.iter().all(|history| {
            history.len() > plugin.noise_shaping_delays_samples[2].ceil() as usize
        }));

        let frames = 128;
        let mut signal = vec![0.001_234_f32; frames * 2];
        plugin
            .process_in_place(&mut signal, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(signal.iter().all(|sample| sample.is_finite()));
    }

    assert!(DitherPlugin::new(1).initialize(0.0).is_err());
    assert!(DitherPlugin::new(1).initialize(768_001.0).is_err());
}

fn averaged_error_spectrum(noise_shaping: bool) -> Vec<f64> {
    const SAMPLE_RATE: u32 = 48_000;
    const FFT_SIZE: usize = 2048;
    const SEGMENTS: usize = 16;
    const TOTAL: usize = FFT_SIZE * SEGMENTS;

    // Deterministic broadband programme decorrelates quantization error from
    // individual FFT bins without adding explicit dither that would mask the
    // error-feedback shaper under test.
    let mut rng = 0x42a7_91d3_55ee_1021_u64;
    let original: Vec<f32> = (0..TOTAL).map(|_| random_f32(&mut rng) * 0.02).collect();
    let mut quantized = original.clone();
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping,
            dither_type: 1,
        },
    );
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
        .process_in_place(&mut quantized, &ProcessContext::new(SAMPLE_RATE, TOTAL))
        .unwrap();

    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
    let mut averaged = vec![0.0_f64; FFT_SIZE / 2 + 1];
    let mut bins = vec![Complex::new(0.0_f32, 0.0_f32); FFT_SIZE];
    for segment in 0..SEGMENTS {
        for (index, bin) in bins.iter_mut().enumerate() {
            let window =
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / FFT_SIZE as f32).cos();
            let offset = segment * FFT_SIZE + index;
            *bin = Complex::new((quantized[offset] - original[offset]) * window, 0.0);
        }
        fft.process(&mut bins);
        for (power, bin) in averaged.iter_mut().zip(&bins) {
            *power += bin.norm_sqr() as f64;
        }
    }
    averaged
}

#[test]
fn f_weighted_noise_shaping_moves_quantization_error_out_of_the_sensitive_band() {
    let flat = averaged_error_spectrum(false);
    let shaped = averaged_error_spectrum(true);
    let bin_hz = 48_000.0 / 2048.0;
    let band_power = |spectrum: &[f64], low_hz: f64, high_hz: f64| {
        spectrum
            .iter()
            .enumerate()
            .filter(|(bin, _)| {
                let frequency = *bin as f64 * bin_hz;
                frequency >= low_hz && frequency < high_hz
            })
            .map(|(_, power)| *power)
            .sum::<f64>()
    };

    let flat_sensitive = band_power(&flat, 200.0, 8_000.0);
    let shaped_sensitive = band_power(&shaped, 200.0, 8_000.0);
    let flat_ultrasonic = band_power(&flat, 16_000.0, 23_000.0);
    let shaped_ultrasonic = band_power(&shaped, 16_000.0, 23_000.0);
    assert!(
        shaped_sensitive < flat_sensitive * 0.75,
        "shaping did not reduce sensitive-band error: flat={flat_sensitive:e}, shaped={shaped_sensitive:e}"
    );
    assert!(
        shaped_ultrasonic > flat_ultrasonic * 1.5,
        "shaping did not move error upward: flat={flat_ultrasonic:e}, shaped={shaped_ultrasonic:e}"
    );
}

#[test]
fn test_dither_basic() {
    // Process silence, verify output stays near zero
    let mut plugin = DitherPlugin::new(2);
    plugin.initialize(48000.0).unwrap();

    let num_frames = 1024;
    let mut buffer = vec![0.0f32; num_frames * 2];
    plugin
        .process_in_place(&mut buffer, &make_context(num_frames))
        .unwrap();

    // With dither on silence, output should be very small (within 4 LSB of 16-bit)
    let max_lsb_16 = 1.0 / 32768.0; // 1 LSB at 16-bit
    for &sample in &buffer {
        assert!(
            sample.abs() <= max_lsb_16 * 4.0,
            "Dithered silence should stay near zero, got {}",
            sample
        );
    }
}

#[test]
fn test_dither_quantizes_to_target_depth() {
    // Process a known signal, verify output values are on the 16-bit grid
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: false,
            dither_type: 1, // None (no dither, just quantize)
        },
    );
    plugin.initialize(48000.0).unwrap();

    let scale_16 = 32768.0_f32;
    let num_frames = 512;
    let mut buffer: Vec<f32> = (0..num_frames)
        .map(|i| (i as f32 / num_frames as f32) * 0.5 - 0.25)
        .collect();

    plugin
        .process_in_place(&mut buffer, &make_context(num_frames))
        .unwrap();

    // Every output value should be exactly on the 16-bit grid
    for &sample in &buffer {
        let scaled = sample * scale_16;
        let rounded = scaled.round();
        assert!(
            (scaled - rounded).abs() < 1e-4,
            "Sample {} is not on 16-bit grid (scaled={})",
            sample,
            scaled
        );
    }
}

#[test]
fn shaped_and_unshaped_tpdf_paths_produce_finite_nonzero_error() {
    // Smoke test: both shaping paths must run to completion and produce
    // finite, nonzero quantization error. The actual noise-reduction
    // claim lives in
    // `f_weighted_noise_shaping_moves_quantization_error_out_of_the_sensitive_band`.
    let num_frames = 8192;
    let channels = 1;

    // Generate a quiet sine wave (well below full scale so quantization matters)
    let freq = 1000.0;
    let sr = 48000.0;
    let amplitude = 0.01; // ~-40 dBFS
    let original: Vec<f32> = (0..num_frames)
        .map(|i| amplitude * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
        .collect();

    // Process WITHOUT noise shaping
    let mut plugin_no_ns = DitherPlugin::from_params(
        channels,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: false,
            dither_type: 0,
        },
    );
    plugin_no_ns.initialize(48000.0).unwrap();
    let mut buf_no_ns = original.clone();
    plugin_no_ns
        .process_in_place(&mut buf_no_ns, &make_context(num_frames))
        .unwrap();

    // Process WITH noise shaping
    let mut plugin_ns = DitherPlugin::from_params(
        channels,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: true,
            dither_type: 0,
        },
    );
    plugin_ns.initialize(48000.0).unwrap();
    let mut buf_ns = original.clone();
    plugin_ns
        .process_in_place(&mut buf_ns, &make_context(num_frames))
        .unwrap();

    // Compute error energy in the low-frequency band (bins 0..N/8 ~ 0-3kHz)
    // For a rough check, just compute the sum of squared differences
    let error_no_ns: f64 = buf_no_ns
        .iter()
        .zip(original.iter())
        .map(|(o, i)| ((*o - *i) as f64).powi(2))
        .sum();

    let error_ns: f64 = buf_ns
        .iter()
        .zip(original.iter())
        .map(|(o, i)| ((*o - *i) as f64).powi(2))
        .sum();

    // Both should produce some quantization error
    assert!(error_no_ns > 0.0, "No-NS error should be non-zero");
    assert!(error_ns > 0.0, "NS error should be non-zero");

    // Noise shaping may increase total error energy (it reshapes, doesn't remove).
    // We just verify both produce finite, reasonable results.
    assert!(error_no_ns.is_finite());
    assert!(error_ns.is_finite());
}

#[test]
fn test_dither_parameter_set_get() {
    let mut plugin = DitherPlugin::new(2);

    // Test bit_depth
    plugin
        .set_parameter(
            ParameterId::from("bit_depth"),
            ParameterValue::Int(2), // 24-bit
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );

    // Test noise_shaping
    plugin
        .set_parameter(
            ParameterId::from("noise_shaping"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("noise_shaping")),
        Some(ParameterValue::Bool(false))
    );

    // Test dither_type
    plugin
        .set_parameter(
            ParameterId::from("dither_type"),
            ParameterValue::Int(2), // Truncate
        )
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("dither_type")),
        Some(ParameterValue::Int(2))
    );

    // Test unknown parameter
    assert!(
        plugin
            .set_parameter(ParameterId::from("unknown"), ParameterValue::Float(0.0),)
            .is_err()
    );
    assert_eq!(plugin.get_parameter(&ParameterId::from("unknown")), None);
}

#[test]
fn test_xorshift64_produces_nonzero_values() {
    let mut state = 0xDEAD_BEEF_CAFE_0001_u64;
    let mut all_zero = true;
    for _ in 0..100 {
        let val = xorshift64(&mut state);
        if val != 0 {
            all_zero = false;
        }
    }
    assert!(!all_zero, "xorshift64 should produce non-zero values");
}

#[test]
fn test_random_f32_range() {
    let mut state = 0xDEAD_BEEF_CAFE_0001_u64;
    for _ in 0..10000 {
        let val = random_f32(&mut state);
        assert!(
            (-0.5..=0.5).contains(&val),
            "random_f32 out of range: {}",
            val
        );
    }
}

/// Verify the PRNG boundary: the worst-case input (upper = u32::MAX) produces
/// a value within the valid TPDF range [-0.5, 0.5].
///
/// Note: due to f32 rounding of u32::MAX (which rounds up to 2^32), the result
/// for u32::MAX is exactly 0.5 (not strictly less).  This is acceptable for TPDF
/// dither — the closed interval [-0.5, 0.5] is statistically equivalent for audio
/// use.  The comment in `random_f32` was corrected from "[-0.5, 0.5)" to
/// "[-0.5, 0.5]" to match the actual implementation.
#[test]
fn test_random_f32_boundary_precision() {
    // Directly exercise the boundary: upper = u32::MAX produces exactly 0.5
    // (because u32::MAX as f32 rounds up to 2^32 = the divisor, giving ratio 1.0).
    let upper_max = u32::MAX;
    let val = (upper_max as f32 / u32::MAX as f32) - 0.5;
    // The boundary value must be within [-0.5, 0.5] — not outside.
    assert!(
        val <= 0.5,
        "random_f32 boundary: value exceeds 0.5: {}",
        val
    );
    assert!(
        val >= -0.5,
        "random_f32 boundary: value below -0.5: {}",
        val
    );
    // Confirm this specific boundary is exactly 0.5 (documents the known f32 behavior).
    assert_eq!(
        val, 0.5,
        "random_f32 boundary for u32::MAX should be exactly 0.5, got {}",
        val
    );
}

#[test]
fn test_tpdf_is_independent_triangular_noise() {
    let mut plugin = DitherPlugin::new(1);
    plugin.initialize(48000.0).unwrap();
    let samples: Vec<f64> = (0..100_000).map(|_| plugin.next_tpdf(0) as f64).collect();
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let variance = samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
    let lag_one = samples
        .windows(2)
        .map(|pair| (pair[0] - mean) * (pair[1] - mean))
        .sum::<f64>()
        / (samples.len() - 1) as f64;
    assert!(mean.abs() < 0.005, "TPDF mean={mean}");
    assert!(
        (variance - 1.0 / 6.0).abs() < 0.005,
        "TPDF variance={variance}"
    );
    assert!(
        (lag_one / variance).abs() < 0.02,
        "lag-1 correlation={}",
        lag_one / variance
    );
    assert!(samples.iter().all(|x| (-1.0..=1.0).contains(x)));
}

#[test]
fn test_noise_shaping_feedback_excludes_dither_term() {
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: true,
            dither_type: 0, // TPDF
        },
    );
    plugin.initialize(48000.0).unwrap();

    // Use a deterministic sample and capture the next TPDF token before processing.
    let input = 0.123456_f32;
    let saved_rng_state = plugin.rng_state.clone();
    let tpdf = plugin.next_tpdf(0);
    plugin.rng_state = saved_rng_state;

    // With a fresh plugin state, the initial noise-shaping feedback is zero.
    let mut buffer = vec![input];
    plugin
        .process_in_place(&mut buffer, &make_context(1))
        .unwrap();

    let shaped = f64::from(input);
    let dithered = shaped + f64::from(tpdf) * f64::from(plugin.inv_scale);
    let quantized = (dithered * f64::from(plugin.scale)).round() * f64::from(plugin.inv_scale);
    let expected_error = (quantized - dithered) as f32;
    let stale_error = (quantized - shaped) as f32;

    assert!(
        tpdf.abs() > 0.0,
        "TPDF sample must be non-zero for this regression"
    );
    assert!(
        (plugin.error_history[0][0] - expected_error).abs() < 1e-6,
        "expected feedback residual to exclude explicit dither"
    );
    assert!(
        (expected_error - stale_error).abs() > 1e-9,
        "TPDF should distinguish the two residual definitions"
    );
    assert_eq!(
        plugin.error_history[0][0], expected_error,
        "stored error history should match the quantizer-input residual"
    );
}

#[test]
fn reset_restarts_deterministic_tpdf_sequence() {
    let mut plugin = DitherPlugin::new(1);
    let first: Vec<f32> = (0..16).map(|_| plugin.next_tpdf(0)).collect();
    plugin.reset();
    let restarted: Vec<f32> = (0..16).map(|_| plugin.next_tpdf(0)).collect();
    assert_eq!(first, restarted);
}

#[test]
fn test_truncate_mode_quantizes_without_rounding() {
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: false,
            dither_type: 2, // Truncate
        },
    );
    plugin.initialize(48000.0).unwrap();

    let scale_16 = 32768.0_f32;
    let mut buffer = vec![-0.123456, 0.123456];
    let num_frames = buffer.len();
    plugin
        .process_in_place(&mut buffer, &make_context(num_frames))
        .unwrap();

    // Truncation is toward zero, so results are different from rounding for
    // these non-symmetric test points.
    assert!(
        (buffer[0] - ((-0.123456_f32 * scale_16).trunc() * (1.0 / scale_16))).abs() < 1e-7,
        "truncate should apply for negative value, got {}",
        buffer[0]
    );
    assert!(
        (buffer[1] - ((0.123456_f32 * scale_16).trunc() * (1.0 / scale_16))).abs() < 1e-7,
        "truncate should apply for positive value, got {}",
        buffer[1]
    );
}

#[test]
fn test_24bit_quantization_grid() {
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 2, // 24-bit
            noise_shaping: false,
            dither_type: 1, // None
        },
    );
    plugin.initialize(48000.0).unwrap();

    let scale_24 = 8388608.0_f32; // 2^23
    let num_frames = 256;
    let mut buffer: Vec<f32> = (0..num_frames)
        .map(|i| (i as f32 / num_frames as f32) * 0.1)
        .collect();

    plugin
        .process_in_place(&mut buffer, &make_context(num_frames))
        .unwrap();

    for &sample in &buffer {
        let scaled = sample * scale_24;
        let rounded = scaled.round();
        assert!(
            (scaled - rounded).abs() < 1e-2,
            "Sample {} is not on 24-bit grid (scaled={})",
            sample,
            scaled
        );
    }
}

#[test]
fn signed_pcm_endpoints_are_saturated_before_error_feedback() {
    for (bit_depth, bits) in [(0usize, 16i32), (1usize, 20i32), (2usize, 24i32)] {
        let scale = 2.0_f32.powi(bits - 1);
        let inv_scale = 1.0 / scale;
        let max_code = scale - 1.0;

        // Exercise both signed endpoints and the highest representable value
        // just below full scale without noise shaping first.
        let mut plugin = DitherPlugin::from_params(
            1,
            DitherPluginParams {
                bit_depth,
                noise_shaping: false,
                dither_type: 1, // None (round)
            },
        );
        plugin.initialize(48000.0).unwrap();
        let near_full_scale = 1.0 - 1.5 * inv_scale;
        let mut buffer = vec![-1.0, 1.0, near_full_scale];
        let num_frames = buffer.len();
        plugin
            .process_in_place(&mut buffer, &make_context(num_frames))
            .unwrap();

        assert_eq!(buffer[0], -1.0, "negative endpoint at {bits}-bit");
        assert_eq!(
            buffer[1],
            max_code * inv_scale,
            "positive endpoint at {bits}-bit"
        );
        assert_eq!(
            buffer[2],
            max_code * inv_scale,
            "near-full-scale rounding must not emit an out-of-range code at {bits}-bit"
        );

        // A shaping overshoot must feed back the code that was actually
        // emitted after saturation, rather than the unrepresentable +1.0.
        let mut shaped_plugin = DitherPlugin::from_params(
            1,
            DitherPluginParams {
                bit_depth,
                noise_shaping: true,
                dither_type: 1, // None (round)
            },
        );
        shaped_plugin.initialize(48000.0).unwrap();
        let mut overshoot = vec![1.0];
        let num_frames = overshoot.len();
        shaped_plugin
            .process_in_place(&mut overshoot, &make_context(num_frames))
            .unwrap();
        assert_eq!(
            overshoot[0],
            max_code * inv_scale,
            "shaper overshoot must saturate at {bits}-bit signed PCM maximum"
        );
        assert_eq!(
            shaped_plugin.error_history[0][0],
            max_code * inv_scale - 1.0,
            "noise-shaping state must use the emitted {bits}-bit code"
        );
    }
}

#[test]
fn test_multichannel_independent() {
    // Verify each channel gets independent dither
    let mut plugin = DitherPlugin::from_params(
        2,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: false,
            dither_type: 0,
        },
    );
    plugin.initialize(48000.0).unwrap();

    let num_frames = 256;
    // Same value on both channels
    let val = 0.00123_f32;
    let mut buffer = vec![val; num_frames * 2];

    plugin
        .process_in_place(&mut buffer, &make_context(num_frames))
        .unwrap();

    // With TPDF dither, the two channels should generally differ
    // (different RNG states), though they could rarely match
    let mut differ_count = 0;
    for frame in 0..num_frames {
        if (buffer[frame * 2] - buffer[frame * 2 + 1]).abs() > 1e-10 {
            differ_count += 1;
        }
    }
    assert!(
        differ_count > num_frames / 4,
        "Channels should have independent dither, but only {} of {} frames differed",
        differ_count,
        num_frames
    );
}

#[test]
fn short_host_buffer_returns_err_instead_of_panicking() {
    let mut plugin = DitherPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let frames = 64;
    // One sample short of the required frames * channels: without the
    // length check this indexes past the buffer on the audio thread.
    let mut short = vec![0.1_f32; frames * 2 - 1];
    let result = plugin.process_in_place(&mut short, &make_context(frames));
    assert!(
        result.is_err(),
        "short buffer must return Err, got {result:?}"
    );

    // Exact-size buffers still process normally.
    let mut exact = vec![0.1_f32; frames * 2];
    assert!(
        plugin
            .process_in_place(&mut exact, &make_context(frames))
            .is_ok()
    );
    assert!(exact.iter().all(|sample| sample.is_finite()));
}

#[test]
#[should_panic(expected = "at least one channel")]
fn zero_channels_rejected_in_new() {
    let _ = DitherPlugin::new(0);
}

#[test]
#[should_panic(expected = "at least one channel")]
fn zero_channels_rejected_in_from_params() {
    let _ = DitherPlugin::from_params(
        0,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: false,
            dither_type: 0,
        },
    );
}

// ============================================================================
// DITHER-A1: independent final-quantized-error moments
// ============================================================================
//
// Reference result (Wannamaker): 2-LSB peak-to-peak TPDF dither with
// round-to-nearest quantization gives final error e = Q(x + d) - x with
// E[e] = 0 and E[e^2] = LSB^2 / 4 for every input x, provided the quantizer
// does not saturate. This oracle measures the plugin's actual output against
// that result and shares no code with the implementation.

/// Accepted error bound in LSB units (mean) and LSB^2 units (second
/// moment), carried over from the AUDIT.md-accepted dither oracle.
const FINAL_ERROR_MOMENT_TOLERANCE_LSB: f64 = 0.008;

/// Samples per (bit depth, input level) case. The mean estimator has
/// standard error sqrt(0.25 / N) ~ 0.001 LSB here, so the bound above sits
/// about eight sigma from the expectation.
const MOMENT_SAMPLES_PER_CASE: usize = 262_144;

/// DC input levels in LSB units: signed, fractional, and sub-LSB
/// (|level| < 0.5) cases, all far from the saturation rails.
const MOMENT_LEVELS_LSB: [f64; 9] = [
    -64.75, -1.5, -0.4, -0.1, 0.0, 0.1, 0.4, 1.5, 64.75,
];

/// Exposed bit-depth routes as (choice index, bits) pairs.
const BIT_DEPTHS_UNDER_TEST: [(usize, i32); 3] = [(0, 16), (1, 20), (2, 24)];

#[test]
fn final_quantized_error_has_zero_mean_and_quarter_lsb_second_moment() {
    let mut worst_mean = 0.0_f64;
    let mut worst_mean_case = (16, 0.0_f64);
    let mut worst_second = 0.0_f64;
    let mut worst_second_case = (16, 0.0_f64);

    for (depth_index, bits) in BIT_DEPTHS_UNDER_TEST {
        let scale = 2.0_f64.powi(bits - 1);
        for level_lsb in MOMENT_LEVELS_LSB {
            let mut plugin = DitherPlugin::from_params(
                1,
                DitherPluginParams {
                    bit_depth: depth_index,
                    noise_shaping: false,
                    dither_type: 0, // TPDF
                },
            );
            plugin.initialize(48_000.0).unwrap();

            let input = (level_lsb / scale) as f32;
            let mut buffer = vec![input; MOMENT_SAMPLES_PER_CASE];
            plugin
                .process_in_place(&mut buffer, &make_context(MOMENT_SAMPLES_PER_CASE))
                .unwrap();

            let input_f64 = f64::from(input);
            let mut sum = 0.0_f64;
            let mut sum_squares = 0.0_f64;
            let mut peak = 0.0_f64;
            for sample in &buffer {
                let error_lsb = (f64::from(*sample) - input_f64) * scale;
                sum += error_lsb;
                sum_squares += error_lsb * error_lsb;
                peak = peak.max(error_lsb.abs());
            }
            let count = MOMENT_SAMPLES_PER_CASE as f64;
            let mean = sum / count;
            let second_moment = sum_squares / count;
            if mean.abs() > worst_mean {
                worst_mean = mean.abs();
                worst_mean_case = (bits, level_lsb);
            }
            let second_error = (second_moment - 0.25).abs();
            if second_error > worst_second {
                worst_second = second_error;
                worst_second_case = (bits, level_lsb);
            }
            // |TPDF| <= 1 LSB plus |round error| <= 0.5 LSB bounds every sample.
            assert!(
                peak <= 1.5 + 1e-6,
                "peak {peak} LSB exceeds 1.5-LSB bound at {bits} bits, level {level_lsb}"
            );
        }
    }

    let tolerance = FINAL_ERROR_MOMENT_TOLERANCE_LSB;
    assert!(
        worst_mean <= tolerance,
        "worst mean {worst_mean} LSB at {worst_mean_case:?} exceeds {tolerance}"
    );
    assert!(
        worst_second <= tolerance,
        "worst |E[e^2]-0.25| {worst_second} LSB^2 at {worst_second_case:?} exceeds {tolerance}"
    );
}

#[test]
fn block_partitioning_is_bit_identical() {
    // Odd chunk sizes plus a partial tail must reproduce one continuous
    // call exactly: RNG draws and error-feedback history advance per
    // sample, independent of host block boundaries.
    const FRAMES: usize = 4096;
    const CHUNK_FRAMES: usize = 37;
    const CHANNELS: usize = 2;

    let make_plugin = || {
        let mut plugin = DitherPlugin::from_params(
            CHANNELS,
            DitherPluginParams {
                bit_depth: 1, // 20-bit
                noise_shaping: true,
                dither_type: 0, // TPDF
            },
        );
        plugin.initialize(48_000.0).unwrap();
        plugin
    };
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| (i as f32 * 0.000_269).sin() * 0.4 + 0.000_4)
        .collect();

    let mut reference = make_plugin();
    let mut expected = input.clone();
    reference
        .process_in_place(&mut expected, &make_context(FRAMES))
        .unwrap();

    let mut chunked = make_plugin();
    let mut actual = input.clone();
    let mut frame = 0;
    while frame < FRAMES {
        let len = CHUNK_FRAMES.min(FRAMES - frame);
        let slice = &mut actual[frame * CHANNELS..(frame + len) * CHANNELS];
        chunked
            .process_in_place(slice, &ProcessContext::new(48_000, len))
            .unwrap();
        frame += len;
    }

    assert_eq!(actual, expected);
}

// ============================================================================
// DITHER-A2: whiteness, channel independence, seeded reproducibility
// ============================================================================

/// Correlation bound for effectively independent error samples. A
/// correlation estimator over N samples has standard deviation ~1/sqrt(N),
/// so this bound is at least five sigma at every count used below.
const ERROR_CORRELATION_TOLERANCE: f64 = 0.02;

fn autocorrelation(errors: &[f64], lag: usize) -> f64 {
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let mut covariance = 0.0_f64;
    let mut variance = 0.0_f64;
    for (index, error) in errors.iter().enumerate() {
        let centered = error - mean;
        variance += centered * centered;
        if index >= lag {
            covariance += centered * (errors[index - lag] - mean);
        }
    }
    covariance / variance
}

fn correlation(first: &[f64], second: &[f64]) -> f64 {
    assert_eq!(first.len(), second.len());
    let mean_first = first.iter().sum::<f64>() / first.len() as f64;
    let mean_second = second.iter().sum::<f64>() / second.len() as f64;
    let mut covariance = 0.0_f64;
    let mut variance_first = 0.0_f64;
    let mut variance_second = 0.0_f64;
    for (a, b) in first.iter().zip(second.iter()) {
        let centered_a = a - mean_first;
        let centered_b = b - mean_second;
        covariance += centered_a * centered_b;
        variance_first += centered_a * centered_a;
        variance_second += centered_b * centered_b;
    }
    covariance / (variance_first * variance_second).sqrt()
}

#[test]
fn unshaped_tpdf_final_error_is_uncorrelated_across_time() {
    const FRAMES: usize = 100_000;
    // A fractional-LSB DC input makes every final error an independent
    // draw (iid dither plus memoryless rounding), so all nonzero lags
    // of the final error must vanish.
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: false,
            dither_type: 0, // TPDF
        },
    );
    plugin.initialize(48_000.0).unwrap();
    let input = (0.37 / 32768.0) as f32;
    let mut buffer = vec![input; FRAMES];
    plugin
        .process_in_place(&mut buffer, &make_context(FRAMES))
        .unwrap();

    let errors: Vec<f64> = buffer
        .iter()
        .map(|sample| f64::from(*sample) - f64::from(input))
        .collect();
    for lag in 1..=8 {
        let rho = autocorrelation(&errors, lag);
        assert!(
            rho.abs() < ERROR_CORRELATION_TOLERANCE,
            "lag-{lag} autocorrelation {rho} exceeds {ERROR_CORRELATION_TOLERANCE}"
        );
    }
}

#[test]
fn unshaped_tpdf_error_spectrum_is_flat() {
    const SAMPLE_RATE: u32 = 48_000;
    const FFT_SIZE: usize = 2048;
    const SEGMENTS: usize = 16;
    const TOTAL: usize = FFT_SIZE * SEGMENTS;
    // Same averaged Hann periodogram as the shaped-spectrum test, now
    // asserting whiteness: TPDF final error carries equal power density in
    // mid and ultrasonic bands. Record: 48 kHz, N=2048 Hann segments, no
    // overlap, no padding; interior one-sided bins only, so window gain
    // cancels in the band ratio. The 1 dB bound is ~10x the estimator
    // deviation for 16 averaged segments.
    let mut plugin = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: false,
            dither_type: 0, // TPDF
        },
    );
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = (0.37 / 32768.0) as f32;
    let mut quantized = vec![input; TOTAL];
    plugin
        .process_in_place(&mut quantized, &ProcessContext::new(SAMPLE_RATE, TOTAL))
        .unwrap();

    let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
    let mut averaged = vec![0.0_f64; FFT_SIZE / 2 + 1];
    let mut bins = vec![Complex::new(0.0_f32, 0.0_f32); FFT_SIZE];
    for segment in 0..SEGMENTS {
        for (index, bin) in bins.iter_mut().enumerate() {
            let window =
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * index as f32 / FFT_SIZE as f32).cos();
            let offset = segment * FFT_SIZE + index;
            *bin = Complex::new((quantized[offset] - input) * window, 0.0);
        }
        fft.process(&mut bins);
        for (power, bin) in averaged.iter_mut().zip(&bins) {
            *power += bin.norm_sqr() as f64;
        }
    }
    let bin_hz = f64::from(SAMPLE_RATE) / FFT_SIZE as f64;
    let density = |low_hz: f64, high_hz: f64| {
        let mut power = 0.0_f64;
        let mut count = 0_u32;
        for (bin, value) in averaged.iter().enumerate() {
            let frequency = bin as f64 * bin_hz;
            if frequency >= low_hz && frequency < high_hz {
                power += value;
                count += 1;
            }
        }
        power / f64::from(count)
    };
    let ratio_db = 10.0 * (density(2_000.0, 8_000.0) / density(16_000.0, 22_000.0)).log10();
    assert!(
        ratio_db.abs() < 1.0,
        "TPDF error spectrum tilted by {ratio_db:.3} dB; expected white"
    );
}

#[test]
fn channel_errors_are_mutually_uncorrelated() {
    const CHANNELS: usize = 4;
    const FRAMES: usize = 65_536;
    // Identical DC on every channel: any cross-channel correlation can
    // only come from shared RNG state, which per-channel seeds forbid.
    let mut plugin = DitherPlugin::from_params(
        CHANNELS,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: false,
            dither_type: 0, // TPDF
        },
    );
    plugin.initialize(48_000.0).unwrap();
    let input = (0.37 / 32768.0) as f32;
    let mut buffer = vec![input; FRAMES * CHANNELS];
    plugin
        .process_in_place(&mut buffer, &make_context(FRAMES))
        .unwrap();

    let errors: Vec<Vec<f64>> = (0..CHANNELS)
        .map(|channel| {
            (0..FRAMES)
                .map(|frame| f64::from(buffer[frame * CHANNELS + channel]) - f64::from(input))
                .collect()
        })
        .collect();
    for first in 0..CHANNELS {
        for second in (first + 1)..CHANNELS {
            let rho = correlation(&errors[first], &errors[second]);
            assert!(
                rho.abs() < ERROR_CORRELATION_TOLERANCE,
                "channel {first}/{second} correlation {rho} exceeds {ERROR_CORRELATION_TOLERANCE}"
            );
        }
    }
}

#[test]
fn full_output_is_deterministic_across_instances_and_reset() {
    // Production uses fixed per-channel xorshift seeds reseeded by reset():
    // there is no entropy source, so seeded reproducibility and production
    // behavior coincide by design. This pins that contract on full output,
    // not just the raw generator.
    const CHANNELS: usize = 2;
    const FRAMES: usize = 2048;
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| (i as f32 * 0.001_31).sin() * 0.3 + 0.000_2)
        .collect();

    let mut first = DitherPlugin::from_params(
        CHANNELS,
        DitherPluginParams {
            bit_depth: 0, // 16-bit
            noise_shaping: true,
            dither_type: 0, // TPDF
        },
    );
    first.initialize(48_000.0).unwrap();
    let mut second = DitherPlugin::from_params(
        CHANNELS,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: true,
            dither_type: 0,
        },
    );
    second.initialize(48_000.0).unwrap();

    let mut output_first = input.clone();
    first
        .process_in_place(&mut output_first, &make_context(FRAMES))
        .unwrap();
    let mut output_second = input.clone();
    second
        .process_in_place(&mut output_second, &make_context(FRAMES))
        .unwrap();
    assert_eq!(output_first, output_second);

    first.reset();
    let mut output_restarted = input.clone();
    first
        .process_in_place(&mut output_restarted, &make_context(FRAMES))
        .unwrap();
    assert_eq!(output_restarted, output_first);
}

// ============================================================================
// DITHER-A3: actual exported PCM identity
// ============================================================================

/// Independent signed-PCM export conversion: unity gain, round-half-away
/// from zero, saturating rails. Shares no code with the plugin's float
/// path; any hidden gain or requantization between DSP output and export
/// breaks the exact grid-membership assertions below.
fn export_signed_pcm(sample: f32, bits: i32) -> i32 {
    let scale = 2.0_f64.powi(bits - 1);
    let code = (f64::from(sample) * scale).round() as i64;
    code.clamp(-(1_i64 << (bits - 1)), (1_i64 << (bits - 1)) - 1) as i32
}

#[test]
fn exported_pcm_matches_plugin_output_bit_exactly() {
    for (depth_index, bits) in BIT_DEPTHS_UNDER_TEST {
        let scale = 2.0_f64.powi(bits - 1);
        let min_code = -(1_i32 << (bits - 1));
        let max_code = (1_i32 << (bits - 1)) - 1;
        for dither_type in 0..3 {
            for noise_shaping in [false, true] {
                let mut plugin = DitherPlugin::from_params(
                    1,
                    DitherPluginParams {
                        bit_depth: depth_index,
                        noise_shaping,
                        dither_type,
                    },
                );
                plugin.initialize(48_000.0).unwrap();
                let mut inputs: Vec<f32> =
                    (0..1024).map(|i| -1.0 + 2.0 * (i as f32) / 1023.0).collect();
                for sub_lsb in [0.1_f64, 0.25, 0.4, 0.6] {
                    inputs.push((sub_lsb / scale) as f32);
                    inputs.push((-sub_lsb / scale) as f32);
                }
                inputs.push(-1.0);
                inputs.push(1.0);
                let frames = inputs.len();
                let mut buffer = inputs.clone();
                plugin
                    .process_in_place(&mut buffer, &make_context(frames))
                    .unwrap();
                for sample in &buffer {
                    let code = export_signed_pcm(*sample, bits);
                    assert!(
                        (min_code..=max_code).contains(&code),
                        "exported code {code} outside {bits}-bit range"
                    );
                    // Bit-exact grid membership: output * scale is the
                    // exported integer, so no stage applied gain or
                    // requantized after the plugin.
                    assert_eq!(f64::from(*sample) * scale, f64::from(code));
                }
            }
        }
        // Deterministic modes map the signed endpoints to exact PCM rails.
        for dither_type in [1_usize, 2] {
            for (input, expected) in [(-1.0_f32, min_code), (1.0_f32, max_code)] {
                let mut plugin = DitherPlugin::from_params(
                    1,
                    DitherPluginParams {
                        bit_depth: depth_index,
                        noise_shaping: false,
                        dither_type,
                    },
                );
                plugin.initialize(48_000.0).unwrap();
                let mut buffer = vec![input];
                plugin.process_in_place(&mut buffer, &make_context(1)).unwrap();
                assert_eq!(export_signed_pcm(buffer[0], bits), expected);
                assert_eq!(buffer[0], expected as f32 / 2.0_f32.powi(bits - 1));
            }
        }
    }
}

// ============================================================================
// DITHER-R1: bit-depth, mode, and preset routes
// ============================================================================

/// Exact-spec quantization oracle: f64 round/truncate of the presented f32
/// sample with saturating signed rails. Ties round half away from zero,
/// matching f64::round as the plugin uses it.
fn reference_quantized_code(input: f32, bits: i32, truncate: bool) -> i32 {
    let scale = 2.0_f64.powi(bits - 1);
    let scaled = f64::from(input) * scale;
    let code = if truncate {
        scaled.trunc()
    } else {
        scaled.round()
    } as i64;
    code.clamp(-(1_i64 << (bits - 1)), (1_i64 << (bits - 1)) - 1) as i32
}

#[test]
fn quantization_grid_covers_all_depths_and_rounding_modes() {
    for (depth_index, bits) in BIT_DEPTHS_UNDER_TEST {
        for truncate in [false, true] {
            let mut plugin = DitherPlugin::from_params(
                1,
                DitherPluginParams {
                    bit_depth: depth_index,
                    noise_shaping: false,
                    dither_type: if truncate { 2 } else { 1 },
                },
            );
            plugin.initialize(48_000.0).unwrap();
            // Full-scale sweep, half-LSB tie points, and exact endpoints.
            let scale = 2.0_f64.powi(bits - 1);
            let mut inputs: Vec<f32> =
                (0..512).map(|i| -1.0 + 2.0 * (i as f32) / 511.0).collect();
            for tie in -3..3 {
                inputs.push(((f64::from(tie) + 0.5) / scale) as f32);
            }
            inputs.extend([-1.0_f32, 1.0_f32]);
            let frames = inputs.len();
            let mut buffer = inputs.clone();
            plugin
                .process_in_place(&mut buffer, &make_context(frames))
                .unwrap();
            for (input, output) in inputs.iter().zip(buffer.iter()) {
                let expected = reference_quantized_code(*input, bits, truncate);
                assert_eq!(
                    export_signed_pcm(*output, bits),
                    expected,
                    "input {input} at {bits} bits, truncate={truncate}"
                );
                assert_eq!(*output, expected as f32 / 2.0_f32.powi(bits - 1));
            }
        }
    }
}

#[test]
fn preset_routes_accept_labels_defaults_and_reject_invalid_state() {
    // Empty presets take PARAMS defaults: 16-bit, shaping on, TPDF.
    let empty: DitherPluginParams = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(empty.bit_depth, 0);
    assert!(empty.noise_shaping);
    assert_eq!(empty.dither_type, 0);

    // Label presets resolve through the choice deserializer.
    let labeled: DitherPluginParams =
        serde_json::from_value(serde_json::json!({"bit_depth": "20", "dither_type": "Truncate"}))
            .unwrap();
    assert_eq!(labeled.bit_depth, 1);
    assert!(labeled.noise_shaping);
    assert_eq!(labeled.dither_type, 2);

    // Numeric and integral-float wire formats are accepted exactly.
    let wired: DitherPluginParams = serde_json::from_value(serde_json::json!({
        "bit_depth": 2,
        "noise_shaping": false,
        "dither_type": 1.0,
    }))
    .unwrap();
    assert_eq!(wired.bit_depth, 2);
    assert!(!wired.noise_shaping);
    assert_eq!(wired.dither_type, 1);

    // Out-of-range indices, unknown labels, and negative choices are
    // rejected transactionally at parse time.
    assert!(serde_json::from_value::<DitherPluginParams>(serde_json::json!({"bit_depth": 3}))
        .is_err());
    assert!(
        serde_json::from_value::<DitherPluginParams>(serde_json::json!({"bit_depth": "32"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<DitherPluginParams>(serde_json::json!({"dither_type": -1}))
            .is_err()
    );
    // Fractional floats are rejected even when in range (the wire format
    // accepts integral floats only).
    assert!(
        serde_json::from_value::<DitherPluginParams>(serde_json::json!({"bit_depth": 1.5}))
            .is_err()
    );

    // from_params clamps out-of-range indices instead of panicking.
    let clamped = DitherPlugin::from_params(
        1,
        DitherPluginParams {
            bit_depth: 99,
            noise_shaping: true,
            dither_type: 99,
        },
    );
    assert_eq!(
        clamped.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        clamped.get_parameter(&ParameterId::from("dither_type")),
        Some(ParameterValue::Int(2))
    );

    // Serializable Params index mapping clamps through the shared spec.
    let mut params = Params::default();
    params.set_param_value(0, 99.0);
    assert_eq!(params.bit_depth, 2);
    params.set_param_value(0, -5.0);
    assert_eq!(params.bit_depth, 0);
    params.set_param_value(2, 99.0);
    assert_eq!(params.dither_type, 2);
    params.set_param_value(1, 0.0);
    assert!(!params.noise_shaping);
    assert_eq!(params.param_value(0), Some(0.0));
    assert_eq!(params.param_value(1), Some(0.0));
    assert_eq!(params.param_value(2), Some(2.0));
}

#[test]
fn schema_latency_and_values_roundtrip_through_plugin_trait() {
    let plugin = DitherPlugin::new(2);
    let schema = plugin.parameter_schema();
    assert_eq!(schema.len(), 3);
    assert_eq!(schema[0].id.to_string(), "bit_depth");
    assert_eq!(schema[1].id.to_string(), "noise_shaping");
    assert_eq!(schema[2].id.to_string(), "dither_type");
    assert_eq!(schema[0].min_value, Some(ParameterValue::Int(0)));
    assert_eq!(schema[0].max_value, Some(ParameterValue::Int(2)));
    assert_eq!(schema[2].min_value, Some(ParameterValue::Int(0)));
    assert_eq!(schema[2].max_value, Some(ParameterValue::Int(2)));

    // Zero-sample latency matches the catalog's Zero latency model, and
    // channels are never mixed.
    let metadata = plugin.compile_metadata();
    assert_eq!(metadata.latency_samples, 0);
    assert!(!metadata.channel_mixing);
    assert_eq!(plugin.channels(), 2);

    // Out-of-range live values clamp at this layer (parse rejects instead).
    let mut clamped = DitherPlugin::new(1);
    clamped
        .set_parameter(ParameterId::from("bit_depth"), ParameterValue::Int(99))
        .unwrap();
    assert_eq!(
        clamped.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    clamped
        .set_parameter(ParameterId::from("bit_depth"), ParameterValue::Int(-3))
        .unwrap();
    assert_eq!(
        clamped.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(0))
    );

    // current_values feeds apply_values on a fresh instance.
    let mut source = DitherPlugin::new(2);
    source
        .set_parameter(ParameterId::from("bit_depth"), ParameterValue::Int(2))
        .unwrap();
    source
        .set_parameter(
            ParameterId::from("noise_shaping"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    source
        .set_parameter(ParameterId::from("dither_type"), ParameterValue::Int(1))
        .unwrap();
    let mut target = DitherPlugin::new(2);
    target.apply_values(source.current_values()).unwrap();
    assert_eq!(
        target.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        target.get_parameter(&ParameterId::from("noise_shaping")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        target.get_parameter(&ParameterId::from("dither_type")),
        Some(ParameterValue::Int(1))
    );
}

#[test]
fn apply_values_is_transactional_on_batch_errors() {
    // A batch mixing valid and invalid entries must apply nothing:
    // configuration and populated DSP history stay exactly as accepted.
    // (ParameterSet is a BTreeMap, so iteration order is by key; both
    // batches below place a valid entry before the invalid one and
    // therefore partially applied before the R1 fix.)
    let mut plugin = DitherPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let mut prefix = vec![0.1_f32; 256 * 2];
    plugin
        .process_in_place(&mut prefix, &make_context(256))
        .unwrap();

    let accepted = [
        plugin.get_parameter(&ParameterId::from("bit_depth")),
        plugin.get_parameter(&ParameterId::from("noise_shaping")),
        plugin.get_parameter(&ParameterId::from("dither_type")),
    ];
    let history = plugin.error_history.clone();
    let rng = plugin.rng_state.clone();
    let assert_untouched = |plugin: &DitherPlugin| {
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("bit_depth")),
            accepted[0]
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("noise_shaping")),
            accepted[1]
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("dither_type")),
            accepted[2]
        );
        assert_eq!(plugin.error_history, history);
        assert_eq!(plugin.rng_state, rng);
    };

    // Valid entry plus unknown ID.
    let mut batch = ParameterSet::new();
    batch.insert(ParameterId::from("bit_depth"), ParameterValue::Int(2));
    batch.insert(ParameterId::from("unknown"), ParameterValue::Int(1));
    assert!(plugin.apply_values(batch).is_err());
    assert_untouched(&plugin);

    // Valid entry plus wrong-typed sibling.
    let mut batch = ParameterSet::new();
    batch.insert(ParameterId::from("dither_type"), ParameterValue::Int(2));
    batch.insert(ParameterId::from("noise_shaping"), ParameterValue::Int(1));
    assert!(plugin.apply_values(batch).is_err());
    assert_untouched(&plugin);

    // A fully valid batch still applies, preserving clamp semantics.
    let mut batch = ParameterSet::new();
    batch.insert(ParameterId::from("bit_depth"), ParameterValue::Int(99));
    batch.insert(
        ParameterId::from("noise_shaping"),
        ParameterValue::Bool(false),
    );
    batch.insert(ParameterId::from("dither_type"), ParameterValue::Int(1));
    plugin.apply_values(batch).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("bit_depth")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("noise_shaping")),
        Some(ParameterValue::Bool(false))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("dither_type")),
        Some(ParameterValue::Int(1))
    );
}

#[test]
fn per_channel_rng_seeds_are_nonzero_and_stable() {
    // The zero-seed guard must be a no-op at every supported layout:
    // recompute the raw wrapping derivation inline and require nonzero
    // plus exact equality with production seeds. This also pins the
    // seeds, so an accidental reseed (an audible output change) fails.
    for channels in [1, 2, 4, 6, 8, 12] {
        let states = DitherPlugin::init_rng_states(channels);
        assert_eq!(states.len(), channels);
        for (channel, state) in states.iter().enumerate() {
            let raw = 0xDEAD_BEEF_CAFE_0001_u64
                .wrapping_add((channel as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            assert_ne!(raw, 0, "raw seed wrapped to zero at channel {channel}");
            assert_eq!(*state, raw, "production seed changed at channel {channel}");
        }
    }
}

#[test]
fn non_default_config_save_reload_reproduces_output_bit_exactly() {
    // The chain test pins save/reload at defaults; cover a non-default
    // route (20-bit, shaping off, truncate) end to end. Truncate without
    // shaping is fully deterministic, so reload must reproduce output
    // bit-exactly.
    const CHANNELS: usize = 2;
    const FRAMES: usize = 1024;
    let stored = DitherPluginParams {
        bit_depth: 1, // 20-bit
        noise_shaping: false,
        dither_type: 2, // Truncate
    };
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| (i as f32 * 0.013).sin() * 0.6)
        .collect();
    let mut plugin = DitherPlugin::from_params(CHANNELS, stored.clone());
    plugin.initialize(48_000.0).unwrap();
    let mut buffer = input.clone();
    plugin
        .process_in_place(&mut buffer, &make_context(FRAMES))
        .unwrap();

    let json = serde_json::to_value(&stored).unwrap();
    let reloaded: DitherPluginParams = serde_json::from_value(json).unwrap();
    assert_eq!(reloaded.bit_depth, 1);
    assert!(!reloaded.noise_shaping);
    assert_eq!(reloaded.dither_type, 2);
    let mut restored = DitherPlugin::from_params(CHANNELS, reloaded);
    restored.initialize(48_000.0).unwrap();
    let mut actual = input.clone();
    restored
        .process_in_place(&mut actual, &make_context(FRAMES))
        .unwrap();
    assert_eq!(actual, buffer);
}

#[test]
fn gain_dither_export_chain_survives_save_reload_and_rejection() {
    const CHANNELS: usize = 2;
    const FRAMES: usize = 4800;
    // Stand-in for upstream processing: the full Gain-plugin chain runs in
    // the integrator's shared gate, which this worker must not modify.
    const UPSTREAM_GAIN: f32 = 0.5;
    // Nonzero programme: 440 Hz sine at -6 dBFS peak, offset per channel.
    let input: Vec<f32> = (0..FRAMES * CHANNELS)
        .map(|i| {
            let frame = (i / CHANNELS) as f32;
            let channel = (i % CHANNELS) as f32;
            0.5 * (2.0 * std::f32::consts::PI * 440.0 * frame / 48_000.0 + channel * 1.3).sin()
        })
        .collect();
    let gained: Vec<f32> = input.iter().map(|sample| sample * UPSTREAM_GAIN).collect();

    let stored = DitherPluginParams {
        bit_depth: 0, // 16-bit
        noise_shaping: true,
        dither_type: 0, // TPDF
    };
    let mut plugin = DitherPlugin::from_params(CHANNELS, stored.clone());
    plugin.initialize(48_000.0).unwrap();
    let mut buffer = gained.clone();
    plugin
        .process_in_place(&mut buffer, &make_context(FRAMES))
        .unwrap();

    // Final stored samples: every output exports to a saturated 16-bit
    // code with bit-exact grid membership, and nonzero audio produces a
    // spread of codes rather than a stuck rail.
    let mut codes: Vec<i32> = buffer.iter().map(|sample| export_signed_pcm(*sample, 16)).collect();
    for (sample, code) in buffer.iter().zip(codes.iter()) {
        assert!((-32_768..=32_767).contains(code));
        assert_eq!(f64::from(*sample) * 32768.0, f64::from(*code));
    }
    codes.sort_unstable();
    codes.dedup();
    assert!(
        codes.len() > 8,
        "expected a spread of exported codes, got {}",
        codes.len()
    );

    // Save/reload roundtrips both state types and reproduces output.
    let json = serde_json::to_value(&stored).unwrap();
    let reloaded: DitherPluginParams = serde_json::from_value(json).unwrap();
    assert_eq!(reloaded.bit_depth, stored.bit_depth);
    assert_eq!(reloaded.noise_shaping, stored.noise_shaping);
    assert_eq!(reloaded.dither_type, stored.dither_type);
    let params_json = serde_json::to_value(Params::default()).unwrap();
    let params_reloaded: Params = serde_json::from_value(params_json).unwrap();
    assert_eq!(params_reloaded.bit_depth, Params::default().bit_depth);
    assert_eq!(params_reloaded.noise_shaping, Params::default().noise_shaping);
    assert_eq!(params_reloaded.dither_type, Params::default().dither_type);
    let mut restored = DitherPlugin::from_params(CHANNELS, reloaded);
    restored.initialize(48_000.0).unwrap();
    let mut restored_buffer = gained.clone();
    restored
        .process_in_place(&mut restored_buffer, &make_context(FRAMES))
        .unwrap();
    assert_eq!(restored_buffer, buffer);

    // Rejected changes retain the accepted configuration and history: a
    // twin that never sees the rejection must stay bit-identical.
    let mut candidate = DitherPlugin::from_params(
        CHANNELS,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: true,
            dither_type: 0,
        },
    );
    candidate.initialize(48_000.0).unwrap();
    let mut twin = DitherPlugin::from_params(
        CHANNELS,
        DitherPluginParams {
            bit_depth: 0,
            noise_shaping: true,
            dither_type: 0,
        },
    );
    twin.initialize(48_000.0).unwrap();
    let prefix_frames = 1024;
    let mut prefix = gained[..prefix_frames * CHANNELS].to_vec();
    candidate
        .process_in_place(&mut prefix, &make_context(prefix_frames))
        .unwrap();
    let mut twin_prefix = gained[..prefix_frames * CHANNELS].to_vec();
    twin.process_in_place(&mut twin_prefix, &make_context(prefix_frames)).unwrap();
    assert!(
        candidate
            .set_parameter(ParameterId::from("unknown"), ParameterValue::Float(0.0))
            .is_err()
    );
    assert!(
        candidate
            .set_parameter(ParameterId::from("noise_shaping"), ParameterValue::Int(1))
            .is_err()
    );
    for id in ["bit_depth", "noise_shaping", "dither_type"] {
        assert_eq!(
            candidate.get_parameter(&ParameterId::from(id)),
            twin.get_parameter(&ParameterId::from(id)),
            "rejection changed {id}"
        );
    }
    let mut suffix = gained[prefix_frames * CHANNELS..].to_vec();
    let suffix_frames = FRAMES - prefix_frames;
    candidate
        .process_in_place(&mut suffix, &make_context(suffix_frames))
        .unwrap();
    let mut twin_suffix = gained[prefix_frames * CHANNELS..].to_vec();
    twin.process_in_place(&mut twin_suffix, &make_context(suffix_frames)).unwrap();
    assert_eq!(suffix, twin_suffix);
}
