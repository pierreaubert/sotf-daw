#![cfg(feature = "external-plugin-vst3")]

use sotf_host::external_plugin::{
    ExternalHostingBackend, ExternalPlugin, NativeAmbisonicsTargetLayout, NativePluginAudioSetup,
    PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{
    MidiEvent, MidiMessage, ParameterEvent, Plugin, ProcessContext, TransportInfo,
};
use sotf_host::serialization::SerializablePlugin;
use sotf_host::{
    ExternalPluginSandboxMode, ExternalPluginSandboxPolicy, ExternalPluginState,
    ExternalPluginWorkerCommand, IsolatedExternalPlugin, IsolatedExternalPluginConfig,
};
use std::path::PathBuf;
use std::time::Duration;

#[test]
#[ignore = "requires SOTF_TEST_VST3_PLUGIN to point to the built plugins-nih gain VST3 library"]
fn native_vst3_gain_processes_audio() {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_VST3_PLUGIN")
            .expect("SOTF_TEST_VST3_PLUGIN must point to a .vst3 file or bundle"),
    );
    let descriptor = PluginDescriptor {
        id: "vst3.SOTF: Gain".into(),
        name: "SOTF: Gain".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Vst3,
        path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    };

    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).expect("load native VST3 gain");
    assert_eq!(plugin.hosting_backend(), ExternalHostingBackend::Vst3);
    assert_eq!(plugin.descriptor().name, "SOTF: Gain");
    assert_ne!(plugin.descriptor().id, descriptor.id);
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    let gain = plugin
        .parameters()
        .into_iter()
        .find(|parameter| {
            parameter.name.to_ascii_lowercase().contains("gain")
                && matches!(
                    parameter.default_value,
                    sotf_host::parameters::ParameterValue::Float(_)
                )
        })
        .expect("VST3 gain parameter metadata");
    plugin
        .set_parameter(gain.id.clone(), gain.default_value.clone())
        .expect("queue VST3 parameter change");
    assert_eq!(
        plugin.get_parameter(&gain.id),
        Some(gain.default_value.clone())
    );

    let frames = 127;
    let input = (0..frames * 2)
        .map(|sample| (sample as f32 / (frames * 2) as f32) * 0.5 - 0.25)
        .collect::<Vec<_>>();
    let mut output = vec![f32::NAN; input.len()];
    let midi = [MidiEvent::new(17, MidiMessage::note_on(0, 60, 100))];
    let automation = [ParameterEvent::new(
        83,
        gain.id.clone(),
        gain.default_value.clone(),
    )];
    let context = ProcessContext::new(48_000, frames)
        .with_transport(TransportInfo::at_sample(96_000, 48_000).with_tempo(75.0, 48_000))
        .with_all_events(&midi, &[], &automation);
    assert_eq!(
        plugin.process(&input, &mut output, &context).unwrap(),
        frames
    );
    for (actual, expected) in output.iter().zip(&input) {
        assert!(actual.is_finite());
        assert!((actual - expected).abs() < 1.0e-5);
    }

    // Do not replay the default-value automation event while measuring a
    // nondefault gain. A fractional value also checks the parameter's type.
    let context = ProcessContext::new(48_000, frames);
    let gain_db = -6.5_f32;
    // The VST3 controller supplies plain-value conversion, so this host API
    // exposes dB rather than the normalized wire representation.
    assert_eq!(gain.min_value, Some(ParameterValue::Float(-60.0)));
    assert_eq!(gain.max_value, Some(ParameterValue::Float(20.0)));
    plugin
        .set_parameter(gain.id, ParameterValue::Float(gain_db))
        .expect("queue fractional VST3 gain");
    for _ in 0..256 {
        plugin.process(&input, &mut output, &context).unwrap();
    }
    let expected_gain = 10.0_f32.powf(gain_db / 20.0);
    for (actual, original) in output.iter().zip(&input) {
        assert!((actual - original * expected_gain).abs() < 1.0e-5);
    }

    let preset = plugin.serialize().expect("save VST3 state");
    let state = preset
        .external_plugin_state()
        .unwrap()
        .expect("external state envelope");
    assert!(!state.opaque_state.is_empty());

    let mut restored =
        ExternalPlugin::from_placeholder_state(&state, 48_000).expect("restore VST3 state");
    let mut restored_output = vec![0.0; input.len()];
    for _ in 0..256 {
        restored
            .process(&input, &mut restored_output, &context)
            .unwrap();
    }
    for (actual, original) in restored_output.iter().zip(&input) {
        assert!((actual - original * expected_gain).abs() < 1.0e-5);
    }
}

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_VST3_PLUGIN to point to the exported Ambisonics VST3 library"]
fn native_vst3_ambisonics_host_default_baseline_and_order_seven_setup() {
    let descriptor = vst3_ambisonics_descriptor();
    let mut default_plugin = ExternalPlugin::new(&descriptor, 48_000)
        .expect("load native VST3 Ambisonics default configuration");
    assert_eq!(default_plugin.input_channels(), 4);
    assert_eq!(default_plugin.output_channels(), 6);

    let frames = 1024;
    let mut input = vec![0.0_f32; frames * 4];
    for frame in 0..frames {
        for channel in 0..4 {
            input[frame * 4 + channel] =
                (((frame * 29 + channel * 83 + frame * channel * 5) % 991) as f32 - 495.0)
                    * 0.000_02;
        }
    }
    let mut baseline = Vec::with_capacity(frames * 6);
    for chunk_start in (0..frames).step_by(127) {
        let count = (frames - chunk_start).min(127);
        let mut output = vec![f32::NAN; count * 6];
        let context = ProcessContext::new(48_000, count);
        default_plugin
            .process(
                &input[chunk_start * 4..(chunk_start + count) * 4],
                &mut output,
                &context,
            )
            .unwrap();
        assert!(output.iter().all(|sample| sample.is_finite()));
        baseline.extend(output);
    }
    if let Some(directory) = std::env::var_os("SOTF_AUDIT_CAPTURE_DIR") {
        let directory = PathBuf::from(directory);
        std::fs::create_dir_all(&directory).expect("create AUD135 baseline directory");
        let bytes = baseline
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        std::fs::write(directory.join("vst3-order1-5.1-host.f32le"), bytes)
            .expect("write VST3 default output baseline");
    }

    let mut value = serde_json::to_value(ExternalPluginState::new(
        descriptor,
        ExternalPluginSandboxMode::InProcess,
        Vec::new(),
    ))
    .unwrap();
    value["audio_setup"] = serde_json::json!({
        "type": "ambisonics",
        "order": 7,
        "target_layout": "nine_one_six_wide"
    });
    let state: ExternalPluginState = serde_json::from_value(value).unwrap();
    let plugin = ExternalPlugin::from_placeholder_state(&state, 48_000)
        .expect("restore typed VST3 Ambisonics setup");
    assert_eq!(plugin.input_channels(), 64);
    assert_eq!(plugin.output_channels(), 16);
    assert_eq!(plugin.descriptor().audio_inputs, 64);
    assert_eq!(plugin.descriptor().audio_outputs, 16);
}

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_VST3_PLUGIN and the exported external-plugin worker"]
fn isolated_vst3_order_seven_late_native_load_failure_preserves_populated_worker() {
    const FRAMES: usize = 64;
    const INPUT_CHANNELS: usize = 64;
    const OUTPUT_CHANNELS: usize = 16;
    const SAMPLE_RATE: u32 = 48_000;

    let descriptor = vst3_ambisonics_descriptor();
    let setup = NativePluginAudioSetup::Ambisonics {
        order: 7,
        target_layout: NativeAmbisonicsTargetLayout::NineOneSixWide,
    };
    let seed = ExternalPlugin::new_with_audio_setup(&descriptor, setup.clone(), SAMPLE_RATE)
        .expect("load the supported order-seven VST3 setup");
    let mut live_state = seed.placeholder_state();
    live_state.sandbox_mode = ExternalPluginSandboxMode::Isolated;
    live_state.audio_setup = Some(setup.clone());
    live_state.opaque_state = seed
        .save_opaque_state()
        .expect("save matching native order-seven state");
    set_vst3_state_bool(&mut live_state.opaque_state, "max_re_weighting", false);
    set_vst3_state_bool(&mut live_state.opaque_state, "dual_band", true);
    live_state
        .validate()
        .expect("the selected typed order-seven envelope is valid");

    let worker_binary = env!("CARGO_BIN_EXE_sotf-external-plugin-worker");
    let config = || IsolatedExternalPluginConfig {
        max_block_frames: FRAMES as u32,
        deadline: Duration::from_secs(2),
        worker_startup_timeout: Duration::from_secs(5),
        worker_command: ExternalPluginWorkerCommand::new(worker_binary)
            .arg("--idle-sleep-micros")
            .arg("50"),
        sandbox_policy: ExternalPluginSandboxPolicy::disabled(),
        ..Default::default()
    };

    let mut live =
        IsolatedExternalPlugin::from_placeholder_state(&live_state, SAMPLE_RATE, config())
            .expect("start the populated-state worker from a valid saved order-seven state");
    let mut twin =
        IsolatedExternalPlugin::from_placeholder_state(&live_state, SAMPLE_RATE, config())
            .expect("start a synchronized order-seven worker twin");
    let mut in_process_state = live_state.clone();
    in_process_state.sandbox_mode = ExternalPluginSandboxMode::InProcess;
    let mut direct_reference = ExternalPlugin::from_placeholder_state_with_max_block_frames(
        &in_process_state,
        SAMPLE_RATE,
        FRAMES,
    )
    .expect("create an in-process reference from the same genuine native state");
    for worker in [&live, &twin] {
        assert_eq!(worker.input_channels(), INPUT_CHANNELS);
        assert_eq!(worker.output_channels(), OUTPUT_CHANNELS);
        assert_eq!(worker.launch_error(), None);
    }
    assert_eq!(
        live.latency_samples(),
        direct_reference.latency_samples() + FRAMES,
        "isolated IPC declares one full block of additional transport latency"
    );
    assert_eq!(live.latency_samples(), twin.latency_samples());

    let warm_inputs = (0..8).map(order_seven_input_block).collect::<Vec<_>>();
    let live_warm = process_order_seven_blocks(&mut live, &warm_inputs, FRAMES, OUTPUT_CHANNELS);
    let twin_warm = process_order_seven_blocks(&mut twin, &warm_inputs, FRAMES, OUTPUT_CHANNELS);
    let direct_warm =
        process_order_seven_blocks(&mut direct_reference, &warm_inputs, FRAMES, OUTPUT_CHANNELS);
    assert_eq!(
        live_warm, twin_warm,
        "worker twins must process every full output vector identically"
    );
    assert_worker_matches_delayed_reference(&live_warm, &direct_warm, None, "initial worker audio");
    assert!(
        live_warm
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1.0e-7)
    );
    assert_eq!(live.block_timeout_count(), 0);
    assert_eq!(twin.block_timeout_count(), 0);
    assert_eq!(live.block_worker_failure_count(), 0);
    assert_eq!(twin.block_worker_failure_count(), 0);

    let live_state_before = live
        .capture_worker_state()
        .expect("capture the populated live worker state before candidate construction");
    let twin_state_before = twin
        .capture_worker_state()
        .expect("capture synchronized twin state before candidate construction");
    assert_eq!(live_state_before, twin_state_before);

    let default_seed = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .expect("create the native default-order state used by the refusal fixture");
    let mut incompatible_native_state = default_seed.placeholder_state();
    incompatible_native_state.sandbox_mode = ExternalPluginSandboxMode::Isolated;
    incompatible_native_state.audio_setup = Some(setup.clone());
    incompatible_native_state.opaque_state = default_seed
        .save_opaque_state()
        .expect("save native default-order state");
    assert_eq!(
        vst3_state_int(&incompatible_native_state.opaque_state, "order"),
        Some(1),
        "candidate must carry a genuine order-one native state blob"
    );
    incompatible_native_state
        .validate()
        .expect("outer descriptor/setup validation succeeds before worker-side native load");

    let native_load_error = match IsolatedExternalPlugin::from_placeholder_state(
        &incompatible_native_state,
        SAMPLE_RATE,
        config(),
    ) {
        Ok(_) => panic!("candidate worker accepted an order-one blob for the order-seven setup"),
        Err(error) => error,
    };
    assert!(
        native_load_error.contains("failed to create external plugin wrapper")
            && native_load_error.contains("failed to restore component state"),
        "outer validation must pass and the detached worker must fail at VST3 native state loading: {native_load_error}"
    );

    let live_state_after = live
        .capture_worker_state()
        .expect("capture live worker state after detached candidate refusal");
    let twin_state_after = twin
        .capture_worker_state()
        .expect("capture twin state after detached candidate refusal");
    assert_eq!(live_state_after, live_state_before);
    assert_eq!(twin_state_after, twin_state_before);
    assert_eq!(live_state_after, twin_state_after);

    let continuation_inputs = (8..12).map(order_seven_input_block).collect::<Vec<_>>();
    let live_continuation =
        process_order_seven_blocks(&mut live, &continuation_inputs, FRAMES, OUTPUT_CHANNELS);
    let twin_continuation =
        process_order_seven_blocks(&mut twin, &continuation_inputs, FRAMES, OUTPUT_CHANNELS);
    let direct_continuation = process_order_seven_blocks(
        &mut direct_reference,
        &continuation_inputs,
        FRAMES,
        OUTPUT_CHANNELS,
    );
    assert_eq!(
        live_continuation, twin_continuation,
        "late candidate refusal must preserve complete populated order-seven output vectors"
    );
    assert_worker_matches_delayed_reference(
        &live_continuation,
        &direct_continuation,
        direct_warm.last().map(Vec::as_slice),
        "worker continuation after candidate refusal",
    );

    let mut alternate_state = live_state.clone();
    set_vst3_state_bool(&mut alternate_state.opaque_state, "max_re_weighting", true);
    alternate_state
        .validate()
        .expect("the alternate persisted control remains a valid outer state");
    let mut alternate =
        IsolatedExternalPlugin::from_placeholder_state(&alternate_state, SAMPLE_RATE, config())
            .expect("start the alternate persisted-control worker");
    let alternate_warm =
        process_order_seven_blocks(&mut alternate, &warm_inputs, FRAMES, OUTPUT_CHANNELS);
    let mut alternate_reference_state = alternate_state.clone();
    alternate_reference_state.sandbox_mode = ExternalPluginSandboxMode::InProcess;
    let mut alternate_reference = ExternalPlugin::from_placeholder_state_with_max_block_frames(
        &alternate_reference_state,
        SAMPLE_RATE,
        FRAMES,
    )
    .expect("create an alternate-state in-process reference");
    let alternate_direct_warm = process_order_seven_blocks(
        &mut alternate_reference,
        &warm_inputs,
        FRAMES,
        OUTPUT_CHANNELS,
    );
    assert_worker_matches_delayed_reference(
        &alternate_warm,
        &alternate_direct_warm,
        None,
        "alternate persisted-state worker audio",
    );
    assert!(
        live_warm
            .iter()
            .flatten()
            .zip(alternate_warm.iter().flatten())
            .any(|(selected, alternate)| (selected - alternate).abs() > 1.0e-6),
        "the persisted Max-rE state must measurably affect complete worker output vectors"
    );

    let zeros = vec![vec![0.0; FRAMES * INPUT_CHANNELS]; 2];
    let live_tail = process_order_seven_blocks(&mut live, &zeros, FRAMES, OUTPUT_CHANNELS);
    let twin_tail = process_order_seven_blocks(&mut twin, &zeros, FRAMES, OUTPUT_CHANNELS);
    let direct_tail =
        process_order_seven_blocks(&mut direct_reference, &zeros, FRAMES, OUTPUT_CHANNELS);
    assert_worker_matches_delayed_reference(
        &live_tail,
        &direct_tail,
        direct_continuation.last().map(Vec::as_slice),
        "populated worker zero-input tail",
    );
    let mut cold =
        IsolatedExternalPlugin::from_placeholder_state(&live_state, SAMPLE_RATE, config())
            .expect("start a cold worker with the same persisted parameters");
    let cold_tail = process_order_seven_blocks(&mut cold, &zeros, FRAMES, OUTPUT_CHANNELS);
    let mut cold_reference_state = in_process_state.clone();
    cold_reference_state.opaque_state = live_state.opaque_state.clone();
    let mut cold_reference = ExternalPlugin::from_placeholder_state_with_max_block_frames(
        &cold_reference_state,
        SAMPLE_RATE,
        FRAMES,
    )
    .expect("create a cold in-process reference with the same persisted parameters");
    let cold_direct_tail =
        process_order_seven_blocks(&mut cold_reference, &zeros, FRAMES, OUTPUT_CHANNELS);
    assert_worker_matches_delayed_reference(
        &cold_tail,
        &cold_direct_tail,
        None,
        "cold worker zero-input control",
    );
    assert_eq!(live_tail, twin_tail);
    assert!(
        live_tail[1]
            .iter()
            .zip(&cold_tail[1])
            .any(|(populated, cold)| (populated - cold).abs() > 1.0e-7),
        "dual-band processing must retain a measurable zero-input tail after populated audio"
    );
    drop(alternate);
    drop(cold);
    drop(live);
    drop(twin);

    let mut retry =
        IsolatedExternalPlugin::from_placeholder_state(&live_state, SAMPLE_RATE, config())
            .expect("valid order-seven state must construct successfully after candidate refusal");
    let mut retry_twin =
        IsolatedExternalPlugin::from_placeholder_state(&live_state, SAMPLE_RATE, config())
            .expect("create an independent twin for the successful retry");
    let retry_inputs = (12..16).map(order_seven_input_block).collect::<Vec<_>>();
    let retry_output =
        process_order_seven_blocks(&mut retry, &retry_inputs, FRAMES, OUTPUT_CHANNELS);
    let retry_twin_output =
        process_order_seven_blocks(&mut retry_twin, &retry_inputs, FRAMES, OUTPUT_CHANNELS);
    let mut retry_reference = ExternalPlugin::from_placeholder_state_with_max_block_frames(
        &in_process_state,
        SAMPLE_RATE,
        FRAMES,
    )
    .expect("create a direct reference for the valid state retry");
    let retry_direct_output =
        process_order_seven_blocks(&mut retry_reference, &retry_inputs, FRAMES, OUTPUT_CHANNELS);
    assert_eq!(
        retry_output, retry_twin_output,
        "successful valid-state retry must render the complete order-seven route"
    );
    assert_worker_matches_delayed_reference(
        &retry_output,
        &retry_direct_output,
        None,
        "successful order-seven worker retry",
    );
    assert_eq!(retry.block_timeout_count(), 0);
    assert_eq!(retry.block_worker_failure_count(), 0);
}

fn order_seven_input_block(block_index: usize) -> Vec<f32> {
    let mut input = vec![0.0; 64 * 64];
    for acn_channel in 0..64 {
        let frame = if block_index.is_multiple_of(2) {
            acn_channel
        } else {
            63 - acn_channel
        };
        let amplitude = 0.001 * (acn_channel + 1) as f32 * (block_index + 1) as f32;
        input[frame * 64 + acn_channel] = amplitude;
    }
    input
}

fn process_order_seven_blocks(
    plugin: &mut dyn Plugin,
    inputs: &[Vec<f32>],
    frames: usize,
    output_channels: usize,
) -> Vec<Vec<f32>> {
    let mut outputs = Vec::with_capacity(inputs.len());
    for input in inputs {
        let mut output = vec![f32::NAN; frames * output_channels];
        assert_eq!(
            plugin
                .process(input, &mut output, &ProcessContext::new(48_000, frames))
                .expect("process an order-seven worker block"),
            frames
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        outputs.push(output);
        std::thread::sleep(Duration::from_millis(10));
    }
    outputs
}

fn assert_worker_matches_delayed_reference(
    worker_outputs: &[Vec<f32>],
    direct_outputs: &[Vec<f32>],
    preceding_direct_block: Option<&[f32]>,
    stage: &str,
) {
    assert_eq!(
        worker_outputs.len(),
        direct_outputs.len(),
        "{stage}: block count"
    );
    for (block_index, worker_block) in worker_outputs.iter().enumerate() {
        let expected = if block_index == 0 {
            preceding_direct_block
        } else {
            direct_outputs.get(block_index - 1).map(Vec::as_slice)
        };
        if let Some(expected) = expected {
            assert_eq!(worker_block.len(), expected.len(), "{stage}: sample count");
            let max_residual = worker_block
                .iter()
                .zip(expected)
                .map(|(actual, expected)| (actual - expected).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                max_residual <= 1.0e-6,
                "{stage} block {block_index}: full-vector max residual {max_residual}"
            );
        } else {
            assert!(
                worker_block.iter().all(|sample| sample.abs() <= 1.0e-7),
                "{stage} initial IPC block should be priming silence"
            );
        }
    }
}

fn set_vst3_state_bool(opaque_state: &mut Vec<u8>, id: &str, value: bool) {
    let mut state: serde_json::Value =
        serde_json::from_slice(opaque_state).expect("VST3 native state is NIH JSON");
    let parameters = state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .expect("VST3 native state contains the parameter object");
    assert!(
        parameters.contains_key(id),
        "native state has parameter {id}"
    );
    parameters.insert(id.to_string(), serde_json::json!({"bool": value}));
    *opaque_state = serde_json::to_vec(&state).expect("serialize modified VST3 state");
}

fn vst3_state_int(opaque_state: &[u8], id: &str) -> Option<i64> {
    serde_json::from_slice::<serde_json::Value>(opaque_state)
        .ok()?
        .get("params")?
        .get(id)?
        .get("i32")?
        .as_i64()
}

fn vst3_ambisonics_descriptor() -> PluginDescriptor {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_AMBISONICS_VST3_PLUGIN")
            .expect("SOTF_TEST_AMBISONICS_VST3_PLUGIN must point to the exported .vst3 library"),
    );
    PluginDescriptor {
        id: "536F7466416D6269736E696330303031".into(),
        name: "SOTF: Ambisonics Decoder".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Vst3,
        path,
        audio_inputs: 4,
        audio_outputs: 6,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}
