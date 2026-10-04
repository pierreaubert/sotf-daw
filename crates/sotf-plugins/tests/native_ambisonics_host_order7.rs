#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

use sotf_host::external_plugin::{
    ExternalPlugin, ExternalPluginSandboxMode, ExternalPluginState, NativeAmbisonicsTargetLayout,
    NativePluginAudioSetup, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::SerializablePlugin;
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 64;
const INPUT_CHANNELS: usize = 64;

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_CLAP_PLUGIN to point to the exported Ambisonics CLAP library"]
fn exported_clap_order_seven_audio_matches_direct_decoder_and_saved_state() {
    verify_native_route(
        PluginFormat::Clap,
        "SOTF_TEST_AMBISONICS_CLAP_PLUGIN",
        "org.spinorama.sotf.ambisonics",
        NativeAmbisonicsTargetLayout::SevenOneFour,
        "7.1.4",
        12,
    );
}

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_VST3_PLUGIN to point to the exported Ambisonics VST3 library"]
fn exported_vst3_order_seven_audio_matches_direct_decoder_and_saved_state() {
    verify_native_route(
        PluginFormat::Vst3,
        "SOTF_TEST_AMBISONICS_VST3_PLUGIN",
        "536F7466416D6269736E696330303031",
        NativeAmbisonicsTargetLayout::NineOneSixWide,
        "9.1.6",
        16,
    );
}

fn verify_native_route(
    format: PluginFormat,
    library_env: &str,
    plugin_id: &str,
    target_layout: NativeAmbisonicsTargetLayout,
    direct_target_layout: &str,
    expected_outputs: usize,
) {
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
    state.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout,
    });

    let seed = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("create selected layout to seed a serialized state");
    state.opaque_state = seed.save_opaque_state().expect("save initial native state");
    set_native_state_bool(&mut state.opaque_state, format, "max_re_weighting", false);
    set_native_state_bool(&mut state.opaque_state, format, "dual_band", false);
    assert_eq!(
        native_state_bool(&state.opaque_state, format, "max_re_weighting"),
        Some(false),
        "fixture must carry a nondefault decoder control"
    );
    assert_eq!(
        native_state_bool(&state.opaque_state, format, "dual_band"),
        Some(false),
        "the main loaded route must retain the single-band compatibility case"
    );

    let mut reference = direct_decoder(direct_target_layout, false);

    let mut plugin = ExternalPlugin::from_placeholder_state(&state, SAMPLE_RATE)
        .expect("create native instance with selected high-order layout");
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), expected_outputs);
    assert_eq!(plugin.discovery_descriptor(), &descriptor);
    assert_eq!(plugin.descriptor().audio_inputs, INPUT_CHANNELS);
    assert_eq!(plugin.descriptor().audio_outputs, expected_outputs);
    assert_eq!(plugin.audio_setup(), state.audio_setup.as_ref());
    let visible_parameters = plugin.parameters();
    assert!(
        visible_parameters
            .iter()
            .all(|parameter| { !matches!(parameter.name.as_str(), "Order" | "Target Layout") }),
        "hidden structural controls stay outside generic automation"
    );

    for block_index in 0..2 {
        let input = sparse_acn_basis_block(block_index);
        let expected = render_block(&mut reference, &input, expected_outputs);
        let actual = render_block(&mut plugin, &input, expected_outputs);
        assert_matches_reference(
            &actual,
            &expected,
            expected_outputs,
            &format!("initial instance block {block_index}"),
        );
        if block_index == 0 {
            assert_high_acn_is_present(&actual, expected_outputs);
            let mut default_control = direct_decoder(direct_target_layout, true);
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

    let preset = plugin.serialize().expect("serialize selected native setup");
    let saved_state = preset
        .external_plugin_state()
        .expect("read external state envelope")
        .expect("serialized external state");
    assert_eq!(saved_state.descriptor, descriptor);
    assert_eq!(saved_state.descriptor.audio_inputs, 4);
    assert_eq!(saved_state.descriptor.audio_outputs, 6);
    assert_eq!(saved_state.audio_setup, state.audio_setup);
    assert_eq!(
        native_state_bool(&saved_state.opaque_state, format, "max_re_weighting"),
        Some(false),
        "serialization must preserve a nondefault control unrelated to bus geometry"
    );
    assert!(
        !saved_state.opaque_state.is_empty(),
        "native plugin state must accompany the typed layout"
    );

    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("restore matching typed and opaque native state");
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
        "restored instance continuation",
    );

    let mut untouched_twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("create transactional restore control instance");

    let default_plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .expect("create legacy default native instance");
    let default_preset = default_plugin
        .serialize()
        .expect("serialize legacy default instance");
    let mut mismatched_state = default_preset
        .external_plugin_state()
        .expect("read legacy default state")
        .expect("legacy default state is external");
    assert!(
        !mismatched_state.opaque_state.is_empty(),
        "fixture needs a native order-one state blob"
    );
    mismatched_state.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout,
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&mismatched_state, SAMPLE_RATE).is_err(),
        "an order-one opaque state must not overwrite an explicit order-seven setup"
    );

    let order_one_blob = mismatched_state.opaque_state.clone();
    assert!(
        restored.load_opaque_state(&order_one_blob).is_err(),
        "opaque state with a conflicting hidden structural tuple must be rejected"
    );
    assert_eq!(restored.audio_setup(), saved_state.audio_setup.as_ref());
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), expected_outputs);

    let mut contradictory_preset = default_plugin
        .serialize()
        .expect("serialize an order-one preset for rejection");
    let mut contradictory_state = contradictory_preset
        .external_plugin_state()
        .expect("read contradictory state")
        .expect("contradictory preset contains external state");
    contradictory_state.audio_setup = state.audio_setup.clone();
    contradictory_preset
        .set_external_plugin_state(&contradictory_state)
        .expect("write contradictory typed setup");
    assert!(
        restored.deserialize(&contradictory_preset).is_err(),
        "preset restore must reject opaque state inconsistent with its typed setup"
    );
    assert_eq!(restored.audio_setup(), saved_state.audio_setup.as_ref());
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), expected_outputs);

    let input = sparse_acn_basis_block(3);
    let untouched_output = render_block(&mut untouched_twin, &input, expected_outputs);
    let retained_output = render_block(&mut restored, &input, expected_outputs);
    assert_eq!(
        retained_output, untouched_output,
        "rejected restores must leave the active native decoder unchanged"
    );

    let mut single_band_late_failure_preset = default_plugin
        .serialize()
        .expect("serialize the old single-band state for candidate rejection");
    let mut single_band_late_failure_state = single_band_late_failure_preset
        .external_plugin_state()
        .expect("read old single-band state")
        .expect("old single-band state contains native data");
    single_band_late_failure_state.audio_setup = state.audio_setup.clone();
    single_band_late_failure_state
        .validate()
        .expect("single-band opaque state has a valid typed order-seven envelope");
    single_band_late_failure_preset
        .set_external_plugin_state(&single_band_late_failure_state)
        .expect("write the valid-envelope single-band candidate");
    let single_band_before_failure = restored
        .serialize()
        .expect("serialize the live single-band route before candidate load")
        .external_plugin_state()
        .expect("read single-band state envelope")
        .expect("live route carries native state");
    let single_band_failure = restored
        .deserialize(&single_band_late_failure_preset)
        .expect_err("the detached native candidate must reject its order-one state");
    let expected_native_load_error = match format {
        PluginFormat::Clap => "rejected persisted state",
        PluginFormat::Vst3 => "failed to restore component state",
        PluginFormat::AudioUnit => unreachable!("the loaded fixture covers CLAP and VST3"),
    };
    assert!(
        single_band_failure
            .to_string()
            .contains(expected_native_load_error),
        "the typed envelope is valid, then candidate native loading rejects it: {single_band_failure}"
    );
    let single_band_after_failure = restored
        .serialize()
        .expect("serialize single-band route after candidate rejection")
        .external_plugin_state()
        .expect("read retained state envelope")
        .expect("retained route carries native state");
    assert_eq!(
        single_band_after_failure, single_band_before_failure,
        "failed single-band candidate load must retain all live opaque and typed state"
    );
    assert_eq!(restored.audio_setup(), state.audio_setup.as_ref());
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), expected_outputs);

    let single_band_continuation = sparse_acn_basis_block(9);
    let retained_single_band =
        render_block(&mut restored, &single_band_continuation, expected_outputs);
    let twin_single_band = render_block(
        &mut untouched_twin,
        &single_band_continuation,
        expected_outputs,
    );
    assert_eq!(
        retained_single_band, twin_single_band,
        "single-band candidate failure must preserve complete output against its synchronized twin"
    );
    let mut alternate_control_state = saved_state.clone();
    set_native_state_bool(
        &mut alternate_control_state.opaque_state,
        format,
        "max_re_weighting",
        true,
    );
    let mut alternate_control =
        ExternalPlugin::from_placeholder_state(&alternate_control_state, SAMPLE_RATE)
            .expect("create same-layout control with the alternate persisted Max-rE value");
    let alternate_output = render_block(
        &mut alternate_control,
        &single_band_continuation,
        expected_outputs,
    );
    assert!(
        retained_single_band
            .iter()
            .zip(&alternate_output)
            .any(|(retained, alternate)| (retained - alternate).abs() > 1.0e-6),
        "the preserved single-band persisted Max-rE value must remain observable"
    );

    restored
        .deserialize(&preset)
        .expect("a valid single-band preset must succeed after candidate refusal");
    let mut single_band_retry_twin =
        ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
            .expect("create a fresh single-band twin for the successful retry");
    let single_band_retry_input = sparse_acn_basis_block(10);
    let single_band_retry = render_block(&mut restored, &single_band_retry_input, expected_outputs);
    let single_band_retry_twin_output = render_block(
        &mut single_band_retry_twin,
        &single_band_retry_input,
        expected_outputs,
    );
    assert_eq!(
        single_band_retry, single_band_retry_twin_output,
        "a valid single-band retry must restore complete output matching an independent twin"
    );
    let mut single_band_retry_reference = direct_decoder(direct_target_layout, false);
    let expected_single_band_retry = render_block(
        &mut single_band_retry_reference,
        &single_band_retry_input,
        expected_outputs,
    );
    assert_matches_reference(
        &single_band_retry,
        &expected_single_band_retry,
        expected_outputs,
        "valid single-band preset after failed candidate load",
    );

    let mut same_width_source_state = ExternalPluginState::new(
        descriptor.clone(),
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    );
    same_width_source_state.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::SevenOne,
    });
    let same_width_source =
        ExternalPlugin::from_placeholder_state(&same_width_source_state, SAMPLE_RATE)
            .expect("create 7.1 state with eight output channels");
    same_width_source_state.opaque_state = same_width_source
        .save_opaque_state()
        .expect("save 7.1 native state");
    same_width_source_state.audio_setup = Some(NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::FiveOneTwo,
    });
    assert!(
        ExternalPlugin::from_placeholder_state(&same_width_source_state, SAMPLE_RATE).is_err(),
        "same-width but different hidden targets must not be conflated"
    );

    verify_deliberate_reconfiguration(
        format,
        &descriptor,
        target_layout,
        direct_target_layout,
        expected_outputs,
    );
}

fn verify_deliberate_reconfiguration(
    format: PluginFormat,
    descriptor: &PluginDescriptor,
    final_layout: NativeAmbisonicsTargetLayout,
    final_target_name: &str,
    final_output_channels: usize,
) {
    let mut plugin = ExternalPlugin::new(descriptor, SAMPLE_RATE)
        .expect("create legacy-default decoder for deliberate reconfiguration");
    let default_state = plugin
        .serialize()
        .expect("serialize legacy default before state changes")
        .external_plugin_state()
        .expect("read default external state")
        .expect("default state contains native plugin data");
    let mut state_bytes = default_state.opaque_state;
    set_native_state_bool(&mut state_bytes, format, "max_re_weighting", false);
    set_native_state_bool(&mut state_bytes, format, "dual_band", true);
    // NIH stores choice controls as their integer parameter index. The
    // human-readable "allrad" label belongs to the SOTF schema, not the
    // serialized native `IntParam` representation.
    set_native_state_int(&mut state_bytes, format, "algorithm", 1);
    plugin
        .load_opaque_state(&state_bytes)
        .expect("load nondefault hidden controls at the existing layout");

    let mut old_layout_reference = direct_decoder_with_controls(1, "5.1", false, true, "allrad");
    for block_index in 0..2 {
        let input = sparse_acn_basis_block(block_index);
        let expected = render_block(&mut old_layout_reference, &input, 6);
        let actual = render_block(&mut plugin, &input, 6);
        assert_matches_reference(
            &actual,
            &expected,
            6,
            "nondefault hidden controls before deliberate layout change",
        );
    }

    let seven_one = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::SevenOne,
    };
    plugin
        .reconfigure_audio_setup(seven_one.clone())
        .expect("deliberately change order while preserving hidden plugin controls");
    assert_eq!(plugin.audio_setup(), Some(&seven_one));
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), 8);
    assert_eq!(plugin.discovery_descriptor(), descriptor);

    let mut seven_one_reference = direct_decoder_with_controls(7, "7.1", false, true, "allrad");
    for block_index in 2..4 {
        let input = sparse_acn_basis_block(block_index);
        let expected = render_block(&mut seven_one_reference, &input, 8);
        let actual = render_block(&mut plugin, &input, 8);
        assert_matches_reference(
            &actual,
            &expected,
            8,
            "order-seven layout after deliberate reconfiguration",
        );
    }

    let same_width = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::FiveOneTwo,
    };
    plugin
        .reconfigure_audio_setup(same_width.clone())
        .expect("select a different named target with the same output width");
    assert_eq!(plugin.output_channels(), 8);
    assert_eq!(plugin.audio_setup(), Some(&same_width));
    let mut same_width_reference = direct_decoder_with_controls(7, "5.1.2", false, true, "allrad");
    let same_width_input = sparse_acn_basis_block(4);
    let expected_same_width = render_block(&mut same_width_reference, &same_width_input, 8);
    let actual_same_width = render_block(&mut plugin, &same_width_input, 8);
    assert_matches_reference(
        &actual_same_width,
        &expected_same_width,
        8,
        "same-width named target reconfiguration",
    );

    let high_order = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: final_layout,
    };
    plugin
        .reconfigure_audio_setup(high_order.clone())
        .expect("select the final higher-order speaker target");
    assert_eq!(plugin.input_channels(), INPUT_CHANNELS);
    assert_eq!(plugin.output_channels(), final_output_channels);
    assert_eq!(plugin.audio_setup(), Some(&high_order));
    assert_eq!(plugin.discovery_descriptor(), descriptor);

    let shrink_setup = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::SevenOne,
    };
    plugin
        .reconfigure_audio_setup(shrink_setup.clone())
        .expect("shrink the active output bus after a wider target");
    assert_eq!(plugin.output_channels(), 8);
    assert_eq!(plugin.audio_setup(), Some(&shrink_setup));
    let mut shrink_reference = direct_decoder_with_controls(7, "7.1", false, true, "allrad");
    let shrink_input = sparse_acn_basis_block(11);
    let expected_shrink = render_block(&mut shrink_reference, &shrink_input, 8);
    let actual_shrink = render_block(&mut plugin, &shrink_input, 8);
    assert_matches_reference(
        &actual_shrink,
        &expected_shrink,
        8,
        "order-seven output shrink after deliberate negotiation",
    );
    plugin
        .reconfigure_audio_setup(high_order.clone())
        .expect("expand back to the selected higher-order speaker target");
    assert_eq!(plugin.output_channels(), final_output_channels);
    let mut expand_reference =
        direct_decoder_with_controls(7, final_target_name, false, true, "allrad");
    let expand_input = sparse_acn_basis_block(12);
    let expected_expand = render_block(&mut expand_reference, &expand_input, final_output_channels);
    let actual_expand = render_block(&mut plugin, &expand_input, final_output_channels);
    assert_matches_reference(
        &actual_expand,
        &expected_expand,
        final_output_channels,
        "order-seven output expansion after deliberate negotiation",
    );

    let saved_preset = plugin
        .serialize()
        .expect("serialize deliberately selected setup and native state");
    let saved_state = saved_preset
        .external_plugin_state()
        .expect("read reconfigured external state")
        .expect("reconfigured state contains native plugin data");
    assert_eq!(saved_state.descriptor, *descriptor);
    assert_eq!(saved_state.audio_setup, Some(high_order.clone()));
    assert_eq!(
        native_state_bool(&saved_state.opaque_state, format, "max_re_weighting"),
        Some(false)
    );
    assert_eq!(
        native_state_bool(&saved_state.opaque_state, format, "dual_band"),
        Some(true)
    );
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "algorithm"),
        Some(1),
        "serialized NIH choice index 1 is AllRAD"
    );
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "order"),
        Some(7)
    );
    let expected_target_layout_index = match final_layout {
        NativeAmbisonicsTargetLayout::FiveOne => 0,
        NativeAmbisonicsTargetLayout::SevenOne => 1,
        NativeAmbisonicsTargetLayout::FiveOneTwo => 2,
        NativeAmbisonicsTargetLayout::FiveOneFour => 3,
        NativeAmbisonicsTargetLayout::SevenOneTwo => 4,
        NativeAmbisonicsTargetLayout::SevenOneFour => 5,
        NativeAmbisonicsTargetLayout::NineOneFour => 6,
        NativeAmbisonicsTargetLayout::NineOneSixWide => 7,
    };
    assert_eq!(
        native_state_int(&saved_state.opaque_state, format, "target_layout"),
        Some(expected_target_layout_index),
        "NIH serializes the named target choice as its integer index"
    );

    let mut restored = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("restore reconfigured typed setup with its matching opaque state");
    assert_eq!(restored.audio_setup(), Some(&high_order));
    assert_eq!(restored.discovery_descriptor(), descriptor);
    let mut untouched_twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("make a synchronized twin before the rejected-change check");
    let mut final_reference =
        direct_decoder_with_controls(7, final_target_name, false, true, "allrad");
    let restore_input = sparse_acn_basis_block(5);
    let expected_restored =
        render_block(&mut final_reference, &restore_input, final_output_channels);
    let actual_restored = render_block(&mut restored, &restore_input, final_output_channels);
    assert_matches_reference(
        &actual_restored,
        &expected_restored,
        final_output_channels,
        "serialized deliberate layout change and restored state",
    );
    let twin_restore_output =
        render_block(&mut untouched_twin, &restore_input, final_output_channels);
    assert_eq!(
        actual_restored, twin_restore_output,
        "the restored instance and rollback twin start from identical state"
    );

    let invalid = NativePluginAudioSetup::Ambisonics {
        order: 8,
        target_layout: final_layout,
    };
    assert!(
        restored.reconfigure_audio_setup(invalid).is_err(),
        "unsupported order must fail before candidate commit"
    );
    let retained_input = sparse_acn_basis_block(6);
    let retained_from_original =
        render_block(&mut restored, &retained_input, final_output_channels);
    let retained_from_twin =
        render_block(&mut untouched_twin, &retained_input, final_output_channels);
    assert_eq!(
        retained_from_original, retained_from_twin,
        "a rejected deliberate change must preserve the current active instance"
    );

    let mut late_failure_preset = ExternalPlugin::new(descriptor, SAMPLE_RATE)
        .expect("create a native order-one state for the late-failure fixture")
        .serialize()
        .expect("serialize native order-one state");
    let mut late_failure_state = late_failure_preset
        .external_plugin_state()
        .expect("read order-one external state")
        .expect("order-one preset contains native state");
    late_failure_state.audio_setup = Some(high_order.clone());
    assert_eq!(
        native_state_int(&late_failure_state.opaque_state, format, "order"),
        Some(1),
        "the late candidate must carry an order-one native blob"
    );
    late_failure_state
        .validate()
        .expect("the typed order-seven setup and descriptor envelope pass outer validation");
    late_failure_preset
        .set_external_plugin_state(&late_failure_state)
        .expect("write the valid-envelope late-failure state");

    let live_state_before_late_failure = restored
        .serialize()
        .expect("serialize populated order-seven decoder before late candidate construction")
        .external_plugin_state()
        .expect("read current external state")
        .expect("current native state is available");
    let late_failure = restored
        .deserialize(&late_failure_preset)
        .expect_err("native state load must reject the mismatched candidate");
    let expected_native_load_error = match format {
        PluginFormat::Clap => "rejected persisted state",
        PluginFormat::Vst3 => "failed to restore component state",
        PluginFormat::AudioUnit => unreachable!("the loaded fixture covers CLAP and VST3"),
    };
    assert!(
        late_failure
            .to_string()
            .contains(expected_native_load_error),
        "outer validation must pass before the detached candidate's native state load rejects the blob; got: {late_failure}"
    );
    assert_eq!(restored.audio_setup(), Some(&high_order));
    assert_eq!(restored.input_channels(), INPUT_CHANNELS);
    assert_eq!(restored.output_channels(), final_output_channels);
    let live_state_after_late_failure = restored
        .serialize()
        .expect("serialize decoder after late candidate rejection")
        .external_plugin_state()
        .expect("read retained external state")
        .expect("retained native state is available");
    assert_eq!(
        live_state_after_late_failure, live_state_before_late_failure,
        "failed late restore must preserve the complete live typed and opaque state"
    );

    let late_failure_input = sparse_acn_basis_block(7);
    let retained_after_late_failure =
        render_block(&mut restored, &late_failure_input, final_output_channels);
    let twin_after_late_failure = render_block(
        &mut untouched_twin,
        &late_failure_input,
        final_output_channels,
    );
    assert_eq!(
        retained_after_late_failure, twin_after_late_failure,
        "failed late restore must preserve populated dual-band audio against its synchronized twin"
    );
    let mut cold_control = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("create a cold control with the same setup and persisted parameters");
    let cold_control_output = render_block(
        &mut cold_control,
        &late_failure_input,
        final_output_channels,
    );
    assert!(
        retained_after_late_failure
            .iter()
            .zip(&cold_control_output)
            .any(|(populated, cold)| (populated - cold).abs() > 1.0e-6),
        "the dual-band crossover should make this continuation sensitive to retained processing history"
    );

    restored
        .deserialize(&saved_preset)
        .expect("a valid order-seven preset must succeed after late candidate refusal");
    let mut retry_twin = ExternalPlugin::from_placeholder_state(&saved_state, SAMPLE_RATE)
        .expect("create a fresh twin for the successful restore retry");
    assert_eq!(restored.audio_setup(), Some(&high_order));
    let retry_input = sparse_acn_basis_block(8);
    let retry_output = render_block(&mut restored, &retry_input, final_output_channels);
    let retry_twin_output = render_block(&mut retry_twin, &retry_input, final_output_channels);
    assert_eq!(
        retry_output, retry_twin_output,
        "successful retry must produce the same complete output as an independently restored twin"
    );
    let mut retry_reference =
        direct_decoder_with_controls(7, final_target_name, false, true, "allrad");
    let expected_retry = render_block(&mut retry_reference, &retry_input, final_output_channels);
    assert_matches_reference(
        &retry_output,
        &expected_retry,
        final_output_channels,
        "valid order-seven state after late candidate retry",
    );
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

fn direct_decoder(target_layout: &str, max_re_weighting: bool) -> AmbisonicsDecoderPlugin {
    direct_decoder_with_controls(7, target_layout, max_re_weighting, false, "mode_matching")
}

fn direct_decoder_with_controls(
    order: usize,
    target_layout: &str,
    max_re_weighting: bool,
    dual_band: bool,
    algorithm: &str,
) -> AmbisonicsDecoderPlugin {
    let mut config = AmbisonicsDecoderConfig::default();
    config.order = order;
    config.target_layout = target_layout.to_string();
    config.max_re_weighting = max_re_weighting;
    config.dual_band = dual_band;
    config.algorithm = algorithm.to_string();
    let mut plugin = AmbisonicsDecoderPlugin::new(&config).expect("build direct reference");
    plugin
        .initialize(f64::from(SAMPLE_RATE))
        .expect("initialize reference");
    plugin
}

fn native_state_bool(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<bool> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("bool")?
        .as_bool()
}

fn native_state_int(opaque_state: &[u8], format: PluginFormat, id: &str) -> Option<i64> {
    native_state_json(opaque_state, format)?
        .get("params")?
        .get(id)?
        .get("i32")?
        .as_i64()
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

fn set_native_state_bool(opaque_state: &mut Vec<u8>, format: PluginFormat, id: &str, value: bool) {
    set_native_state_value(opaque_state, format, id, serde_json::json!({"bool": value}));
}

fn set_native_state_int(opaque_state: &mut Vec<u8>, format: PluginFormat, id: &str, value: i32) {
    set_native_state_value(opaque_state, format, id, serde_json::json!({"i32": value}));
}

fn set_native_state_value(
    opaque_state: &mut Vec<u8>,
    format: PluginFormat,
    id: &str,
    value: serde_json::Value,
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
    let parameter = state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|params| params.get_mut(id))
        .expect("serialized state contains requested parameter");
    *parameter = value;
    let serialized = serde_json::to_vec(&state).expect("serialize mutated native state");
    opaque_state.clear();
    if format == PluginFormat::Clap {
        opaque_state.extend_from_slice(&(serialized.len() as u64).to_le_bytes());
    }
    opaque_state.extend_from_slice(&serialized);
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
        "ACN 63 basis impulse must reach the selected native decoder"
    );
}
