// ============================================================================
// Shared Parameter Bridge Integration Tests
// ============================================================================
//
// These tests exercise two direct DSP instances with different parameter paths.
// They do not load a CLAP, VST3, or AU binary. Native format tests live in
// sotf-host/tests/native_* and plugins-nih/src/wrapper/transport/tests.rs.
//
// Tests three critical invariants:
// 1. Parameter normalize/denormalize round-trip accuracy
// 2. Audio output equivalence: bridge path vs direct path
// 3. State save/load round-trip preserves parameters
//
// These tests catch:
// - Log/linear normalization mismatches
// - Parameter clamping differences between bridge and plugin
// - State serialization losing precision

use plugins_bridge::factory::{available_plugin_types, create_plugin};
use plugins_bridge::param_bridge::ParamBridge;
use sotf_host::param_specs::ParamType;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};

const SAMPLE_RATE: u32 = 48000;
const NUM_FRAMES: usize = 1024;
const CHANNELS: usize = 2;

/// Generate a stereo test signal (440Hz sine + 1kHz sine)
fn test_signal(num_frames: usize, channels: usize) -> Vec<f32> {
    let mut buf = vec![0.0f32; num_frames * channels];
    for i in 0..num_frames {
        let t = i as f32 / SAMPLE_RATE as f32;
        let sample = 0.25 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
            + 0.15 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
        for ch in 0..channels {
            buf[i * channels + ch] = sample;
        }
    }
    buf
}

/// Get ParamSpec for a plugin type (via its PARAMS constant)
fn get_param_specs(plugin_type: &str) -> &'static [sotf_host::param_specs::ParamSpec] {
    match plugin_type {
        "Gain" => sotf_plugin_gain::params::PARAMS,
        "Limiter" => sotf_plugin_limiter::params::PARAMS,
        "Gate" => sotf_plugin_gate::params::PARAMS,
        "Delay" => sotf_plugin_delay::params::PARAMS,
        "Crossfeed" => sotf_plugin_crossfeed::params::PARAMS,
        "Saturation" => sotf_plugin_saturation::params::PARAMS,
        "Denoiser" => sotf_plugin_denoiser::params::PARAMS,
        "ChannelMuteSolo" => sotf_plugin_channel_mute_solo::params::PARAMS,
        "PND" => sotf_plugin_pnd::params::PARAMS,
        "MonoToStereo" => sotf_plugin_mono_to_stereo::params::PARAMS,
        "StereoImager" => sotf_plugin_stereo_imager::params::PARAMS,
        "TransientShaper" => sotf_plugin_transient_shaper::params::PARAMS,
        "ABCompare" => sotf_plugin_ab_compare::params::PARAMS,
        "Dither" => sotf_plugin_dither::params::PARAMS,
        _ => &[], // Plugins with dynamic/complex params (EQ, Upmixer, etc.)
    }
}

/// Explicit coverage list for the direct shared-bridge integration.
fn testable_plugins() -> Vec<&'static str> {
    vec![
        "Gain",
        "Limiter",
        "Gate",
        "Delay",
        "Saturation",
        "ChannelMuteSolo",
        "PND",
        "StereoImager",
        "TransientShaper",
        "ABCompare",
        "Dither",
    ]
}

// ============================================================================
// Test 1: Parameter normalize/denormalize round-trip
// ============================================================================

#[test]
fn test_param_normalize_denormalize_roundtrip() {
    let mut failures = Vec::new();

    for &plugin_type in &testable_plugins() {
        let specs = get_param_specs(plugin_type);
        assert!(
            !specs.is_empty(),
            "{plugin_type} needs explicit parameter coverage"
        );

        let bridge = ParamBridge::new(specs);

        for idx in 0..bridge.count() {
            let info = bridge.info(idx).unwrap();
            let spec = bridge.spec(idx).unwrap();

            // Skip FilePath params — they can't be meaningfully normalized
            if matches!(spec.param_type, ParamType::FilePath) {
                continue;
            }

            // Test several values across the range
            for &normalized in &[0.0, 0.25, 0.5, 0.75, 1.0] {
                let raw = bridge.denormalize(idx, normalized).unwrap();
                let renormalized = bridge.normalize(idx, raw).unwrap();
                let tolerance = if info.steps > 0 && info.logarithmic {
                    // Log-scale with quantization: step rounding in log space
                    // causes larger normalized error than linear step rounding
                    0.05
                } else if info.steps > 0 {
                    // Discrete params may quantize — allow 1 step of error
                    1.0 / (info.steps as f64).max(1.0) + 1e-6
                } else if info.logarithmic {
                    // Log-scale round-trip has inherent precision loss
                    0.02
                } else {
                    1e-4
                };

                if (renormalized - normalized).abs() > tolerance {
                    failures.push(format!(
                        "{plugin_type}/{} (idx {idx}): norm {normalized:.4} -> raw {raw:.4} -> renorm {renormalized:.4} (diff {:.6}, tol {tolerance:.6})",
                        info.name,
                        (renormalized - normalized).abs()
                    ));
                }
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "Parameter round-trip failures ({}):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
}

// ============================================================================
// Test 2: Audio output equivalence — direct vs bridge
// ============================================================================

fn nondefault_setting(plugin_type: &str) -> (&'static str, ParameterValue) {
    match plugin_type {
        "Gain" => ("gain_db", ParameterValue::Float(-6.0)),
        "Limiter" => ("threshold", ParameterValue::Float(-12.0)),
        "Gate" => ("threshold", ParameterValue::Float(-18.0)),
        "Delay" => ("delay_ms", ParameterValue::Float(17.0)),
        "Saturation" => ("drive", ParameterValue::Float(3.7)),
        "ChannelMuteSolo" => ("dim_gain_db", ParameterValue::Float(-12.0)),
        "PND" => ("correction_strength", ParameterValue::Float(0.65)),
        "StereoImager" => ("width", ParameterValue::Float(0.4)),
        "TransientShaper" => ("output_gain", ParameterValue::Float(-3.0)),
        "ABCompare" => ("mix", ParameterValue::Float(0.6)),
        "Dither" => ("bit_depth", ParameterValue::Int(1)),
        _ => panic!("Missing explicit fixture for {plugin_type}"),
    }
}

fn prepared_plugin(plugin_type: &str) -> Box<dyn Plugin> {
    let mut plugin = create_plugin(plugin_type, CHANNELS, f64::from(SAMPLE_RATE), "{}")
        .unwrap_or_else(|error| panic!("{plugin_type} construction: {error}"));
    plugin
        .initialize(f64::from(SAMPLE_RATE))
        .unwrap_or_else(|error| panic!("{plugin_type} initialization: {error}"));
    assert_eq!(plugin.input_channels(), CHANNELS);
    assert_eq!(plugin.output_channels(), CHANNELS);
    plugin
}

fn compare_audio<'a>(plugin_type: &str, direct: &'a mut dyn Plugin, bridged: &'a mut dyn Plugin) {
    // Distinct channels expose stereo mapping errors. Run long enough to exceed
    // the largest analysis latency in this explicit fixture list.
    let mut signal = test_signal(NUM_FRAMES, CHANNELS);
    for frame in signal.as_chunks_mut::<CHANNELS>().0 {
        frame[1] *= -0.7;
    }
    let mut direct_output = vec![f32::NAN; signal.len()];
    let mut bridge_output = direct_output.clone();
    let mut energy = 0.0_f64;
    for block in 0..64 {
        direct_output.fill(f32::NAN);
        bridge_output.fill(f32::NAN);
        let mut context = ProcessContext::new(SAMPLE_RATE, NUM_FRAMES);
        context.transport.sample_position = (block * NUM_FRAMES) as u64;
        for (plugin, output) in [
            (&mut *direct, &mut direct_output),
            (&mut *bridged, &mut bridge_output),
        ] {
            let frames = plugin
                .process(&signal, output, &context)
                .unwrap_or_else(|error| panic!("{plugin_type} block {block}: {error}"));
            assert_eq!(frames, NUM_FRAMES, "{plugin_type} block {block}");
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "{plugin_type} unwritten/nonfinite output"
            );
        }
        for (a, b) in direct_output.iter().zip(&bridge_output) {
            assert!(
                (a - b).abs() <= 1e-6,
                "{plugin_type} block {block}: {a} versus {b}"
            );
            energy += f64::from(*a).powi(2);
        }
    }
    assert!(
        energy > 1e-3,
        "{plugin_type}: identical silence is insufficient evidence"
    );
}

#[test]
fn nondefault_normalized_parameters_match_direct_typed_audio() {
    for plugin_type in testable_plugins() {
        let mut direct = prepared_plugin(plugin_type);
        let mut bridged = prepared_plugin(plugin_type);
        let bridge = ParamBridge::new(get_param_specs(plugin_type));
        let (key, value) = nondefault_setting(plugin_type);
        let id = ParameterId::from(key);
        assert_ne!(
            direct.get_parameter(&id),
            Some(value.clone()),
            "{plugin_type} fixture must be nondefault"
        );
        direct.set_parameter(id.clone(), value.clone()).unwrap();
        let raw = match value {
            ParameterValue::Float(value) => f64::from(value),
            ParameterValue::Int(value) => f64::from(value),
            ParameterValue::Bool(value) => f64::from(value),
            _ => unreachable!(),
        };
        let index = bridge.find_index(key).unwrap();
        bridge
            .set_normalized(&mut *bridged, index, bridge.normalize(index, raw).unwrap())
            .unwrap();
        assert_eq!(direct.get_parameter(&id), Some(value.clone()));
        assert_eq!(bridged.get_parameter(&id), Some(value));
        compare_audio(plugin_type, &mut *direct, &mut *bridged);
    }
}

// ============================================================================
// Test 3: Parameter bridge covers all factory plugin types
// ============================================================================

#[test]
fn supported_default_factory_configurations_create_successfully() {
    let mut failures = Vec::new();

    // Plugins that require non-empty config JSON (mandatory fields with no serde default)
    let needs_config: &[&str] = &[
        "Convolution", // requires ir_file
        "Downmix",     // requires input_channels
        "Binaural",    // requires input_channels
        "Crossover",   // requires type
    ];

    for &plugin_type in available_plugin_types() {
        if needs_config.contains(&plugin_type) {
            continue;
        }
        // MonoToStereo has a deliberately fixed one-channel input contract;
        // all other factory entries use the stereo smoke-test layout.
        let input_channels = match plugin_type {
            "AmbisonicsDecoder" => 4,
            "MonoToStereo" => 1,
            _ => 2,
        };
        match create_plugin(plugin_type, input_channels, f64::from(SAMPLE_RATE), "{}") {
            Ok(mut plugin) => {
                if let Err(e) = plugin.initialize(f64::from(SAMPLE_RATE)) {
                    failures.push(format!("{plugin_type}: initialize failed: {e}"));
                }
            }
            Err(e) => {
                failures.push(format!("{plugin_type}: create failed: {e}"));
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "Factory plugin creation failures ({}):\n  {}",
            failures.len(),
            failures.join("\n  ")
        );
    }
}

#[test]
fn spectral_compressor_bridge_factory_is_fallible_and_preserves_advanced_state() {
    let plugin = create_plugin(
        "SpectralCompressor",
        2,
        48_000.0,
        r#"{"target_mode":2,"adaptive_threshold":true,"adaptive_offset_db":3.0,"channel_link":1.0}"#,
    )
    .expect("valid complete spectral compressor state");
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("target_mode")),
        Some(ParameterValue::Int(2))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("channel_link")),
        Some(ParameterValue::Float(1.0))
    );
    assert!(
        create_plugin(
            "SpectralCompressor",
            2,
            48_000.0,
            r#"{"spectral_smoothing":2.0}"#,
        )
        .is_err()
    );
}

// ============================================================================
// Test 4: Bridge parameter set/get round-trip on live plugin
// ============================================================================

#[test]
fn test_bridge_set_get_roundtrip_on_plugin() {
    for plugin_type in testable_plugins() {
        let mut plugin = prepared_plugin(plugin_type);
        let bridge = ParamBridge::new(get_param_specs(plugin_type));
        assert!(bridge.count() > 0, "{plugin_type} needs explicit coverage");
        for index in 0..bridge.count() {
            let spec = bridge.spec(index).unwrap();
            if matches!(spec.param_type, ParamType::FilePath) {
                continue;
            }
            // Structural controls are tested with lifecycle reconstruction in
            // the NIH/FFI suites. Here, verify their readable current setting.
            let normalized = if spec.update_mode == sotf_host::param_specs::UpdateMode::Realtime {
                0.6
            } else {
                bridge
                    .get_normalized(&*plugin, index)
                    .expect("structural getter")
            };
            let raw = bridge.denormalize(index, normalized).unwrap();
            let expected = bridge.normalize(index, raw).unwrap();
            bridge
                .set_normalized(&mut *plugin, index, normalized)
                .unwrap_or_else(|error| panic!("{plugin_type}/{}: {error}", spec.engine_key));
            let actual = bridge
                .get_normalized(&*plugin, index)
                .unwrap_or_else(|| panic!("{plugin_type}/{} has no getter", spec.engine_key));
            assert!(
                (actual - expected).abs() < 1e-6,
                "{plugin_type}/{}: actual {actual} versus quantized {expected}",
                spec.engine_key
            );
        }
    }
}

// ============================================================================
// Test 5: State save/load round-trip preserves output
// ============================================================================

#[test]
fn nondefault_state_restore_preserves_parameters_and_audio() {
    for plugin_type in testable_plugins() {
        let mut original = prepared_plugin(plugin_type);
        let (key, value) = nondefault_setting(plugin_type);
        let id = ParameterId::from(key);
        original.set_parameter(id.clone(), value.clone()).unwrap();
        let state = plugins_bridge::state::save_state(&*original);
        assert!(!state.is_empty());
        let mut restored = prepared_plugin(plugin_type);
        plugins_bridge::state::load_state(&mut *restored, &state)
            .unwrap_or_else(|error| panic!("{plugin_type} restore: {error}"));
        assert_eq!(restored.get_parameter(&id), Some(value));
        for parameter in original.parameters() {
            let expected = original
                .get_parameter(&parameter.id)
                .unwrap_or_else(|| panic!("{plugin_type}/{} unreadable", parameter.id));
            assert_eq!(
                restored.get_parameter(&parameter.id),
                Some(expected),
                "{plugin_type}/{}",
                parameter.id
            );
        }
        original.reset();
        restored.reset();
        compare_audio(plugin_type, &mut *original, &mut *restored);
    }
}

#[test]
fn unchanged_limiter_restore_preserves_audio_already_in_flight() {
    let mut original = prepared_plugin("Limiter");
    let mut restored = prepared_plugin("Limiter");
    let input = test_signal(127, CHANNELS);
    let mut output = vec![0.0; input.len()];
    for plugin in [&mut original, &mut restored] {
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(SAMPLE_RATE, 127))
                .unwrap(),
            127
        );
    }
    let state = plugins_bridge::state::save_state(&*restored);
    plugins_bridge::state::load_state(&mut *restored, &state).unwrap();
    // The 127-frame prefix is shorter than the default 240-frame lookahead.
    // Any reset or unnecessary rebuild would discard its queued signal.
    compare_audio("Limiter", &mut *original, &mut *restored);
}
