//! Persisted engine Denoiser settings reach the public DSP constructor.
//!
//! Settings save/restore travel through [`PluginSettings::to_plugin_config`]
//! and the hosted factory to initialized DSP, then render nonzero processed
//! audio plus a complete EOF drain. Non-default curve knots and audition are
//! proven by independent audio references (default/cleaned/residual/clean),
//! never by stored values alone; fresh/restored twins render bit-identically.
//! Rejected factory candidates leave the accepted live chain untouched.
//!
//! Predeclared bounds: cleaned+residual reconstructs the delayed input within
//! 1e-6 (owned R2 contract); a released low band preserves the 100 Hz tone by
//! more than 6 dB over the default curve (unity shape gain vs Wiener
//! suppression); drain tail is exactly 3072 frames for hop-aligned input
//! (2N - hop at zero source phase, N = 2048).

// Rust guideline compliant 2026-02-21
use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::{DenoiserData, ParameterId, ParameterValue, ProcessContext, create_plugin};

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// Denoiser WOLA latency in frames (FFT size 2048, default mode).
const LATENCY: usize = 2048;
/// Exact EOF tail for hop-aligned input: 2N - hop + (hop - 0) % hop.
const DRAIN_TAIL_FRAMES: usize = 2 * 2048 - 1024;
const BLOCK_FRAMES: usize = 4096;

fn lcg_noise(frames: usize, channels: usize, amplitude: f32, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..frames * channels)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / 2_147_483_648.0) * 2.0 - 1.0
        })
        .map(|sample| sample * amplitude)
        .collect()
}

fn tone_plus_noise(frames: usize, channels: usize, seed: u64) -> (Vec<f32>, Vec<f32>) {
    let noise = lcg_noise(frames, channels, 0.05, seed);
    let mut input = noise.clone();
    let mut clean = vec![0.0; frames * channels];
    for frame in 0..frames {
        let t = frame as f32 / RATE as f32;
        let tone = 0.1 * (2.0 * std::f32::consts::PI * 100.0 * t).sin()
            + 0.05 * (2.0 * std::f32::consts::PI * 3150.0 * t).sin();
        for ch in 0..channels {
            input[frame * channels + ch] += tone;
            clean[frame * channels + ch] = tone;
        }
    }
    (input, clean)
}

/// Hann-windowed Goertzel magnitude squared at `freq_hz` (mono frames).
fn goertzel_mag2(samples: &[f32], freq_hz: f64) -> f64 {
    use std::f64::consts::PI;
    let n = samples.len();
    let omega = 2.0 * PI * freq_hz / f64::from(RATE);
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for (i, &x) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
        let s0 = window * f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(1e-30)
}

fn channel_zero(interleaved: &[f32], channels: usize) -> Vec<f32> {
    interleaved.iter().step_by(channels).copied().collect()
}

fn denoiser_settings_with(
    curve_low: f64,
    curve_mid: f64,
    curve_high: f64,
    audition_residual: bool,
) -> PluginSettings {
    let mut settings = PluginSettings::default_for(&PluginType::Denoiser).unwrap();
    let PluginSettings::Denoiser {
        curve_low: low,
        curve_mid: mid,
        curve_high: high,
        audition_residual: audition,
        ..
    } = &mut settings
    else {
        panic!("Denoiser default must have Denoiser settings")
    };
    *low = curve_low;
    *mid = curve_mid;
    *high = curve_high;
    *audition = audition_residual;
    settings
}

/// Renders through settings, converter, and the hosted factory.
///
/// Returns the full output (process frames plus complete EOF drain) and the
/// contract latency. Input length must be a multiple of the block size.
fn render_hosted_through_settings(
    settings: &PluginSettings,
    channels: usize,
    input: &[f32],
) -> (Vec<f32>, usize) {
    let config = settings.to_plugin_config(f64::from(RATE));
    let mut plugin =
        create_plugin(&config.plugin_type, &config.parameters, channels, RATE).unwrap();
    plugin.initialize(f64::from(RATE)).unwrap();
    let latency = plugin.latency_samples();
    let frames = input.len() / channels;
    assert_eq!(
        frames % BLOCK_FRAMES,
        0,
        "test input must fill whole blocks"
    );
    let mut output = vec![f32::NAN; input.len()];
    for start in (0..frames).step_by(BLOCK_FRAMES) {
        let processed = plugin
            .process(
                &input[start * channels..(start + BLOCK_FRAMES) * channels],
                &mut output[start * channels..(start + BLOCK_FRAMES) * channels],
                &ProcessContext::new(RATE, BLOCK_FRAMES),
            )
            .unwrap();
        assert_eq!(
            processed, BLOCK_FRAMES,
            "hosted process must preserve length"
        );
    }
    let mut full = output;
    for _ in 0..4096 {
        let mut block = vec![0.0; 1024 * channels];
        let status = plugin
            .drain(&mut block, &ProcessContext::new(RATE, 1024))
            .unwrap();
        full.extend_from_slice(&block[..status.frames * channels]);
        if status.complete {
            return (full, latency);
        }
    }
    panic!("drain did not complete")
}

fn assert_nonzero_finite(output: &[f32], context: &str) {
    assert!(
        output.iter().all(|s| s.is_finite()),
        "{context}: output must be finite"
    );
    let peak = output.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    assert!(
        peak > 1e-6,
        "{context}: output must be nonzero, peak {peak:.3e}"
    );
}

/// Asserts two Denoiser settings agree key by key as JSON.
///
/// Any serde-default/PARAMS-default drift fails here naming the key and
/// both values, instead of surfacing later as a giant audio-array dump.
fn assert_denoiser_settings_json_eq(legacy: &PluginSettings, explicit: &PluginSettings) {
    let left = serde_json::to_value(legacy).unwrap();
    let right = serde_json::to_value(explicit).unwrap();
    let left_map = left
        .get("Denoiser")
        .and_then(serde_json::Value::as_object)
        .unwrap();
    let right_map = right
        .get("Denoiser")
        .and_then(serde_json::Value::as_object)
        .unwrap();
    for (key, left_value) in left_map {
        let right_value = right_map.get(key).unwrap_or(&serde_json::Value::Null);
        assert_eq!(
            left_value, right_value,
            "settings key `{key}` drifted between legacy and explicit defaults"
        );
    }
    for key in right_map.keys() {
        assert!(
            left_map.contains_key(key),
            "settings key `{key}` missing on the legacy side"
        );
    }
}

/// Bit-exact equality with a one-line report.
///
/// Same pass/fail semantics as `assert_eq!` on slices, but a failure
/// prints only lengths, first mismatch, differing count, and max diff.
fn assert_audio_bit_exact(left: &[f32], right: &[f32], context: &str) {
    assert_eq!(
        left.len(),
        right.len(),
        "{context}: lengths {} vs {}",
        left.len(),
        right.len()
    );
    let mut first: Option<(usize, f32, f32)> = None;
    let mut count = 0;
    let mut max_diff = 0.0f32;
    for (index, (&a, &b)) in left.iter().zip(right.iter()).enumerate() {
        max_diff = max_diff.max((a - b).abs());
        if a != b {
            count += 1;
            if first.is_none() {
                first = Some((index, a, b));
            }
        }
    }
    if let Some((index, a, b)) = first {
        panic!(
            "{context}: first mismatch at {index} ({a} vs {b}), {count} diffs, max {max_diff:e}"
        );
    }
}

#[test]
fn engine_denoiser_settings_roundtrip_reaches_dsp() {
    for strength in [0.0, 0.9, 1.0, 1.01] {
        let mut settings = PluginSettings::default_for(&PluginType::Denoiser).unwrap();
        let PluginSettings::Denoiser {
            harmonic_percussive,
            spatial_denoise,
            spatial_strength,
            ..
        } = &mut settings
        else {
            panic!("Denoiser default must have Denoiser settings")
        };
        *harmonic_percussive = true;
        *spatial_denoise = true;
        *spatial_strength = strength;
        let encoded = serde_json::to_vec(&settings).unwrap();
        let restored: PluginSettings = serde_json::from_slice(&encoded).unwrap();
        let config = restored.to_plugin_config(48_000.0);
        let result = create_plugin(&config.plugin_type, &config.parameters, 2, 48_000);
        if strength > 1.0 {
            assert!(result.is_err());
            continue;
        }
        let mut plugin = result.unwrap();
        plugin.initialize(48_000.0).unwrap();
        for (id, expected) in [
            ("harmonic_percussive", ParameterValue::Bool(true)),
            ("spatial_denoise", ParameterValue::Bool(true)),
            ("spatial_strength", ParameterValue::Float(strength as f32)),
        ] {
            assert_eq!(plugin.get_parameter(&ParameterId::from(id)), Some(expected));
        }
        // Accepted controls also render: nonzero processed audio plus the
        // exact EOF tail, not just stored values.
        let input = lcg_noise(8192, 2, 0.05, 0xd515e);
        let mut output = vec![f32::NAN; input.len()];
        for start in (0..8192).step_by(BLOCK_FRAMES) {
            let processed = plugin
                .process(
                    &input[start * 2..(start + BLOCK_FRAMES) * 2],
                    &mut output[start * 2..(start + BLOCK_FRAMES) * 2],
                    &ProcessContext::new(48_000, BLOCK_FRAMES),
                )
                .unwrap();
            assert_eq!(processed, BLOCK_FRAMES);
        }
        assert_nonzero_finite(&output, "spatial path");
        let mut tail = 0;
        let mut complete = false;
        for _ in 0..4096 {
            let mut block = vec![0.0; 1024 * 2];
            let status = plugin
                .drain(&mut block, &ProcessContext::new(48_000, 1024))
                .unwrap();
            tail += status.frames;
            if status.complete {
                complete = true;
                break;
            }
        }
        assert!(complete, "drain must complete");
        assert_eq!(tail, DRAIN_TAIL_FRAMES, "exact EOF tail for aligned input");
    }
}

#[test]
fn engine_denoiser_curve_and_audition_shape_audio_end_to_end() {
    let frames = 65_536;
    let (input, _clean) = tone_plus_noise(frames, CHANNELS, 0xc07e);
    // Non-default knots + audition on: converter must forward exact keys.
    let shaped = denoiser_settings_with(0.0, 0.5, 1.0, false);
    let shaped_config = shaped.to_plugin_config(f64::from(RATE));
    assert_eq!(shaped_config.plugin_type, "denoiser");
    assert_eq!(shaped_config.parameters["curve_low"], 0.0);
    assert_eq!(shaped_config.parameters["curve_mid"], 0.5);
    assert_eq!(shaped_config.parameters["curve_high"], 1.0);
    assert_eq!(shaped_config.parameters["audition_residual"], false);
    let residual_settings = denoiser_settings_with(1.0, 1.0, 1.0, true);
    let residual_config = residual_settings.to_plugin_config(f64::from(RATE));
    assert_eq!(residual_config.parameters["audition_residual"], true);

    let default = denoiser_settings_with(1.0, 1.0, 1.0, false);
    let (cleaned, latency) = render_hosted_through_settings(&default, CHANNELS, &input);
    assert_eq!(latency, LATENCY, "contract latency");
    let (shaped_out, shaped_latency) = render_hosted_through_settings(&shaped, CHANNELS, &input);
    let (residual, residual_latency) =
        render_hosted_through_settings(&residual_settings, CHANNELS, &input);
    assert_eq!(shaped_latency, LATENCY);
    assert_eq!(residual_latency, LATENCY);
    for (output, context) in [
        (&cleaned, "default cleaned"),
        (&shaped_out, "shaped"),
        (&residual, "residual"),
    ] {
        assert_eq!(
            output.len(),
            (frames + DRAIN_TAIL_FRAMES) * CHANNELS,
            "{context}: process frames plus complete drain"
        );
        assert_nonzero_finite(output, context);
    }
    // The shaped curve audibly differs from default...
    assert_ne!(
        shaped_out, cleaned,
        "curve_low = 0 must audibly release the low band"
    );
    // ...and preserves the released 100 Hz tone: unity shape gain passes
    // the band unprocessed while the default curve Wiener-suppresses it.
    let steady = |output: &[f32]| {
        let ch0 = channel_zero(output, CHANNELS);
        ch0[16_384..frames].to_vec()
    };
    let default_tone = goertzel_mag2(&steady(&cleaned), 100.0);
    let shaped_tone = goertzel_mag2(&steady(&shaped_out), 100.0);
    let preservation_db = 10.0 * (shaped_tone / default_tone).log10();
    println!("engine curve: 100 Hz tone {preservation_db:.2} dB above default curve");
    assert!(
        preservation_db > 6.0,
        "released low band must preserve the tone, got {preservation_db:.2} dB"
    );
    // Audition switches the output path to the residual...
    assert_ne!(
        residual, cleaned,
        "audition must switch to the residual path"
    );
    // ...and cleaned + residual reconstructs the latency-delayed input
    // within the owned R2 bound (steady region, both channels).
    let mut worst = 0.0f32;
    for o in 4096..frames {
        for ch in 0..CHANNELS {
            let sum = cleaned[o * CHANNELS + ch] + residual[o * CHANNELS + ch];
            let dry = input[(o - LATENCY) * CHANNELS + ch];
            worst = worst.max((sum - dry).abs());
        }
    }
    println!("engine audition: reconstruction worst error {worst:e}");
    assert!(
        worst <= 1e-6,
        "cleaned + residual must reconstruct the delayed input, worst {worst:e}"
    );
    // Fresh/restored twins render bit-identically (process + full drain).
    let saved = serde_json::to_vec(&shaped).unwrap();
    let restored: PluginSettings = serde_json::from_slice(&saved).unwrap();
    let (again, _) = render_hosted_through_settings(&restored, CHANNELS, &input);
    assert_eq!(
        shaped_out, again,
        "save/restore must reproduce audio bit-exactly"
    );
    // Live hosted control reaches DSP: curve/audition set + get round-trip
    // on the running chain and audio continues nonzero.
    let mut live = create_plugin(
        &shaped_config.plugin_type,
        &shaped_config.parameters,
        CHANNELS,
        RATE,
    )
    .unwrap();
    live.initialize(f64::from(RATE)).unwrap();
    live.set_parameter(ParameterId::from("curve_mid"), ParameterValue::Float(0.0))
        .unwrap();
    live.set_parameter(
        ParameterId::from("audition_residual"),
        ParameterValue::Bool(true),
    )
    .unwrap();
    assert_eq!(
        live.get_parameter(&ParameterId::from("curve_mid")),
        Some(ParameterValue::Float(0.0))
    );
    assert_eq!(
        live.get_parameter(&ParameterId::from("audition_residual")),
        Some(ParameterValue::Bool(true))
    );
    let mut probe = vec![f32::NAN; BLOCK_FRAMES * CHANNELS];
    let processed = live
        .process(
            &input[..BLOCK_FRAMES * CHANNELS],
            &mut probe,
            &ProcessContext::new(RATE, BLOCK_FRAMES),
        )
        .unwrap();
    assert_eq!(processed, BLOCK_FRAMES);
    assert_nonzero_finite(&probe, "live-controlled chain");
}

#[test]
fn engine_denoiser_legacy_settings_keep_defaults_metadata_and_audio() {
    // 29-key-era JSON: no curve/audition keys at all.
    let legacy: PluginSettings = serde_json::from_value(serde_json::json!({
        "Denoiser": {
            "reduction_db": 12.0,
            "spatial_strength": 0.5
        }
    }))
    .unwrap();
    // Exhaustive: pins the owned settings shape including the new fields.
    let PluginSettings::Denoiser {
        reduction_db,
        floor_db,
        smoothing: _smoothing,
        attack_ms: _attack_ms,
        release_ms: _release_ms,
        low_latency: _low_latency,
        polyphonic_detection: _polyphonic_detection,
        mcra_alpha_s: _mcra_alpha_s,
        mcra_alpha_p: _mcra_alpha_p,
        mcra_l: _mcra_l,
        mcra_delta: _mcra_delta,
        transparency: _transparency,
        dd_enabled: _dd_enabled,
        dd_alpha: _dd_alpha,
        psychoacoustic_masking: _psychoacoustic_masking,
        spectral_smoothing_enabled: _spectral_smoothing_enabled,
        temporal_smoothing_enabled: _temporal_smoothing_enabled,
        spectral_sub_enabled: _spectral_sub_enabled,
        spectral_sub_alpha: _spectral_sub_alpha,
        spectral_sub_beta: _spectral_sub_beta,
        learn_noise: _learn_noise,
        use_captured_profile: _use_captured_profile,
        clear_profile: _clear_profile,
        formant_preservation: _formant_preservation,
        formant_strength: _formant_strength,
        multi_resolution: _multi_resolution,
        harmonic_percussive: _harmonic_percussive,
        spatial_denoise: _spatial_denoise,
        spatial_strength,
        curve_low,
        curve_mid,
        curve_high,
        audition_residual,
    } = &legacy
    else {
        panic!("legacy preset must deserialize to Denoiser settings")
    };
    assert_eq!(*reduction_db, 12.0);
    assert_eq!(*spatial_strength, 0.5);
    assert_eq!(*curve_low, 1.0);
    assert_eq!(*curve_mid, 1.0);
    assert_eq!(*curve_high, 1.0);
    assert!(!*audition_residual);
    assert_eq!(*floor_db, -20.0);
    // Metadata: 33 specs, appended ids in exact PARAMS 29-32 order.
    let specs = legacy.param_specs();
    assert_eq!(specs.len(), 33);
    for (index, key) in [
        (29, "curve_low"),
        (30, "curve_mid"),
        (31, "curve_high"),
        (32, "audition_residual"),
    ] {
        assert_eq!(specs[index].engine_key, key, "spec {index} moved");
    }
    assert!(legacy.layout().is_some(), "CURVE/MONITOR layout must flow");
    // Accessor get/set in exact order, with spec-clamped adjust.
    assert_eq!(legacy.param_value(29), Some(1.0));
    assert_eq!(legacy.param_value(30), Some(1.0));
    assert_eq!(legacy.param_value(31), Some(1.0));
    assert_eq!(legacy.param_value(32), Some(0.0));
    let mut edited = legacy.clone();
    edited.set_param_value(29, 0.25);
    edited.set_param_value(32, 1.0);
    assert_eq!(edited.param_value(29), Some(0.25));
    assert_eq!(edited.param_value(32), Some(1.0));
    // adjust_f64 advances delta * spec-step per call (curve step 0.01), so
    // one +10 reaches exactly 0.25 + 10 * 0.01 (same ops, same order):
    // clamp needs derived overshoot, applied as bounded stepping.
    assert!(edited.adjust_param_value(29, 10.0));
    assert_eq!(edited.param_value(29), Some(0.25 + 10.0 * 0.01));
    let mut ups = 0;
    while edited.param_value(29) != Some(1.0) && ups < 1000 {
        assert!(edited.adjust_param_value(29, 10.0));
        ups += 1;
    }
    assert_eq!(
        edited.param_value(29),
        Some(1.0),
        "up-clamp after {ups} steps"
    );
    let mut downs = 0;
    while edited.param_value(29) != Some(0.0) && downs < 1000 {
        assert!(edited.adjust_param_value(29, -10.0));
        downs += 1;
    }
    assert_eq!(
        edited.param_value(29),
        Some(0.0),
        "down-clamp after {downs} steps"
    );
    // Legacy construction renders exactly like explicit defaults. The
    // legacy preset pins a non-default reduction (12 dB vs the 10 dB
    // PARAMS default), so the explicit baseline carries the same pin;
    // every other key must already agree (the macro-generated serde
    // mirrors read the same PARAMS as `default_for`), proven
    // field-by-field before the audio oracle runs.
    let input = lcg_noise(8192, CHANNELS, 0.05, 0x1e6ac7);
    let (legacy_out, _) = render_hosted_through_settings(&legacy, CHANNELS, &input);
    let mut explicit = denoiser_settings_with(1.0, 1.0, 1.0, false);
    let PluginSettings::Denoiser { reduction_db, .. } = &mut explicit else {
        panic!("explicit baseline must be Denoiser settings")
    };
    *reduction_db = 12.0;
    assert_denoiser_settings_json_eq(&legacy, &explicit);
    let (explicit_out, _) = render_hosted_through_settings(&explicit, CHANNELS, &input);
    assert_nonzero_finite(&legacy_out, "legacy path");
    assert_audio_bit_exact(
        &legacy_out,
        &explicit_out,
        "legacy defaults must preserve old audio bit-exactly",
    );
}

#[test]
fn engine_denoiser_rejected_candidate_retains_live_chain_history() {
    // Accepted config with populated DSP history on a live chain plus an
    // uninterrupted reference twin.
    let accepted = denoiser_settings_with(0.25, 0.5, 0.75, false);
    let accepted_json = serde_json::to_vec(&accepted).unwrap();
    let config = accepted.to_plugin_config(f64::from(RATE));
    let mut live = create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    live.initialize(f64::from(RATE)).unwrap();
    let mut reference =
        create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    reference.initialize(f64::from(RATE)).unwrap();
    let (input, _) = tone_plus_noise(8 * BLOCK_FRAMES, CHANNELS, 0xace97);
    let mut live_out = vec![0.0; input.len()];
    let mut ref_out = vec![0.0; input.len()];
    for block in 0..4 {
        let range = block * BLOCK_FRAMES * CHANNELS..(block + 1) * BLOCK_FRAMES * CHANNELS;
        let context = ProcessContext::new(RATE, BLOCK_FRAMES);
        assert_eq!(
            live.process(
                &input[range.clone()],
                &mut live_out[range.clone()],
                &context
            )
            .unwrap(),
            BLOCK_FRAMES
        );
        assert_eq!(
            reference
                .process(&input[range.clone()], &mut ref_out[range], &context)
                .unwrap(),
            BLOCK_FRAMES
        );
    }
    assert_eq!(
        &live_out[..4 * BLOCK_FRAMES * CHANNELS],
        &ref_out[..4 * BLOCK_FRAMES * CHANNELS]
    );
    // Rejected candidates: out-of-range knots fail factory construction
    // with field-named errors; a wrong type fails JSON parsing. Each knot
    // travels the actual settings -> serde -> converter -> factory route
    // (not factory JSON alone), so converter passthrough is proven too.
    for (mutated, bad_value) in [("curve_low", 1.5), ("curve_mid", -0.5), ("curve_high", 2.0)] {
        let mut bad_settings = accepted.clone();
        let PluginSettings::Denoiser {
            curve_low,
            curve_mid,
            curve_high,
            ..
        } = &mut bad_settings
        else {
            panic!("Denoiser default must have Denoiser settings")
        };
        match mutated {
            "curve_low" => *curve_low = bad_value,
            "curve_mid" => *curve_mid = bad_value,
            _ => *curve_high = bad_value,
        }
        let encoded = serde_json::to_vec(&bad_settings).unwrap();
        let restored: PluginSettings = serde_json::from_slice(&encoded).unwrap();
        let bad_config = restored.to_plugin_config(f64::from(RATE));
        let error = create_plugin(
            &bad_config.plugin_type,
            &bad_config.parameters,
            CHANNELS,
            RATE,
        )
        .err()
        .unwrap_or_else(|| panic!("{mutated} out of range must fail construction"));
        assert!(
            error.contains(mutated),
            "rejection must name the field, got: {error}"
        );
        let mut bad = config.parameters.clone();
        bad[mutated] = serde_json::json!(bad_value);
        let error = create_plugin(&config.plugin_type, &bad, CHANNELS, RATE)
            .err()
            .unwrap_or_else(|| panic!("{mutated} out of range must fail construction"));
        assert!(
            error.contains(mutated),
            "rejection must name the field, got: {error}"
        );
    }
    let mut mistyped = config.parameters.clone();
    mistyped["curve_low"] = serde_json::json!("0.5");
    assert!(
        create_plugin(&config.plugin_type, &mistyped, CHANNELS, RATE).is_err(),
        "mistyped knot must fail JSON parsing"
    );
    // The live chain never resets: its populated history continues
    // bit-identical to the uninterrupted twin after every rejection.
    for block in 4..8 {
        let range = block * BLOCK_FRAMES * CHANNELS..(block + 1) * BLOCK_FRAMES * CHANNELS;
        let context = ProcessContext::new(RATE, BLOCK_FRAMES);
        assert_eq!(
            live.process(
                &input[range.clone()],
                &mut live_out[range.clone()],
                &context
            )
            .unwrap(),
            BLOCK_FRAMES
        );
        assert_eq!(
            reference
                .process(&input[range.clone()], &mut ref_out[range], &context)
                .unwrap(),
            BLOCK_FRAMES
        );
    }
    assert_eq!(
        live_out, ref_out,
        "rejected candidates must not disturb live DSP history"
    );
    assert_nonzero_finite(&live_out, "retained live chain");
    // The accepted settings object is untouched by the rejected candidates.
    assert_eq!(serde_json::to_vec(&accepted).unwrap(), accepted_json);
}

#[test]
fn engine_denoiser_profile_learn_and_use_end_to_end() {
    // Capture completes through the hosted factory object: the learn
    // trigger runs the live capture engine on noise-only input.
    let settings = denoiser_settings_with(1.0, 1.0, 1.0, false);
    let config = settings.to_plugin_config(f64::from(RATE));
    let mut profiled =
        create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    profiled.initialize(f64::from(RATE)).unwrap();
    profiled
        .set_parameter(ParameterId::from("learn_noise"), ParameterValue::Bool(true))
        .unwrap();
    // 24 whole blocks (96 STFT hops): capture needs 47 hops at 48 kHz.
    let noise = lcg_noise(24 * BLOCK_FRAMES, CHANNELS, 0.05, 0x90a115);
    let mut sink = vec![0.0; BLOCK_FRAMES * CHANNELS];
    for chunk in noise.chunks(BLOCK_FRAMES * CHANNELS) {
        assert_eq!(
            profiled
                .process(chunk, &mut sink, &ProcessContext::new(RATE, BLOCK_FRAMES))
                .unwrap(),
            BLOCK_FRAMES
        );
    }
    let data = profiled
        .get_data()
        .unwrap()
        .downcast::<DenoiserData>()
        .unwrap();
    assert!(data.has_captured_profile, "hosted capture must complete");
    assert!(data.using_captured_profile, "capture must engage use");
    drop(data);
    // Reset stream state (the captured profile survives reset by design) so
    // the profiled/unprofiled mixture comparison below differs only by the
    // captured profile, not by capture-era DSP history.
    profiled.reset();
    // The captured profile audibly engages DSP on a mixture: profiled vs
    // unprofiled twins diverge, both render nonzero audio.
    let (input, _) = tone_plus_noise(16_384, CHANNELS, 0x90a115);
    let mut profiled_out = vec![f32::NAN; input.len()];
    for start in (0..16_384).step_by(BLOCK_FRAMES) {
        let range = start * CHANNELS..(start + BLOCK_FRAMES) * CHANNELS;
        assert_eq!(
            profiled
                .process(
                    &input[range.clone()],
                    &mut profiled_out[range],
                    &ProcessContext::new(RATE, BLOCK_FRAMES)
                )
                .unwrap(),
            BLOCK_FRAMES
        );
    }
    let mut unprofiled =
        create_plugin(&config.plugin_type, &config.parameters, CHANNELS, RATE).unwrap();
    unprofiled.initialize(f64::from(RATE)).unwrap();
    let mut unprofiled_out = vec![f32::NAN; input.len()];
    for start in (0..16_384).step_by(BLOCK_FRAMES) {
        let range = start * CHANNELS..(start + BLOCK_FRAMES) * CHANNELS;
        assert_eq!(
            unprofiled
                .process(
                    &input[range.clone()],
                    &mut unprofiled_out[range],
                    &ProcessContext::new(RATE, BLOCK_FRAMES)
                )
                .unwrap(),
            BLOCK_FRAMES
        );
    }
    assert_nonzero_finite(&profiled_out, "profiled mixture");
    assert_nonzero_finite(&unprofiled_out, "unprofiled mixture");
    assert_ne!(
        profiled_out, unprofiled_out,
        "captured profile must audibly engage DSP"
    );
    // The use-captured-profile control forwards through settings.
    let mut use_settings = denoiser_settings_with(1.0, 1.0, 1.0, false);
    let PluginSettings::Denoiser {
        use_captured_profile,
        ..
    } = &mut use_settings
    else {
        panic!("Denoiser default must have Denoiser settings")
    };
    *use_captured_profile = true;
    let use_config = use_settings.to_plugin_config(f64::from(RATE));
    assert_eq!(use_config.parameters["use_captured_profile"], true);
    let use_plugin = create_plugin(
        &use_config.plugin_type,
        &use_config.parameters,
        CHANNELS,
        RATE,
    )
    .unwrap();
    assert_eq!(
        use_plugin.get_parameter(&ParameterId::from("use_captured_profile")),
        Some(ParameterValue::Bool(true))
    );
    // NOTE (documented gap, not claimed): denoiser captured profiles are
    // in-memory DSP state with no persisted blob/file carrier (unlike Hiss
    // v1/v2 `captured_profile`). Settings save/reload cannot carry profile
    // bytes until the owned crate defines a persisted carrier; only the
    // learn/use controls travel end to end today.
}
