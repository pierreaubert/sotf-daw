#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

//! Loaded native regressions for retained-node sample-rate transitions.

use sotf_host::external_plugin::{
    ExternalPlugin, ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
use sotf_host::host::DawHost;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, PluginInfo, ProcessContext};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const OLD_RATE: u32 = 96_000;
const NEW_RATE: u32 = 48_000;
const CROSSOVER_CLAP_ID: &str = "org.spinorama.sotf.crossover";
const CONVOLUTION_VST3_CLASS_ID: &str = "536F7466436F6E766F6C75746E303031";
const IR_RESOURCE_FIELD: &str = "sotf_convolution_ir_resource";
const IR_TAPS: [[i16; 4]; 4] = [
    [14_000, 2_000, -3_000, 10_000],
    [3_000, -2_000, 4_000, 1_000],
    [-1_000, 500, 2_000, -1_500],
    [512, -256, 768, 384],
];

/// Test-only upstream node that models a host path whose input rate changes
/// after the node is removed. It duplicates frames; it is not a resampler.
struct SyntheticRateDoubler;

impl Plugin for SyntheticRateDoubler {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Synthetic rate doubler", "test", "SOTF tests")
    }

    fn input_channels(&self) -> usize {
        2
    }

    fn output_channels(&self) -> usize {
        2
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(format!("unknown synthetic parameter '{id}'"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != context.num_frames * 2 || output.len() < context.num_frames * 4 {
            return Err("synthetic rate doubler received invalid buffer geometry".into());
        }
        for frame in 0..context.num_frames {
            let left = input[frame * 2];
            let right = input[frame * 2 + 1];
            let out = frame * 4;
            output[out..out + 4].copy_from_slice(&[left, right, left, right]);
        }
        Ok(context.num_frames * 2)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames.saturating_mul(2)
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        input_rate * 2.0
    }
}

fn clap_descriptor() -> PluginDescriptor {
    descriptor(
        PluginFormat::Clap,
        CROSSOVER_CLAP_ID,
        "SOTF_TEST_NATIVE_RATE_CLAP_PLUGIN",
        2,
        2,
    )
}

fn convolution_descriptor() -> PluginDescriptor {
    descriptor(
        PluginFormat::Vst3,
        CONVOLUTION_VST3_CLASS_ID,
        "SOTF_TEST_NATIVE_RATE_CONVOLUTION_PLUGIN",
        2,
        2,
    )
}

fn descriptor(
    format: PluginFormat,
    id: &str,
    path_env: &str,
    audio_inputs: usize,
    audio_outputs: usize,
) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: match format {
            PluginFormat::Clap => "SOTF: Crossover".into(),
            PluginFormat::Vst3 => "SOTF: Convolution".into(),
            PluginFormat::AudioUnit => unreachable!("test uses CLAP and VST3 only"),
        },
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: PathBuf::from(
            std::env::var_os(path_env)
                .unwrap_or_else(|| panic!("{path_env} must point to a built native plugin bundle")),
        ),
        audio_inputs,
        audio_outputs,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn crossover_state(descriptor: PluginDescriptor, opaque_state: Vec<u8>) -> ExternalPluginState {
    let mut value = serde_json::to_value(ExternalPluginState::new(
        descriptor,
        ExternalPluginSandboxMode::InProcess,
        opaque_state,
    ))
    .expect("serialize Crossover placeholder state");
    value["audio_setup"] = serde_json::json!({
        "type": "crossover",
        "input_layout": "stereo",
        "num_bands": 2,
        "topology": "bands",
        "mode": "lowpass",
        "output_layout": "clap_packed"
    });
    serde_json::from_value(value).expect("decode typed Crossover placeholder state")
}

fn crossover_frequency_id(plugin: &ExternalPlugin) -> ParameterId {
    plugin
        .parameters()
        .into_iter()
        .find(|parameter| parameter.name.eq_ignore_ascii_case("frequency"))
        .unwrap_or_else(|| {
            panic!(
                "loaded Crossover has no Frequency parameter; exposed parameters: {:?}",
                plugin
                    .parameters()
                    .into_iter()
                    .map(|parameter| parameter.name)
                    .collect::<Vec<_>>()
            )
        })
        .id
}

fn sine_input(frames: usize, frequency_hz: f32, sample_rate: u32) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let phase = std::f32::consts::TAU * frequency_hz * frame as f32 / sample_rate as f32;
            let sample = 0.3 * phase.sin();
            [sample, sample]
        })
        .collect()
}

fn render_external<P: Plugin + ?Sized>(
    plugin: &mut P,
    input: &[f32],
    sample_rate: u32,
) -> Vec<f32> {
    let frames = input.len() / plugin.input_channels();
    let mut output = vec![f32::NAN; frames * plugin.output_channels()];
    assert_eq!(
        plugin
            .process(
                input,
                &mut output,
                &ProcessContext::new(sample_rate, frames)
            )
            .expect("render loaded native plugin"),
        frames
    );
    output
}

fn assert_waveforms_match(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context} length");
    assert!(
        actual.iter().all(|sample| sample.is_finite()),
        "{context} finite output"
    );
    let rms_error = (actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| f64::from(*actual - *expected).powi(2))
        .sum::<f64>()
        / actual.len() as f64)
        .sqrt();
    assert!(rms_error <= 2.0e-6, "{context}: RMS error {rms_error}");
}

fn assert_waveforms_differ(actual: &[f32], expected: &[f32], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context} length");
    let rms_difference = (actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| f64::from(*actual - *expected).powi(2))
        .sum::<f64>()
        / actual.len() as f64)
        .sqrt();
    assert!(
        rms_difference > 1.0e-3,
        "{context}: RMS difference {rms_difference} did not exceed 1e-3"
    );
}

#[test]
#[ignore = "requires SOTF_TEST_NATIVE_RATE_CLAP_PLUGIN to point to the captured Crossover CLAP library"]
fn dawhost_retained_clap_crossover_uses_new_rate_and_keeps_unprocessed_scalar_change() {
    let descriptor = clap_descriptor();
    // This immutable bundle exposes the Frequency float through its normalized
    // [0, 1] CLAP parameter range. Select 600 Hz, near the 500 Hz test tone.
    let frequency_parameter = (600.0_f32 - 20.0) / (20_000.0 - 20.0);
    let mut retained = ExternalPlugin::from_placeholder_state(
        &crossover_state(descriptor.clone(), Vec::new()),
        OLD_RATE,
    )
    .expect("load typed native Crossover CLAP at the old rate");
    let frequency_id = crossover_frequency_id(&retained);
    retained
        .set_parameter(
            frequency_id.clone(),
            ParameterValue::Float(frequency_parameter),
        )
        .expect("queue a scalar Frequency change before the first audio callback");
    assert_eq!(
        retained.get_parameter(&frequency_id),
        Some(ParameterValue::Float(frequency_parameter)),
        "the loaded CLAP backend exposes its pending parameter value"
    );

    let mut host = DawHost::new(2, NEW_RATE);
    host.add_plugin(Box::new(SyntheticRateDoubler))
        .expect("add synthetic upstream rate-changing test node");
    host.add_plugin(Box::new(retained))
        .expect("add the loaded Crossover after the upstream node");
    host.build()
        .expect("prepare the retained Crossover at 96 kHz");
    assert_eq!(
        host.output_sample_rate(NEW_RATE).unwrap(),
        f64::from(OLD_RATE)
    );
    drop(
        host.remove_plugin(0)
            .expect("remove the upstream synthetic rate-changing node"),
    );

    let input = sine_input(4096, 500.0, NEW_RATE);
    let mut transitioned_output = vec![f32::NAN; input.len()];
    assert_eq!(
        host.process(&input, &mut transitioned_output)
            .expect("DawHost rebuild initializes the retained Crossover at 48 kHz"),
        input.len() / 2
    );
    assert_eq!(
        host.output_sample_rate(NEW_RATE).unwrap(),
        f64::from(NEW_RATE)
    );

    let mut reference = ExternalPlugin::from_placeholder_state(
        &crossover_state(descriptor.clone(), Vec::new()),
        NEW_RATE,
    )
    .expect("load a typed, correctly configured 48 kHz native reference");
    let reference_frequency_id = crossover_frequency_id(&reference);
    reference
        .set_parameter(
            reference_frequency_id.clone(),
            ParameterValue::Float(frequency_parameter),
        )
        .expect("set the same Frequency on the fresh native reference");
    assert_eq!(
        reference.get_parameter(&reference_frequency_id),
        Some(ParameterValue::Float(frequency_parameter))
    );
    let pending_state_before_reset = reference
        .save_opaque_state()
        .expect("save CLAP state while Frequency is pending");
    assert_eq!(
        native_clap_frequency(&pending_state_before_reset),
        Some(1_000.0),
        "opaque CLAP state contains committed values before the pending event is processed"
    );
    reference.reset();
    let pending_state_after_reset = reference
        .save_opaque_state()
        .expect("save CLAP state after reset but before processing the pending event");
    assert_eq!(
        native_clap_frequency(&pending_state_after_reset),
        Some(1_000.0),
        "reset preserves the old committed state while the scalar event remains queued"
    );
    assert_eq!(
        reference.get_parameter(&reference_frequency_id),
        Some(ParameterValue::Float(frequency_parameter)),
        "reset does not discard the queued scalar override"
    );
    let reference_output = render_external(&mut reference, &input, NEW_RATE);
    let committed_reference_state = reference
        .save_opaque_state()
        .expect("save the reference after processing the pending Frequency event");
    let committed_reference_frequency = native_clap_frequency(&committed_reference_state)
        .expect("processed CLAP state contains its native Frequency");
    assert!(
        (committed_reference_frequency - 600.0).abs() <= 0.01,
        "processing commits native Frequency {committed_reference_frequency} Hz"
    );
    assert_waveforms_match(
        &transitioned_output,
        &reference_output,
        "DawHost retained Crossover at the new sample rate",
    );
    assert!(
        transitioned_output
            .iter()
            .any(|sample| sample.abs() > 1.0e-5)
    );

    let mut stale_rate_control = ExternalPlugin::from_placeholder_state(
        &crossover_state(descriptor.clone(), Vec::new()),
        OLD_RATE,
    )
    .expect("load a deliberately stale typed 96 kHz sensitivity control");
    stale_rate_control
        .set_parameter(
            crossover_frequency_id(&stale_rate_control),
            ParameterValue::Float(frequency_parameter),
        )
        .expect("set the same Frequency on the stale control");
    let stale_output = render_external(&mut stale_rate_control, &input, NEW_RATE);
    assert_waveforms_differ(
        &transitioned_output,
        &stale_output,
        "new-rate Crossover waveform versus stale 96 kHz initialization",
    );

    let committed_state = host
        .get_plugin(0)
        .expect("retained native node remains installed")
        .save_opaque_state()
        .expect("save the post-render native state");
    let committed_frequency = native_clap_frequency(&committed_state)
        .expect("post-render CLAP state contains its scalar Frequency");
    assert!(
        (committed_frequency - 600.0).abs() <= 0.01,
        "post-render native Frequency is {committed_frequency} Hz, expected 600 Hz"
    );

    let mut retained_after_transition = host
        .remove_plugin(0)
        .expect("extract the same Crossover object DawHost transitioned from 96 to 48 kHz");
    assert_eq!(retained_after_transition.input_channels(), 2);
    assert_eq!(retained_after_transition.output_channels(), 2);
    retained_after_transition
        .load_opaque_state(&committed_state)
        .expect("restore committed state on the actual transitioned object");
    retained_after_transition.reset();
    let restored_output = render_external(&mut *retained_after_transition, &input, NEW_RATE);

    let mut committed_state_reference = ExternalPlugin::from_placeholder_state(
        &crossover_state(descriptor, committed_state.clone()),
        NEW_RATE,
    )
    .expect("create a fresh 48 kHz twin from the same committed state");
    committed_state_reference.reset();
    let reset_reference_output = render_external(&mut committed_state_reference, &input, NEW_RATE);
    assert_waveforms_match(
        &restored_output,
        &reset_reference_output,
        "new-rate saved state survives restore and reset",
    );
}

fn native_clap_frequency(state: &[u8]) -> Option<f32> {
    let prefix: [u8; 8] = state.get(..8)?.try_into().ok()?;
    let payload = state.get(8..)?;
    if usize::try_from(u64::from_le_bytes(prefix)).ok()? != payload.len() {
        return None;
    }
    let state: serde_json::Value = serde_json::from_slice(payload).ok()?;
    state["params"]["frequency"]["f32"]
        .as_f64()
        .map(|value| value as f32)
}

struct ImpulseFixture(PathBuf);

impl ImpulseFixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time after UNIX epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "sotf-native-rate-convolution-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("create unique convolution fixture directory");
        let fixture = Self(directory);
        write_pcm16_four_channel_wav(&fixture.ir_path(), &IR_TAPS);
        fixture
    }

    fn ir_path(&self) -> PathBuf {
        self.0.join("true-stereo.wav")
    }
}

impl Drop for ImpulseFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_pcm16_four_channel_wav(path: &Path, taps: &[[i16; 4]]) {
    let data_len = u32::try_from(taps.len() * 8).expect("small WAV fixture");
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&NEW_RATE.to_le_bytes());
    bytes.extend_from_slice(&(NEW_RATE * 8).to_le_bytes());
    bytes.extend_from_slice(&8_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for [ll, lr, rl, rr] in taps {
        for sample in [ll, lr, rl, rr] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
    }
    fs::write(path, bytes).expect("write PCM16 true-stereo IR");
}

fn convolution_state(ir_path: &Path) -> Vec<u8> {
    let resource = serde_json::json!({
        "version": 1,
        "path": ir_path.to_string_lossy(),
    });
    serde_json::to_vec(&serde_json::json!({
        "version": "1.0.0",
        "params": {
            "mix": {"f32": 0.37},
            "gain_db": {"f32": 0.0},
            "use_nupc": {"bool": true},
            "zero_latency_head": {"bool": false},
            "head_taps": {"i32": 128},
            "true_stereo": {"bool": true},
        },
        "fields": {
            IR_RESOURCE_FIELD: serde_json::to_string(&resource).unwrap(),
        },
    }))
    .expect("serialize native Convolution state")
}

fn repeated_input(input: &[f32]) -> Vec<f32> {
    let (frames, remainder) = input.as_chunks::<2>();
    assert!(
        remainder.is_empty(),
        "Convolution input must contain stereo frames"
    );
    frames
        .iter()
        .flat_map(|frame| [frame[0], frame[1], frame[0], frame[1]])
        .collect()
}

fn render_plugin(plugin: &mut dyn Plugin, input: &[f32], sample_rate: u32) -> Vec<f32> {
    let frames = input.len() / plugin.input_channels();
    let mut output = vec![f32::NAN; frames * plugin.output_channels()];
    assert_eq!(
        plugin
            .process(
                input,
                &mut output,
                &ProcessContext::new(sample_rate, frames)
            )
            .expect("render loaded VST3 plugin"),
        frames
    );
    output
}

fn assert_resource_path(state: &[u8], expected_path: &Path) {
    let state: serde_json::Value =
        serde_json::from_slice(state).expect("saved VST3 Convolution state is JSON");
    assert_eq!(
        state["params"]["mix"]["f32"]
            .as_f64()
            .map(|value| value as f32),
        Some(0.37),
        "saved scalar mix control remains unchanged"
    );
    let resource: serde_json::Value = serde_json::from_str(
        state["fields"][IR_RESOURCE_FIELD]
            .as_str()
            .expect("saved state contains its IR resource path"),
    )
    .expect("saved IR resource field is JSON");
    assert_eq!(resource["path"], expected_path.to_string_lossy().as_ref());
}

#[test]
#[ignore = "requires SOTF_TEST_NATIVE_RATE_CONVOLUTION_PLUGIN to point to the captured Convolution VST3 bundle"]
fn dawhost_failed_vst3_rate_preparation_preserves_retained_native_object_history() {
    let fixture = ImpulseFixture::new();
    let descriptor = convolution_descriptor();
    let state = convolution_state(&fixture.ir_path());
    let mut retained = ExternalPlugin::new(&descriptor, OLD_RATE)
        .expect("load captured Convolution VST3 at the old rate");
    retained
        .load_opaque_state(&state)
        .expect("load the existing four-channel IR resource");
    retained.refresh_control_thread_metadata();
    let mut twin = ExternalPlugin::new(&descriptor, OLD_RATE)
        .expect("load synchronized Convolution comparison twin");
    twin.load_opaque_state(&state)
        .expect("load the same IR resource on the comparison twin");
    twin.refresh_control_thread_metadata();

    let mut host = DawHost::new(2, NEW_RATE);
    host.add_plugin(Box::new(SyntheticRateDoubler))
        .expect("add synthetic upstream rate-changing test node");
    host.add_plugin(Box::new(retained))
        .expect("retain Convolution after the upstream node");
    host.build()
        .expect("prepare retained Convolution at 96 kHz");

    let input = sine_input(2048, 431.0, NEW_RATE);
    let old_rate_input = repeated_input(&input);
    let mut host_before_failure = vec![f32::NAN; old_rate_input.len()];
    assert_eq!(
        host.process(&input, &mut host_before_failure)
            .expect("populate retained Convolution history through DawHost"),
        old_rate_input.len() / 2
    );
    let twin_before_failure = render_external(&mut twin, &old_rate_input, OLD_RATE);
    assert_waveforms_match(
        &host_before_failure,
        &twin_before_failure,
        "host and twin are synchronized before failed preparation",
    );
    let (latency_before_failure, tail_before_failure, state_before_failure) = {
        let retained = host.get_plugin(1).expect("retained node remains installed");
        let state = retained
            .save_opaque_state()
            .expect("save retained native state before failure");
        assert_resource_path(&state, &fixture.ir_path());
        (retained.latency_samples(), retained.tail_length(), state)
    };

    fs::remove_file(fixture.ir_path()).expect("remove source IR after both instances loaded it");
    drop(
        host.remove_plugin(0)
            .expect("remove upstream rate-changing node"),
    );
    let new_rate_input = sine_input(128, 0.0, NEW_RATE);
    let mut rejected_output = vec![f32::NAN; new_rate_input.len()];
    let error = host
        .process(&new_rate_input, &mut rejected_output)
        .expect_err("detached 48 kHz VST3 preparation must fail after IR removal");
    assert!(
        error.contains("failed to restore component state"),
        "candidate failure comes from VST3 state/resource restore: {error}"
    );
    eprintln!("expected detached Convolution preparation error: {error}");

    // The graph rebuild failed after the upstream node was removed. Extract the
    // still-installed old-rate native object and compare its history only.
    let mut extracted = host
        .remove_plugin(0)
        .expect("extract the retained native object after failed graph rebuild");
    assert_eq!(extracted.input_channels(), 2);
    assert_eq!(extracted.output_channels(), 2);
    assert_eq!(extracted.latency_samples(), latency_before_failure);
    assert_eq!(extracted.tail_length(), tail_before_failure);
    let state_after_failure = extracted
        .save_opaque_state()
        .expect("native state remains readable after candidate failure");
    assert_resource_path(&state_after_failure, &fixture.ir_path());
    assert_eq!(
        state_after_failure, state_before_failure,
        "failed candidate preparation leaves native state and scalar controls unchanged"
    );
    extracted
        .initialize(f64::from(OLD_RATE))
        .expect("failed transition leaves the old sample rate installed");
    assert_eq!(extracted.latency_samples(), latency_before_failure);
    assert_eq!(extracted.tail_length(), tail_before_failure);

    let continuation = vec![0.0_f32; 512 * 2];
    let actual = render_plugin(extracted.as_mut(), &continuation, OLD_RATE);
    let expected = render_external(&mut twin, &continuation, OLD_RATE);
    assert_eq!(
        actual, expected,
        "native Convolution history after failed 48 kHz candidate preparation"
    );
    assert!(
        actual.iter().any(|sample| sample.abs() > 1.0e-5),
        "continuation includes populated Convolution history beyond its latency"
    );
}
