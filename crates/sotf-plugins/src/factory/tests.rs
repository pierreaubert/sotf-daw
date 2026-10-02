use super::catalog::catalog_entry;
use super::create::create_plugin;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::create::create_plugin_with_sandbox_grants;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::create::create_plugin_with_sandbox_grants_for_backend;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::create::create_plugin_with_sandbox_grants_for_backend_and_launcher;
use super::is::is_supported_plugin_type;
use super::parse::parse_external_plugin_descriptor;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::parse::parse_isolated_external_plugin_config;
use super::validate::validate_plugin_security_config;
use crate::{
    ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::{ExternalPluginSandboxTiming, ExternalPluginTrust};
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::Plugin;
use std::path::PathBuf;

use tempfile::tempdir;

mod misc;

#[test]
fn band_merge_factory_rejects_invalid_or_unknown_state() {
    for (channels, parameters) in [
        (0, serde_json::json!({"bands": 2})),
        (2, serde_json::json!({"bands": 1})),
        (2, serde_json::json!({"bands": 9})),
        (2, serde_json::json!({"bands": 2, "band_gains_db": [25.0]})),
        (2, serde_json::json!({"bands": 2, "obsolete": true})),
    ] {
        assert!(
            create_plugin("band_merge", &parameters, channels, 48_000).is_err(),
            "invalid Band Merge preset was accepted: {parameters}"
        );
    }
    let plugin = create_plugin(
        "band_merge",
        &serde_json::json!({
            "bands": 4,
            "band_gains_db": [0.0, -3.0, 2.0, 0.0],
            "band_mutes": [false, true, false, false]
        }),
        8,
        48_000,
    )
    .expect("valid Band Merge preset must construct");
    assert_eq!(plugin.input_channels(), 8);
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn convolution_factory_reconstructs_opt_in_true_stereo_and_keeps_neutral_prehydration() {
    use sotf_plugin_convolution::params::Params as ConvolutionState;

    for invalid in [serde_json::json!("yes"), serde_json::Value::Null] {
        let parameters = serde_json::json!({
            "ir_file": "",
            "mix": 1.0,
            "gain_db": 0.0,
            "true_stereo": invalid
        });
        assert!(
            create_plugin("convolution", &parameters, 2, 48_000).is_err(),
            "non-boolean true_stereo value was accepted: {parameters}"
        );
    }

    for channels in [1, 3] {
        assert!(
            create_plugin(
                "convolution",
                &serde_json::json!({
                    "ir_file": "",
                    "mix": 1.0,
                    "gain_db": 0.0,
                    "true_stereo": true
                }),
                channels,
                48_000,
            )
            .is_err(),
            "true-stereo mode accepted {channels} plugin channels"
        );
    }

    let state = ConvolutionState {
        true_stereo: true,
        ..ConvolutionState::default()
    };
    let mut persisted = serde_json::to_value(state).expect("serialize host convolution state");
    persisted
        .as_object_mut()
        .expect("convolution state serializes as an object")
        .insert("ir_file".into(), serde_json::Value::String(String::new()));

    let mut plugin = create_plugin("convolution", &persisted, 2, 48_000)
        .expect("persisted true-stereo state should construct through the factory");
    let id = ParameterId::from("true_stereo");
    assert_eq!(plugin.get_parameter(&id), Some(ParameterValue::Bool(true)));
    assert!(
        plugin
            .set_parameter(id, ParameterValue::Bool(false))
            .is_err(),
        "structural routing mode must require reconstruction"
    );

    // A matrix-configured plugin with no IR is a defined neutral pre-load route:
    // it retains the normal NUPC dry latency and then reproduces both channels.
    plugin.initialize(48_000).unwrap();
    let latency = plugin.latency_samples();
    let frames = latency + 64;
    let mut input = vec![0.0_f32; frames * 2];
    input[0] = 0.5;
    input[1] = -0.25;
    input[14] = -0.75;
    input[15] = 0.125;
    let mut output = vec![0.0_f32; input.len()];
    plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(48_000, frames),
        )
        .unwrap();
    assert!(
        output[..latency * 2]
            .iter()
            .all(|sample| sample.abs() < 1e-7)
    );
    for frame in 0..64 {
        assert_eq!(output[(frame + latency) * 2], input[frame * 2]);
        assert_eq!(output[(frame + latency) * 2 + 1], input[frame * 2 + 1]);
    }

    let legacy = create_plugin(
        "convolution",
        &serde_json::json!({"ir_file": "", "mix": 1.0, "gain_db": 0.0}),
        2,
        48_000,
    )
    .expect("old convolution preset should still construct");
    assert_eq!(
        legacy.get_parameter(&ParameterId::from("true_stereo")),
        Some(ParameterValue::Bool(false))
    );
}

#[test]
fn band_split_factory_rejects_invalid_topology_and_unknown_state() {
    assert!(create_plugin("band_split", &serde_json::json!({}), 0, 48_000).is_err());
    for parameters in [
        serde_json::json!({"frequencies": [500.0, 500.0]}),
        serde_json::json!({"frequencies": [2_000.0, 500.0]}),
        serde_json::json!({"frequency": 0.0}),
        serde_json::json!({"type": "LR96"}),
        serde_json::json!({"obsolete_split_field": true}),
    ] {
        assert!(
            create_plugin("band_split", &parameters, 2, 48_000).is_err(),
            "invalid Band Split preset was accepted: {parameters}"
        );
    }
    let plugin = create_plugin(
        "band_split",
        &serde_json::json!({"frequencies": [500.0, 2_000.0, 8_000.0], "type": "LR48"}),
        12,
        48_000,
    )
    .expect("valid Band Split preset must construct");
    assert_eq!(plugin.input_channels(), 12);
    assert_eq!(plugin.output_channels(), 48);
}

#[test]
fn band_split_factory_normalizes_toolbar_choice_forms_without_reinterpreting_counts() {
    let mode_forms = [
        (serde_json::json!(0), 0),
        (serde_json::json!(1), 1),
        (serde_json::json!("Legacy Cascade"), 0),
        (serde_json::json!("Phase Compensated"), 1),
    ];
    for (mode_form, expected_mode) in mode_forms {
        for (band_label, expected_bands) in [("2 Bands", 2), ("3 Bands", 3), ("4 Bands", 4)] {
            let plugin = create_plugin(
                "band_split",
                &serde_json::json!({
                    "recombination_mode": mode_form,
                    "num_bands": band_label
                }),
                2,
                48_000,
            )
            .expect("toolbar choice labels and mode indices must construct");
            assert_eq!(plugin.output_channels(), 2 * expected_bands);
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("recombination_mode")),
                Some(ParameterValue::Int(expected_mode))
            );
        }
    }

    // Constructor configs already use actual counts. Preserve those values,
    // including 2, instead of interpreting them as toolbar choice indices.
    for band_count in 2..=4 {
        let plugin = create_plugin(
            "band_split",
            &serde_json::json!({ "num_bands": band_count }),
            2,
            48_000,
        )
        .expect("actual constructor band counts remain valid");
        assert_eq!(plugin.output_channels(), 2 * band_count);
    }
    for invalid_count in [0, 1, 5] {
        assert!(
            create_plugin(
                "band_split",
                &serde_json::json!({ "num_bands": invalid_count }),
                2,
                48_000,
            )
            .is_err(),
            "invalid actual band count {invalid_count} must remain rejected"
        );
    }
}

#[test]
fn gate_factory_rejects_invalid_or_unknown_preset_state() {
    assert!(create_plugin("gate", &serde_json::json!({}), 0, 48_000).is_err());
    for parameters in [
        serde_json::json!({"attack_ms": 0.0}),
        serde_json::json!({"release_ms": 2_001.0}),
        serde_json::json!({"sidechain_hpf_order": "8th"}),
        serde_json::json!({"detection_mode": "average"}),
        serde_json::json!({"obsolete_gate_field": true}),
    ] {
        assert!(
            create_plugin("gate", &parameters, 2, 48_000).is_err(),
            "invalid Gate preset was accepted: {parameters}"
        );
    }

    let plugin = create_plugin(
        "gate",
        &serde_json::json!({
            "threshold_db": -35.0,
            "attack_ms": 2.0,
            "release_ms": 150.0,
            "detection_mode": "RMS",
            "sidechain_hpf_order": "4th"
        }),
        2,
        48_000,
    )
    .expect("valid Gate preset must construct through the factory");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn hiss_reducer_factory_validates_topology_rate_and_persisted_state() {
    assert!(create_plugin("hiss_reducer", &serde_json::json!({}), 0, 48_000).is_err());
    assert!(create_plugin("hiss_reducer", &serde_json::json!({}), 1, 0).is_err());
    assert!(
        create_plugin(
            "hiss_reducer",
            &serde_json::json!({"obsolete_fft_mode": true}),
            1,
            48_000,
        )
        .is_err()
    );

    let plugin = create_plugin(
        "hiss_reducer",
        &serde_json::json!({"frequency_hz": 16_000.0}),
        1,
        8_000,
    )
    .unwrap();
    assert_eq!(
        plugin
            .get_parameter(&ParameterId::from("frequency_hz"))
            .and_then(|value| value.as_float()),
        Some(3_600.0)
    );
}

#[test]
fn hiss_reducer_factory_carries_transient_guard_through_render_and_reload() {
    // Guard-on construction from JSON renders real audio; serializing the
    // construction parameters and recreating re-renders bit-exactly with
    // the guard still engaged.
    let parameters = serde_json::json!({
        "enabled": true,
        "threshold_db": -35.0,
        "frequency_hz": 3000.0,
        "strength": 0.7,
        "spectral_mode": true,
        "use_captured_profile": false,
        "curve_low": 1.0,
        "curve_mid": 1.0,
        "curve_high": 1.0,
        "link_mode": 0,
        "transient_guard": true,
    });
    let frames = 8192;
    let input: Vec<f32> = (0..frames)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 1000.0 * n as f32 / 48_000.0).sin() * 0.4
                + (2.0 * std::f32::consts::PI * 9000.0 * n as f32 / 48_000.0).sin() * 0.1
        })
        .collect();
    let render = |parameters: &serde_json::Value| {
        let mut plugin = create_plugin("hiss_reducer", parameters, 1, 48_000).unwrap();
        plugin.initialize(48_000).unwrap();
        let mut output = vec![f32::NAN; input.len()];
        assert_eq!(
            plugin
                .process(
                    &input,
                    &mut output,
                    &sotf_host::ProcessContext::new(48_000, frames),
                )
                .unwrap(),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        output
    };
    let first = render(&parameters);
    assert!(first.iter().any(|sample| *sample != 0.0));

    let saved = serde_json::to_string(&parameters).unwrap();
    let reloaded: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(reloaded["transient_guard"], true);
    assert_eq!(render(&reloaded), first);
    let rebuilt = create_plugin("hiss_reducer", &reloaded, 1, 48_000).unwrap();
    assert_eq!(
        rebuilt.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(true))
    );

    // Old-state JSON without the key constructs guard-off (serde default)
    // and renders the legacy path bit-identically to explicit false.
    let mut legacy = parameters.clone();
    assert!(
        legacy
            .as_object_mut()
            .unwrap()
            .remove("transient_guard")
            .is_some()
    );
    let legacy_plugin = create_plugin("hiss_reducer", &legacy, 1, 48_000).unwrap();
    assert_eq!(
        legacy_plugin.get_parameter(&ParameterId::from("transient_guard")),
        Some(ParameterValue::Bool(false))
    );
    let mut explicit_off = parameters.clone();
    explicit_off["transient_guard"] = serde_json::json!(false);
    assert_eq!(render(&legacy), render(&explicit_off));
}

#[test]
fn aec_catalog_factory_and_runtime_schema_are_canonical() {
    let entry = catalog_entry("aec").expect("AEC catalog entry");
    assert_eq!(entry.metadata.owning_crate, "sotf-plugin-aec");
    assert_eq!(
        entry.metadata.parameter_schema,
        super::catalog::PluginParameterSchema::Static("sotf_plugin_aec::params::PARAMS")
    );
    assert!(create_plugin("aec", &serde_json::json!({}), 1, 48_000).is_err());
    assert!(create_plugin("aec", &serde_json::json!({}), 3, 48_000).is_err());
    let plugin = create_plugin(
        "aec",
        &serde_json::json!({
            "echo_tail_ms": 100.0,
            "step_size": 0.4,
            "post_filter_enabled": false
        }),
        2,
        48_000,
    )
    .expect("canonical factory must construct AEC");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 1);
    assert_eq!(
        plugin.get_parameter(&sotf_host::parameters::ParameterId::from("step_size")),
        Some(sotf_host::parameters::ParameterValue::Float(0.4))
    );
}

#[test]
fn ambisonics_catalog_admits_every_supported_order() {
    let entry = catalog_entry("ambisonics_decoder").unwrap();
    assert_eq!(
        entry.metadata.channel_layout.supported_inputs,
        super::catalog::PluginSupportedInputLayouts::Enumerated(&[4, 9, 16, 25, 36, 49, 64])
    );
    for (order, channels, layout) in [
        (1, 4, "5.1"),
        (2, 9, "7.1.4"),
        (3, 16, "9.1.6"),
        (4, 25, "5.1"),
        (5, 36, "5.1"),
        (6, 49, "5.1"),
        (7, 64, "5.1"),
    ] {
        let plugin = create_plugin(
            "ambisonics_decoder",
            &serde_json::json!({"order": order, "target_layout": layout}),
            channels,
            48_000,
        )
        .unwrap();
        assert_eq!(plugin.input_channels(), channels);
    }
}

#[test]
fn dither_catalog_and_factory_are_canonical() {
    let entry = catalog_entry("dither").expect("dither catalog entry");
    assert_eq!(entry.metadata.owning_crate, "sotf-plugin-dither");
    assert_eq!(entry.metadata.exposed_name, "Dither");
    assert_eq!(
        entry.metadata.parameter_schema,
        super::catalog::PluginParameterSchema::Static("sotf_plugin_dither::params::PARAMS")
    );

    let plugin = create_plugin(
        "dither",
        &serde_json::json!({
            "bit_depth": 2,
            "noise_shaping": false,
            "dither_type": 1
        }),
        2,
        96_000,
    )
    .expect("canonical factory must construct Dither");
    assert_eq!(plugin.info().name, "Dither");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    assert_eq!(
        plugin.get_parameter(&sotf_host::parameters::ParameterId::from("bit_depth")),
        Some(sotf_host::parameters::ParameterValue::Int(2))
    );
}

#[test]
fn compressor_catalog_and_factory_expose_true_broadband_mode() {
    let entry = catalog_entry("compressor").expect("compressor catalog entry");
    assert_eq!(entry.metadata.exposed_name, "Compressor");
    assert_eq!(
        entry.metadata.parameter_schema,
        super::catalog::PluginParameterSchema::Static(
            "runtime broadband schema (unsupported legacy sidechain controls rejected)"
        )
    );

    let config = serde_json::json!({
        "threshold_db": -24.0,
        "ratio": 4.0,
        "attack_ms": 1.0,
        "release_ms": 40.0,
        "knee_db": 3.0
    });
    let mut plugin = create_plugin("compressor", &config, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();
    assert_eq!(plugin.info().name, "Compressor");
    assert!(
        plugin
            .parameters()
            .iter()
            .all(|parameter| parameter.id.as_str() != "num_bands"
                && !parameter.id.as_str().starts_with("crossover"))
    );

    let mut params: crate::MultibandCompressorPluginParams =
        serde_json::from_value(config).unwrap();
    params.num_bands = 1;
    let mut reference =
        crate::MultibandCompressorPlugin::try_from_params(2, params, 48_000).unwrap();
    sotf_host::ParametricInPlacePlugin::initialize(&mut reference, 48_000).unwrap();
    let frames = 4096;
    let input: Vec<f32> = (0..frames)
        .flat_map(|frame| {
            let t = frame as f32 / 48_000.0;
            let sample = 0.3 * (2.0 * std::f32::consts::PI * 110.0 * t).sin()
                + 0.2 * (2.0 * std::f32::consts::PI * 4_000.0 * t).sin();
            [sample, sample * 0.7]
        })
        .collect();
    let mut factory_output = vec![0.0; input.len()];
    plugin
        .process(
            &input,
            &mut factory_output,
            &sotf_host::ProcessContext::new(48_000, frames),
        )
        .unwrap();
    let mut reference_output = input;
    sotf_host::ParametricInPlacePlugin::process_in_place(
        &mut reference,
        &mut reference_output,
        &sotf_host::ProcessContext::new(48_000, frames),
    )
    .unwrap();
    assert_eq!(factory_output, reference_output);
}

#[test]
fn ambisonics_catalog_matches_factory_order_contract() {
    let entry = catalog_entry("ambisonics_decoder").expect("ambisonics catalog entry");
    let super::catalog::PluginSupportedInputLayouts::Enumerated(widths) =
        entry.metadata.channel_layout.supported_inputs
    else {
        panic!("Ambisonics channel contract must enumerate supported HOA widths");
    };
    assert_eq!(widths, &[4, 9, 16, 25, 36, 49, 64]);

    for (channels, order, layout) in [
        (4, 1, "5.1"),
        (9, 2, "7.1.4"),
        (16, 3, "9.1.6"),
        (25, 4, "5.1"),
        (36, 5, "5.1"),
        (49, 6, "5.1"),
        (64, 7, "5.1"),
    ] {
        let mut plugin = create_plugin(
            "ambisonics_decoder",
            &serde_json::json!({
                "order": order,
                "target_layout": layout,
            }),
            channels,
            48_000,
        )
        .unwrap_or_else(|error| panic!("order-{order} factory contract failed: {error}"));
        assert_eq!(plugin.input_channels(), channels);
        plugin.initialize(48_000).unwrap();
        let frames = 3;
        let mut input = vec![0.0; frames * channels];
        for frame in 0..frames {
            input[frame * channels] = 1.0;
        }
        let mut output = vec![f32::NAN; frames * plugin.output_channels()];
        assert_eq!(
            plugin
                .process(
                    &input,
                    &mut output,
                    &sotf_host::ProcessContext::new(48_000, frames),
                )
                .unwrap(),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
    }

    let mismatched_order = create_plugin(
        "ambisonics_decoder",
        &serde_json::json!({"order": 1, "target_layout": "5.1"}),
        64,
        48_000,
    );
    assert!(
        matches!(mismatched_order, Err(error) if error.contains("Order-1 ambisonics requires 4 input channels")),
        "an explicit order must still reject a mismatched input width"
    );
}

#[test]
fn ambisonics_factory_custom_geometry_decodes_and_matches_direct_construction() {
    let parameters = serde_json::json!({
        "order": 1,
        "target_layout": "custom",
        "custom_layout": {
            "name": "stereo",
            "speakers": [
                {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
                {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        }
    });
    let mut plugin = create_plugin("ambisonics_decoder", &parameters, 4, 48_000)
        .unwrap_or_else(|error| panic!("custom factory route failed: {error}"));
    assert_eq!(plugin.input_channels(), 4);
    // No named layout decodes to two channels, so this width proves the
    // user-geometry constructor ran.
    assert_eq!(plugin.output_channels(), 2);

    let custom: sotf_plugin_ambisonics::CustomDecoderConfig =
        serde_json::from_value(parameters).unwrap();
    let mut reference =
        sotf_plugin_ambisonics::AmbisonicsDecoderPlugin::new_custom(&custom).unwrap();
    reference.initialize(48_000).unwrap();

    let frames = 8;
    let mut input = vec![0.0; frames * 4];
    for frame in 0..frames {
        input[frame * 4] = 1.0;
    }
    let mut output = vec![f32::NAN; frames * 2];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut output,
                &sotf_host::ProcessContext::new(48_000, frames),
            )
            .unwrap(),
        frames
    );
    let mut expected = vec![f32::NAN; frames * 2];
    assert_eq!(
        reference
            .process(
                &input,
                &mut expected,
                &sotf_host::ProcessContext::new(48_000, frames),
            )
            .unwrap(),
        frames
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
    assert_eq!(output, expected);
}

#[test]
fn ambisonics_factory_custom_routing_accepts_legacy_numeric_targets() {
    for target in [
        serde_json::json!("custom"),
        serde_json::json!(8),
        serde_json::json!(8.0),
    ] {
        let plugin = create_plugin(
            "ambisonics_decoder",
            &serde_json::json!({
                "order": 1,
                "target_layout": target,
                "custom_layout": {
                    "name": "mono",
                    "speakers": [
                        {"label": "C", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false}
                    ]
                }
            }),
            4,
            48_000,
        )
        .unwrap_or_else(|error| panic!("target {target} must route custom: {error}"));
        assert_eq!(plugin.output_channels(), 1, "target {target}");
    }

    // Named numerics keep their historical widths.
    for (target, width) in [
        (serde_json::json!(0), 6),
        (serde_json::json!("5.1"), 6),
        (serde_json::json!(1), 8),
        (serde_json::json!("9.1.6"), 16),
    ] {
        let plugin = create_plugin(
            "ambisonics_decoder",
            &serde_json::json!({"order": 1, "target_layout": target}),
            4,
            48_000,
        )
        .unwrap_or_else(|error| panic!("target {target} must stay named: {error}"));
        assert_eq!(plugin.output_channels(), width, "target {target}");
    }

    // Pre-existing JSON without a target keeps the named default.
    let plugin = create_plugin(
        "ambisonics_decoder",
        &serde_json::json!({"order": 1}),
        4,
        48_000,
    )
    .expect("missing target_layout keeps the named default");
    assert_eq!(plugin.output_channels(), 6);
}

#[test]
fn ambisonics_factory_custom_geometry_rejections_name_custom() {
    // Missing geometry: routing parses, then the custom constructor errors.
    let missing = create_plugin(
        "ambisonics_decoder",
        &serde_json::json!({"order": 1, "target_layout": "custom"}),
        4,
        48_000,
    );
    match missing {
        Err(error) if error.contains("custom") => {}
        Err(error) => panic!("missing custom geometry must name custom, got Err({error})"),
        Ok(_) => panic!("missing custom geometry must name custom, got Ok"),
    }

    let bad_layouts = [
        // Duplicate labels.
        serde_json::json!({
            "name": "stereo",
            "speakers": [
                {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
                {"label": "FL", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        }),
        // Empty name.
        serde_json::json!({
            "name": "",
            "speakers": [
                {"label": "C", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        }),
        // Out-of-range azimuth.
        serde_json::json!({
            "name": "mono",
            "speakers": [
                {"label": "C", "azimuth_deg": 200.0, "elevation_deg": 0.0, "is_lfe": false}
            ]
        }),
        // All LFE.
        serde_json::json!({
            "name": "lfe",
            "speakers": [
                {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true}
            ]
        }),
    ];
    for layout in bad_layouts {
        let rejected = create_plugin(
            "ambisonics_decoder",
            &serde_json::json!({
                "order": 1,
                "target_layout": "custom",
                "custom_layout": layout,
            }),
            4,
            48_000,
        );
        match rejected {
            Err(error) if error.to_ascii_lowercase().contains("custom") => {}
            Err(error) => {
                panic!("malformed custom geometry must name custom, got Err({error})")
            }
            Ok(_) => panic!("malformed custom geometry must name custom, got Ok"),
        }
    }

    // Unknown string targets fail layout validation before DSP construction.
    let unknown = create_plugin(
        "ambisonics_decoder",
        &serde_json::json!({"order": 1, "target_layout": "nope"}),
        4,
        48_000,
    );
    match unknown {
        Err(error) if error.contains("Unknown speaker layout 'nope'") => {}
        Err(error) => panic!("unknown targets must name the invalid layout, got Err({error})"),
        Ok(_) => panic!("unknown targets must be rejected, got Ok"),
    }

    // Wrong input width is still rejected after custom construction.
    let mismatched = create_plugin(
        "ambisonics_decoder",
        &serde_json::json!({
            "order": 1,
            "target_layout": "custom",
            "custom_layout": {
                "name": "mono",
                "speakers": [
                    {"label": "C", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false}
                ]
            }
        }),
        9,
        48_000,
    );
    match mismatched {
        Err(error) if error.contains("Order-1 ambisonics requires 4 input channels") => {}
        Err(error) => panic!("custom must keep the order/width check, got Err({error})"),
        Ok(_) => panic!("custom must keep the order/width check, got Ok"),
    }
}

#[test]
fn resampler_facade_factory_honors_cutoff_smoothing_and_renders() {
    // Legacy params (no smoothing key) default to false = bit-exact legacy
    // audio. Explicit true reaches the instance before any audio flows.
    // Smoothing alone is inert on a non-dynamic instance (`set_ratio` fails),
    // so validation enables `dynamic_ratio` before rendering; the full upward
    // 0.5→2.0 schedule distinction + identical lengths are proven in the
    // resampler crate's `smooth_cutoff` suite via the concrete API.
    let mut legacy = create_plugin(
        "resampler",
        &serde_json::json!({"input_sample_rate": 48_000, "output_sample_rate": 48_000}),
        2,
        48_000,
    )
    .expect("legacy resampler params must construct");
    assert_eq!(
        legacy.get_parameter(&ParameterId::from("cutoff_smoothing")),
        Some(ParameterValue::Bool(false))
    );
    let mut smoothed = create_plugin(
        "resampler",
        &serde_json::json!({
            "input_sample_rate": 48_000,
            "output_sample_rate": 48_000,
            "cutoff_smoothing": true,
        }),
        2,
        48_000,
    )
    .expect("smoothing resampler params must construct");
    assert_eq!(
        smoothed.get_parameter(&ParameterId::from("cutoff_smoothing")),
        Some(ParameterValue::Bool(true))
    );

    for plugin in [&mut legacy, &mut smoothed] {
        plugin
            .set_parameter(
                ParameterId::from("dynamic_ratio"),
                ParameterValue::Bool(true),
            )
            .expect("dynamic_ratio must enable on facade instance");
        plugin.initialize(48_000).unwrap();
    }
    let frames = 512;
    let input: Vec<f32> = (0..frames * 2)
        .map(|n| {
            (2.0 * std::f32::consts::PI * 440.0 * (n / 2) as f32 / 48_000.0).sin() * 0.5
        })
        .collect();
    let render = |plugin: &mut Box<dyn sotf_host::Plugin>| {
        // Dynamic unity-rate conversion uses the sinc backend. Its first
        // short callback buffers input and truthfully produces zero frames.
        let capacity = plugin.output_frames_for_input(frames);
        let mut scratch = vec![f32::NAN; capacity * 2 + 4];
        let written = plugin.process(&input, &mut scratch,
            &sotf_host::ProcessContext::new(48_000, frames)).unwrap();
        assert_eq!(written, 0, "sub-chunk input must stay buffered");
        assert!(scratch[written * 2..].iter().all(|sample| sample.is_nan()));
        let mut output = scratch[..written * 2].to_vec();
        let calls = plugin.drain_call_bound().expect("finite resampler drain bound").get();
        let mut complete = false;
        for _ in 0..calls {
            scratch = vec![f32::NAN; plugin.drain_output_frames_max() * 2 + 4];
            let result = plugin.drain(&mut scratch,
                &sotf_host::ProcessContext::new(48_000, 0)).unwrap();
            output.extend_from_slice(&scratch[..result.frames * 2]);
            assert!(scratch[result.frames * 2..].iter().all(|sample| sample.is_nan()));
            if result.complete {
                complete = true;
                break;
            }
        }
        assert!(complete, "drain must finish inside its declared bound");
        assert!(output.len() >= input.len(), "retain the signal and interpolation suffix");
        output
    };
    let legacy_out = render(&mut legacy);
    let smoothed_out = render(&mut smoothed);
    assert_eq!(legacy_out.len(), smoothed_out.len());
    assert!(legacy_out.iter().all(|sample| sample.is_finite()));
    assert!(smoothed_out.iter().all(|sample| sample.is_finite()));
    assert!(legacy_out.iter().any(|sample| *sample != 0.0));
    assert!(smoothed_out.iter().any(|sample| *sample != 0.0));
    assert_eq!(legacy_out, smoothed_out, "smoothing is inert at a fixed ratio");
}

#[test]
fn transient_shaper_facade_factory_validates_constructor_contract() {
    let out_of_range = create_plugin(
        "transient_shaper",
        &serde_json::json!({"attack": 101.0}),
        2,
        48_000,
    );
    assert!(
        out_of_range.is_err(),
        "facade factory must reject transient-shaper values outside the parameter schema"
    );

    let zero_channels = create_plugin("transient_shaper", &serde_json::json!({}), 0, 48_000);
    assert!(
        zero_channels.is_err(),
        "facade factory must reject a zero-channel transient shaper"
    );
}

#[test]
fn delay_facade_factory_validates_scalar_and_per_channel_contracts() {
    for parameters in [
        serde_json::json!({"delay_ms": -0.01}),
        serde_json::json!({"delay_ms": 5_000.01}),
        serde_json::json!({"feedback": -0.96}),
        serde_json::json!({"feedback": 0.96}),
        serde_json::json!({"mix": -0.01}),
        serde_json::json!({"mix": 1.01}),
        serde_json::json!({"lfo_rate_hz": 20.01}),
        serde_json::json!({"lfo_depth_ms": 10.01}),
        serde_json::json!({"allpass_coeff": 0.991}),
    ] {
        assert!(
            create_plugin("delay", &parameters, 2, 48_000).is_err(),
            "facade factory accepted invalid Delay parameters: {parameters}"
        );
    }
    assert!(create_plugin("delay", &serde_json::json!({}), 0, 48_000).is_err());

    for parameters in [
        serde_json::json!({
            "channel_delays_ms": [1.0, 2.0],
            "feedback": 0.1,
            "mix": 1.0
        }),
        serde_json::json!({
            "channel_delays_ms": [1.0, 2.0],
            "feedback": 0.0,
            "mix": 1.0,
            "lfo_rate_hz": 1.0,
            "lfo_depth_ms": 1.0
        }),
        serde_json::json!({
            "channel_delays_ms": [1.0, 2.0],
            "feedback": 0.0,
            "mix": 1.0,
            "allpass_feedback": true
        }),
    ] {
        assert!(
            create_plugin("delay", &parameters, 2, 48_000).is_err(),
            "per-channel routing mode accepted effect controls: {parameters}"
        );
    }

    let plugin = create_plugin(
        "delay",
        &serde_json::json!({
            "channel_delays_ms": [0.0, 2.0],
            "feedback": 0.0,
            "mix": 1.0
        }),
        2,
        48_000,
    )
    .expect("pure per-channel routing delay must construct");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn expander_factory_is_broadband_and_validates_presets() {
    let plugin = create_plugin(
        "expander",
        &serde_json::json!({
            "threshold_db": -35.0,
            "ratio": 4.0,
            "detection_mode": "RMS",
            "sidechain_hpf_hz": 80.0
        }),
        2,
        48_000,
    )
    .expect("valid broadband expander");
    assert_eq!(plugin.info().name, "Expander");
    let ids: Vec<_> = plugin
        .parameters()
        .into_iter()
        .map(|parameter| parameter.id)
        .collect();
    assert!(!ids.iter().any(|id| id.as_str() == "num_bands"));
    assert!(!ids.iter().any(|id| id.as_str().starts_with("crossover_")));

    for parameters in [
        serde_json::json!({"threshold_db": f64::NAN}),
        serde_json::json!({"ratio": 0.9}),
        serde_json::json!({"detection_mode": "average"}),
    ] {
        assert!(
            create_plugin("expander", &parameters, 2, 48_000).is_err(),
            "invalid expander preset accepted: {parameters}"
        );
    }
    assert!(create_plugin("expander", &serde_json::json!({}), 0, 48_000).is_err());

    assert!(
        create_plugin(
            "multiband_expander",
            &serde_json::json!({"num_bands": 1}),
            2,
            48_000,
        )
        .is_err()
    );
}

#[test]
fn spectral_compressor_factory_preserves_complete_state_and_rejects_drift() {
    let plugin = create_plugin(
        "spectral_compressor",
        &serde_json::json!({
            "fft_size_index": 0,
            "threshold_db": -31.0,
            "ratio": 4.0,
            "attack_ms": 7.0,
            "release_ms": 90.0,
            "knee_db": 3.0,
            "spectral_smoothing": 0.4,
            "mix": 0.8,
            "target_mode": 2,
            "delta_listen": true,
            "adaptive_threshold": true,
            "adaptive_offset_db": 4.0,
            "channel_link": 0.75
        }),
        2,
        48_000,
    )
    .expect("complete spectral-compressor state must construct");
    for (id, expected) in [
        ("target_mode", ParameterValue::Int(2)),
        ("delta_listen", ParameterValue::Bool(true)),
        ("adaptive_threshold", ParameterValue::Bool(true)),
        ("adaptive_offset_db", ParameterValue::Float(4.0)),
        ("channel_link", ParameterValue::Float(0.75)),
    ] {
        assert_eq!(plugin.get_parameter(&ParameterId::from(id)), Some(expected));
    }

    for parameters in [
        serde_json::json!({"channel_link": 1.01}),
        serde_json::json!({"target_mode": 3}),
        serde_json::json!({"unknown_future_control": true}),
    ] {
        assert!(
            create_plugin("spectral_compressor", &parameters, 2, 48_000).is_err(),
            "invalid spectral-compressor state was accepted: {parameters}"
        );
    }
}

#[test]
fn channel_mute_solo_facade_factory_validates_constructor_contract() {
    let out_of_range = create_plugin(
        "channel_mute_solo",
        &serde_json::json!({"dim_gain_db": 1.0}),
        2,
        48_000,
    );
    assert!(
        out_of_range.is_err(),
        "facade factory must reject dim gain above the attenuation range"
    );

    let zero_channels = create_plugin("channel_mute_solo", &serde_json::json!({}), 0, 48_000);
    assert!(
        zero_channels.is_err(),
        "facade factory must reject a zero-channel mute/solo plugin"
    );
}

#[test]
fn mono_to_stereo_facade_factory_validates_constructor_contract() {
    for parameters in [
        serde_json::json!({"stereo_width": -0.01}),
        serde_json::json!({"stereo_width": 1.01}),
        serde_json::json!({"haas_delay_ms": -0.01}),
        serde_json::json!({"haas_delay_ms": 5.01}),
        serde_json::json!({"decor_low_hz": 99.0}),
        serde_json::json!({"decor_low_hz": 501.0}),
        serde_json::json!({"decor_high_hz": 999.0}),
        serde_json::json!({"decor_high_hz": 5_001.0}),
    ] {
        assert!(
            create_plugin("mono_to_stereo", &parameters, 1, 48_000).is_err(),
            "facade factory must reject values outside the Mono-to-Stereo schema: {parameters}"
        );
    }

    assert!(
        create_plugin("mono_to_stereo", &serde_json::json!({}), 2, 48_000).is_err(),
        "facade factory must reject a non-mono input layout"
    );

    for parameters in [
        serde_json::json!({
            "stereo_width": 0.0,
            "haas_delay_ms": 0.0,
            "decor_low_hz": 100.0,
            "decor_high_hz": 1_000.0
        }),
        serde_json::json!({
            "stereo_width": 1.0,
            "haas_delay_ms": 5.0,
            "decor_low_hz": 500.0,
            "decor_high_hz": 5_000.0
        }),
    ] {
        let plugin = create_plugin("mono_to_stereo", &parameters, 1, 48_000)
            .unwrap_or_else(|error| panic!("schema endpoint must construct: {error}"));
        assert_eq!(plugin.input_channels(), 1);
        assert_eq!(plugin.output_channels(), 2);
        assert_eq!(
            plugin.get_parameter(&sotf_host::parameters::ParameterId::from("decor_low_hz")),
            Some(sotf_host::parameters::ParameterValue::Float(
                parameters["decor_low_hz"].as_f64().unwrap() as f32
            ))
        );
        assert_eq!(
            plugin.get_parameter(&sotf_host::parameters::ParameterId::from("decor_high_hz")),
            Some(sotf_host::parameters::ParameterValue::Float(
                parameters["decor_high_hz"].as_f64().unwrap() as f32
            ))
        );
    }

    assert!(
        create_plugin(
            "mono_to_stereo",
            &serde_json::json!({"decor_high_hz": 5_000.0}),
            1,
            8_000,
        )
        .is_err(),
        "facade factory must reject a decorrelator crossover above Nyquist"
    );
}

#[test]
fn ab_compare_facade_injects_factory_before_initial_path_build() {
    let parameters = serde_json::json!({
        "path_a": {
            "type": "Plugin",
            "plugin_type": "expander",
            "parameters": {}
        },
        "path_b": {
            "type": "Rack",
            "plugins": [{"plugin_type": "hiss_reducer", "parameters": {}}]
        },
        "auto_gain_enabled": false
    });
    let plugin = create_plugin("ab_compare", &parameters, 2, 48_000)
        .expect("canonical factory must be installed before nested path construction");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn binaural_catalog_and_factory_share_exact_layout_contract() {
    let entry = catalog_entry("binaural_decoder").unwrap();
    assert_eq!(
        entry.metadata.channel_layout.supported_inputs,
        super::catalog::PluginSupportedInputLayouts::Enumerated(&[
            1, 2, 3, 5, 6, 8, 10, 12, 14, 16
        ])
    );
    for channels in [1, 2, 3, 5, 6, 8, 10, 12, 14, 16] {
        let plugin = create_plugin(
            "binaural_decoder",
            &serde_json::json!({"input_channels": channels, "diffuse_field_eq": false}),
            channels,
            48_000,
        )
        .unwrap();
        assert_eq!(plugin.input_channels(), channels);
    }
    for channels in [4, 7, 9, 11, 13, 15] {
        assert!(
            create_plugin(
                "binaural_decoder",
                &serde_json::json!({"input_channels": channels}),
                channels,
                48_000,
            )
            .is_err()
        );
    }
}

#[test]
fn crossover_catalog_and_factory_report_compiled_topology() {
    let entry = catalog_entry("crossover").unwrap();
    assert!(matches!(
        entry.metadata.channel_layout.output,
        super::catalog::PluginChannelOutputModel::Configurable { .. }
    ));
    let plugin = create_plugin(
        "crossover",
        &serde_json::json!({
            "type": "LR24",
            "frequency": 500.0,
            "output": "both",
            "extra_frequencies": [2_000.0]
        }),
        2,
        48_000,
    )
    .unwrap();
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 6);
    let parameters = plugin.parameters();
    let ids: Vec<_> = parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect();
    assert_eq!(ids, ["type", "frequency", "mode", "frequency_2"]);
}

#[test]
fn crossover_factory_canonicalizes_choice_labels_and_preserves_numeric_counts() {
    let base = serde_json::json!({
        "type": "LR24",
        "frequency": 500.0,
        "output": "both",
        "extra_frequencies": [2_000.0, 8_000.0],
        "channel_frequencies_hz": [700.0, 1_400.0],
        "channel_modes": ["lowpass", "highpass"]
    });

    for (wire_count, expected_count) in [
        (serde_json::json!("2"), 2),
        (serde_json::json!("3"), 3),
        (serde_json::json!("4"), 4),
        (serde_json::json!(2), 2),
        (serde_json::json!(3), 3),
        (serde_json::json!(4), 4),
    ] {
        let mut parameters = base.clone();
        parameters["band_count"] = wire_count.clone();
        parameters["topology"] = serde_json::json!("Bands");
        let plugin = create_plugin("crossover", &parameters, 2, 48_000)
            .unwrap_or_else(|error| panic!("Crossover count {wire_count} was rejected: {error}"));
        assert_eq!(
            plugin.output_channels(),
            2 * expected_count,
            "numeric counts remain actual counts and visible labels map to those counts"
        );
    }

    for (wire_topology, per_channel) in [
        (serde_json::json!(0), false),
        (serde_json::json!("Bands"), false),
        (serde_json::json!("bands"), false),
        (serde_json::json!(1), true),
        (serde_json::json!("Per Channel"), true),
        (serde_json::json!("per_channel"), true),
    ] {
        let mut parameters = base.clone();
        parameters["band_count"] = serde_json::json!(2);
        parameters["topology"] = wire_topology.clone();
        let plugin = create_plugin("crossover", &parameters, 2, 48_000).unwrap_or_else(|error| {
            panic!("Crossover topology {wire_topology} was rejected: {error}")
        });
        assert_eq!(
            plugin.output_channels(),
            if per_channel { 2 } else { 4 },
            "topology form {wire_topology} selected the wrong route"
        );
    }

    for invalid_count in [0, 1, 5] {
        let mut parameters = base.clone();
        parameters["band_count"] = serde_json::json!(invalid_count);
        parameters["topology"] = serde_json::json!("bands");
        assert!(
            create_plugin("crossover", &parameters, 2, 48_000).is_err(),
            "invalid numeric band count {invalid_count} must remain invalid"
        );
    }
}

#[test]
fn compressor_factory_rejects_invalid_dsp_configuration() {
    let invalid_ratio = serde_json::json!({"ratio": 0.5});
    let error = match create_plugin("compressor", &invalid_ratio, 2, 48_000) {
        Err(error) => error,
        Ok(_) => panic!("compressor factory must reject ratios below its schema range"),
    };
    assert!(error.contains("ratio"), "unexpected error: {error}");

    let descending_crossovers = serde_json::json!({
        "crossover_frequencies": [200.0, 100.0, 8_000.0, 12_000.0]
    });
    let error = match create_plugin("multiband_compressor", &descending_crossovers, 2, 48_000) {
        Err(error) => error,
        Ok(_) => panic!("factory must reject descending crossover frequencies"),
    };
    assert!(error.contains("crossover"), "unexpected error: {error}");
}

#[test]
fn supported_plugin_type_list_covers_factory_aliases() {
    assert!(is_supported_plugin_type("gain"));
    assert!(is_supported_plugin_type("EQ"));
    assert!(is_supported_plugin_type("rnnoise"));
    assert!(is_supported_plugin_type("active_acoustic_enhancement"));
    assert!(is_supported_plugin_type("external"));
    assert!(is_supported_plugin_type("external_plugin"));
    assert!(!is_supported_plugin_type("definitely_missing"));
}

#[test]
fn factory_rejects_invalid_de_esser_configuration() {
    let invalid_mode = serde_json::json!({
        "mode": "not-a-de-esser-mode",
    });
    assert!(
        create_plugin("de_esser", &invalid_mode, 1, 48_000).is_err(),
        "factory must not silently map an unknown De-Esser mode to Split-Band"
    );

    let out_of_range = serde_json::json!({
        "frequency": 16_001.0,
    });
    assert!(
        create_plugin("de_esser", &out_of_range, 1, 48_000).is_err(),
        "factory must reject De-Esser values outside the public schema"
    );

    let nyquist_invalid = serde_json::json!({
        "frequency": 16_000.0,
    });
    assert!(
        create_plugin("de_esser", &nyquist_invalid, 1, 22_050).is_err(),
        "factory must reject a De-Esser band that cannot be represented at the host rate"
    );
}

#[test]
fn factory_rejects_invalid_dynamic_eq_configuration() {
    let invalid_global = serde_json::json!({
        "threshold": 1.0,
    });
    assert!(
        create_plugin("dynamic_eq", &invalid_global, 1, 48_000).is_err(),
        "factory must reject Dynamic EQ global values outside the public schema"
    );

    let invalid_band = serde_json::json!({
        "bands": [{"frequency": 0.0}],
    });
    assert!(
        create_plugin("dynamic_eq", &invalid_band, 1, 48_000).is_err(),
        "factory must reject Dynamic EQ per-band values outside the public schema"
    );

    let low_rate_invalid = serde_json::json!({
        "bands": [{"frequency": 10_000.0}],
    });
    assert!(
        create_plugin("dynamic_eq", &low_rate_invalid, 16, 16_000).is_err(),
        "factory must reject a Dynamic EQ band outside the host Nyquist margin"
    );
}

#[test]
fn create_external_plugin_from_path() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Test",
        "format": "clap",
        "start_worker": false,
    });

    let plugin = create_plugin("external", &params, 2, 48_000).unwrap();
    assert_eq!(plugin.input_channels(), 2);
}

#[test]
fn create_external_plugin_from_path_string() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-string.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();

    let plugin = create_plugin(
        "external",
        &serde_json::json!({
            "path": plugin_path.to_string_lossy(),
            "audio_inputs": 2,
            "audio_outputs": 2,
            "name": "External Test",
            "format": "clap",
            "start_worker": false,
        }),
        2,
        48_000,
    )
    .unwrap();
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
}

#[test]
fn create_external_plugin_from_embedded_descriptor() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let descriptor = PluginDescriptor {
        id: "test.external".into(),
        name: "Embedded External Test".into(),
        vendor: "Test".into(),
        version: "0.1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["testing".into()],
        scan_status: PluginScanStatus::Discovered,
    };

    let plugin = create_plugin(
        "external_plugin",
        &serde_json::json!({"descriptor": descriptor, "start_worker": false}),
        2,
        48_000,
    )
    .unwrap();
    assert_eq!(plugin.output_channels(), 2);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn factory_rejects_external_state_for_a_different_sandbox_mode() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-state-mode.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let descriptor = PluginDescriptor {
        id: "test.external.state-mode".into(),
        name: "External State Mode Test".into(),
        vendor: "Test".into(),
        version: "0.1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["testing".into()],
        scan_status: PluginScanStatus::Discovered,
    };
    let descriptor = parse_external_plugin_descriptor(&serde_json::json!({
        "descriptor": descriptor
    }))
    .unwrap();
    let state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        vec![1, 2, 3],
    );

    let error = match create_plugin(
        "external",
        &serde_json::json!({
            "descriptor": descriptor,
            "external_state": state,
            "start_worker": false,
        }),
        2,
        48_000,
    ) {
        Ok(_) => panic!("in-process state must not load into the default isolated host"),
        Err(error) => error,
    };
    assert!(error.contains("cannot restore isolated plugin"), "{error}");
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_defaults_to_isolated_when_trust_unknown() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-isolated.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Isolated Test",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
        "deadline_micros": 0,
        "max_block_frames": 2,
        "_sotf_instance_id": 37,
    });

    let mut plugin = create_plugin("external", &params, 2, 48_000).unwrap();
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    let isolated = plugin
        .as_any()
        .and_then(|plugin| plugin.downcast_ref::<crate::IsolatedExternalPlugin>())
        .expect("factory must construct an isolated external plugin");
    assert_eq!(isolated.plugin_instance_id(), Some(37));
    assert_eq!(plugin.latency_samples(), 2);

    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    let frames = plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(48_000, 2),
        )
        .unwrap();
    assert_eq!(frames, 2);
    assert_eq!(output, vec![0.0; input.len()]);

    // The isolated transport has a declared two-frame fixed latency. With no
    // worker, the following callback receives the matching delayed fallback.
    let frames = plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(48_000, 2),
        )
        .unwrap();
    assert_eq!(frames, 2);
    assert_eq!(output, input);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_respects_backend_for_host_owned_sandbox_grants() {
    use crate::{
        PluginSandboxGrantStore, PluginSandboxIdentity, PluginSandboxNetworkGrant,
        PluginSandboxPermission, PluginSandboxUserGrant,
    };

    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-grants.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Grant Test",
        "vendor": "Test Vendor",
        "id": "com.test.grants",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
    });
    let descriptor = parse_external_plugin_descriptor(&params).unwrap();
    let identity = PluginSandboxIdentity::from_descriptor(&descriptor);
    let mut grants = PluginSandboxGrantStore::default();
    grants.remember(PluginSandboxUserGrant {
        identity,
        permission: PluginSandboxPermission::Network(PluginSandboxNetworkGrant::AnyOutbound),
    });

    let expected_policy = grants.strict_policy_for_plugin(&descriptor, dir.path().join("presets"));
    let backend_can_launch = expected_policy
        .current_backend_launch_plan()
        .validate_for_launch(&expected_policy)
        .is_ok();

    let result = create_plugin_with_sandbox_grants(
        "external",
        &params,
        2,
        48_000,
        &grants,
        dir.path().join("presets"),
    );

    if backend_can_launch {
        let plugin = result.unwrap();
        assert_eq!(plugin.input_channels(), 2);
        assert_eq!(plugin.output_channels(), 2);
    } else {
        let err = match result {
            Ok(_) => panic!("expected unsupported sandbox backend to fail"),
            Err(err) => err,
        };
        assert!(err.contains("cannot satisfy required policy"));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_accepts_host_selected_store_sandbox_backend() {
    use crate::{
        ExternalPluginWorkerCommand, PluginSandboxGrantStore, PluginSandboxLaunchBackend,
        PluginSandboxNetworkGrant, PluginSandboxPermission, PluginSandboxUserGrant,
    };

    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-store-helper.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Store Helper Test",
        "vendor": "Test Vendor",
        "id": "com.test.store-helper",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
    });
    let descriptor = parse_external_plugin_descriptor(&params).unwrap();
    let mut grants = PluginSandboxGrantStore::default();
    grants.remember(PluginSandboxUserGrant {
        identity: crate::PluginSandboxIdentity::from_descriptor(&descriptor),
        permission: PluginSandboxPermission::Network(PluginSandboxNetworkGrant::AnyOutbound),
    });

    let plugin = create_plugin_with_sandbox_grants_for_backend_and_launcher(
        "external",
        &params,
        2,
        48_000,
        &grants,
        dir.path().join("presets"),
        PluginSandboxLaunchBackend::MacosAppSandboxHelper,
        Some(ExternalPluginWorkerCommand::new("/tmp/sotf-sandbox-helper")),
    )
    .unwrap();

    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_rejects_helper_backend_without_launcher() {
    use crate::{PluginSandboxGrantStore, PluginSandboxLaunchBackend};

    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-no-helper.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Missing Helper Test",
        "vendor": "Test Vendor",
        "id": "com.test.no-helper",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
    });
    let grants = PluginSandboxGrantStore::default();

    let err = match create_plugin_with_sandbox_grants_for_backend(
        "external",
        &params,
        2,
        48_000,
        &grants,
        dir.path().join("presets"),
        PluginSandboxLaunchBackend::MacosAppSandboxHelper,
    ) {
        Ok(_) => panic!("expected helper backend to require launcher command"),
        Err(err) => err,
    };

    assert!(err.contains("requires a host-owned sandbox launcher command"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_rejects_unrepresentable_host_owned_sandbox_grants() {
    use crate::{
        PluginSandboxGrantStore, PluginSandboxIdentity, PluginSandboxNetworkGrant,
        PluginSandboxPermission, PluginSandboxUserGrant,
    };

    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-test-plugin-loopback.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "External Loopback Test",
        "vendor": "Test Vendor",
        "id": "com.test.loopback",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
    });
    let descriptor = parse_external_plugin_descriptor(&params).unwrap();
    let identity = PluginSandboxIdentity::from_descriptor(&descriptor);
    let mut grants = PluginSandboxGrantStore::default();
    grants.remember(PluginSandboxUserGrant {
        identity,
        permission: PluginSandboxPermission::Network(PluginSandboxNetworkGrant::LoopbackOnly),
    });

    let err = match create_plugin_with_sandbox_grants(
        "external",
        &params,
        2,
        48_000,
        &grants,
        dir.path().join("presets"),
    ) {
        Ok(_) => panic!("expected unrepresentable sandbox grant to fail"),
        Err(err) => err,
    };

    assert!(
        err.contains("cannot launch current worker policy")
            || err.contains("cannot satisfy required policy")
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn create_external_plugin_rejects_worker_overrides_from_config() {
    let err = parse_isolated_external_plugin_config(
        &serde_json::json!({
            "worker_path": "/usr/bin/sotf-test-worker",
            "start_worker": false,
        }),
        ExternalPluginTrust::Unknown,
    )
    .unwrap_err();

    assert!(err.contains("worker_path"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn isolated_external_plugin_config_uses_bundled_worker() {
    let config = parse_isolated_external_plugin_config(
        &serde_json::json!({
            "start_worker": false,
            "deadline_micros": 250,
            "max_block_frames": 1024,
            "_sotf_instance_id": 37,
        }),
        ExternalPluginTrust::Unknown,
    )
    .unwrap();

    assert!(config.worker_command.program().is_absolute());
    assert!(config.worker_command.command_args().is_empty());
    assert!(config.worker_command.command_env().is_empty());
    assert!(!config.start_worker);
    assert_eq!(config.deadline, std::time::Duration::from_micros(250));
    assert_eq!(config.max_block_frames, 1024);
    assert_eq!(config.plugin_instance_id, Some(37));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn isolated_external_plugin_config_maps_trust_to_sandbox_timing() {
    let signed = parse_isolated_external_plugin_config(
        &serde_json::json!({
            "sandbox_read_paths": ["/Library/Audio/Plug-Ins"],
            "sandbox_write_paths": ["/tmp/sotf-plugin-cache"],
        }),
        ExternalPluginTrust::Signed,
    )
    .unwrap();
    assert_eq!(
        signed.sandbox_policy.timing,
        ExternalPluginSandboxTiming::AfterPluginLoad
    );
    assert!(!signed.sandbox_policy.require_platform_sandbox);
    assert_eq!(
        signed.sandbox_policy.extra_read_paths,
        vec![PathBuf::from("/Library/Audio/Plug-Ins")]
    );

    let untrusted = parse_isolated_external_plugin_config(
        &serde_json::json!({
            "plugin_trust": "untrusted"
        }),
        ExternalPluginTrust::Untrusted,
    )
    .unwrap();
    assert_eq!(
        untrusted.sandbox_policy.timing,
        ExternalPluginSandboxTiming::BeforePluginLoad
    );
    assert_eq!(
        untrusted.sandbox_policy.require_platform_sandbox,
        cfg!(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows"
        ))
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn external_plugin_security_rejects_self_declared_signed_trust() {
    let err = validate_plugin_security_config(
        "external",
        &serde_json::json!({
            "path": "/tmp/fake.clap",
            "plugin_trust": "signed"
        }),
    )
    .unwrap_err();

    assert!(err.contains("cannot mark external plugins as signed"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn external_plugin_security_rejects_untrusted_in_process() {
    let err = validate_plugin_security_config(
        "external",
        &serde_json::json!({
            "path": "/tmp/fake.clap",
            "isolated": false
        }),
    )
    .unwrap_err();

    assert!(err.contains("cannot disable process isolation"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn external_plugin_security_rejects_relaxed_untrusted_sandbox() {
    let err = validate_plugin_security_config(
        "external",
        &serde_json::json!({
            "path": "/tmp/fake.clap",
            "sandbox_timing": "disabled",
            "start_worker": false
        }),
    )
    .unwrap_err();

    assert!(err.contains("before plugin load"));
}

#[test]
fn create_external_plugin_reports_invalid_parameters() {
    let err = match create_plugin(
        "external",
        &serde_json::json!({"audio_inputs": 2}),
        2,
        48_000,
    ) {
        Ok(_) => panic!("external plugin creation should fail"),
        Err(err) => err,
    };
    assert!(
        err.contains("External plugin descriptor is missing required `path`")
            || err.contains("path")
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn isolated_untrusted_config_rejects_broad_read_write_paths() {
    let err = parse_isolated_external_plugin_config(
        &serde_json::json!({
            "sandbox_read_paths": ["/"],
            "sandbox_write_paths": ["/tmp"],
        }),
        ExternalPluginTrust::Untrusted,
    )
    .unwrap_err();
    assert!(err.contains("cannot expand sandbox filesystem access"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn isolated_untrusted_config_rejects_network_grant() {
    let err = parse_isolated_external_plugin_config(
        &serde_json::json!({"sandbox_allow_network": true}),
        ExternalPluginTrust::Untrusted,
    )
    .unwrap_err();
    assert!(err.contains("cannot allow network access"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn isolated_untrusted_config_rejects_child_process_grant() {
    let err = parse_isolated_external_plugin_config(
        &serde_json::json!({"sandbox_allow_child_processes": true}),
        ExternalPluginTrust::Untrusted,
    )
    .unwrap_err();
    assert!(err.contains("cannot allow child processes"));
}

#[test]
fn create_external_plugin_rejects_missing_file_path() {
    let err = match create_plugin(
        "external",
        &serde_json::json!({
            "path": "/nonexistent/path/to/plugin.clap",
            "audio_inputs": 2,
            "audio_outputs": 2,
            "name": "Missing Plugin",
            "format": "clap",
        }),
        2,
        48_000,
    ) {
        Ok(_) => panic!("expected missing external plugin path to fail"),
        Err(err) => err,
    };
    assert!(
        err.to_ascii_lowercase().contains("path")
            || err.to_ascii_lowercase().contains("file")
            || err.to_ascii_lowercase().contains("no such")
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn external_plugin_state_stays_consistent_after_invalid_parameter_changes() {
    let dir = tempdir().unwrap();
    let plugin_path = dir.path().join("external-param-corruption.clap");
    std::fs::write(&plugin_path, b"stub plugin").unwrap();
    let params = serde_json::json!({
        "path": plugin_path.to_string_lossy(),
        "audio_inputs": 2,
        "audio_outputs": 2,
        "name": "Param Corruption Test",
        "format": "clap",
        "plugin_trust": "unknown",
        "start_worker": false,
    });

    let mut plugin = create_plugin("external", &params, 2, 48_000).unwrap();
    plugin.initialize(48_000).unwrap();

    // Unknown parameter id should be ignored, not corrupt state.
    let _ = plugin.set_parameter(
        sotf_host::parameters::ParameterId::from("definitely_not_a_real_parameter"),
        sotf_host::parameters::ParameterValue::Float(1.0),
    );

    // Out-of-range value should be rejected, not corrupt state.
    let _ = plugin.set_parameter(
        sotf_host::parameters::ParameterId::from("mix"),
        sotf_host::parameters::ParameterValue::Float(f32::NAN),
    );

    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    let frames = plugin
        .process(
            &input,
            &mut output,
            &sotf_host::ProcessContext::new(48_000, 2),
        )
        .unwrap();
    assert_eq!(frames, 2);
}

#[test]
fn beamformer_factory_matches_fallible_constructor_validation() {
    for params in [
        serde_json::json!({"num_mics": 1}),
        serde_json::json!({"num_mics": 2, "mic_spacing_cm": 100.0}),
        serde_json::json!({"num_mics": 2, "steer_angle_deg": 200.0}),
        serde_json::json!({"num_mics": 2, "beamformer_type": "unknown"}),
    ] {
        assert!(
            create_plugin("beamformer", &params, 2, 48_000).is_err(),
            "{params}"
        );
    }
    assert!(
        create_plugin(
            "beamformer",
            &serde_json::json!({"num_mics": 2, "beamformer_type": "Superdirective"}),
            2,
            48_000,
        )
        .is_ok()
    );
}
