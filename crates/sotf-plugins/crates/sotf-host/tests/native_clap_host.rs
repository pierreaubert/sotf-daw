#![cfg(feature = "external-plugin-clap")]

use sotf_host::external_plugin::{
    ExternalHostingBackend, ExternalPlugin, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_host::serialization::SerializablePlugin;
use sotf_host::{
    ExternalPluginSandboxMode, ExternalPluginSandboxPolicy, ExternalPluginState,
    ExternalPluginWorkerCommand, IsolatedExternalPlugin, IsolatedExternalPluginConfig,
};
use std::path::PathBuf;
use std::time::Duration;

#[test]
#[ignore = "requires SOTF_TEST_CLAP_PLUGIN to point to the built plugins-nih gain CLAP library"]
fn native_clap_gain_processes_and_round_trips_state() {
    let descriptor = clap_gain_descriptor();

    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).expect("load native CLAP gain");
    assert_eq!(plugin.hosting_backend(), ExternalHostingBackend::Clap);
    assert_eq!(plugin.descriptor().id, "org.spinorama.sotf.gain");
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    let gain = plugin
        .parameters()
        .into_iter()
        .find(|parameter| parameter.name.to_ascii_lowercase().contains("gain"))
        .expect("CLAP gain parameter metadata");
    plugin
        .set_parameter(gain.id.clone(), gain.default_value.clone())
        .expect("queue CLAP parameter event");
    assert_eq!(plugin.get_parameter(&gain.id), Some(gain.default_value));

    let frames = 127;
    let input = (0..frames * 2)
        .map(|sample| (sample as f32 / (frames * 2) as f32) * 0.5 - 0.25)
        .collect::<Vec<_>>();
    let mut output = vec![f32::NAN; input.len()];
    let context = ProcessContext::new(48_000, frames);
    assert_eq!(
        plugin.process(&input, &mut output, &context).unwrap(),
        frames
    );
    for (actual, expected) in output.iter().zip(&input) {
        assert!(actual.is_finite());
        assert!((actual - expected).abs() < 1.0e-5);
    }

    // A fractional value detects accidental integer parameter registration.
    // Allow the documented smoothing to settle before checking absolute gain.
    let gain_db = -6.5_f32;
    // NIH exposes normalized values to CLAP for continuous parameters.
    // The gain DSP range is linear in dB from -60 to +20.
    assert_eq!(gain.min_value, Some(ParameterValue::Float(0.0)));
    assert_eq!(gain.max_value, Some(ParameterValue::Float(1.0)));
    plugin
        .set_parameter(gain.id, ParameterValue::Float((gain_db + 60.0) / 80.0))
        .expect("queue fractional CLAP gain");
    for _ in 0..256 {
        plugin.process(&input, &mut output, &context).unwrap();
    }
    let expected_gain = 10.0_f32.powf(gain_db / 20.0);
    for (actual, original) in output.iter().zip(&input) {
        assert!((actual - original * expected_gain).abs() < 1.0e-5);
    }

    let preset = plugin.serialize().expect("save CLAP state");
    let state = preset
        .external_plugin_state()
        .unwrap()
        .expect("external state envelope");
    assert!(!state.opaque_state.is_empty());

    let mut restored =
        ExternalPlugin::from_placeholder_state(&state, 48_000).expect("restore CLAP state");
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
#[ignore = "requires SOTF_TEST_CLAP_PLUGIN to point to the built plugins-nih gain CLAP library"]
fn isolated_native_clap_worker_processes_and_restores_state() {
    let descriptor = clap_gain_descriptor();
    let mut source = ExternalPlugin::new(&descriptor, 48_000).expect("load native CLAP gain");
    let gain = source
        .parameters()
        .into_iter()
        .find(|parameter| parameter.name.to_ascii_lowercase().contains("gain"))
        .expect("CLAP gain parameter metadata");
    let gain_db = -6.5_f32;
    source
        .set_parameter(gain.id, ParameterValue::Float((gain_db + 60.0) / 80.0))
        .expect("queue non-default CLAP gain");

    let frames = 127;
    let input = (0..frames * 2)
        .map(|sample| (sample as f32 / (frames * 2) as f32) * 0.5 - 0.25)
        .collect::<Vec<_>>();
    let context = ProcessContext::new(48_000, frames);
    let mut source_output = vec![0.0; input.len()];
    for _ in 0..256 {
        source
            .process(&input, &mut source_output, &context)
            .unwrap();
    }

    let preset = source.serialize().expect("save CLAP state");
    let in_process_state = preset
        .external_plugin_state()
        .unwrap()
        .expect("external state envelope");
    let isolated_state = ExternalPluginState::new(
        in_process_state.descriptor.clone(),
        ExternalPluginSandboxMode::Isolated,
        in_process_state.opaque_state.clone(),
    );

    let worker_binary = env!("CARGO_BIN_EXE_sotf-external-plugin-worker");
    let config = || IsolatedExternalPluginConfig {
        max_block_frames: frames as u32,
        deadline: Duration::from_secs(2),
        worker_command: ExternalPluginWorkerCommand::new(worker_binary)
            .arg("--idle-sleep-micros")
            .arg("50"),
        sandbox_policy: ExternalPluginSandboxPolicy::disabled(),
        initial_state: Some(isolated_state.clone()),
        ..Default::default()
    };

    let mut first =
        IsolatedExternalPlugin::from_placeholder_state(&isolated_state, 48_000, config())
            .expect("start native CLAP worker from serialized state");
    assert_eq!(first.launch_error(), None);
    let mut first_output = vec![0.0; input.len()];
    for _ in 0..64 {
        first.process(&input, &mut first_output, &context).unwrap();
        // Simulate callback pacing so the asynchronous worker can publish the
        // preceding block before its fixed one-block transport deadline.
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut second =
        IsolatedExternalPlugin::from_placeholder_state(&isolated_state, 48_000, config())
            .expect("start fresh native CLAP worker from serialized state");
    assert_eq!(second.launch_error(), None);
    let mut second_output = vec![0.0; input.len()];
    for _ in 0..64 {
        second
            .process(&input, &mut second_output, &context)
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(first.latency_samples(), source.latency_samples() + frames);
    assert_eq!(second.latency_samples(), source.latency_samples() + frames);
    let expected_gain = 10.0_f32.powf(gain_db / 20.0);
    for rendered in [&source_output, &first_output, &second_output] {
        for (actual, original) in rendered.iter().zip(&input) {
            assert!((actual - original * expected_gain).abs() < 1.0e-5);
        }
    }
}

fn clap_gain_descriptor() -> PluginDescriptor {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_CLAP_PLUGIN")
            .expect("SOTF_TEST_CLAP_PLUGIN must point to a .clap file or bundle"),
    );
    PluginDescriptor {
        id: "org.spinorama.sotf.gain".into(),
        name: "SOTF: Gain".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Clap,
        path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

#[test]
#[ignore = "requires SOTF_TEST_AMBISONICS_CLAP_PLUGIN to point to the exported Ambisonics CLAP library"]
fn native_clap_ambisonics_host_default_baseline_and_order_seven_setup() {
    let descriptor = clap_ambisonics_descriptor();
    let mut default_plugin = ExternalPlugin::new(&descriptor, 48_000)
        .expect("load native CLAP Ambisonics default configuration");
    assert_eq!(default_plugin.input_channels(), 4);
    assert_eq!(default_plugin.output_channels(), 6);

    let frames = 1024;
    let mut input = vec![0.0_f32; frames * 4];
    for frame in 0..frames {
        for channel in 0..4 {
            input[frame * 4 + channel] =
                (((frame * 17 + channel * 131 + frame * channel * 3) % 997) as f32 - 498.0)
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
        std::fs::write(directory.join("clap-order1-5.1-host.f32le"), bytes)
            .expect("write CLAP default output baseline");
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
        "target_layout": "seven_one_four"
    });
    let state: ExternalPluginState = serde_json::from_value(value).unwrap();
    let plugin = ExternalPlugin::from_placeholder_state(&state, 48_000)
        .expect("restore typed CLAP Ambisonics setup");
    assert_eq!(plugin.input_channels(), 64);
    assert_eq!(plugin.output_channels(), 12);
    assert_eq!(plugin.descriptor().audio_inputs, 64);
    assert_eq!(plugin.descriptor().audio_outputs, 12);
}

fn clap_ambisonics_descriptor() -> PluginDescriptor {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_AMBISONICS_CLAP_PLUGIN")
            .expect("SOTF_TEST_AMBISONICS_CLAP_PLUGIN must point to the exported .clap library"),
    );
    PluginDescriptor {
        id: "org.spinorama.sotf.ambisonics".into(),
        name: "SOTF: Ambisonics Decoder".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Clap,
        path,
        audio_inputs: 4,
        audio_outputs: 6,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}
