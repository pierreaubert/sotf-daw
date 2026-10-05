//! Loaded CLAP/VST3 tests for native Ambisonics custom geometry.
//!
//! UNEXECUTED: requires fresh Ambisonics binaries plus
//! `SOTF_TEST_AMBISONICS_CLAP_PLUGIN` / `SOTF_TEST_AMBISONICS_VST3_PLUGIN`.
//! Two test functions (one per format); every subcase is embedded, so the
//! runner expects exactly 2 executed with `--ignored` (2 ignored without).
//! Covers the custom route, single/dual-band streams to EOF with explicit
//! latency/drain proofs, 10-channel customs, format rejection layers with
//! retained-history twins, and named 5.1.4/7.1.2 audio on the canonical
//! F1 slots (the NIH wire-order swap is fixed; nothing avoids them).

#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

use sotf_host::external_plugin::{
    ExternalPlugin, ExternalPluginSandboxMode, ExternalPluginState, NativeAmbisonicsCustomGeometry,
    NativeAmbisonicsTargetLayout, NativePluginAudioSetup, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_host::serialization::SerializablePlugin;
use sotf_plugin_ambisonics::custom_layout::{CustomDecoderConfig, CustomLayout};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 64;
const INPUT_CHANNELS: usize = 64;
const CUSTOM_STATE_FIELD: &str = sotf_host::external_plugin::AMBISONICS_CUSTOM_STATE_FIELD;

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_CLAP_PLUGIN to point to the exported Ambisonics CLAP library"]
fn exported_clap_order_seven_custom_audio_matches_direct_decoder_and_saved_state() {
    verify_native_custom_route(
        PluginFormat::Clap,
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
        custom_7_1_4_geometry(),
        12,
    );
    verify_loaded_custom_eof(
        PluginFormat::Clap,
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
        custom_7_1_4_geometry(),
        12,
    );
    verify_loaded_custom_10ch(
        PluginFormat::Clap,
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
        custom_7_1_2_geometry(),
    );
    verify_loaded_format_rejections_clap(
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
        custom_7_1_4_geometry(),
        12,
    );
    verify_native_named_10ch_route(
        PluginFormat::Clap,
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_VST3_PLUGIN to point to the exported Ambisonics VST3 library"]
fn exported_vst3_order_seven_custom_audio_matches_direct_decoder_and_saved_state() {
    verify_native_custom_route(
        PluginFormat::Vst3,
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
        custom_moved_lfe_9_1_6_geometry(),
        16,
    );
    verify_loaded_custom_eof(
        PluginFormat::Vst3,
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
        custom_moved_lfe_9_1_6_geometry(),
        16,
    );
    verify_loaded_custom_10ch(
        PluginFormat::Vst3,
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
        custom_5_1_4_geometry(),
    );
    verify_loaded_format_rejections_vst3(
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
        custom_moved_lfe_9_1_6_geometry(),
        16,
    );
    verify_native_named_10ch_route(
        PluginFormat::Vst3,
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
    );
}

fn custom_7_1_4_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-7.1.4",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "SL", "azimuth_deg": 90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "SR", "azimuth_deg": -90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BL", "azimuth_deg": 150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BR", "azimuth_deg": -150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "TFL", "azimuth_deg": 30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TFR", "azimuth_deg": -30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBL", "azimuth_deg": 150.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBR", "azimuth_deg": -150.0, "elevation_deg": 45.0, "is_lfe": false}
        ]
    }))
    .expect("custom 7.1.4 fixture parses")
}

fn custom_moved_lfe_9_1_6_geometry() -> NativeAmbisonicsCustomGeometry {
    let mut geometry: NativeAmbisonicsCustomGeometry = serde_json::from_value(serde_json::json!({
        "name": "loaded-9.1.6",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "SL", "azimuth_deg": 90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "SR", "azimuth_deg": -90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BL", "azimuth_deg": 150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BR", "azimuth_deg": -150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "WL", "azimuth_deg": 60.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "WR", "azimuth_deg": -60.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "TFL", "azimuth_deg": 30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TFR", "azimuth_deg": -30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBL", "azimuth_deg": 150.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBR", "azimuth_deg": -150.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TMiL", "azimuth_deg": 90.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TMiR", "azimuth_deg": -90.0, "elevation_deg": 45.0, "is_lfe": false}
        ]
    }))
    .expect("custom 9.1.6 fixture parses");
    geometry.speakers.swap(3, 5);
    geometry
}

fn custom_stereo_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-stereo",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false}
        ]
    }))
    .expect("custom stereo fixture parses")
}

fn custom_5_1_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-5.1",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "SL", "azimuth_deg": 110.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "SR", "azimuth_deg": -110.0, "elevation_deg": 0.0, "is_lfe": false}
        ]
    }))
    .expect("custom 5.1 fixture parses")
}

fn verify_native_custom_route(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    custom: NativeAmbisonicsCustomGeometry,
    expected_outputs: usize,
) {
    custom.validate().expect("custom fixture validates");
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    let descriptor = PluginDescriptor {
        id: plugin_id.into(),
        name: "SOTF: Ambisonics Decoder".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: library_path,
        audio_inputs: 4,
        audio_outputs: 6,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    };

    let mut state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: custom.clone(),
    });

    let seed = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("create selected custom layout to seed a serialized state");
    state.opaque_state = seed.save_opaque_state().expect("save initial native state");
    set_native_state_bool(&mut state.opaque_state, format, "max_re_weighting", false);
    set_native_state_bool(&mut state.opaque_state, format, "dual_band", false);
    assert_eq!(
        native_state_int(&state.opaque_state, format, "target_layout"),
        Some(8),
        "seeded custom state must carry target index 8"
    );
    assert_eq!(
        native_state_custom_geometry(&state.opaque_state, format),
        Some(serde_json::to_value(&custom).expect("fixture serializes")),
        "seeded custom state must carry the typed geometry"
    );

    let mut reference = direct_custom_decoder(&custom, 7, false, false);

    let mut plugin = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("create native instance with selected custom layout");
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), expected_outputs);
    assert_eq!(plugin.discovery_descriptor(), &descriptor);
    assert_eq!(plugin.descriptor().audio_inputs, INPUT_CHANNELS);
    assert_eq!(plugin.descriptor().audio_outputs, expected_outputs);
    assert_eq!(plugin.audio_setup(), state.audio_setup.as_ref());

    for block_index in 0..2 {
        let input = sparse_acn_basis_block(block_index);
        let expected = render_block(&mut reference, &input, expected_outputs);
        let actual = render_block(&mut plugin, &input, expected_outputs);
        assert_matches_reference(
            &actual,
            &expected,
            expected_outputs,
            &format!("initial custom instance block {block_index}"),
        );
        if block_index == 0 {
            assert_high_acn_is_present(&actual, expected_outputs);
            let mut default_control = direct_custom_decoder(&custom, 7, true, false);
            let default_output = render_block(&mut default_control, &input, expected_outputs);
            assert!(
                actual
                    .iter()
                    .zip(&default_output)
                    .any(|(selected, default)| (selected - default).abs() > 1.0e-6),
                "the nondefault Max-rE state must measurably affect the decoded waveform"
            );
        }
    }

    let preset = plugin.serialize().expect("serialize selected custom setup");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    assert_eq!(saved_state.descriptor, descriptor);
    assert_eq!(saved_state.audio_setup, state.audio_setup);
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "target_layout"),
        Some(8),
        "serialization must preserve the custom target index"
    );
    assert_eq!(
        native_state_custom_geometry(&saved_state.opaque_state, format),
        Some(serde_json::to_value(&custom).expect("fixture serializes")),
        "serialization must preserve the custom geometry"
    );

    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("restore matching typed and opaque custom state");
    assert_eq!(restored.discovery_descriptor(), &descriptor);
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), expected_outputs);
    let input = sparse_acn_basis_block(2);
    let expected_after_restore = render_block(&mut reference, &input, expected_outputs);
    let restored_output = render_block(&mut restored, &input, expected_outputs);
    assert_matches_reference(
        &restored_output,
        &expected_after_restore,
        expected_outputs,
        "restored custom instance continuation",
    );

    let mut untouched_twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("create transactional restore control instance");

    // A valid but different custom geometry must not pass agreement.
    let mut swapped_state = saved_state.clone();
    let swapped_json =
        serde_json::to_string(&custom_5_1_geometry()).expect("swap fixture serializes");
    set_native_state_custom_field(&mut swapped_state.opaque_state, format, &swapped_json);
    assert!(
        ExternalPlugin::from_placeholder_state(&swapped_state, SAMPLE_RATE).is_err(),
        "opaque custom geometry disagreeing with the typed setup must be rejected"
    );
    assert!(
        restored
            .load_opaque_state(&swapped_state.opaque_state)
            .is_err(),
        "loading swapped custom geometry must be rejected"
    );
    assert_eq!(restored.audio_setup(), saved_state.audio_setup.as_ref());
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), expected_outputs);

    // A malformed custom field must fail before mutating anything accepted.
    let mut malformed_state = saved_state.clone();
    set_native_state_custom_field(&mut malformed_state.opaque_state, format, "not json");
    assert!(
        ExternalPlugin::from_placeholder_state(&malformed_state, SAMPLE_RATE).is_err(),
        "malformed custom geometry must be rejected"
    );

    // A named opaque state must not overwrite an explicit custom setup.
    let default_plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .expect("create legacy default native instance");
    let default_preset = default_plugin
        .serialize()
        .expect("serialize legacy default instance");
    let mut mismatched_state = default_preset
        .external_plugin_state()
        .expect("read legacy default state")
        .expect("legacy default state is external");
    mismatched_state.audio_setup = state.audio_setup.clone();
    assert!(
        ExternalPlugin::from_placeholder_state(&mismatched_state, SAMPLE_RATE).is_err(),
        "a named opaque state must not overwrite an explicit custom setup"
    );

    let input = sparse_acn_basis_block(3);
    let untouched_output = render_block(&mut untouched_twin, &input, expected_outputs);
    let retained_output = render_block(&mut restored, &input, expected_outputs);
    assert_eq!(
        retained_output, untouched_output,
        "rejected restores must leave the active custom decoder unchanged"
    );

    // Unoffered widths fail the full loaded path with explicit reasons.
    let mut stereo_state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    stereo_state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: custom_stereo_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&stereo_state, SAMPLE_RATE).is_err(),
        "a stereo custom setup has no advertised CLAP/VST3 configuration"
    );

    verify_deliberate_custom_reconfiguration(&descriptor, &custom, expected_outputs);
}

fn verify_deliberate_custom_reconfiguration(
    descriptor: &PluginDescriptor,
    custom: &NativeAmbisonicsCustomGeometry,
    custom_outputs: usize,
) {
    let mut plugin = ExternalPlugin::new(descriptor, SAMPLE_RATE)
        .expect("create legacy-default decoder for deliberate reconfiguration");
    let named = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::SevenOne,
    };
    plugin
        .reconfigure_audio_setup(named.clone())
        .expect("select a named layout before the custom change");
    assert_eq!(plugin.output_channels(), 8);

    let custom_setup = NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: custom.clone(),
    };
    plugin
        .reconfigure_audio_setup(custom_setup.clone())
        .expect("deliberately change to the custom layout");
    assert_eq!(plugin.audio_setup(), Some(&custom_setup));
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), custom_outputs);
    let mut reference = direct_custom_decoder(custom, 7, true, false);
    let input = sparse_acn_basis_block(4);
    let expected = render_block(&mut reference, &input, custom_outputs);
    let actual = render_block(&mut plugin, &input, custom_outputs);
    assert_matches_reference(
        &actual,
        &expected,
        custom_outputs,
        "custom layout after deliberate reconfiguration",
    );

    let invalid = NativePluginAudioSetup::AmbisonicsCustom {
        order: 8,
        custom: custom.clone(),
    };
    assert!(
        plugin.reconfigure_audio_setup(invalid).is_err(),
        "unsupported custom order must fail before candidate commit"
    );
    assert_eq!(plugin.audio_setup(), Some(&custom_setup));
    assert_eq!(plugin.output_channels(), custom_outputs);

    plugin
        .reconfigure_audio_setup(named.clone())
        .expect("deliberately change back to the named layout");
    assert_eq!(plugin.audio_setup(), Some(&named));
    assert_eq!(plugin.output_channels(), 8);
}

fn direct_custom_decoder(
    custom: &NativeAmbisonicsCustomGeometry,
    order: usize,
    max_re_weighting: bool,
    dual_band: bool,
) -> AmbisonicsDecoderPlugin {
    let layout_json = serde_json::to_value(custom).expect("typed geometry serializes");
    let custom_layout: CustomLayout =
        serde_json::from_value(layout_json).expect("DSP layout parses");
    custom_layout
        .validate()
        .expect("reference geometry validates");
    let config = CustomDecoderConfig {
        params: AmbisonicsDecoderConfig {
            order,
            target_layout: "custom".to_string(),
            max_re_weighting,
            dual_band,
            algorithm: "mode_matching".to_string(),
        },
        custom_layout,
    };
    let mut plugin =
        AmbisonicsDecoderPlugin::new_custom(&config).expect("build direct custom reference");
    plugin
        .initialize(f64::from(SAMPLE_RATE))
        .expect("initialize reference");
    plugin
}

fn sparse_acn_basis_block(block_index: usize) -> Vec<f32> {
    let mut input = vec![0.0; FRAMES * INPUT_CHANNELS];
    for acn_channel in 0..INPUT_CHANNELS {
        let frame = match block_index {
            0 => acn_channel,
            1 => INPUT_CHANNELS - 1 - acn_channel,
            _ => (acn_channel * 17) % INPUT_CHANNELS,
        };
        let amplitude = 0.001 * (acn_channel + 1) as f32 * (block_index + 1) as f32;
        input[frame * INPUT_CHANNELS + acn_channel] = amplitude;
    }
    input
}

fn render_block(plugin: &mut dyn Plugin, input: &[f32], output_channels: usize) -> Vec<f32> {
    let context = ProcessContext::new(SAMPLE_RATE, FRAMES);
    let mut output = vec![f32::NAN; FRAMES * output_channels];
    assert_eq!(
        plugin.process(input, &mut output, &context).unwrap(),
        FRAMES
    );
    output
}

fn assert_matches_reference(actual: &[f32], expected: &[f32], channels: usize, instance: &str) {
    assert_eq!(actual.len(), expected.len());
    assert!(actual.iter().all(|sample| sample.is_finite()));
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 2.0e-5,
            "{instance} frame {}, SOTF speaker {}: actual={actual}, expected={expected}",
            index / channels,
            index % channels
        );
    }
}

fn assert_high_acn_is_present(output: &[f32], channels: usize) {
    let high_order_frame = INPUT_CHANNELS - 1;
    let frame = &output[high_order_frame * channels..(high_order_frame + 1) * channels];
    assert!(
        frame.iter().any(|sample| sample.abs() > 1.0e-8),
        "ACN 63 basis impulse must reach the selected custom decoder"
    );
}

fn native_state_json(opaque_state: &[u8], format: PluginFormat) -> Option<serde_json::Value> {
    let payload = match format {
        PluginFormat::Clap => {
            let length_bytes: [u8; 8] = opaque_state.get(..8)?.try_into().ok()?;
            let length = usize::try_from(u64::from_le_bytes(length_bytes)).ok()?;
            let payload = opaque_state.get(8..)?;
            if payload.len() != length {
                return None;
            }
            payload
        }
        PluginFormat::Vst3 => opaque_state,
        PluginFormat::AudioUnit => return None,
    };
    serde_json::from_slice(payload).ok()
}

fn native_state_int(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<i64> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("i32")?
        .as_i64()
}

fn native_state_custom_geometry(
    opaque_state: &[u8],
    format: PluginFormat,
) -> Option<serde_json::Value> {
    let state = native_state_json(opaque_state, format)?;
    let encoded = state.get("fields")?.get(CUSTOM_STATE_FIELD)?.as_str()?;
    serde_json::from_str(encoded).ok()
}

fn set_native_state_bool(opaque_state: &mut Vec<u8>, format: PluginFormat, id: &str, value: bool) {
    set_native_state_param(opaque_state, format, id, serde_json::json!({"bool": value}));
}

fn set_native_state_param(
    opaque_state: &mut Vec<u8>,
    format: PluginFormat,
    id: &str,
    value: serde_json::Value,
) {
    mutate_native_state_json(opaque_state, format, |state| {
        let parameter = state
            .get_mut("params")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|params| params.get_mut(id))
            .expect("serialized state contains requested parameter");
        *parameter = value;
    });
}

fn set_native_state_custom_field(opaque_state: &mut Vec<u8>, format: PluginFormat, value: &str) {
    mutate_native_state_json(opaque_state, format, |state| {
        let fields = state
            .as_object_mut()
            .expect("native state is a JSON object");
        let fields = fields
            .entry("fields".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        let fields = fields
            .as_object_mut()
            .expect("native state fields are an object");
        fields.insert(
            CUSTOM_STATE_FIELD.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    });
}

fn mutate_native_state_json(
    opaque_state: &mut Vec<u8>,
    format: PluginFormat,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let payload = match format {
        PluginFormat::Clap => {
            let length_bytes: [u8; 8] = opaque_state
                .get(..8)
                .expect("CLAP state includes its length prefix")
                .try_into()
                .expect("CLAP length prefix is eight bytes");
            let length = usize::try_from(u64::from_le_bytes(length_bytes))
                .expect("CLAP state length fits usize");
            let payload = opaque_state
                .get(8..)
                .expect("CLAP state includes its serialized body");
            assert_eq!(payload.len(), length);
            payload
        }
        PluginFormat::Vst3 => opaque_state.as_slice(),
        PluginFormat::AudioUnit => panic!("fixture is only for CLAP/VST3 state formats"),
    };
    let mut state: serde_json::Value =
        serde_json::from_slice(payload).expect("native state is NIH-plug JSON");
    mutate(&mut state);
    let serialized = serde_json::to_vec(&state).expect("serialize mutated native state");
    opaque_state.clear();
    if format == PluginFormat::Clap {
        opaque_state.extend_from_slice(&(serialized.len() as u64).to_le_bytes());
    }
    opaque_state.extend_from_slice(&serialized);
}

// ---------------------------------------------------------------------------
// Round-3 loaded proofs: EOF streams, 10-channel roles, rejection layers,
// and named audio on the canonical F1 slots.
// ---------------------------------------------------------------------------

/// Silence threshold for loaded drain proofs, matching the nonzero threshold.
const DRAIN_QUIET_PEAK: f32 = 1.0e-6;

fn test_descriptor(format: PluginFormat, library_env: &str, plugin_id: &str) -> PluginDescriptor {
    let library_path = PathBuf::from(
        std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
    );
    PluginDescriptor {
        id: plugin_id.into(),
        name: "SOTF: Ambisonics Decoder".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: library_path,
        audio_inputs: 4,
        audio_outputs: 6,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

/// 10-channel 7.1.2-shaped custom geometry (exact standard angles).
fn custom_7_1_2_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-7.1.2",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "SL", "azimuth_deg": 90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "SR", "azimuth_deg": -90.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BL", "azimuth_deg": 150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "BR", "azimuth_deg": -150.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "TFL", "azimuth_deg": 30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TFR", "azimuth_deg": -30.0, "elevation_deg": 45.0, "is_lfe": false}
        ]
    }))
    .expect("custom 7.1.2 fixture parses")
}

/// 10-channel 5.1.4-shaped custom geometry (exact standard angles).
fn custom_5_1_4_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-5.1.4",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "SL", "azimuth_deg": 110.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "SR", "azimuth_deg": -110.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "TFL", "azimuth_deg": 30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TFR", "azimuth_deg": -30.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBL", "azimuth_deg": 150.0, "elevation_deg": 45.0, "is_lfe": false},
            {"label": "TBR", "azimuth_deg": -150.0, "elevation_deg": 45.0, "is_lfe": false}
        ]
    }))
    .expect("custom 5.1.4 fixture parses")
}

/// 7.1.4-shaped geometry with the LFE moved off channel 3.
fn moved_lfe_7_1_4_geometry() -> NativeAmbisonicsCustomGeometry {
    let mut geometry = custom_7_1_4_geometry();
    geometry.speakers.swap(3, 5);
    geometry
}

/// 6-channel geometry whose surrounds match no standard role (±45°).
fn nonstandard_6ch_geometry() -> NativeAmbisonicsCustomGeometry {
    serde_json::from_value(serde_json::json!({
        "name": "loaded-nonstandard",
        "speakers": [
            {"label": "FL", "azimuth_deg": 30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FR", "azimuth_deg": -30.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "FC", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "LFE", "azimuth_deg": 0.0, "elevation_deg": 0.0, "is_lfe": true},
            {"label": "NL", "azimuth_deg": 45.0, "elevation_deg": 0.0, "is_lfe": false},
            {"label": "NR", "azimuth_deg": -45.0, "elevation_deg": 0.0, "is_lfe": false}
        ]
    }))
    .expect("nonstandard fixture parses")
}

/// 7.1.4-shaped geometry with a second LFE (duplicated role bit/id 3).
fn multi_lfe_7_1_4_geometry() -> NativeAmbisonicsCustomGeometry {
    let mut geometry = custom_7_1_4_geometry();
    geometry.speakers[10].is_lfe = true;
    geometry.speakers[10].label = "LFE2".to_string();
    geometry
}

/// Deterministic 64-speaker Fibonacci sphere (wide-width rejection case).
fn wide_64_geometry() -> NativeAmbisonicsCustomGeometry {
    let mut speakers = Vec::with_capacity(64);
    for index in 0..64 {
        let i = index as f32;
        let y = 1.0 - 2.0 * (i + 0.5) / 64.0;
        let radius = (1.0 - y * y).sqrt();
        let phi = i * 2.399_963_f32;
        speakers.push(serde_json::json!({
            "label": format!("S{index:02}"),
            "azimuth_deg": (radius * phi.cos()).atan2(y).to_degrees(),
            "elevation_deg": (radius * phi.sin()).asin().to_degrees(),
            "is_lfe": false,
        }));
    }
    serde_json::from_value(serde_json::json!({"name": "loaded-wide-64", "speakers": speakers}))
        .expect("wide fixture parses")
}

fn direct_named_decoder_with_rate(
    order: usize,
    target_layout: &str,
    dual_band: bool,
    sample_rate: u32,
) -> AmbisonicsDecoderPlugin {
    let mut config = AmbisonicsDecoderConfig::default();
    config.order = order;
    config.target_layout = target_layout.to_string();
    config.max_re_weighting = false;
    config.dual_band = dual_band;
    config.algorithm = "mode_matching".to_string();
    let mut plugin = AmbisonicsDecoderPlugin::new(&config).expect("build direct named reference");
    plugin
        .initialize(f64::from(sample_rate))
        .expect("initialize named reference");
    plugin
}

/// Sparse ACN stream for arbitrary frame counts: channel `c` fires when
/// `(frame * 31 + c * 17 + seed * 101) % 64 == 0`, so every channel fires
/// at least once per 64 consecutive frames (31 and 17 are coprime to 64).
fn sparse_acn_stream(frames: usize, inputs: usize, seed: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * inputs];
    for frame in 0..frames {
        for channel in 0..inputs {
            if (frame * 31 + channel * 17 + seed * 101) % 64 == 0 {
                input[frame * inputs + channel] = 0.001 * (channel + 1) as f32;
            }
        }
    }
    input
}

fn render_sized_block(
    plugin: &mut dyn Plugin,
    input: &[f32],
    output_channels: usize,
    sample_rate: u32,
    frames: usize,
) -> Vec<f32> {
    let context = ProcessContext::new(sample_rate, frames);
    let mut output = vec![f32::NAN; frames * output_channels];
    assert_eq!(
        plugin.process(input, &mut output, &context).unwrap(),
        frames
    );
    output
}

fn lcg_partitions(total: usize, seed: u32) -> Vec<usize> {
    let mut state = seed;
    let mut sizes = Vec::new();
    let mut remaining = total;
    while remaining > 0 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let size = (1 + (state as usize % remaining.min(17))).min(remaining);
        sizes.push(size);
        remaining -= size;
    }
    sizes
}

struct LoadedStreamEof {
    signal_frames: usize,
    drain_blocks: usize,
    first_drain_peak: f32,
}

/// Stream parameters shared by the signal and drain phases.
struct LoadedStreamSpec {
    inputs: usize,
    outputs: usize,
    sample_rate: u32,
    seed: usize,
    drain_block: usize,
    drain_cap_blocks: usize,
}

/// Render odd/partial partitions plus a bounded zero-input drain with a
/// per-block lockstep reference (the independent stream clock): every
/// block matches within tolerance, accounts its frames exactly, and the
/// drain settles below `DRAIN_QUIET_PEAK` within its cap.
fn render_partitions_to_eof_loaded(
    plugin: &mut dyn Plugin,
    reference: &mut AmbisonicsDecoderPlugin,
    partitions: &[usize],
    instance: &str,
    spec: &LoadedStreamSpec,
) -> LoadedStreamEof {
    let total: usize = partitions.iter().sum();
    let signal = sparse_acn_stream(total, spec.inputs, spec.seed);
    let mut offset = 0;
    for (block, &frames) in partitions.iter().enumerate() {
        let block_in = &signal[offset * spec.inputs..(offset + frames) * spec.inputs];
        let expected =
            render_sized_block(reference, block_in, spec.outputs, spec.sample_rate, frames);
        let actual = render_sized_block(plugin, block_in, spec.outputs, spec.sample_rate, frames);
        assert_matches_reference(
            &actual,
            &expected,
            spec.outputs,
            &format!("{instance} signal partition {block} ({frames} frames)"),
        );
        offset += frames;
    }
    let mut drain_blocks = 0;
    let mut first_drain_peak = 0.0;
    let mut quiet = false;
    while !quiet && drain_blocks < spec.drain_cap_blocks {
        let zeros = vec![0.0; spec.drain_block * spec.inputs];
        let expected = render_sized_block(
            reference,
            &zeros,
            spec.outputs,
            spec.sample_rate,
            spec.drain_block,
        );
        let actual = render_sized_block(
            plugin,
            &zeros,
            spec.outputs,
            spec.sample_rate,
            spec.drain_block,
        );
        assert_matches_reference(
            &actual,
            &expected,
            spec.outputs,
            &format!("{instance} drain block {drain_blocks}"),
        );
        let peak = actual
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if drain_blocks == 0 {
            first_drain_peak = peak;
        }
        drain_blocks += 1;
        quiet = peak < DRAIN_QUIET_PEAK;
    }
    assert!(
        quiet,
        "{instance}: drain must settle within {} blocks",
        spec.drain_cap_blocks
    );
    LoadedStreamEof {
        signal_frames: total,
        drain_blocks,
        first_drain_peak,
    }
}

fn verify_loaded_custom_eof(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    custom: NativeAmbisonicsCustomGeometry,
    expected_outputs: usize,
) {
    for dual_band in [false, true] {
        let descriptor = test_descriptor(format, library_env, plugin_id);
        let mut state = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            Vec::new(),
        );
        state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
            order: 7,
            custom: custom.clone(),
        });
        let seed = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
            .expect("seed custom state for EOF stream");
        state.opaque_state = seed.save_opaque_state().expect("save seeded state");
        set_native_state_bool(&mut state.opaque_state, format, "max_re_weighting", false);
        set_native_state_bool(&mut state.opaque_state, format, "dual_band", dual_band);
        let mut plugin = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
            .expect("load custom instance for EOF stream");
        assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
        assert_eq!(plugin.output_channels(), expected_outputs);
        let mut reference = direct_custom_decoder(&custom, 7, false, dual_band);

        // Explicit latency: both modes report zero fixed host latency.
        assert_eq!(
            plugin.latency_samples(),
            0,
            "loaded custom latency must be explicit zero"
        );
        // Explicit tail: refresh first (VST3 caches tail metadata on the
        // control thread). Dual-band reads Infinite: the NIH u32::MAX
        // sentinel roundtrips through both format ABIs.
        plugin.refresh_control_thread_metadata();
        let expected_tail = if dual_band {
            TailLength::Infinite
        } else {
            TailLength::Finite(0)
        };
        assert_eq!(
            plugin.tail_length(),
            expected_tail,
            "loaded custom tail contract"
        );
        // The direct DSP reference pins the tail contract at the source:
        // single-band is memoryless, dual-band keeps recursive history.
        assert_eq!(
            reference.tail_length(),
            if dual_band {
                TailLength::Unknown
            } else {
                TailLength::Finite(0)
            },
            "direct reference tail contract"
        );

        let mut partitions = vec![1, 7, 31, 63, 65, 127];
        partitions.extend(lcg_partitions(200, 0xE0F0_2000 + dual_band as u32));
        let stream = render_partitions_to_eof_loaded(
            &mut plugin,
            &mut reference,
            &partitions,
            &format!("loaded custom dual_band={dual_band}"),
            &LoadedStreamSpec {
                inputs: INPUT_CHANNELS,
                outputs: expected_outputs,
                sample_rate: SAMPLE_RATE,
                seed: 0xE0F0_3000,
                drain_block: 64,
                drain_cap_blocks: if dual_band { 32 } else { 4 },
            },
        );
        assert!(stream.signal_frames > 0);
        if dual_band {
            assert!(
                stream.first_drain_peak > DRAIN_QUIET_PEAK,
                "dual-band loaded tail must exist (first drain peak {})",
                stream.first_drain_peak
            );
        } else {
            assert_eq!(
                stream.first_drain_peak, 0.0,
                "single-band loaded tail is exactly zero"
            );
            assert_eq!(
                stream.drain_blocks, 1,
                "memoryless loaded matrix settles on the first zero block"
            );
        }

        // Save/reload roundtrip after EOF. Single-band continues exactly
        // against a fresh reference (stateless); dual-band reloads to a
        // working nonzero decoder (recursive state is not serialized).
        let preset = plugin.serialize().expect("serialize after EOF");
        let saved_state = preset
            .external_plugin_state()
            .expect("read external state envelope")
            .expect("serialized external state");
        let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
            .expect("reload custom after EOF");
        assert_eq!(restored.input_channels(), INPUT_CHANNELS);
        assert_eq!(restored.output_channels(), expected_outputs);
        let input = sparse_acn_stream(FRAMES, INPUT_CHANNELS, 0xE0F0_4000);
        let output = render_block(&mut restored, &input, expected_outputs);
        assert!(output.iter().all(|sample| sample.is_finite()));
        if dual_band {
            assert!(
                output.iter().any(|sample| sample.abs() > 1.0e-6),
                "reloaded dual-band custom renders nonzero"
            );
        } else {
            let mut fresh = direct_custom_decoder(&custom, 7, false, false);
            let expected = render_block(&mut fresh, &input, expected_outputs);
            assert_matches_reference(
                &output,
                &expected,
                expected_outputs,
                "reloaded single-band continuation",
            );
        }
    }
}

fn verify_loaded_custom_10ch(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    custom: NativeAmbisonicsCustomGeometry,
) {
    custom.validate().expect("10ch fixture validates");
    let descriptor = test_descriptor(format, library_env, plugin_id);
    let mut state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: custom.clone(),
    });
    let seed = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("seed 10ch custom state");
    state.opaque_state = seed.save_opaque_state().expect("save seeded state");
    set_native_state_bool(&mut state.opaque_state, format, "max_re_weighting", false);
    set_native_state_bool(&mut state.opaque_state, format, "dual_band", false);
    assert_eq!(
        native_state_int(&state.opaque_state, format, "target_layout"),
        Some(8),
        "seeded 10ch custom state must carry target index 8"
    );
    let mut plugin = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("load 10ch custom instance");
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), 10);
    let mut reference = direct_custom_decoder(&custom, 7, false, false);
    for block_index in 0..2 {
        let input = sparse_acn_basis_block(block_index);
        let expected = render_block(&mut reference, &input, 10);
        let actual = render_block(&mut plugin, &input, 10);
        assert_matches_reference(
            &actual,
            &expected,
            10,
            &format!("10ch custom block {block_index}"),
        );
    }
    let preset = plugin.serialize().expect("serialize 10ch custom");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("restore 10ch custom");
    let input = sparse_acn_basis_block(2);
    let expected = render_block(&mut reference, &input, 10);
    let actual = render_block(&mut restored, &input, 10);
    assert_matches_reference(&actual, &expected, 10, "restored 10ch continuation");
}

fn verify_loaded_format_rejections_clap(
    library_env: &str,
    plugin_id: &str,
    accepted_custom: NativeAmbisonicsCustomGeometry,
    expected_outputs: usize,
) {
    let descriptor = test_descriptor(PluginFormat::Clap, library_env, plugin_id);
    // Accepted dual instance plus a twin with populated recursive history.
    let mut accepted_state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    accepted_state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: accepted_custom.clone(),
    });
    let seed = ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE)
        .expect("seed accepted dual custom");
    accepted_state.opaque_state = seed.save_opaque_state().expect("save seed");
    set_native_state_bool(
        &mut accepted_state.opaque_state,
        PluginFormat::Clap,
        "max_re_weighting",
        false,
    );
    set_native_state_bool(
        &mut accepted_state.opaque_state,
        PluginFormat::Clap,
        "dual_band",
        true,
    );
    let mut plugin = ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE)
        .expect("load accepted dual custom");
    let mut twin =
        ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE).expect("load twin");
    let history = sparse_acn_stream(128, INPUT_CHANNELS, 0x6198_0001);
    let rendered = render_sized_block(&mut plugin, &history, expected_outputs, SAMPLE_RATE, 128);
    let twinned = render_sized_block(&mut twin, &history, expected_outputs, SAMPLE_RATE, 128);
    assert_eq!(
        rendered, twinned,
        "twin dual customs render bit-exact populated history"
    );

    // 16-channel customs: host preflight rejects before backend load, so
    // no backend callback runs for this setup by design.
    let mut wide = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    wide.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: custom_moved_lfe_9_1_6_geometry(),
    });
    let Err(error) = ExternalPlugin::from_placeholder_state(&wide, SAMPLE_RATE) else {
        panic!("16ch CLAP preflight must reject before backend load");
    };
    assert!(
        error.contains("offer 6, 8, 10 or 12"),
        "16ch CLAP preflight must name offered widths: {error}"
    );

    // Moved-LFE 7.1.4: role order disagrees with every static map.
    let mut moved = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    moved.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: moved_lfe_7_1_4_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&moved, SAMPLE_RATE).is_err(),
        "host preflight must reject moved-LFE CLAP customs before backend load"
    );

    // Nonstandard roles match no static speaker.
    let mut nonstandard = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    nonstandard.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: nonstandard_6ch_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&nonstandard, SAMPLE_RATE).is_err(),
        "host preflight must reject nonstandard CLAP customs before backend load"
    );

    // Live-state path: the replacement backend runs the binary's own
    // set_state; a disagreeing valid field is refused with host
    // agreement guarding the commit.
    let mut swapped = accepted_state.clone();
    let swapped_json =
        serde_json::to_string(&custom_moved_lfe_9_1_6_geometry()).expect("swap fixture serializes");
    set_native_state_custom_field(&mut swapped.opaque_state, PluginFormat::Clap, &swapped_json);
    assert!(
        plugin.load_opaque_state(&swapped.opaque_state).is_err(),
        "live-state path must refuse disagreeing CLAP geometry"
    );
    // A malformed field is refused by the binary's own set_state
    // before host agreement runs.
    let mut malformed = accepted_state.clone();
    set_native_state_custom_field(&mut malformed.opaque_state, PluginFormat::Clap, "not json");
    assert!(
        plugin.load_opaque_state(&malformed.opaque_state).is_err(),
        "live binary set_state must refuse a malformed CLAP field"
    );

    // Retained populated history renders bit-exact against the twin.
    let retained = render_sized_block(&mut plugin, &history, expected_outputs, SAMPLE_RATE, 128);
    let untouched = render_sized_block(&mut twin, &history, expected_outputs, SAMPLE_RATE, 128);
    assert_eq!(
        retained, untouched,
        "rejected restores must leave populated dual history unchanged"
    );
}

fn verify_loaded_format_rejections_vst3(
    library_env: &str,
    plugin_id: &str,
    accepted_custom: NativeAmbisonicsCustomGeometry,
    expected_outputs: usize,
) {
    let descriptor = test_descriptor(PluginFormat::Vst3, library_env, plugin_id);
    // Accepted dual instance plus a twin with populated recursive history.
    let mut accepted_state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    accepted_state.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: accepted_custom.clone(),
    });
    let seed = ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE)
        .expect("seed accepted dual custom");
    accepted_state.opaque_state = seed.save_opaque_state().expect("save seed");
    set_native_state_bool(
        &mut accepted_state.opaque_state,
        PluginFormat::Vst3,
        "max_re_weighting",
        false,
    );
    set_native_state_bool(
        &mut accepted_state.opaque_state,
        PluginFormat::Vst3,
        "dual_band",
        true,
    );
    let mut plugin = ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE)
        .expect("load accepted dual custom");
    let mut twin =
        ExternalPlugin::from_placeholder_state(&accepted_state, SAMPLE_RATE).expect("load twin");
    let history = sparse_acn_stream(128, INPUT_CHANNELS, 0x6198_0002);
    let rendered = render_sized_block(&mut plugin, &history, expected_outputs, SAMPLE_RATE, 128);
    let twinned = render_sized_block(&mut twin, &history, expected_outputs, SAMPLE_RATE, 128);
    assert_eq!(
        rendered, twinned,
        "twin dual customs render bit-exact populated history"
    );

    // Multi-LFE customs duplicate role bit 3: host preflight rejects
    // before backend load, so no backend callback runs by design.
    let mut multi_lfe = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    multi_lfe.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: multi_lfe_7_1_4_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&multi_lfe, SAMPLE_RATE).is_err(),
        "host preflight must reject multi-LFE VST3 customs before backend load"
    );

    // Nonstandard roles match no standard VST3 speaker.
    let mut nonstandard = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    nonstandard.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: nonstandard_6ch_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&nonstandard, SAMPLE_RATE).is_err(),
        "host preflight must reject nonstandard VST3 customs before backend load"
    );

    // Wide-width customs have no advertised arrangement.
    let mut wide = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    wide.audio_setup = Some(NativePluginAudioSetup::AmbisonicsCustom {
        order: 7,
        custom: wide_64_geometry(),
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&wide, SAMPLE_RATE).is_err(),
        "host preflight must reject wide VST3 customs before backend load"
    );

    // Live-state path: the replacement backend runs the binary's own
    // set_state; a disagreeing valid field is refused with host
    // agreement guarding the commit.
    let mut swapped = accepted_state.clone();
    let swapped_json =
        serde_json::to_string(&nonstandard_6ch_geometry()).expect("swap fixture serializes");
    set_native_state_custom_field(&mut swapped.opaque_state, PluginFormat::Vst3, &swapped_json);
    assert!(
        plugin.load_opaque_state(&swapped.opaque_state).is_err(),
        "live-state path must refuse disagreeing VST3 geometry"
    );
    // A malformed field is refused by the binary's own set_state
    // before host agreement runs.
    let mut malformed = accepted_state.clone();
    set_native_state_custom_field(&mut malformed.opaque_state, PluginFormat::Vst3, "not json");
    assert!(
        plugin.load_opaque_state(&malformed.opaque_state).is_err(),
        "live binary set_state must refuse a malformed VST3 field"
    );

    // Retained populated history renders bit-exact against the twin.
    let retained = render_sized_block(&mut plugin, &history, expected_outputs, SAMPLE_RATE, 128);
    let untouched = render_sized_block(&mut twin, &history, expected_outputs, SAMPLE_RATE, 128);
    assert_eq!(
        retained, untouched,
        "rejected restores must leave populated dual history unchanged"
    );
}

fn verify_native_named_10ch_route(format: PluginFormat, library_env: &str, plugin_id: &str) {
    for (target_layout, name, index) in [
        (NativeAmbisonicsTargetLayout::FiveOneFour, "5.1.4", 3),
        (NativeAmbisonicsTargetLayout::SevenOneTwo, "7.1.2", 4),
    ] {
        // Order 7 at 48 kHz with save/reload continuation. The reference
        // is built from the canonical DSP layout name, independent of
        // NIH slot order, so this fails loudly while F1 is open.
        let descriptor = test_descriptor(format, library_env, plugin_id);
        let mut state = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            Vec::new(),
        );
        state.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
            order: 7,
            target_layout,
        });
        let seed = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
            .expect("load named 10ch target on canonical F1 slots");
        state.opaque_state = seed.save_opaque_state().expect("save seed");
        set_native_state_bool(&mut state.opaque_state, format, "max_re_weighting", false);
        assert_eq!(
            native_state_int(&state.opaque_state, format, "target_layout"),
            Some(index),
            "seeded named state must carry its canonical target index"
        );
        let mut plugin = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
            .expect("load named 10ch instance");
        assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
        assert_eq!(plugin.output_channels(), 10);
        let mut reference = direct_named_decoder_with_rate(7, name, false, SAMPLE_RATE);
        for block_index in 0..2 {
            let input = sparse_acn_basis_block(block_index);
            let expected = render_block(&mut reference, &input, 10);
            let actual = render_block(&mut plugin, &input, 10);
            assert_matches_reference(
                &actual,
                &expected,
                10,
                &format!("named {name} block {block_index}"),
            );
        }
        let preset = plugin.serialize().expect("serialize named 10ch");
        let saved_state = preset
            .external_plugin_state()
            .expect("read external state envelope")
            .expect("serialized external state");
        assert_eq!(
            native_state_int(&saved_state.opaque_state, format, "target_layout"),
            Some(index),
            "serialization must preserve the named target index"
        );
        let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
            .expect("restore named 10ch");
        let input = sparse_acn_basis_block(2);
        let expected = render_block(&mut reference, &input, 10);
        let actual = render_block(&mut restored, &input, 10);
        assert_matches_reference(&actual, &expected, 10, &format!("restored named {name}"));

        // Cross order/rate render: order 2 at 44.1 kHz.
        const CROSS_RATE: u32 = 44_100;
        let mut cross = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            Vec::new(),
        );
        cross.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
            order: 2,
            target_layout,
        });
        let seed = ExternalPlugin::from_placeholder_state(&cross, CROSS_RATE)
            .expect("load cross-rate named 10ch");
        cross.opaque_state = seed.save_opaque_state().expect("save cross seed");
        set_native_state_bool(&mut cross.opaque_state, format, "max_re_weighting", false);
        let mut plugin = ExternalPlugin::from_placeholder_state(&cross, CROSS_RATE)
            .expect("load cross-rate instance");
        assert_eq!(plugin.input_channels(), 9);
        assert_eq!(plugin.output_channels(), 10);
        let mut reference = direct_named_decoder_with_rate(2, name, false, CROSS_RATE);
        let input = sparse_acn_stream(FRAMES, 9, 0x4410_0000 + index as usize);
        let expected = render_sized_block(&mut reference, &input, 10, CROSS_RATE, FRAMES);
        let actual = render_sized_block(&mut plugin, &input, 10, CROSS_RATE, FRAMES);
        assert_matches_reference(
            &actual,
            &expected,
            10,
            &format!("named {name} order-2 44.1k"),
        );
    }
}
