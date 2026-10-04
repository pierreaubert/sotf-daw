use super::de_esser_plugin::DeEsserPlugin;
use super::types::DeEsserPluginParams;
use crate::DeEsserData;
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::plugin::TailLength;

fn make_sine(freq_hz: f32, sample_rate: u32, num_frames: usize, amplitude: f32) -> Vec<f32> {
    (0..num_frames)
        .map(|i| {
            amplitude * (2.0 * std::f32::consts::PI * freq_hz * i as f32 / sample_rate as f32).sin()
        })
        .collect()
}

fn rms(buf: &[f32]) -> f32 {
    let sum: f32 = buf.iter().map(|x| x * x).sum();
    (sum / buf.len() as f32).sqrt()
}

#[test]
fn test_de_esser_reduces_sibilance() {
    let sr = 48000u32;
    let num_frames = 48000; // 1 second
    let amplitude = 0.5;

    let mut plugin = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 8000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    // 8kHz sine in the sibilance range
    let mut buf = make_sine(8000.0, sr, num_frames, amplitude);
    let input_rms = rms(&buf);

    let ctx = ProcessContext::new(sr, num_frames);
    plugin.process_in_place(&mut buf, &ctx).unwrap();

    // Use the second half to allow attack to settle
    let output_rms = rms(&buf[num_frames / 2..]);

    // Output should be significantly quieter
    assert!(
        output_rms < input_rms * 0.5,
        "8kHz signal should be reduced: input_rms={:.4}, output_rms={:.4}",
        input_rms,
        output_rms
    );
}

#[test]
fn test_wideband_reduction_is_channel_specific() {
    let sr = 48000u32;
    let num_frames = 48000; // 1 second
    let sample_count = num_frames * 2;
    let amplitude = 0.5;

    let mut plugin = DeEsserPlugin::from_params(
        2,
        DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -35.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    let mut buf = Vec::with_capacity(sample_count);
    let mut low_input = Vec::with_capacity(num_frames);
    let mut high_input = Vec::with_capacity(num_frames);
    for i in 0..num_frames {
        let low = amplitude * (2.0 * std::f32::consts::PI * 200.0 * i as f32 / sr as f32).sin();
        let high = amplitude * (2.0 * std::f32::consts::PI * 8000.0 * i as f32 / sr as f32).sin();
        buf.push(low);
        buf.push(high);
        low_input.push(low);
        high_input.push(high);
    }

    let input_low_rms = rms(&low_input);
    let input_high_rms = rms(&high_input);

    let ctx = ProcessContext::new(sr, num_frames);
    plugin.process_in_place(&mut buf, &ctx).unwrap();

    let mut low_output = Vec::with_capacity(num_frames);
    let mut high_output = Vec::with_capacity(num_frames);
    for frame in 0..num_frames {
        low_output.push(buf[frame * 2]);
        high_output.push(buf[frame * 2 + 1]);
    }

    let output_low_rms = rms(&low_output);
    let output_high_rms = rms(&high_output);

    assert!(
        output_low_rms > input_low_rms * 0.9,
        "Low band should remain mostly untouched: input={:.4}, output={:.4}",
        input_low_rms,
        output_low_rms
    );
    assert!(
        output_high_rms < input_high_rms * 0.7,
        "High band should be reduced by sidechain: input={:.4}, output={:.4}",
        input_high_rms,
        output_high_rms
    );
    assert!(
        plugin.monitoring_gr[0].is_finite() && plugin.monitoring_gr[1].is_finite(),
        "Monitoring values should remain finite after processing.",
    );
}

#[test]
fn test_de_esser_passes_low_frequencies() {
    let sr = 48000u32;
    let num_frames = 48000; // 1 second
    let amplitude = 0.5;

    let mut plugin = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    // 200Hz sine — well below detection range
    let mut buf = make_sine(200.0, sr, num_frames, amplitude);
    let input_rms = rms(&buf);

    let ctx = ProcessContext::new(sr, num_frames);
    plugin.process_in_place(&mut buf, &ctx).unwrap();

    let output_rms = rms(&buf[num_frames / 2..]);

    // Low-frequency signal should pass through mostly unchanged
    assert!(
        output_rms > input_rms * 0.9,
        "200Hz signal should pass through: input_rms={:.4}, output_rms={:.4}",
        input_rms,
        output_rms
    );
}

#[test]
fn test_de_esser_parameter_set_get() {
    let mut plugin = DeEsserPlugin::new(2);
    plugin.initialize(48000.0).unwrap();

    // Set threshold
    plugin
        .parametric_set_parameter(ParameterId::from("threshold"), ParameterValue::Float(-30.0))
        .unwrap();
    let val = plugin.parametric_get_parameter(&ParameterId::from("threshold"));
    assert_eq!(val, Some(ParameterValue::Float(-30.0)));

    // Set mix
    plugin
        .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.5))
        .unwrap();
    let val = plugin.parametric_get_parameter(&ParameterId::from("mix"));
    assert_eq!(val, Some(ParameterValue::Float(0.5)));
}

/// Verify that the mix smoother advances per-sample during a block, not as a
/// block-constant value. If `next_n(num_frames)` were used (old code), the
/// smoother would jump to its target on the first block and the first sample
/// would already be at the target. With per-sample `advance()`, the value
/// ramps smoothly: the very first sample is close to the *starting* value,
/// not the target value.
#[test]
fn test_mix_smoother_ramps_per_sample() {
    let sr = 48000u32;
    // Start mix at 0 (dry)
    let mut plugin = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 0.0, // fully dry initially
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    // Now request mix = 1.0 (fully wet). The smoother has a 5 ms ramp.
    plugin
        .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
        .unwrap();

    // A silent input: output should also be silent regardless of mix
    // Use a 1 kHz tone instead so we can measure dry-vs-wet differences.
    // Use a 100-sample block — well within the 5 ms ramp (~240 samples at 48 kHz).
    let num_frames = 100;
    let mut buf: Vec<f32> = (0..num_frames)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr as f32).sin())
        .collect();

    // Capture the first sample's input value
    let first_input = buf[0];

    let ctx = ProcessContext::new(sr, num_frames);
    plugin.process_in_place(&mut buf, &ctx).unwrap();

    // With mix=0 at block start and a 5ms ramp, after only 100 samples
    // (~2ms) the smoother should still be far below 1.0. The first output
    // sample must still be close to dry (= input * gain).
    // Specifically: if the smoother was block-constant, it would jump to ~1.0
    // and the first output would be purely wet. If it ramps per-sample,
    // the first output should be much closer to the dry value.
    //
    // We assert that the smoother has NOT jumped all the way to fully wet
    // on the first sample: the first output must not equal the fully-wet value.
    //
    // For a 1 kHz sine at mix=0 (dry), the output is approximately input*gain.
    // For mix=1 (wet), at this threshold the 1kHz tone is below the detection
    // range so gain≈1 and wet≈input. The meaningful test is therefore to
    // observe that the mix value at sample 0 is near 0, not near 1.
    //
    // We do this indirectly: set mix from 0 to 1 and verify the per-sample
    // smoother current value starts near 0. We read back the smoother
    // state by checking it hasn't already converged in 100 samples.
    // At 48kHz with a 5ms ramp, coeff = exp(-1/(0.005*48000)) = exp(-1/240) ≈ 0.9958.
    // After 100 samples: value ≈ 1 - 0.9958^100 * 1 ≈ 1 - 0.665 = 0.335.
    // The block-constant version would give 1 - 0.9958^100 ≈ 0.335 at the END
    // of block but apply that single value as-if the whole block ran at 0.335.
    // The per-sample version truly ramps 0..0.335 across the 100 samples.
    //
    // A simpler check: the first output sample should NOT be at full wet.
    // At the first sample, mix is approximately 0 (start value). So output[0]
    // should be very close to input[0] (dry) rather than whatever wet[0] would be.
    // Since there is no gain reduction yet (envelope not triggered), wet = input,
    // so dry ≈ wet in this case and the test is degenerate. Instead we verify
    // the smoother stays monotone: use a plugin with 0 threshold so gain ≈ 0 (heavy).
    let _ = first_input; // suppress unused warning

    // --- New approach: heavy compression so wet != dry ---
    let mut plugin2 = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 2000.0, // lower contract edge and test-tone center
            q: 0.5,            // wide bandwidth around the test tone
            threshold: -60.0,  // extremely low threshold → heavy compression
            ratio: 20.0,       // max ratio → near total gain kill
            attack_ms: 0.1,    // fast attack
            release_ms: 200.0,
            mode: "Wideband".to_string(),
            mix: 0.0, // start dry
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin2.initialize(f64::from(sr)).unwrap();

    // Ramp to fully wet over 5ms
    plugin2
        .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(1.0))
        .unwrap();

    // One block of 100 samples — still in the ramp window
    let mut buf2: Vec<f32> = (0..num_frames)
        .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / sr as f32).sin())
        .collect();
    let dry_ref = buf2.clone(); // original input (dry output when mix=0)

    plugin2.process_in_place(&mut buf2, &ctx).unwrap();

    // Wet output (after heavy GR) should be near-silent. Dry output = original.
    // With per-sample ramp starting at mix=0, the first samples lean dry.
    // With block-constant mix, the whole block has mix≈0.335 (already ramped).
    //
    // The first sample of buf2 should be between dry_ref[0] (when mix≈0)
    // and near-zero (when mix≈1 and gain≈0). It must not equal dry_ref[0]
    // exactly (some ramp happened) but must not be at 0 either.
    //
    // Most importantly: the output must NOT be identical to the full-wet result
    // for the entire block. We verify at least the first sample has nonzero
    // dry component.
    let first_out = buf2[0];
    let first_dry = dry_ref[0];
    // If the smoother started truly at 0 and ramped, the first sample is
    // output = 0 * wet + 1 * dry = dry (approximately, mix≈0 at t=0).
    // Allow a small tolerance since one-pole starts advancing immediately.
    assert!(
        (first_out - first_dry).abs() < first_dry.abs() * 0.2 + 1e-4,
        "First output sample should be near dry (mix≈0 at t=0): \
             first_out={:.6}, first_dry={:.6}",
        first_out,
        first_dry
    );
}

#[test]
fn test_split_band_mode() {
    let sr = 48000u32;
    let num_frames = 48000; // 1 second
    let amplitude = 0.5;

    let mut plugin = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Split-Band".to_string(),
            mix: 1.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    // --- Test that HF is attenuated ---
    let mut buf_hf = make_sine(8000.0, sr, num_frames, amplitude);
    let input_rms_hf = rms(&buf_hf);

    let ctx = ProcessContext::new(sr, num_frames);
    plugin.process_in_place(&mut buf_hf, &ctx).unwrap();
    let output_rms_hf = rms(&buf_hf[num_frames / 2..]);

    assert!(
        output_rms_hf < input_rms_hf * 0.7,
        "Split-band: 8kHz should be reduced: input={:.4}, output={:.4}",
        input_rms_hf,
        output_rms_hf
    );

    // --- Test that LF passes through ---
    plugin.reset();
    let mut buf_lf = make_sine(200.0, sr, num_frames, amplitude);
    let input_rms_lf = rms(&buf_lf);

    plugin.process_in_place(&mut buf_lf, &ctx).unwrap();
    let output_rms_lf = rms(&buf_lf[num_frames / 2..]);

    assert!(
        output_rms_lf > input_rms_lf * 0.85,
        "Split-band: 200Hz should pass through: input={:.4}, output={:.4}",
        input_rms_lf,
        output_rms_lf
    );
}

// -------------------------------------------------------------------------
// set_parameter smoke tests and edge cases
// -------------------------------------------------------------------------

#[test]
fn test_set_parameter_all_float_params_roundtrip() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    let cases: &[(&str, f32)] = &[
        ("threshold", -30.0),
        ("ratio", 8.0),
        ("attack", 2.0),
        ("release", 50.0),
        ("mix", 0.25),
    ];

    for &(id, value) in cases {
        plugin
            .parametric_set_parameter(ParameterId::from(id), ParameterValue::Float(value))
            .unwrap();
        let got = plugin.parametric_get_parameter(&ParameterId::from(id));
        assert_eq!(
            got,
            Some(ParameterValue::Float(value)),
            "roundtrip failed for {}",
            id
        );
    }
}

#[test]
fn test_set_parameter_out_of_bounds_returns_error() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    // Frequency range [2000, 16000]
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("frequency"), ParameterValue::Float(100.0))
            .is_err()
    );
    assert!(
        plugin
            .parametric_set_parameter(
                ParameterId::from("frequency"),
                ParameterValue::Float(20000.0)
            )
            .is_err()
    );

    // Q range [0.5, 5.0]
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("q"), ParameterValue::Float(0.1))
            .is_err()
    );

    // Threshold range [-60, 0]
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("threshold"), ParameterValue::Float(5.0))
            .is_err()
    );

    // Mix range [0, 1]
    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(-0.1))
            .is_err()
    );
}

#[test]
fn test_set_parameter_nan_returns_error() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    assert!(
        plugin
            .parametric_set_parameter(
                ParameterId::from("frequency"),
                ParameterValue::Float(f32::NAN)
            )
            .is_err()
    );
    assert!(
        plugin
            .parametric_set_parameter(
                ParameterId::from("threshold"),
                ParameterValue::Float(f32::NAN)
            )
            .is_err()
    );
}

#[test]
fn test_set_parameter_unknown_id_returns_error() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    assert!(
        plugin
            .parametric_set_parameter(ParameterId::from("not_a_param"), ParameterValue::Float(1.0))
            .is_err()
    );
}

#[test]
fn test_set_parameter_mode_variants() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("mode"),
            ParameterValue::String("Wideband".to_string()),
        )
        .unwrap_err();
    assert!(error.contains("structural"));
    assert_eq!(plugin.mode_index, 1);

    plugin
        .parametric_set_parameter(
            ParameterId::from("mode"),
            ParameterValue::String("Split-Band".to_string()),
        )
        .unwrap();
    assert_eq!(plugin.mode_index, 1);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::String("Split-Band".to_string()))
    );
}

#[test]
fn from_params_rejects_out_of_range_values() {
    let result = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 100.0, // below min
            q: 10.0,          // above max
            threshold: 10.0,  // above max
            ratio: 0.5,       // below min
            attack_ms: 0.01,  // below min
            release_ms: 1.0,  // below min
            mode: "Wideband".to_string(),
            mix: -1.0, // below min
            ..Default::default()
        },
    );
    assert!(result.is_err(), "invalid serialized state must be rejected");
}

#[test]
fn test_process_empty_buffer_returns_zero() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    let mut buf = vec![0.0f32; 0];
    let ctx = ProcessContext::new(48000, 0);
    let frames = plugin.process_in_place(&mut buf, &ctx).unwrap();
    assert_eq!(frames, 0);
}

#[test]
fn test_process_zero_channels_returns_num_frames() {
    assert!(DeEsserPlugin::try_from_params(0, DeEsserPluginParams::default()).is_err());
}

#[test]
fn test_fallible_constructor_rejects_invalid_values() {
    let invalid = DeEsserPluginParams {
        frequency: f32::NAN,
        ..Default::default()
    };
    assert!(DeEsserPlugin::try_from_params(1, invalid).is_err());
    let invalid = DeEsserPluginParams {
        mode: "Mystery".into(),
        ..Default::default()
    };
    assert!(DeEsserPlugin::try_from_params(1, invalid).is_err());
}

#[test]
fn test_short_buffer_returns_error() {
    let mut plugin = DeEsserPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let mut buffer = vec![0.0; 7];
    assert!(
        plugin
            .process_in_place(&mut buffer, &ProcessContext::new(48_000, 4))
            .is_err()
    );
}

#[test]
fn test_short_buffer_is_rejected_before_state_or_data_changes() {
    let mut rejected = DeEsserPlugin::new(2);
    let mut reference = DeEsserPlugin::new(2);
    rejected.initialize(48_000.0).unwrap();
    reference.initialize(48_000.0).unwrap();

    let mut short = vec![99.0_f32; 7];
    let error = rejected
        .process_in_place(&mut short, &ProcessContext::new(48_000, 4))
        .unwrap_err();
    assert!(error.contains("buffer too small"));
    assert!(short.iter().all(|sample| *sample == 99.0));

    let mut rejected_output = vec![0.1_f32; 8];
    let mut reference_output = rejected_output.clone();
    rejected
        .process_in_place(&mut rejected_output, &ProcessContext::new(48_000, 4))
        .unwrap();
    reference
        .process_in_place(&mut reference_output, &ProcessContext::new(48_000, 4))
        .unwrap();
    assert_eq!(rejected_output, reference_output);
}

#[test]
fn test_frame_channel_overflow_is_rejected_before_dsp() {
    let mut plugin = DeEsserPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let mut buffer = vec![0.25_f32; 8];
    let error = plugin
        .process_in_place(
            &mut buffer,
            &ProcessContext::new(48_000, usize::MAX / 2 + 1),
        )
        .unwrap_err();
    assert!(error.contains("sample count overflow"));
    assert!(buffer.iter().all(|sample| *sample == 0.25));
}

#[test]
fn test_monitoring_cache_vector_updates() {
    let mut data = DeEsserData::new(2);
    let snapshot = data.clone();
    data.update(&[3.0, 6.0]);
    assert_eq!(&*data.gain_reduction_db, &[3.0, 6.0]);
    assert_eq!(&*snapshot.gain_reduction_db, &[0.0, 0.0]);
}

#[test]
fn test_info_and_channels() {
    let plugin = DeEsserPlugin::new(2);
    assert_eq!(plugin.channels(), 2);
    let info = plugin.info();
    assert_eq!(info.name, "DeEsser");
}

#[test]
fn test_reset_clears_filter_state() {
    let sr = 48000u32;
    let mut plugin = DeEsserPlugin::from_params(
        1,
        DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ..Default::default()
        },
    )
    .expect("valid De-Esser parameters");
    plugin.initialize(f64::from(sr)).unwrap();

    let mut buf = make_sine(8000.0, sr, 4800, 0.5);
    let ctx = ProcessContext::new(sr, 4800);
    plugin.process_in_place(&mut buf, &ctx).unwrap();

    // After processing, filter state and cores have state
    plugin.reset();

    // Post-reset, processing a quiet signal should behave as at startup
    let mut buf2 = make_sine(200.0, sr, 4800, 0.5);
    let input_rms = rms(&buf2);
    plugin.process_in_place(&mut buf2, &ctx).unwrap();
    let output_rms = rms(&buf2);
    assert!(
        output_rms > input_rms * 0.9,
        "reset should restore LF pass-through behavior"
    );
}

// -------------------------------------------------------------------------
// set_parameter extended coverage
// -------------------------------------------------------------------------

#[test]
fn test_structural_q_update_requires_host_rebuild() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    let original_q = plugin.q;
    let error = plugin
        .parametric_set_parameter(ParameterId::from("q"), ParameterValue::Float(4.0))
        .unwrap_err();
    assert!(error.contains("structural"));
    assert_eq!(plugin.q, original_q);
}

#[test]
fn test_parameter_updates_reject_detector_band_at_initialized_nyquist() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(32_000.0).unwrap();
    let original_frequency = plugin.frequency;

    // 16 kHz is inside the serialized parameter range, but it is not a valid
    // detector center at this sample rate. The setter must reject it before
    // changing the stored value or rebuilding filters at/above Nyquist.
    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("frequency"),
            ParameterValue::Float(16_000.0),
        )
        .unwrap_err();
    assert!(error.contains("Nyquist"), "unexpected error: {error}");
    assert_eq!(plugin.frequency, original_frequency);
}

#[test]
fn test_set_parameter_attack_updates_cores() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    plugin
        .parametric_set_parameter(ParameterId::from("attack"), ParameterValue::Float(5.0))
        .unwrap();
    assert_eq!(plugin.attack_ms, 5.0);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("attack")),
        Some(ParameterValue::Float(5.0))
    );
}

#[test]
fn test_set_parameter_release_updates_cores() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    plugin
        .parametric_set_parameter(ParameterId::from("release"), ParameterValue::Float(100.0))
        .unwrap();
    assert_eq!(plugin.release_ms, 100.0);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("release")),
        Some(ParameterValue::Float(100.0))
    );
}

#[test]
fn test_initialize_different_sample_rate() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(44100.0).unwrap();
    assert_eq!(plugin.sample_rate, 44100.0);

    plugin.initialize(96000.0).unwrap();
    assert_eq!(plugin.sample_rate, 96000.0);
    // Filters and crossovers should have been rebuilt for the new rate without panic
}

#[test]
fn test_set_parameter_mode_unknown_string_is_rejected() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("mode"),
            ParameterValue::String("Unknown".to_string()),
        )
        .unwrap_err();
    assert!(error.contains("Unknown De-Esser mode"));
    assert_eq!(plugin.mode_index, 1);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("mode")),
        Some(ParameterValue::String("Split-Band".to_string()))
    );
}

#[test]
fn test_set_parameter_mix_updates_smoother_target() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48000.0).unwrap();

    plugin
        .parametric_set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.75))
        .unwrap();
    assert_eq!(plugin.mix, 0.75);
    assert!((plugin.mix_smoother.target() - 0.75).abs() < 1e-4);
}

#[test]
fn split_band_inactive_output_is_mix_invariant() {
    fn render(mix: f32) -> Vec<f32> {
        let params = DeEsserPluginParams {
            frequency: 7_000.0,
            q: 1.5,
            threshold: -30.0,
            ratio: 1.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Split-Band".into(),
            mix,
            ..Default::default()
        };
        let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, 48_000).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let mut signal: Vec<f32> = (0..16_384)
            .map(|frame| {
                0.2 * (std::f32::consts::TAU * 997.0 * frame as f32 / 48_000.0).sin()
                    + 0.2 * (std::f32::consts::TAU * 8_123.0 * frame as f32 / 48_000.0).sin()
            })
            .collect();
        plugin
            .process_in_place(&mut signal, &ProcessContext::new(48_000, 16_384))
            .unwrap();
        signal
    }

    let reference = render(0.0);
    for mix in [0.25, 0.5, 1.0] {
        let output = render(mix);
        let max_error = output
            .iter()
            .zip(&reference)
            .map(|(actual, expected)| (actual - expected).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_error < 1e-6,
            "inactive mix={mix} changed output by {max_error}"
        );
    }
}

#[test]
fn detector_q_defines_bandwidth_once_with_butterworth_poles() {
    for q in [0.5, 1.5, 5.0] {
        let params = DeEsserPluginParams {
            q,
            ..Default::default()
        };
        let plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, 48_000).unwrap();
        let (low, high) = DeEsserPlugin::bandpass_edges(plugin.frequency, q);
        assert!(low < plugin.frequency && high > plugin.frequency);
        assert!((plugin.hp_filters.q - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        assert!((plugin.lp_filters.q - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
    }
}

#[test]
fn structural_detector_controls_are_marked_and_transactional() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48_000.0).unwrap();
    for id in ["frequency", "q", "mode"] {
        let parameter = plugin
            .parameter_schema()
            .into_iter()
            .find(|parameter| parameter.id.as_str() == id)
            .unwrap();
        assert_eq!(parameter.update_mode, UpdateMode::Structural);
    }

    let old_frequency = plugin.frequency;
    let old_filter_frequency = plugin.hp_filters.freq;
    assert!(
        plugin
            .parametric_set_parameter(
                ParameterId::from("frequency"),
                ParameterValue::Float(8_000.0),
            )
            .unwrap_err()
            .contains("structural")
    );
    assert_eq!(plugin.frequency, old_frequency);
    assert_eq!(plugin.hp_filters.freq, old_filter_frequency);
}

#[test]
fn meter_cadence_depends_on_samples_not_callback_count() {
    fn counter_after(block: usize, total: usize) -> usize {
        let mut plugin = DeEsserPlugin::new(1);
        plugin.initialize(48_000.0).unwrap();
        let mut remaining = total;
        while remaining > 0 {
            let frames = remaining.min(block);
            let mut buffer = vec![0.0; frames];
            plugin
                .process_in_place(&mut buffer, &ProcessContext::new(48_000, frames))
                .unwrap();
            remaining -= frames;
        }
        plugin.cache_counter
    }

    assert_eq!(counter_after(32, 5_123), counter_after(1024, 5_123));
    assert_eq!(counter_after(4096, 5_123), 5_123 % 1_600);
}

#[test]
fn held_monitor_snapshot_does_not_freeze_future_publication() {
    let params = DeEsserPluginParams {
        frequency: 8_000.0,
        q: 1.5,
        threshold: -40.0,
        ratio: 20.0,
        attack_ms: 0.1,
        release_ms: 20.0,
        mode: "Wideband".into(),
        mix: 1.0,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, 48_000).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let held = plugin.get_data().unwrap();
    let mut signal = make_sine(8_000.0, 48_000, 4_800, 0.8);
    plugin
        .process_in_place(&mut signal, &ProcessContext::new(48_000, 4_800))
        .unwrap();
    let current = plugin
        .get_data()
        .unwrap()
        .downcast::<DeEsserData>()
        .unwrap();
    assert!(current.gain_reduction_db[0] > 1.0);
    let held = held.downcast::<DeEsserData>().unwrap();
    assert_eq!(held.gain_reduction_db[0], 0.0);
}

#[test]
fn non_finite_audio_is_sanitized_without_poisoning_state() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48_000.0).unwrap();
    let mut malformed = vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.25];
    plugin
        .process_in_place(&mut malformed, &ProcessContext::new(48_000, 4))
        .unwrap();
    assert!(malformed.iter().all(|sample| sample.is_finite()));
    let mut followup = make_sine(8_000.0, 48_000, 4_096, 0.25);
    plugin
        .process_in_place(&mut followup, &ProcessContext::new(48_000, 4_096))
        .unwrap();
    assert!(followup.iter().all(|sample| sample.is_finite()));
}

// -------------------------------------------------------------------------
// Audit: lookahead (R1), linear-phase split (R2), M/S + sidechain (R3),
// metadata (R4), independent accuracy (A1-A3)
// -------------------------------------------------------------------------

/// Unity de-esser: threshold 0 dB with ratio 1 gives zero gain reduction for
/// any input (zero slope), so the output is the pure delayed dry path.
fn unity_params() -> DeEsserPluginParams {
    DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: 0.0,
        ratio: 1.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        ..Default::default()
    }
}

fn max_abs_diff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0_f32, f32::max)
}

/// Render `input` through `plugin` in `block`-frame callbacks.
fn render_partitioned(
    plugin: &mut DeEsserPlugin,
    input: &[f32],
    channels: usize,
    sample_rate: u32,
    block: usize,
) -> Vec<f32> {
    let stride = plugin.input_channels();
    assert_eq!(input.len() % stride, 0);
    let total_frames = input.len() / stride;
    let mut output = input.to_vec();
    let mut done = 0;
    while done < total_frames {
        let frames = (total_frames - done).min(block);
        let range = done * stride..(done + frames) * stride;
        // Copy the input window into a scratch block so partial-block writes
        // cannot alias unread input.
        let mut scratch = output[range.clone()].to_vec();
        let processed = plugin
            .process_in_place(&mut scratch, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert_eq!(processed, frames);
        output[range].copy_from_slice(&scratch);
        done += frames;
    }
    // Program width equals channel count on direct calls; ext-key tests keep
    // the key region and compare full stride.
    let _ = channels;
    output
}

/// Independent standard soft-knee compressor curve in f64. The detector gain
/// offset is unknown to this reference, so law tests use differences of this
/// curve (offset-free) rather than absolute values.
fn static_compress_gr_f64(input_db: f64, threshold: f64, ratio: f64, knee_db: f64) -> f64 {
    let slope = 1.0 - 1.0 / ratio.max(1.0);
    if knee_db < 0.1 {
        if input_db <= threshold {
            0.0
        } else {
            (input_db - threshold) * slope
        }
    } else if input_db < threshold - knee_db / 2.0 {
        0.0
    } else if input_db > threshold + knee_db / 2.0 {
        (input_db - threshold) * slope
    } else {
        let overshoot = input_db - threshold + knee_db / 2.0;
        let kf = overshoot / knee_db;
        kf * kf * (knee_db / 2.0) * slope
    }
}

/// Independent f64 DTFT magnitude of published FIR taps. Uses only the tap
/// values and the standard Fourier sum; no plugin or math-crate filter code.
fn dtft_magnitude_db(taps: &[f32], freq_hz: f64, sample_rate: f64) -> f64 {
    let (mut real, mut imag) = (0.0f64, 0.0f64);
    for (k, &tap) in taps.iter().enumerate() {
        let phase = -2.0 * std::f64::consts::PI * freq_hz * k as f64 / sample_rate;
        real += tap as f64 * phase.cos();
        imag += tap as f64 * phase.sin();
    }
    20.0 * (real * real + imag * imag).sqrt().max(1e-12).log10()
}

/// Independent f64 DTFT magnitude of f64 taps: the textbook-reference sibling
/// of `dtft_magnitude_db`, sharing no code with the plugin or math crates.
fn dtft_magnitude_db_f64(taps: &[f64], freq_hz: f64, sample_rate: f64) -> f64 {
    let (mut real, mut imag) = (0.0f64, 0.0f64);
    for (k, &tap) in taps.iter().enumerate() {
        let phase = -2.0 * std::f64::consts::PI * freq_hz * k as f64 / sample_rate;
        real += tap * phase.cos();
        imag += tap * phase.sin();
    }
    20.0 * (real * real + imag * imag).sqrt().max(1e-12).log10()
}

/// Modified Bessel function of the first kind, order zero, from its power
/// series (Abramowitz & Stegun 9.6.10): I0(x) = sum_{k>=0} (x^2/4)^k/(k!)^2.
/// Converges to f64 precision in about 25 terms for beta <= 8.
fn bessel_i0_f64(x: f64) -> f64 {
    let mut sum = 1.0f64;
    let mut term = 1.0f64;
    let z = (x * x) / 4.0;
    let mut k = 1.0f64;
    while term > 1e-16 * sum.max(1.0) {
        term *= z / (k * k);
        sum += term;
        k += 1.0;
    }
    sum
}

/// Independent textbook Kaiser windowed-sinc lowpass design in f64
/// (Oppenheim & Schafer, Discrete-Time Signal Processing): the ideal sinc
/// impulse response times a Kaiser window with the published beta,
/// normalized to unit DC gain. Shares no code with the math-crate `Fir`
/// designer; only the published design parameters (cutoff, rate, tap count,
/// Kaiser beta 8) are inputs.
fn textbook_kaiser_lowpass_f64(
    cutoff_hz: f64,
    sample_rate: f64,
    taps: usize,
    beta: f64,
) -> Vec<f64> {
    assert_eq!(taps % 2, 1, "symmetric design needs odd taps");
    let fc = cutoff_hz / sample_rate;
    let middle = (taps - 1) as f64 / 2.0;
    let i0_beta = bessel_i0_f64(beta);
    let mut h = Vec::with_capacity(taps);
    for n in 0..taps {
        let x = n as f64 - middle;
        let sinc = if x == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
        };
        let ratio = x / middle;
        let window = bessel_i0_f64(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) / i0_beta;
        h.push(sinc * window);
    }
    let sum: f64 = h.iter().sum();
    for tap in &mut h {
        *tap /= sum;
    }
    h
}

#[test]
fn lookahead_impulse_emerges_exactly_at_reported_latency() {
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for ms in [0.0f32, 0.5, 2.0, 5.0, 20.0] {
            let params = DeEsserPluginParams {
                lookahead_ms: ms,
                ..unity_params()
            };
            let mut plugin =
                DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
            plugin.initialize(f64::from(sample_rate)).unwrap();
            let expected = if ms > 0.0 {
                (ms * 0.001 * sample_rate as f32).round() as usize
            } else {
                0
            };
            assert_eq!(
                plugin.latency_samples(),
                expected,
                "reported latency sr={sample_rate} ms={ms}"
            );
            let latency = plugin.latency_samples();
            let frames = latency + 64;
            let mut buf = vec![0.0f32; frames];
            buf[0] = 1.0;
            plugin
                .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            assert_eq!(buf[latency], 1.0, "impulse value sr={sample_rate} ms={ms}");
            assert!(
                buf[..latency].iter().all(|&x| x == 0.0),
                "pre-impulse silence sr={sample_rate} ms={ms}"
            );
            assert!(
                buf[latency + 1..].iter().all(|&x| x == 0.0),
                "post-impulse silence sr={sample_rate} ms={ms}"
            );
        }
    }
}

#[test]
fn lookahead_dry_path_is_aligned_with_wet() {
    // Mix 0 must equal the delayed dry signal exactly: the dry reference is
    // taken after the lookahead delay, never from undelayed input.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        mix: 0.0,
        lookahead_ms: 2.0,
        ..unity_params()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let latency = plugin.latency_samples();
    assert_eq!(latency, 96);
    // Settle the mix smoother (fresh instances ramp toward a non-default mix).
    let mut warmup = vec![0.0f32; sample_rate as usize];
    plugin
        .process_in_place(
            &mut warmup,
            &ProcessContext::new(sample_rate, sample_rate as usize),
        )
        .unwrap();
    let frames = 8_192;
    let input: Vec<f32> = (0..frames)
        .map(|i| {
            0.2 * (std::f32::consts::TAU * 997.0 * i as f32 / sample_rate as f32).sin()
                + 0.2 * (std::f32::consts::TAU * 8_123.0 * i as f32 / sample_rate as f32).sin()
        })
        .collect();
    let mut output = input.clone();
    plugin
        .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let mut expected = vec![0.0f32; frames];
    expected[latency..].copy_from_slice(&input[..frames - latency]);
    assert_eq!(
        max_abs_diff(&output, &expected),
        0.0,
        "dry path must be the exactly delayed input"
    );
}

#[test]
fn lookahead_detector_reacts_before_delayed_program() {
    // Slow attack lets burst onsets leak without lookahead; delaying the
    // program while detecting undelayed input removes the leak.
    fn render(ms: f32) -> (Vec<f32>, usize) {
        let sample_rate = 48_000u32;
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -40.0,
            ratio: 20.0,
            attack_ms: 2.0,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            lookahead_ms: ms,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let latency = plugin.latency_samples();
        let frames = 9_600;
        let mut buf = vec![0.0f32; frames];
        for (i, sample) in buf.iter_mut().enumerate().take(480) {
            *sample = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin();
        }
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        (buf, latency)
    }
    let input_peak = 0.5 / std::f32::consts::SQRT_2;
    let (plain, _) = render(0.0);
    let (ahead, delay) = render(5.0);
    assert_eq!(delay, 240);
    let onset_plain = rms(&plain[0..96]);
    let onset_ahead = rms(&ahead[delay..delay + 96]);
    assert!(
        onset_plain > input_peak * 0.1,
        "without lookahead the 2 ms attack must leak the onset: {onset_plain:.4}"
    );
    assert!(
        onset_ahead < onset_plain * 0.5,
        "lookahead must at least halve onset leak: plain={onset_plain:.4} ahead={onset_ahead:.4}"
    );
    assert!(
        onset_ahead < input_peak * 0.2,
        "lookahead onset must be firmly reduced: {onset_ahead:.4}"
    );
}

#[test]
fn fir_split_reports_group_delay_and_reconstructs_exactly() {
    let sample_rate = 48_000u32;
    let taps = sotf_host::fir_crossover::DEFAULT_FIR_CROSSOVER_TAPS;
    assert_eq!(taps % 2, 1, "shared FIR taps must stay odd");
    let params = DeEsserPluginParams {
        mode: "Split-Band".to_string(),
        split_topology: "Linear-Phase".to_string(),
        ..unity_params()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(plugin.latency_samples(), (taps - 1) / 2);
    // Published taps are symmetric: the bank is linear-phase by design.
    let lp = plugin
        .fir_split
        .as_ref()
        .unwrap()
        .lowpass_coefficients()
        .to_vec();
    assert!(
        lp.iter()
            .zip(lp.iter().rev())
            .all(|(a, b)| (a - b).abs() < 1e-6),
        "FIR taps must be symmetric"
    );
    // Impulse reconstructs the delayed delta: high = delayed - low holds
    // sample by sample, so only a few ulps of f32 error are possible.
    let latency = plugin.latency_samples();
    let frames = latency + 64;
    let mut buf = vec![0.0f32; frames];
    buf[0] = 1.0;
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(
        (buf[latency] - 1.0).abs() < 1e-6,
        "impulse peak {}, want 1.0",
        buf[latency]
    );
    let peak = buf
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        .unwrap()
        .0;
    assert_eq!(
        peak, latency,
        "impulse peak must sit exactly on the latency"
    );
    for (i, &x) in buf.iter().enumerate() {
        if i != latency {
            assert!(x.abs() < 1e-6, "sidelobe at {i}: {x}");
        }
    }
}

#[test]
fn fir_split_dense_signal_reconstructs_delayed_input() {
    // 1025-tap f32 convolution accumulates worst-case ~1e-4 error
    // (taps * eps * signal); the bound below budgets twice that.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        mode: "Split-Band".to_string(),
        split_topology: "Linear-Phase".to_string(),
        ..unity_params()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let latency = plugin.latency_samples();
    let frames = 16_384;
    let input: Vec<f32> = (0..frames)
        .map(|i| {
            0.2 * (std::f32::consts::TAU * 997.0 * i as f32 / sample_rate as f32).sin()
                + 0.2 * (std::f32::consts::TAU * 8_123.0 * i as f32 / sample_rate as f32).sin()
        })
        .collect();
    let mut output = input.clone();
    plugin
        .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let mut expected = vec![0.0f32; frames];
    expected[latency..].copy_from_slice(&input[..frames - latency]);
    let error = max_abs_diff(&output, &expected);
    assert!(error < 2e-4, "dense reconstruction error {error}");
}

#[test]
fn fir_taps_match_independent_kaiser_windowed_sinc_design() {
    // Tap-design independence (F7): the bank's published taps must agree with
    // a from-textbook Kaiser windowed-sinc design built only from the
    // published parameters (cutoff, rate, 1025 taps, Kaiser beta 8). A wrong
    // window, beta, cutoff normalization or DC-gain convention in the tap
    // designer would move taps by ~1e-3 and responses by dBs; two correct
    // implementations agree to f32 rounding, so the 2e-6 tap bound and the
    // 0.1 dB response bound are sensitive to design bugs, not arithmetic.
    // Responses are compared only where both references sit above -60 dB:
    // deep stopband nulls amplify sub-ulp tap differences meaninglessly.
    // The high-side reference uses spectral inversion (delayed delta minus
    // lowpass), the bank's published complement law.
    let taps = sotf_host::fir_crossover::DEFAULT_FIR_CROSSOVER_TAPS;
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for center in [3_000.0f64, 7_000.0, 12_000.0] {
            let params = DeEsserPluginParams {
                frequency: center as f32,
                mode: "Split-Band".to_string(),
                split_topology: "Linear-Phase".to_string(),
                ..unity_params()
            };
            let mut plugin =
                DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
            plugin.initialize(f64::from(sample_rate)).unwrap();
            let fir = plugin.fir_split.as_ref().unwrap();
            let published_lp = fir.lowpass_coefficients().to_vec();
            assert_eq!(published_lp.len(), taps);
            let published_hp = fir.highpass_coefficients();
            let reference_lp = textbook_kaiser_lowpass_f64(center, sample_rate as f64, taps, 8.0);
            let mut reference_hp: Vec<f64> = reference_lp.iter().map(|&c| -c).collect();
            reference_hp[(taps - 1) / 2] += 1.0;
            let worst_tap = published_lp
                .iter()
                .zip(reference_lp.iter())
                .map(|(a, b)| (*a as f64 - *b).abs())
                .fold(0.0f64, f64::max);
            assert!(
                worst_tap < 2e-6,
                "tap design sr={sample_rate} center={center}: worst tap diff {worst_tap:.2e}"
            );
            for tone in [200.0f64, 1_000.0, 3_000.0, 7_000.0, 9_000.0, 12_000.0] {
                if tone > sample_rate as f64 * 0.4 {
                    continue;
                }
                let got_lp = dtft_magnitude_db(&published_lp, tone, sample_rate as f64);
                let want_lp = dtft_magnitude_db_f64(&reference_lp, tone, sample_rate as f64);
                if got_lp > -60.0 && want_lp > -60.0 {
                    assert!(
                        (got_lp - want_lp).abs() < 0.1,
                        "LP design sr={sample_rate} center={center} tone={tone}: \
                         published={got_lp:.3} textbook={want_lp:.3}"
                    );
                }
                let got_hp = dtft_magnitude_db(&published_hp, tone, sample_rate as f64);
                let want_hp = dtft_magnitude_db_f64(&reference_hp, tone, sample_rate as f64);
                if got_hp > -60.0 && want_hp > -60.0 {
                    assert!(
                        (got_hp - want_hp).abs() < 0.1,
                        "HP design sr={sample_rate} center={center} tone={tone}: \
                         published={got_hp:.3} textbook={want_hp:.3}"
                    );
                }
            }
        }
    }
}

#[test]
fn fir_split_bands_match_independent_dtft_reference() {
    // Isolate the bands with two runs: unity (low+high) and full HF kill
    // (low only). The kill render is driven by an external key tone at the
    // detector center: self-detection of a probe far from the center (e.g. a
    // 12 kHz probe with a 3 kHz center) rides the detector lowpass skirt
    // (~-20 dB two octaves out), which caps the kill near 23 dB and leaks
    // ~0.6 dB into the extracted high band. A hot in-band key removes that
    // stimulus dependence. Reference magnitudes come from an independent f64
    // DTFT of the published taps, so this test proves the bank is *used*
    // correctly (convolution, complement, gain path); tap-*design*
    // independence is covered by
    // `fir_taps_match_independent_kaiser_windowed_sinc_design`.
    // Only bands 40 dB above the residual-gain leakage floor are compared;
    // bound 0.5 dB budgets residual gain, envelope ripple and f32 arithmetic.
    // Frame count is capped at 24k so the 192 kHz lane stays practical; the
    // steady window still holds the FIR support several times over plus a
    // settled 5 ms release envelope.
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for center in [3_000.0f32, 7_000.0, 12_000.0] {
            let frequency = center;
            let mut probe = DeEsserPlugin::try_from_params_at_sample_rate(
                1,
                DeEsserPluginParams {
                    frequency,
                    mode: "Split-Band".to_string(),
                    split_topology: "Linear-Phase".to_string(),
                    ..unity_params()
                },
                sample_rate,
            )
            .unwrap();
            // Taps are rate-dependent; rebuild the bank at the test rate first.
            probe.initialize(f64::from(sample_rate)).unwrap();
            let fir = probe.fir_split.as_ref().unwrap();
            let lp = fir.lowpass_coefficients().to_vec();
            let hp = fir.highpass_coefficients();
            for tone in [200.0f64, 1_000.0, 3_000.0, 7_000.0, 9_000.0, 12_000.0] {
                if tone > sample_rate as f64 * 0.4 {
                    continue;
                }
                let frames = (sample_rate as usize / 4).min(24_000);
                let tone_f32 = tone as f32;
                let input: Vec<f32> = (0..frames)
                    .map(|i| {
                        0.316_227_8
                            * (std::f32::consts::TAU * tone_f32 * i as f32 / sample_rate as f32)
                                .sin()
                    })
                    .collect();
                let input_rms = rms(&input[input.len() / 2..]);
                let render_unity = || {
                    let params = DeEsserPluginParams {
                        frequency,
                        mode: "Split-Band".to_string(),
                        split_topology: "Linear-Phase".to_string(),
                        ..unity_params()
                    };
                    let mut plugin =
                        DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate)
                            .unwrap();
                    plugin.initialize(f64::from(sample_rate)).unwrap();
                    let mut buf = input.clone();
                    plugin
                        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
                        .unwrap();
                    buf
                };
                let render_low_only = || {
                    // Program carries the probe; the key carries a hot tone at
                    // the detector center so the kill is deep in every swept
                    // case. The program path is identical with or without a
                    // key bus; only the detector source changes.
                    let params = DeEsserPluginParams {
                        frequency,
                        q: 1.5,
                        threshold: -60.0,
                        ratio: 20.0,
                        range_db: 60.0,
                        attack_ms: 0.1,
                        release_ms: 5.0,
                        mode: "Split-Band".to_string(),
                        mix: 1.0,
                        split_topology: "Linear-Phase".to_string(),
                        sidechain_external: true,
                        ..Default::default()
                    };
                    let mut plugin =
                        DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate)
                            .unwrap();
                    plugin.initialize(f64::from(sample_rate)).unwrap();
                    assert_eq!(plugin.input_channels(), 2);
                    let mut buf = vec![0.0f32; frames * 2];
                    for i in 0..frames {
                        let key = 0.8
                            * (std::f32::consts::TAU * frequency * i as f32 / sample_rate as f32)
                                .sin();
                        buf[i * 2] = input[i];
                        buf[i * 2 + 1] = key;
                    }
                    plugin
                        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
                        .unwrap();
                    // Verified isolation precondition: the band comparison
                    // below assumes a full HF kill, so the achieved kill is
                    // asserted here instead of assumed. A 0.8-peak in-band
                    // key sits ~55 dB over the threshold for ~52 dB of GR;
                    // requiring 40 dB keeps 10+ dB of margin while bounding
                    // the leakage shortfall 20*log10(1 - 10^(-GR/20)) to
                    // 0.09 dB, inside the 0.5 dB band budget with room for
                    // envelope ripple and f32 noise.
                    assert!(
                        plugin.monitoring_gr[0] > 40.0,
                        "kill isolation sr={sample_rate} center={center} tone={tone}: \
                         GR={:.1} dB, want > 40",
                        plugin.monitoring_gr[0]
                    );
                    buf.iter().step_by(2).copied().collect()
                };
                let summed = render_unity();
                let low_only: Vec<f32> = render_low_only();
                let steady = frames / 2..;
                let low_db = 20.0 * (rms(&low_only[steady.clone()]) / input_rms).log10();
                let high_wave: Vec<f32> = summed
                    .iter()
                    .zip(low_only.iter())
                    .map(|(a, b)| a - b)
                    .collect();
                let high_db = 20.0 * (rms(&high_wave[steady]) / input_rms).log10();
                let ref_lp = dtft_magnitude_db(&lp, tone, sample_rate as f64);
                let ref_hp = dtft_magnitude_db(&hp, tone, sample_rate as f64);
                if ref_lp > -40.0 {
                    assert!(
                        (low_db as f64 - ref_lp).abs() < 0.5,
                        "low band sr={sample_rate} center={center} tone={tone}: \
                         measured={low_db:.2} ref={ref_lp:.2}"
                    );
                }
                if ref_hp > -40.0 {
                    assert!(
                        (high_db as f64 - ref_hp).abs() < 0.5,
                        "high band sr={sample_rate} center={center} tone={tone}: \
                         measured={high_db:.2} ref={ref_hp:.2}"
                    );
                }
            }
        }
    }
}

#[test]
fn lr4_split_recombine_is_allpass_flat() {
    // LR4 low+high is an all-pass: magnitude 1 at every frequency. This
    // property is independent of the implementation under test. Swept over
    // split centers (F8); the 2/16 kHz parameter-range edges are covered by
    // factory validation, and the windowed-sinc/LR4 responses scale smoothly
    // with center, so 3/7/12 kHz covers the interpolation.
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for center in [3_000.0f32, 7_000.0, 12_000.0] {
            for tone in [
                100.0f32, 300.0, 1_000.0, 3_000.0, 5_000.0, 7_000.0, 9_000.0, 12_000.0, 15_000.0,
            ] {
                if tone > sample_rate as f32 * 0.4 {
                    continue;
                }
                let params = DeEsserPluginParams {
                    frequency: center,
                    mode: "Split-Band".to_string(),
                    ..unity_params()
                };
                let mut plugin =
                    DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
                plugin.initialize(f64::from(sample_rate)).unwrap();
                assert_eq!(plugin.latency_samples(), 0);
                let frames = sample_rate as usize / 2;
                let mut buf: Vec<f32> = (0..frames)
                    .map(|i| {
                        0.4 * (std::f32::consts::TAU * tone * i as f32 / sample_rate as f32).sin()
                    })
                    .collect();
                let input_rms = rms(&buf[frames / 2..]);
                plugin
                    .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
                    .unwrap();
                let ratio_db = 20.0 * (rms(&buf[frames / 2..]) / input_rms).log10();
                assert!(
                    ratio_db.abs() < 0.05,
                    "all-pass flatness sr={sample_rate} center={center} tone={tone}: \
                     {ratio_db:.3} dB"
                );
            }
        }
    }
}

#[test]
fn split_topology_is_inert_in_wideband() {
    let sample_rate = 48_000u32;
    let input: Vec<f32> = (0..8_192)
        .map(|i| 0.3 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin())
        .collect();
    let render = |topology: &str| {
        let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(
            1,
            DeEsserPluginParams {
                mode: "Wideband".to_string(),
                split_topology: topology.to_string(),
                threshold: -20.0,
                ratio: 4.0,
                ..Default::default()
            },
            sample_rate,
        )
        .unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        assert_eq!(plugin.latency_samples(), 0);
        let mut buf = input.clone();
        let frames = buf.len();
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        buf
    };
    let min = render("Minimum-Phase");
    let lin = render("Linear-Phase");
    assert_eq!(
        max_abs_diff(&min, &lin),
        0.0,
        "topology must not affect wideband output"
    );
}

/// Steady-state gain reduction for a center-frequency tone. The 200 ms
/// release freezes the envelope near the peak-target value so the final
/// meter reading is phase-independent.
fn steady_gr_db(sample_rate: u32, center_hz: f32, ratio: f32, peak_db: f32, range_db: f32) -> f32 {
    let params = DeEsserPluginParams {
        frequency: center_hz,
        q: 1.5,
        threshold: -30.0,
        ratio,
        attack_ms: 0.1,
        release_ms: 200.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        range_db,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = sample_rate as usize / 2;
    let peak = 10.0f32.powf(peak_db / 20.0);
    let mut buf: Vec<f32> = (0..frames)
        .map(|i| peak * (std::f32::consts::TAU * center_hz * i as f32 / sample_rate as f32).sin())
        .collect();
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    plugin.monitoring_gr[0]
}

#[test]
fn gr_slope_matches_ratio_over_frequencies_levels_rates() {
    // Offset-free law check: both levels sit in the linear region, so the
    // unknown detector gain cancels and the GR difference must equal the
    // level difference times (1 - 1/ratio). Bound 0.5 dB budgets envelope
    // ripple and f32/fast-log error; an unwired ratio would miss by 5+ dB.
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for center in [3_000.0f32, 7_000.0, 12_000.0] {
            for ratio in [2.0f32, 4.0, 10.0] {
                let hi = steady_gr_db(sample_rate, center, ratio, -10.0, 60.0);
                let lo = steady_gr_db(sample_rate, center, ratio, -20.0, 60.0);
                let predicted = static_compress_gr_f64(-10.0, -30.0, ratio as f64, 6.0)
                    - static_compress_gr_f64(-20.0, -30.0, ratio as f64, 6.0);
                assert!(
                    ((hi - lo) as f64 - predicted).abs() < 0.5,
                    "slope sr={sample_rate} center={center} ratio={ratio}: \
                     measured diff={:.2} predicted={predicted:.2}",
                    hi - lo
                );
            }
        }
    }
}

#[test]
fn gr_range_cap_is_exact_and_zero_range_is_unity() {
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        for center in [3_000.0f32, 7_000.0, 12_000.0] {
            for range in [0.0f32, 3.0, 12.0] {
                let gr = steady_gr_db(sample_rate, center, 20.0, -6.0, range);
                assert!(
                    gr <= range + 0.05,
                    "cap sr={sample_rate} center={center} range={range}: gr={gr}"
                );
                assert!(
                    gr >= range - 1.0,
                    "cap must engage sr={sample_rate} center={center} range={range}: gr={gr}"
                );
            }
        }
    }
    // Zero range is unity: gain is exactly 1 for every sample.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        threshold: -60.0,
        ratio: 20.0,
        range_db: 0.0,
        ..unity_params()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = 8_192;
    let input: Vec<f32> = (0..frames)
        .map(|i| 0.4 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin())
        .collect();
    let mut output = input.clone();
    plugin
        .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(
        max_abs_diff(&output, &input) < 1e-6,
        "zero range must be unity"
    );
}

#[test]
fn gr_is_zero_below_knee() {
    for sample_rate in [44_100u32, 48_000, 96_000, 192_000] {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.1,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = sample_rate as usize;
        let peak = 10.0f32.powf(-30.0 / 20.0);
        let mut buf: Vec<f32> = (0..frames)
            .map(|i| peak * (std::f32::consts::TAU * 7_000.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        let input = buf.clone();
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(
            plugin.monitoring_gr[0] < 1e-6,
            "below-knee GR must be zero sr={sample_rate}: {}",
            plugin.monitoring_gr[0]
        );
        assert!(
            max_abs_diff(&buf, &input) < 1e-6,
            "below-knee output must be unity sr={sample_rate}"
        );
    }
}

#[test]
fn output_gain_matches_measured_gr() {
    // End-to-end gain mapping: output/input must equal 10^(-GR/20) for the
    // measured reduction, independent of detector calibration.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -30.0,
        ratio: 4.0,
        attack_ms: 0.1,
        release_ms: 200.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = sample_rate as usize;
    let peak = 10.0f32.powf(-10.0 / 20.0);
    let input: Vec<f32> = (0..frames)
        .map(|i| peak * (std::f32::consts::TAU * 7_000.0 * i as f32 / sample_rate as f32).sin())
        .collect();
    let mut output = input.clone();
    plugin
        .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let measured_db = 20.0 * (rms(&output[frames / 2..]) / rms(&input[frames / 2..])).log10();
    let predicted_db = -plugin.monitoring_gr[0];
    assert!(
        (measured_db - predicted_db).abs() < 0.5,
        "gain mapping: measured={measured_db:.2} predicted={predicted_db:.2}"
    );
}

#[test]
fn full_link_equalizes_channel_reduction() {
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -30.0,
        ratio: 10.0,
        attack_ms: 0.1,
        release_ms: 200.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        stereo_link: 1.0,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = sample_rate as usize;
    // Hot left, quiet right: unlinked, only the left would reduce.
    let mut buf = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let phase = std::f32::consts::TAU * 7_000.0 * i as f32 / sample_rate as f32;
        buf[i * 2] = 0.316_227_8 * phase.sin();
        buf[i * 2 + 1] = 0.003_162_278 * phase.sin();
    }
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(
        (plugin.monitoring_gr[0] - plugin.monitoring_gr[1]).abs() < 1e-3,
        "linked GR must match: {:?}",
        plugin.monitoring_gr
    );
    assert!(
        plugin.monitoring_gr[1] > 5.0,
        "quiet channel must share the reduction: {:?}",
        plugin.monitoring_gr
    );
}

#[test]
fn ms_roundtrip_is_transparent_without_reduction() {
    // M/S encode/decode wraps each processing path; with no reduction the
    // output must equal the input within f32 rounding.
    let sample_rate = 48_000u32;
    for (mode, topology) in [
        ("Wideband", "Minimum-Phase"),
        ("Split-Band", "Minimum-Phase"),
        ("Split-Band", "Linear-Phase"),
    ] {
        let params = DeEsserPluginParams {
            mode: mode.to_string(),
            split_topology: topology.to_string(),
            ms_mode: true,
            ..unity_params()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = 8_192;
        let mut input = vec![0.0f32; frames * 2];
        for i in 0..frames {
            input[i * 2] = 0.3
                * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin()
                + 0.1 * (std::f32::consts::TAU * 300.0 * i as f32 / sample_rate as f32).sin();
            input[i * 2 + 1] = 0.2
                * (std::f32::consts::TAU * 6_000.0 * i as f32 / sample_rate as f32).sin()
                + 0.15 * (std::f32::consts::TAU * 450.0 * i as f32 / sample_rate as f32).sin();
        }
        let mut output = input.clone();
        plugin
            .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        if mode == "Split-Band" && topology == "Minimum-Phase" {
            // The LR4 low+high sum is an all-pass, not the identity: phase
            // rotation moves samples by ~0.5 at unity gain (see
            // `lr4_split_recombine_is_allpass_flat`), so comparing against
            // raw input is the wrong oracle for this topology. M/S
            // encode/decode are linear memoryless maps and the zero-state
            // LR4 bank is LTI, hence Decode(Bank(Encode(x))) equals Bank(x)
            // exactly in real arithmetic: the M/S render must match the L/R
            // render sample-wise. The 1e-5 bound budgets f32 cascade
            // accumulation (~100 dB below the 0.4-peak signal; the drain
            // suite uses the same bound for LR4 paths); a wiring bug such
            // as a missing encode/decode step still shows at dB scale. An
            // RMS all-pass check against the input keeps a direct
            // transparency assertion with the correct metric.
            let mut lr_plugin = DeEsserPlugin::try_from_params_at_sample_rate(
                2,
                DeEsserPluginParams {
                    mode: mode.to_string(),
                    split_topology: topology.to_string(),
                    ms_mode: false,
                    ..unity_params()
                },
                sample_rate,
            )
            .unwrap();
            lr_plugin.initialize(f64::from(sample_rate)).unwrap();
            let mut lr_output = input.clone();
            lr_plugin
                .process_in_place(&mut lr_output, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            let error = max_abs_diff(&output, &lr_output);
            assert!(error < 1e-5, "M/S vs L/R {mode}/{topology} error {error}");
            let half = output.len() / 2;
            let ratio_db = 20.0 * (rms(&output[half..]) / rms(&input[half..])).log10();
            assert!(
                ratio_db.abs() < 0.05,
                "M/S all-pass flatness {mode}/{topology}: {ratio_db:.3} dB"
            );
            continue;
        }
        let latency = plugin.latency_samples() * 2;
        let error = max_abs_diff(&output[latency..], &input[..input.len() - latency]);
        // Dense FIR convolution accumulates ~1e-4; IIR/direct paths round only.
        let bound = if topology == "Linear-Phase" {
            2e-4
        } else {
            1e-6
        };
        assert!(
            error < bound,
            "M/S roundtrip {mode}/{topology} error {error}"
        );
    }
}

#[test]
fn ms_mode_processes_sum_and_difference_channels() {
    // Left-only sibilance: L/R mode reduces L alone, M/S mode works the
    // sum/difference pair, so L lands on a different operating point while
    // the silent R stays silent in both modes.
    let sample_rate = 48_000u32;
    let render = |ms_mode: bool| {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -24.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            ms_mode,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = sample_rate as usize;
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            buf[i * 2] =
                0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin();
        }
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        buf
    };
    let lr = render(false);
    let ms = render(true);
    let half = lr.len() / 2;
    let left_lr: Vec<f32> = lr[half..].iter().step_by(2).copied().collect();
    let left_ms: Vec<f32> = ms[half..].iter().step_by(2).copied().collect();
    let right_ms: Vec<f32> = ms[half..].iter().skip(1).step_by(2).copied().collect();
    let divergence = (rms(&left_ms) - rms(&left_lr)).abs() / rms(&left_lr);
    assert!(
        divergence > 0.01,
        "M/S must move L to a different operating point: {divergence:.4}"
    );
    assert!(
        rms(&right_ms) < 1e-9,
        "silent R must stay silent in M/S: {}",
        rms(&right_ms)
    );
}

#[test]
fn ms_center_image_is_preserved() {
    // Identical L/R sibilance lives purely in Mid; both outputs stay equal
    // and reduced together.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        ms_mode: true,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = sample_rate as usize;
    let mut buf = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let s = 0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin();
        buf[i * 2] = s;
        buf[i * 2 + 1] = s;
    }
    let input_rms = rms(&buf[buf.len() / 2..]);
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let mut peak_lr_diff = 0.0f32;
    for i in frames / 2..frames {
        peak_lr_diff = peak_lr_diff.max((buf[i * 2] - buf[i * 2 + 1]).abs());
    }
    assert!(
        peak_lr_diff < 1e-6,
        "center image must stay centered: {peak_lr_diff}"
    );
    assert!(
        rms(&buf[buf.len() / 2..]) < input_rms * 0.7,
        "center sibilance must still be reduced"
    );
}

#[test]
fn ms_mode_is_ignored_when_not_stereo() {
    let sample_rate = 48_000u32;
    let input: Vec<f32> = (0..8_192)
        .map(|i| 0.4 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin())
        .collect();
    let render = |ms_mode: bool| {
        let params = DeEsserPluginParams {
            threshold: -20.0,
            ratio: 4.0,
            ms_mode,
            ..unity_params()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let mut buf = input.clone();
        let frames = buf.len();
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        buf
    };
    assert_eq!(
        max_abs_diff(&render(false), &render(true)),
        0.0,
        "M/S must be inert on mono instances"
    );
}

#[test]
fn external_sidechain_drives_detection_and_preserves_key() {
    let sample_rate = 48_000u32;
    // Program alone sits 10 dB below threshold; a hot key must still reduce it.
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -20.0,
        ratio: 10.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        sidechain_external: true,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(plugin.input_channels(), 2);
    let frames = sample_rate as usize;
    let mut buf = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let phase = std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32;
        buf[i * 2] = 0.031_622_78 * phase.sin(); // program -30 dB
        buf[i * 2 + 1] = 0.5 * phase.sin(); // key -6 dB
    }
    let key_before: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
    let program_in: Vec<f32> = buf.iter().step_by(2).copied().collect();
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let program_out: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let key_after: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
    let half = frames / 2;
    assert!(
        rms(&program_out[half..]) < rms(&program_in[half..]) * 0.7,
        "hot key must reduce the quiet program"
    );
    assert!(
        plugin.monitoring_gr[0] > 3.0,
        "meter must reflect key-driven reduction: {}",
        plugin.monitoring_gr[0]
    );
    assert_eq!(
        max_abs_diff(&key_before, &key_after),
        0.0,
        "key bus must be preserved bitwise"
    );
}

#[test]
fn external_sidechain_isolation_both_directions() {
    // (a) LF-only key must not trigger on an HF program; (b) a silent key
    // must not let a loud HF program self-trigger: the detector reads the
    // key bus only.
    let sample_rate = 48_000u32;
    let render = |program_hz: f32, program_db: f32, key: Option<(f32, f32)>| {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -20.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            sidechain_external: true,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = sample_rate as usize;
        let program_peak = 10.0f32.powf(program_db / 20.0);
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            buf[i * 2] = program_peak
                * (std::f32::consts::TAU * program_hz * i as f32 / sample_rate as f32).sin();
            if let Some((key_hz, key_db)) = key {
                let key_peak = 10.0f32.powf(key_db / 20.0);
                buf[i * 2 + 1] = key_peak
                    * (std::f32::consts::TAU * key_hz * i as f32 / sample_rate as f32).sin();
            }
        }
        let program_in: Vec<f32> = buf.iter().step_by(2).copied().collect();
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        let program_out: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let half = frames / 2;
        rms(&program_out[half..]) / rms(&program_in[half..])
    };
    let lf_key = render(8_000.0, -10.0, Some((200.0, -6.0)));
    assert!(
        lf_key > 0.9,
        "LF key must not trigger HF reduction: {lf_key:.3}"
    );
    let silent_key = render(8_000.0, -10.0, None);
    assert!(
        silent_key > 0.9,
        "program must not self-trigger with silent key: {silent_key:.3}"
    );
}

#[test]
fn missing_key_is_rejected_before_state_changes() {
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        sidechain_external: true,
        ..unity_params()
    };
    let mut rejected =
        DeEsserPlugin::try_from_params_at_sample_rate(1, params.clone(), sample_rate).unwrap();
    rejected.initialize(f64::from(sample_rate)).unwrap();
    let mut reference =
        DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    reference.initialize(f64::from(sample_rate)).unwrap();
    // Program-width buffer without the key bus must be rejected untouched.
    let mut short = vec![0.25f32; 64];
    let error = rejected
        .process_in_place(&mut short, &ProcessContext::new(sample_rate, 64))
        .unwrap_err();
    assert!(error.contains("too small"), "unexpected error: {error}");
    assert!(short.iter().all(|&x| x == 0.25));
    // Rejected call must not advance any DSP state.
    let mut rejected_out = vec![0.1f32; 128];
    let mut reference_out = rejected_out.clone();
    rejected
        .process_in_place(&mut rejected_out, &ProcessContext::new(sample_rate, 64))
        .unwrap();
    reference
        .process_in_place(&mut reference_out, &ProcessContext::new(sample_rate, 64))
        .unwrap();
    assert_eq!(rejected_out, reference_out);
}

#[test]
fn sibilant_burst_reduced_while_thump_and_image_preserved() {
    // A2: HF burst cut, LF thump preserved after release settles, and the
    // unlinked clean channel untouched during the burst.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let burst = 1_440; // 30 ms sibilance
    let gap = 9_600; // 200 ms release settle
    let thump = 1_440; // 30 ms LF transient
    let frames = burst + gap + thump + 480;
    let mut buf = vec![0.0f32; frames * 2];
    for i in 0..frames {
        let t = i as f32 / sample_rate as f32;
        let mut left = 0.0;
        if i < burst {
            left = 0.5 * (std::f32::consts::TAU * 8_000.0 * t).sin();
        } else if i >= burst + gap && i < burst + gap + thump {
            left = 0.5 * (std::f32::consts::TAU * 150.0 * t).sin();
        }
        buf[i * 2] = left;
        buf[i * 2 + 1] = 0.2 * (std::f32::consts::TAU * 300.0 * t).sin()
            + 0.1 * (std::f32::consts::TAU * 1_000.0 * t).sin();
    }
    let input = buf.clone();
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    let left_in: Vec<f32> = input.iter().step_by(2).copied().collect();
    let left_out: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let right_in: Vec<f32> = input.iter().skip(1).step_by(2).copied().collect();
    let right_out: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
    let burst_ratio = rms(&left_out[96..burst - 96]) / rms(&left_in[96..burst - 96]);
    assert!(
        burst_ratio < 0.5,
        "sibilant burst must be cut: {burst_ratio:.3}"
    );
    let thump_start = burst + gap;
    let thump_ratio = rms(&left_out[thump_start + 96..thump_start + thump - 96])
        / rms(&left_in[thump_start + 96..thump_start + thump - 96]);
    assert!(
        thump_ratio > 0.9,
        "LF thump must survive after release: {thump_ratio:.3}"
    );
    let image_ratio = rms(&right_out[96..burst - 96]) / rms(&right_in[96..burst - 96]);
    assert!(
        image_ratio > 0.95,
        "unlinked clean channel must hold still: {image_ratio:.3}"
    );
}

#[test]
fn processing_is_partition_invariant() {
    // Every mode combination must render bitwise-identical audio under
    // arbitrary ordered partitions, including external-key stride.
    let sample_rate = 48_000u32;
    let configs: Vec<(&str, DeEsserPluginParams)> = vec![
        ("wideband", unity_params()),
        (
            "lookahead",
            DeEsserPluginParams {
                lookahead_ms: 2.0,
                ..unity_params()
            },
        ),
        (
            "lr4",
            DeEsserPluginParams {
                mode: "Split-Band".to_string(),
                threshold: -24.0,
                ratio: 8.0,
                ..unity_params()
            },
        ),
        (
            "fir",
            DeEsserPluginParams {
                mode: "Split-Band".to_string(),
                split_topology: "Linear-Phase".to_string(),
                threshold: -24.0,
                ratio: 8.0,
                ..unity_params()
            },
        ),
        (
            "ms",
            DeEsserPluginParams {
                threshold: -24.0,
                ratio: 8.0,
                ms_mode: true,
                ..unity_params()
            },
        ),
        (
            "ext",
            DeEsserPluginParams {
                threshold: -24.0,
                ratio: 8.0,
                sidechain_external: true,
                ..unity_params()
            },
        ),
    ];
    for (name, params) in configs {
        let channels = if name == "ms" { 2 } else { 1 };
        let stride = if name == "ms" || name == "ext" { 2 } else { 1 };
        let frames = 5_000;
        let mut input = vec![0.0f32; frames * stride];
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let program = 0.3 * (std::f32::consts::TAU * 8_000.0 * t).sin()
                + 0.2 * (std::f32::consts::TAU * 333.0 * t).sin();
            input[i * stride] = if i == 100 { 0.9 } else { program };
            if name == "ms" {
                input[i * stride + 1] = 0.25 * (std::f32::consts::TAU * 6_000.0 * t).sin();
            }
            if name == "ext" {
                input[i * stride + 1] = 0.4 * (std::f32::consts::TAU * 7_500.0 * t).sin();
            }
        }
        let mut reference =
            DeEsserPlugin::try_from_params_at_sample_rate(channels, params.clone(), sample_rate)
                .unwrap();
        reference.initialize(f64::from(sample_rate)).unwrap();
        let expected = render_partitioned(&mut reference, &input, channels, sample_rate, frames);
        for block in [1_024, 63, 7] {
            let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(
                channels,
                params.clone(),
                sample_rate,
            )
            .unwrap();
            plugin.initialize(f64::from(sample_rate)).unwrap();
            let output = render_partitioned(&mut plugin, &input, channels, sample_rate, block);
            assert_eq!(output, expected, "partition {block} diverged for {name}");
        }
    }
}

#[test]
fn drain_emits_exactly_the_retained_tail() {
    // A3: stream + drain preserves every program sample; an impulse on the
    // last input frame emerges at (N-1)+latency with its exact value.
    let sample_rate = 48_000u32;
    for (mode, topology, ms) in [
        ("Wideband", "Minimum-Phase", 2.0f32),
        ("Split-Band", "Minimum-Phase", 2.0),
        ("Split-Band", "Linear-Phase", 0.0),
        ("Split-Band", "Linear-Phase", 1.0),
    ] {
        let params = DeEsserPluginParams {
            mode: mode.to_string(),
            split_topology: topology.to_string(),
            lookahead_ms: ms,
            ..unity_params()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let latency = plugin.latency_samples();
        let retained = plugin.retained_frames();
        assert_eq!(plugin.drain_output_frames_max(), retained.min(256));
        let frames = 4_096;
        let mut buf = vec![0.0f32; frames];
        buf[frames - 1] = 1.0;
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        let bound = plugin.drain_call_bound().unwrap().get() as usize;
        assert_eq!(bound, retained.div_ceil(256).max(1));
        let mut stream = buf;
        let mut drained = Vec::new();
        let last_chunk = if retained.is_multiple_of(256) {
            256
        } else {
            retained % 256
        };
        loop {
            let mut out = vec![0.0f32; 256];
            let result = plugin
                .drain(&mut out, &ProcessContext::new(sample_rate, 256))
                .unwrap();
            drained.extend_from_slice(&out[..result.frames]);
            if result.complete {
                assert_eq!(result.frames, last_chunk);
                break;
            }
            assert_eq!(result.frames, 256);
        }
        assert_eq!(drained.len(), retained, "drained length {mode}/{topology}");
        stream.append(&mut drained);
        assert_eq!(stream.len(), frames + retained);
        // Pure-delay paths (wideband, linear-phase FIR) reproduce the impulse
        // at (N-1)+latency; the LR4 all-pass disperses it, so only the
        // retained length and completion are asserted there.
        if mode == "Wideband" || topology == "Linear-Phase" {
            let at = frames - 1 + latency;
            assert!(
                (stream[at] - 1.0).abs() < 1e-5,
                "tail impulse {mode}/{topology}: got {} at {at}",
                stream[at]
            );
            assert!(
                stream[..at].iter().all(|&x| x.abs() < 1e-5),
                "pre-impulse tail must be silent {mode}/{topology}"
            );
        }
        // Terminal drain is complete and idempotent; input needs a reset.
        let mut out = vec![0.0f32; 256];
        let terminal = plugin
            .drain(&mut out, &ProcessContext::new(sample_rate, 256))
            .unwrap();
        assert_eq!((terminal.frames, terminal.complete), (0, true));
        let mut more = vec![0.0f32; 64];
        assert!(
            plugin
                .process_in_place(&mut more, &ProcessContext::new(sample_rate, 64))
                .is_err(),
            "input after drain must require reset"
        );
        plugin.reset();
        plugin
            .process_in_place(&mut more, &ProcessContext::new(sample_rate, 64))
            .unwrap();
    }
}

#[test]
fn new_controls_register_with_automation_classification() {
    let mut plugin = DeEsserPlugin::new(2);
    plugin.initialize(48_000.0).unwrap();
    let schema = plugin.parameter_schema();
    assert_eq!(schema.len(), 14, "ten legacy plus four audit controls");
    let mode_of = |id: &str| {
        schema
            .iter()
            .find(|p| p.id.as_str() == id)
            .unwrap_or_else(|| panic!("missing parameter {id}"))
            .update_mode
    };
    for id in [
        "frequency",
        "q",
        "mode",
        "lookahead_ms",
        "split_topology",
        "sidechain_external",
    ] {
        assert_eq!(
            mode_of(id),
            UpdateMode::Structural,
            "{id} must be structural"
        );
    }
    for id in [
        "threshold",
        "ratio",
        "attack",
        "release",
        "mix",
        "range_db",
        "stereo_link",
        "ms_mode",
    ] {
        assert_eq!(mode_of(id), UpdateMode::Realtime, "{id} must be realtime");
    }
}

#[test]
fn structural_topology_controls_reject_live_changes() {
    let mut plugin = DeEsserPlugin::new(1);
    plugin.initialize(48_000.0).unwrap();
    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("lookahead_ms"),
            ParameterValue::Float(5.0),
        )
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected: {error}");
    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("split_topology"),
            ParameterValue::String("Linear-Phase".to_string()),
        )
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected: {error}");
    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("sidechain_external"),
            ParameterValue::Bool(true),
        )
        .unwrap_err();
    assert!(error.contains("structural"), "unexpected: {error}");
    // Same-value writes are accepted (idempotent host reconciliation).
    plugin
        .parametric_set_parameter(
            ParameterId::from("lookahead_ms"),
            ParameterValue::Float(0.0),
        )
        .unwrap();
    plugin
        .parametric_set_parameter(
            ParameterId::from("split_topology"),
            ParameterValue::String("Minimum-Phase".to_string()),
        )
        .unwrap();
    plugin
        .parametric_set_parameter(
            ParameterId::from("sidechain_external"),
            ParameterValue::Bool(false),
        )
        .unwrap();
    // Unknown topology spellings are rejected without touching state.
    let error = plugin
        .parametric_set_parameter(
            ParameterId::from("split_topology"),
            ParameterValue::String("Brickwall".to_string()),
        )
        .unwrap_err();
    assert!(
        error.contains("Unknown De-Esser split topology"),
        "unexpected: {error}"
    );
    assert_eq!(plugin.split_topology_index, 0);
    // M/S mode is a realtime toggle.
    plugin
        .parametric_set_parameter(ParameterId::from("ms_mode"), ParameterValue::Bool(true))
        .unwrap();
    assert!(plugin.ms_mode);
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("ms_mode")),
        Some(ParameterValue::Bool(true))
    );
    plugin
        .parametric_set_parameter(ParameterId::from("ms_mode"), ParameterValue::Bool(false))
        .unwrap();
    assert!(!plugin.ms_mode);
}

#[test]
fn legacy_presets_default_to_legacy_behavior() {
    // Old presets without the audit fields deserialize to today's behavior:
    // zero lookahead, minimum-phase split, L/R, internal detection.
    let params: DeEsserPluginParams = serde_json::from_str(
        r#"{"frequency":8000.0,"q":2.0,"threshold":-18.0,"ratio":6.0,
            "attack_ms":1.0,"release_ms":30.0,"mode":"Split-Band","mix":0.8,
            "range_db":12.0,"stereo_link":0.5}"#,
    )
    .unwrap();
    assert_eq!(params.lookahead_ms, 0.0);
    assert_eq!(params.split_topology, "Minimum-Phase");
    assert!(!params.ms_mode);
    assert!(!params.sidechain_external);
    let plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, 48_000).unwrap();
    assert_eq!(plugin.latency_samples(), 0);
    assert_eq!(plugin.input_channels(), 2);
    assert!(!plugin.use_ms());
    assert!(!plugin.use_fir_split());
}

#[test]
fn factory_rejects_invalid_audit_state() {
    assert!(
        DeEsserPlugin::try_from_params(
            1,
            DeEsserPluginParams {
                lookahead_ms: -1.0,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        DeEsserPlugin::try_from_params(
            1,
            DeEsserPluginParams {
                lookahead_ms: 25.0,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        DeEsserPlugin::try_from_params(
            1,
            DeEsserPluginParams {
                lookahead_ms: f32::NAN,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        DeEsserPlugin::try_from_params(
            1,
            DeEsserPluginParams {
                split_topology: "Brickwall".to_string(),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        DeEsserPlugin::try_from_params_at_sample_rate(
            usize::MAX,
            DeEsserPluginParams {
                sidechain_external: true,
                ..Default::default()
            },
            48_000
        )
        .is_err()
    );
}

#[test]
fn compile_metadata_reports_latency_and_coupling() {
    let sample_rate = 48_000u32;
    // Legacy defaults: zero latency, no channel mixing.
    let plain = DeEsserPlugin::try_from_params(1, DeEsserPluginParams::default()).unwrap();
    assert_eq!(plain.compile_metadata().latency_samples, 0);
    assert!(!plain.compile_metadata().channel_mixing);
    // Lookahead latency is reported.
    let mut ahead = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            lookahead_ms: 5.0,
            ..Default::default()
        },
    )
    .unwrap();
    ahead.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(ahead.compile_metadata().latency_samples, 240);
    // Linear-phase split latency is reported.
    let fir = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            mode: "Split-Band".to_string(),
            split_topology: "Linear-Phase".to_string(),
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    assert_eq!(
        fir.compile_metadata().latency_samples,
        (sotf_host::fir_crossover::DEFAULT_FIR_CROSSOVER_TAPS - 1) / 2
    );
    // Stereo link, M/S and external key couple channels.
    for params in [
        DeEsserPluginParams {
            stereo_link: 0.5,
            ..DeEsserPluginParams::default()
        },
        DeEsserPluginParams {
            ms_mode: true,
            ..DeEsserPluginParams::default()
        },
        DeEsserPluginParams {
            sidechain_external: true,
            ..DeEsserPluginParams::default()
        },
    ] {
        let plugin = DeEsserPlugin::try_from_params(2, params).unwrap();
        assert!(plugin.compile_metadata().channel_mixing);
    }
}

#[test]
fn tail_length_is_finite_except_recursive_split() {
    let sample_rate = 48_000u32;
    // Uninitialized: unknown.
    let fresh = DeEsserPlugin::try_from_params(1, DeEsserPluginParams::default()).unwrap();
    assert_eq!(fresh.tail_length(), TailLength::Unknown);
    // Wideband + lookahead: finite retained delay.
    let mut ahead = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            mode: "Wideband".to_string(),
            lookahead_ms: 2.0,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    ahead.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(ahead.tail_length(), TailLength::Finite(96));
    // Linear-phase split: finite retained support.
    let mut fir = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            mode: "Split-Band".to_string(),
            split_topology: "Linear-Phase".to_string(),
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    fir.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(
        fir.tail_length(),
        TailLength::Finite((sotf_host::fir_crossover::DEFAULT_FIR_CROSSOVER_TAPS - 1) as u64)
    );
    // Minimum-phase split: recursive LR4 program path stays unknown.
    let mut lr4 = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            mode: "Split-Band".to_string(),
            lookahead_ms: 2.0,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    lr4.initialize(f64::from(sample_rate)).unwrap();
    assert_eq!(lr4.tail_length(), TailLength::Unknown);
}

#[test]
fn drain_without_input_or_latency_completes_immediately() {
    let sample_rate = 48_000u32;
    let mut plugin = DeEsserPlugin::try_from_params(1, DeEsserPluginParams::default()).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    // No input yet: complete without output.
    let mut out = vec![0.0f32; 256];
    let idle = plugin
        .drain(&mut out, &ProcessContext::new(sample_rate, 256))
        .unwrap();
    assert_eq!((idle.frames, idle.complete), (0, true));
    // Zero-latency stream: nothing retained.
    let mut buf = vec![0.25f32; 512];
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, 512))
        .unwrap();
    let empty = plugin
        .drain(&mut out, &ProcessContext::new(sample_rate, 256))
        .unwrap();
    assert_eq!((empty.frames, empty.complete), (0, true));
}

#[test]
fn process_rejects_uninitialized_and_rate_mismatch() {
    // Lifecycle guards (gate precedent): processing before `initialize` or
    // at a non-initialized rate is rejected before any buffer mutation, so
    // hosts can never run mismatched filter/FIR/lookahead coefficients.
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(
        1,
        DeEsserPluginParams {
            lookahead_ms: 2.0,
            ..unity_params()
        },
        44_100,
    )
    .unwrap();
    let mut buf = vec![0.25f32; 64];
    let error = plugin
        .process_in_place(&mut buf, &ProcessContext::new(44_100, 64))
        .unwrap_err();
    assert!(error.contains("initialized"), "unexpected: {error}");
    assert!(buf.iter().all(|&x| x == 0.25));
    plugin.initialize(44_100.0).unwrap();
    let error = plugin
        .process_in_place(&mut buf, &ProcessContext::new(48_000, 64))
        .unwrap_err();
    assert!(error.contains("does not match"), "unexpected: {error}");
    assert!(buf.iter().all(|&x| x == 0.25));
    // The rejected calls advanced no state: a valid call behaves exactly
    // like a fresh instance.
    let mut reference = DeEsserPlugin::try_from_params_at_sample_rate(
        1,
        DeEsserPluginParams {
            lookahead_ms: 2.0,
            ..unity_params()
        },
        44_100,
    )
    .unwrap();
    reference.initialize(44_100.0).unwrap();
    let mut out = vec![0.1f32; 64];
    let mut expected = out.clone();
    plugin
        .process_in_place(&mut out, &ProcessContext::new(44_100, 64))
        .unwrap();
    reference
        .process_in_place(&mut expected, &ProcessContext::new(44_100, 64))
        .unwrap();
    assert_eq!(out, expected);
}

#[test]
fn drain_with_engaged_reduction_completes_exactly() {
    // A3: drain while gain reduction is engaged (the frozen 200 ms release
    // keeps the envelope near its peak through the short tail). Completion
    // stays finite with the exact retained length, and the tail carries
    // reduced audio rather than silence or blowup.
    let sample_rate = 48_000u32;
    for (mode, topology, lookahead_ms) in [
        ("Wideband", "Minimum-Phase", 2.0f32),
        ("Split-Band", "Linear-Phase", 0.0),
    ] {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -30.0,
            ratio: 10.0,
            attack_ms: 0.5,
            release_ms: 200.0,
            mode: mode.to_string(),
            mix: 1.0,
            split_topology: topology.to_string(),
            lookahead_ms,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let retained = plugin.retained_frames();
        assert!(retained > 0);
        let frames = sample_rate as usize / 2;
        let peak = 10.0f32.powf(-10.0 / 20.0);
        let mut buf: Vec<f32> = (0..frames)
            .map(|i| peak * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin())
            .collect();
        let input_rms = rms(&buf[frames / 2..]);
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(
            plugin.monitoring_gr[0] > 3.0,
            "reduction must be engaged {mode}/{topology}: {}",
            plugin.monitoring_gr[0]
        );
        let mut drained = Vec::new();
        let mut calls = 0;
        loop {
            let mut out = vec![0.0f32; 256];
            let result = plugin
                .drain(&mut out, &ProcessContext::new(sample_rate, 256))
                .unwrap();
            calls += 1;
            drained.extend_from_slice(&out[..result.frames]);
            if result.complete {
                break;
            }
        }
        assert_eq!(drained.len(), retained, "drained length {mode}/{topology}");
        assert!(
            drained.iter().all(|x| x.is_finite()),
            "drained tail must be finite {mode}/{topology}"
        );
        // The tail is the delayed program: reduced but nonzero.
        let tail_rms = rms(&drained);
        assert!(
            tail_rms > 0.01 && tail_rms < input_rms,
            "tail must carry reduced audio {mode}/{topology}: {tail_rms:.4} vs {input_rms:.4}"
        );
        assert_eq!(
            calls,
            retained.div_ceil(256).max(1),
            "drain call count {mode}/{topology}"
        );
        // Terminal drain is idempotent; reset recovers live processing.
        let mut out = vec![0.0f32; 256];
        let terminal = plugin
            .drain(&mut out, &ProcessContext::new(sample_rate, 256))
            .unwrap();
        assert_eq!((terminal.frames, terminal.complete), (0, true));
        plugin.reset();
        let mut more = vec![0.1f32; 64];
        plugin
            .process_in_place(&mut more, &ProcessContext::new(sample_rate, 64))
            .unwrap();
        assert!(more.iter().all(|x| x.is_finite()));
    }
}

#[test]
fn drain_supports_irregular_output_sizes() {
    // A3: drain with odd output sizes and an oversized final call. Every
    // retained frame is emitted exactly once, `complete` fires only on the
    // last call, and the terminal call is idempotent.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        mode: "Wideband".to_string(),
        lookahead_ms: 2.0,
        ..unity_params()
    };
    let mk = || {
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(1, params.clone(), sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = 4_096;
        let mut buf = vec![0.0f32; frames];
        buf[frames - 1] = 1.0;
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        plugin
    };
    let retained = mk().retained_frames();
    assert_eq!(retained, 96);
    // Odd output sizes: exact total, completion only on the last call.
    for output_frames in [63usize, 7] {
        let mut plugin = mk();
        let mut drained = Vec::new();
        let mut calls = 0;
        loop {
            let mut out = vec![0.0f32; output_frames];
            let result = plugin
                .drain(&mut out, &ProcessContext::new(sample_rate, output_frames))
                .unwrap();
            calls += 1;
            assert!(
                result.frames <= output_frames,
                "drain must not overfill the output"
            );
            drained.extend_from_slice(&out[..result.frames]);
            if result.complete {
                break;
            }
            assert_eq!(result.frames, output_frames);
        }
        assert_eq!(drained.len(), retained);
        assert_eq!(calls, retained.div_ceil(output_frames));
        // Terminal drain is complete and idempotent.
        let mut out = vec![0.0f32; output_frames];
        let terminal = plugin
            .drain(&mut out, &ProcessContext::new(sample_rate, output_frames))
            .unwrap();
        assert_eq!((terminal.frames, terminal.complete), (0, true));
    }
    // Oversized output: the retained tail still completes in one call.
    let mut plugin = mk();
    let mut out = vec![0.0f32; 4_096];
    let result = plugin
        .drain(&mut out, &ProcessContext::new(sample_rate, 4_096))
        .unwrap();
    assert_eq!((result.frames, result.complete), (retained, true));
    let mut tail = vec![0.0f32; 4_096];
    let terminal = plugin
        .drain(&mut tail, &ProcessContext::new(sample_rate, 4_096))
        .unwrap();
    assert_eq!((terminal.frames, terminal.complete), (0, true));
    // Full-capacity bound stays consistent with the exact count.
    let plugin = mk();
    assert_eq!(
        plugin.drain_call_bound().unwrap().get() as usize,
        retained.div_ceil(256).max(1)
    );
}

#[test]
fn multichannel_new_paths_render_finite_and_ms_inert() {
    // R1-R3 smoke beyond stereo: FIR + lookahead + full link render finite
    // audio with correct latency/retained counts, M/S stays bitwise inert
    // off stereo, and partitioning is invariant.
    let sample_rate = 48_000u32;
    for channels in [4usize, 6] {
        let base = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -24.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: "Split-Band".to_string(),
            mix: 1.0,
            stereo_link: 1.0,
            lookahead_ms: 2.0,
            split_topology: "Linear-Phase".to_string(),
            ms_mode: true,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(channels, base.clone(), sample_rate)
                .unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        assert_eq!(plugin.latency_samples(), 96 + 512);
        assert_eq!(plugin.retained_frames(), 96 + 1024);
        let frames = 4_096;
        let mut input = vec![0.0f32; frames * channels];
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            for ch in 0..channels {
                let hz = 6_000.0 + ch as f32 * 500.0;
                input[i * channels + ch] =
                    0.3 * (std::f32::consts::TAU * hz * t).sin() * (ch as f32 + 1.0)
                        / channels as f32;
            }
        }
        let mut output = input.clone();
        plugin
            .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert!(
            output.iter().all(|x| x.is_finite()),
            "{channels}ch output must be finite"
        );
        assert!(
            plugin.monitoring_gr.iter().all(|&g| g.is_finite()),
            "{channels}ch meters must be finite"
        );
        // M/S is inert off stereo: bitwise identical with the toggle off.
        let mut plain = DeEsserPlugin::try_from_params_at_sample_rate(
            channels,
            DeEsserPluginParams {
                ms_mode: false,
                ..base.clone()
            },
            sample_rate,
        )
        .unwrap();
        plain.initialize(f64::from(sample_rate)).unwrap();
        let mut expected = input.clone();
        plain
            .process_in_place(&mut expected, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert_eq!(
            max_abs_diff(&output, &expected),
            0.0,
            "{channels}ch M/S must be inert"
        );
        // Partition invariance on the multichannel path.
        let mut chunked =
            DeEsserPlugin::try_from_params_at_sample_rate(channels, base, sample_rate).unwrap();
        chunked.initialize(f64::from(sample_rate)).unwrap();
        assert_eq!(
            render_partitioned(&mut chunked, &input, channels, sample_rate, 63),
            output
        );
    }
}

#[test]
fn half_link_interpolates_reduction() {
    // The link law interpolates in dB toward the strongest reduction; at 0.5
    // each channel must sit halfway between its independent operating point
    // and the linked maximum. Bound 1e-2 dB budgets f32 association on
    // ~10 dB values (link 0/1 endpoints are covered elsewhere).
    let sample_rate = 48_000u32;
    let render = |stereo_link: f32| {
        let params = DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -30.0,
            ratio: 10.0,
            attack_ms: 0.1,
            release_ms: 200.0,
            mode: "Wideband".to_string(),
            mix: 1.0,
            stereo_link,
            ..Default::default()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let frames = sample_rate as usize;
        let mut buf = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let phase = std::f32::consts::TAU * 7_000.0 * i as f32 / sample_rate as f32;
            buf[i * 2] = 0.316_227_8 * phase.sin();
            buf[i * 2 + 1] = 0.003_162_278 * phase.sin();
        }
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        plugin.monitoring_gr.clone()
    };
    let unlinked = render(0.0);
    assert!(
        unlinked[0] > unlinked[1] + 5.0,
        "hot/quiet separation must hold: {unlinked:?}"
    );
    let half = render(0.5);
    for ch in 0..2 {
        let predicted = unlinked[ch] + 0.5 * (unlinked[0] - unlinked[ch]);
        assert!(
            (half[ch] - predicted).abs() < 1e-2,
            "link 0.5 ch{ch}: measured={} predicted={predicted}",
            half[ch]
        );
    }
}

#[test]
fn noise_and_dc_offset_render_finite_through_new_modes() {
    // Noise + DC-offset inputs stay finite through FIR + lookahead + M/S and
    // through the external-key path: no NaN, no DC blowup, bounded output.
    let sample_rate = 48_000u32;
    // Deterministic LCG noise (no rng dependency in tests).
    let mut state = 0x1234_5678_9abc_def1u64;
    let mut next_noise = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    // (a) Stereo FIR + lookahead + M/S + link, noise plus 0.25 DC.
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Split-Band".to_string(),
        mix: 1.0,
        stereo_link: 0.5,
        lookahead_ms: 2.0,
        split_topology: "Linear-Phase".to_string(),
        ms_mode: true,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let frames = 16_384;
    let mut buf = vec![0.0f32; frames * 2];
    for sample in buf.iter_mut() {
        *sample = 0.2 * next_noise() + 0.25;
    }
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(
        buf.iter().all(|x| x.is_finite()),
        "stereo path must be finite"
    );
    let peak = buf.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    assert!(peak < 2.0, "stereo path must not blow up: {peak}");
    // (b) External key: noisy program plus DC, independent noisy key.
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Wideband".to_string(),
        mix: 1.0,
        sidechain_external: true,
        ..Default::default()
    };
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    let mut buf = vec![0.0f32; frames * 2];
    for i in 0..frames {
        buf[i * 2] = 0.2 * next_noise() + 0.25;
        buf[i * 2 + 1] = 0.4 * next_noise();
    }
    plugin
        .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
        .unwrap();
    assert!(buf.iter().all(|x| x.is_finite()), "key path must be finite");
    let program_peak = buf
        .iter()
        .step_by(2)
        .map(|x| x.abs())
        .fold(0.0f32, f32::max);
    assert!(
        program_peak < 2.0,
        "key path must not blow up: {program_peak}"
    );
}

#[test]
fn processing_is_invariant_under_seeded_random_partitions() {
    // COMMON asks for randomized partitioning: a seeded xorshift block-size
    // sequence must render bitwise-identical audio to one-shot processing on
    // the FIR + lookahead + external-key path. The seed is fixed, so the run
    // is deterministic.
    let sample_rate = 48_000u32;
    let params = DeEsserPluginParams {
        frequency: 7000.0,
        q: 1.5,
        threshold: -24.0,
        ratio: 8.0,
        attack_ms: 0.5,
        release_ms: 20.0,
        mode: "Split-Band".to_string(),
        mix: 1.0,
        lookahead_ms: 2.0,
        split_topology: "Linear-Phase".to_string(),
        sidechain_external: true,
        ..Default::default()
    };
    let stride = 2;
    let frames = 5_000;
    let mut input = vec![0.0f32; frames * stride];
    for i in 0..frames {
        let t = i as f32 / sample_rate as f32;
        input[i * stride] = if i == 100 {
            0.9
        } else {
            0.3 * (std::f32::consts::TAU * 8_000.0 * t).sin()
                + 0.2 * (std::f32::consts::TAU * 333.0 * t).sin()
        };
        input[i * stride + 1] = 0.4 * (std::f32::consts::TAU * 7_500.0 * t).sin();
    }
    let mut reference =
        DeEsserPlugin::try_from_params_at_sample_rate(1, params.clone(), sample_rate).unwrap();
    reference.initialize(f64::from(sample_rate)).unwrap();
    let expected = render_partitioned(&mut reference, &input, 1, sample_rate, frames);
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
    plugin.initialize(f64::from(sample_rate)).unwrap();
    // Seeded xorshift64: deterministic pseudo-random blocks in 1..=1500.
    let mut rng = 0x243f_6a88_85a3_08d3u64;
    let mut output = input.clone();
    let mut done = 0;
    while done < frames {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let raw = ((rng >> 11) % 1500) as usize;
        let block = (1 + raw).min(frames - done);
        let range = done * stride..(done + block) * stride;
        let mut scratch = output[range.clone()].to_vec();
        let processed = plugin
            .process_in_place(&mut scratch, &ProcessContext::new(sample_rate, block))
            .unwrap();
        assert_eq!(processed, block);
        output[range].copy_from_slice(&scratch);
        done += block;
    }
    assert_eq!(output, expected, "seeded random partitions must match");
}

#[test]
fn split_band_dry_path_is_aligned_for_both_topologies() {
    // Mix 0 in split-band mode must reproduce the delayed dry reference for
    // both topologies: with unity gain every Mix value yields the same
    // response (documented reduction-depth law), and the lookahead version
    // must equal the plain version shifted by the reported latency. The FIR
    // bank additionally reconstructs the delayed input (linear phase = pure
    // delay, 2e-4 dense-convolution bound); the LR4 bank is all-pass, so its
    // dry reference is the shifted plain render, not the raw input.
    let sample_rate = 48_000u32;
    let frames = 8_192;
    let input: Vec<f32> = (0..frames)
        .map(|i| {
            0.2 * (std::f32::consts::TAU * 997.0 * i as f32 / sample_rate as f32).sin()
                + 0.2 * (std::f32::consts::TAU * 8_123.0 * i as f32 / sample_rate as f32).sin()
        })
        .collect();
    for topology in ["Minimum-Phase", "Linear-Phase"] {
        let render = |mix: f32, lookahead_ms: f32| {
            let params = DeEsserPluginParams {
                lookahead_ms,
                mode: "Split-Band".to_string(),
                mix,
                split_topology: topology.to_string(),
                ..unity_params()
            };
            let mut plugin =
                DeEsserPlugin::try_from_params_at_sample_rate(1, params, sample_rate).unwrap();
            plugin.initialize(f64::from(sample_rate)).unwrap();
            // Settle the mix smoother toward a non-default mix.
            let mut warmup = vec![0.0f32; sample_rate as usize];
            plugin
                .process_in_place(
                    &mut warmup,
                    &ProcessContext::new(sample_rate, sample_rate as usize),
                )
                .unwrap();
            let latency = plugin.latency_samples();
            let mut output = input.clone();
            plugin
                .process_in_place(&mut output, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            (output, latency)
        };
        // Unity gain: Mix 0 and Mix 1 render bitwise-identical dry audio.
        let (dry, latency) = render(0.0, 2.0);
        let (wet, wet_latency) = render(1.0, 2.0);
        assert_eq!(latency, wet_latency);
        assert_eq!(
            max_abs_diff(&dry, &wet),
            0.0,
            "mix law {topology}: unity gain must be mix-independent"
        );
        // Lookahead shifts the whole dry response by the reported latency.
        let (plain, plain_latency) = render(0.0, 0.0);
        let shift = latency - plain_latency;
        assert_eq!(shift, 96);
        assert_eq!(
            max_abs_diff(&dry[shift..], &plain[..frames - shift]),
            0.0,
            "dry alignment {topology}: lookahead must shift the dry response"
        );
        // FIR only: the dry response is the delayed input itself.
        if topology == "Linear-Phase" {
            let mut expected = vec![0.0f32; frames];
            expected[latency..].copy_from_slice(&input[..frames - latency]);
            let error = max_abs_diff(&dry, &expected);
            assert!(
                error < 2e-4,
                "FIR dry reconstruction {topology} error {error}"
            );
        }
    }
}

#[test]
fn compile_metadata_ignores_inert_coupling_modes() {
    // channel_mixing must reflect effective coupling: link on mono and M/S
    // on mono are inert and must not report mixing, while stereo M/S and any
    // external key bus do.
    let plain = DeEsserPlugin::try_from_params(1, DeEsserPluginParams::default()).unwrap();
    assert!(!plain.compile_metadata().channel_mixing);
    let mono_link = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            stereo_link: 1.0,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    assert!(!mono_link.compile_metadata().channel_mixing);
    let mono_ms = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            ms_mode: true,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    assert!(!mono_ms.compile_metadata().channel_mixing);
    let stereo_ms = DeEsserPlugin::try_from_params(
        2,
        DeEsserPluginParams {
            ms_mode: true,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    assert!(stereo_ms.compile_metadata().channel_mixing);
    let mono_ext = DeEsserPlugin::try_from_params(
        1,
        DeEsserPluginParams {
            sidechain_external: true,
            ..DeEsserPluginParams::default()
        },
    )
    .unwrap();
    assert!(mono_ext.compile_metadata().channel_mixing);
}

#[test]
fn split_band_ms_mode_engages_sum_and_difference_under_reduction() {
    // P2-D1: the M/S-vs-L/R unity checks pass vacuously if the split-arm
    // wrap were dead, and the Wideband divergence test does not execute
    // the split arms. Left-only sibilance through Split-Band with M/S
    // must reduce (nontrivial GR asserted on the meter), land L on a
    // different operating point than the L/R render (a dead wrap renders
    // bitwise-equal and fails the divergence bound), and keep the silent
    // R silent through encode/GR/decode. Both topologies.
    let sample_rate = 48_000u32;
    for topology in ["Minimum-Phase", "Linear-Phase"] {
        let render = |ms_mode: bool| {
            let params = DeEsserPluginParams {
                frequency: 7000.0,
                q: 1.5,
                threshold: -24.0,
                ratio: 8.0,
                attack_ms: 0.5,
                release_ms: 20.0,
                mode: "Split-Band".to_string(),
                mix: 1.0,
                split_topology: topology.to_string(),
                ms_mode,
                ..Default::default()
            };
            let mut plugin =
                DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
            plugin.initialize(f64::from(sample_rate)).unwrap();
            let frames = sample_rate as usize;
            let mut buf = vec![0.0f32; frames * 2];
            for i in 0..frames {
                buf[i * 2] =
                    0.5 * (std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32).sin();
            }
            plugin
                .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
                .unwrap();
            (buf, plugin.monitoring_gr.clone())
        };
        let (lr, gr_lr) = render(false);
        let (ms, gr_ms) = render(true);
        let engaged_lr = gr_lr.iter().copied().fold(0.0f32, f32::max);
        let engaged_ms = gr_ms.iter().copied().fold(0.0f32, f32::max);
        // Diagnostic: deterministic measured margins for --nocapture logs.
        println!("{topology}: engaged GR L/R={engaged_lr:.2} M/S={engaged_ms:.2} dB");
        // Engagement: both renders must actually reduce (GR well above 0).
        assert!(
            engaged_lr > 3.0,
            "{topology} L/R render must engage GR: {gr_lr:?}"
        );
        assert!(
            engaged_ms > 3.0,
            "{topology} M/S render must engage GR: {gr_ms:?}"
        );
        let half = lr.len() / 2;
        let left_lr: Vec<f32> = lr[half..].iter().step_by(2).copied().collect();
        let left_ms: Vec<f32> = ms[half..].iter().step_by(2).copied().collect();
        let right_ms: Vec<f32> = ms[half..].iter().skip(1).step_by(2).copied().collect();
        // Mid works the half-level sum, so L lands far from the L/R point;
        // the 1% bound mirrors the Wideband divergence test.
        let divergence = (rms(&left_ms) - rms(&left_lr)).abs() / rms(&left_lr);
        let r_rms = rms(&right_ms);
        println!("{topology}: L divergence={divergence:.4} R RMS={r_rms:.2e}");
        assert!(
            divergence > 0.01,
            "{topology} M/S must move split-band L off the L/R point: {divergence:.4}"
        );
        // M and S carry identical half-level signals, so identical gains
        // cancel exactly on decode and the silent R stays silent.
        assert!(
            r_rms < 1e-9,
            "{topology} silent R must stay silent in split M/S: {}",
            rms(&right_ms)
        );
    }
}

#[test]
fn stereo_drain_preserves_ms_and_eof_contract() {
    // P2-D2: drain execution is mono-only, leaving the M/S wrap inside
    // drain unexecuted. Stereo stream + drain with and without M/S must
    // preserve every program sample (L impulse at (N-1)+latency with its
    // exact value, silent R throughout), emit exactly the retained tail,
    // honor the call bound, complete idempotently, and require reset
    // before further input. The external-key case pins program-only drain
    // width over the doubled input stride. Pure-delay paths only (LR4
    // disperses impulses; the mono suite covers its accounting).
    let sample_rate = 48_000u32;
    for (mode, topology, lookahead_ms, ms_mode, external) in [
        ("Wideband", "Minimum-Phase", 2.0f32, false, false),
        ("Wideband", "Minimum-Phase", 2.0, true, false),
        ("Split-Band", "Linear-Phase", 0.0, false, false),
        ("Split-Band", "Linear-Phase", 0.0, true, false),
        ("Wideband", "Minimum-Phase", 2.0, true, true),
    ] {
        let params = DeEsserPluginParams {
            mode: mode.to_string(),
            split_topology: topology.to_string(),
            lookahead_ms,
            ms_mode,
            sidechain_external: external,
            ..unity_params()
        };
        let mut plugin =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params, sample_rate).unwrap();
        plugin.initialize(f64::from(sample_rate)).unwrap();
        let latency = plugin.latency_samples();
        let retained = plugin.retained_frames();
        assert_eq!(plugin.drain_output_frames_max(), retained.min(256));
        let stride = plugin.input_channels();
        assert_eq!(stride, if external { 4 } else { 2 });
        let frames = 4_096;
        let mut buf = vec![0.0f32; frames * stride];
        buf[(frames - 1) * stride] = 1.0;
        plugin
            .process_in_place(&mut buf, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        let bound = plugin.drain_call_bound().unwrap().get() as usize;
        assert_eq!(bound, retained.div_ceil(256).max(1));
        // The stream keeps program channels only; the key region (if any)
        // is deinterleaved before the tail is appended.
        let mut stream: Vec<f32> = Vec::with_capacity(frames * 2);
        for frame in 0..frames {
            stream.extend_from_slice(&buf[frame * stride..frame * stride + 2]);
        }
        let mut drained = Vec::new();
        let last_chunk = if retained.is_multiple_of(256) {
            256
        } else {
            retained % 256
        };
        loop {
            let mut out = vec![0.0f32; 256 * 2];
            let result = plugin
                .drain(&mut out, &ProcessContext::new(sample_rate, 256))
                .unwrap();
            drained.extend_from_slice(&out[..result.frames * 2]);
            if result.complete {
                assert_eq!(result.frames, last_chunk);
                break;
            }
            assert_eq!(result.frames, 256);
        }
        assert_eq!(
            drained.len(),
            retained * 2,
            "stereo drained length {mode}/{topology}/ms={ms_mode}/ext={external}"
        );
        let drained_len = drained.len();
        stream.append(&mut drained);
        assert_eq!(stream.len(), (frames + retained) * 2);
        let at = (frames - 1 + latency) * 2;
        let impulse_error = (stream[at] - 1.0).abs();
        let right: Vec<f32> = stream.iter().skip(1).step_by(2).copied().collect();
        let r_rms = rms(&right);
        // Diagnostic: deterministic measured margins for --nocapture logs.
        println!(
            "{mode}/{topology}/ms={ms_mode}/ext={external}: drained={drained_len} \
             retained={} L impulse err={impulse_error:.2e} R RMS={r_rms:.2e}",
            retained * 2,
        );
        assert!(
            impulse_error < 1e-5,
            "stereo tail impulse L: got {} at frame {} ({mode}/{topology})",
            stream[at],
            at / 2,
        );
        assert!(
            stream[..at].iter().step_by(2).all(|&x| x.abs() < 1e-5),
            "pre-impulse L tail must be silent ({mode}/{topology})"
        );
        assert!(
            r_rms < 1e-9,
            "R must stay silent through stereo drain: {} ({mode}/{topology})",
            rms(&right)
        );
        // Terminal drain is complete and idempotent; input needs a reset.
        let mut out = vec![0.0f32; 256 * 2];
        let terminal = plugin
            .drain(&mut out, &ProcessContext::new(sample_rate, 256))
            .unwrap();
        assert_eq!((terminal.frames, terminal.complete), (0, true));
        let mut more = vec![0.0f32; 64 * stride];
        assert!(
            plugin
                .process_in_place(&mut more, &ProcessContext::new(sample_rate, 64))
                .is_err(),
            "input after drain must require reset"
        );
        plugin.reset();
        plugin
            .process_in_place(&mut more, &ProcessContext::new(sample_rate, 64))
            .unwrap();
    }
}

#[test]
fn reset_replay_matches_fresh_with_populated_detector_and_key() {
    // P2-D3: reset determinism is proven for internal detection, but no DSP
    // test replays populated detector/key history against a fresh instance
    // bitwise. Each config renders engaged audio first (GR asserted above
    // 3 dB, so detector filters, envelopes, crossovers and delay lines
    // hold history), then resets, re-renders the identical input, and must
    // match a fresh instance sample-for-sample, meters included. All cases
    // use the external key bus; the automation suite already covers
    // internal-detection reset for both modes.
    let sample_rate = 48_000u32;
    for (mode, topology, lookahead_ms, ms_mode) in [
        ("Split-Band", "Linear-Phase", 2.0f32, true),
        ("Split-Band", "Minimum-Phase", 5.0, false),
        ("Wideband", "Minimum-Phase", 2.0, true),
    ] {
        let params = || DeEsserPluginParams {
            frequency: 7000.0,
            q: 1.5,
            threshold: -24.0,
            ratio: 8.0,
            attack_ms: 0.5,
            release_ms: 20.0,
            mode: mode.to_string(),
            mix: 1.0,
            split_topology: topology.to_string(),
            lookahead_ms,
            ms_mode,
            sidechain_external: true,
            ..Default::default()
        };
        let frames = 8_192;
        let input = || {
            let mut buf = vec![0.0f32; frames * 4];
            for i in 0..frames {
                let phase = std::f32::consts::TAU * 8_000.0 * i as f32 / sample_rate as f32;
                buf[i * 4] = 0.5 * phase.sin();
                buf[i * 4 + 1] = 0.2 * phase.sin();
                buf[i * 4 + 2] = 0.5 * phase.sin();
                buf[i * 4 + 3] = 0.5 * phase.sin();
            }
            buf
        };
        let mut used =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params(), sample_rate).unwrap();
        used.initialize(f64::from(sample_rate)).unwrap();
        assert_eq!(used.input_channels(), 4);
        let mut first = input();
        used.process_in_place(&mut first, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        // Populated proof: the hot key must have driven real reduction.
        assert!(
            used.monitoring_gr.iter().copied().fold(0.0f32, f32::max) > 3.0,
            "{mode}/{topology} key render must engage GR: {:?}",
            used.monitoring_gr
        );
        used.reset();
        let mut fresh =
            DeEsserPlugin::try_from_params_at_sample_rate(2, params(), sample_rate).unwrap();
        fresh.initialize(f64::from(sample_rate)).unwrap();
        let mut replay = input();
        let mut reference = input();
        used.process_in_place(&mut replay, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        fresh
            .process_in_place(&mut reference, &ProcessContext::new(sample_rate, frames))
            .unwrap();
        assert_eq!(
            replay, reference,
            "{mode}/{topology} reset must clear detector/key history bitwise"
        );
        assert_eq!(
            used.monitoring_gr, fresh.monitoring_gr,
            "{mode}/{topology} reset must clear meters bitwise"
        );
    }
}
