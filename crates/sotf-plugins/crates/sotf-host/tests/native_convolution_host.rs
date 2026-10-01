#![cfg(feature = "external-plugin-vst3")]

use sotf_host::ExternalPluginSandboxMode;
use sotf_host::external_plugin::{
    ExternalHostingBackend, ExternalPlugin, ExternalPluginState, PluginDescriptor, PluginFormat,
    PluginScanStatus,
};
use sotf_host::parameters::ParameterValue;
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SAMPLE_RATE: u32 = 48_000;
const CONVOLUTION_CLASS_ID: &str = "536F7466436F6E766F6C75746E303031";
const IR_RESOURCE_FIELD: &str = "sotf_convolution_ir_resource";
const MIX: f32 = 0.37;
const REPLACEMENT_MIX: f32 = 0.61;
const IMPULSE: [[i16; 4]; 4] = [
    [14_000, 2_000, -3_000, 10_000],
    [3_000, -2_000, 4_000, 1_000],
    [-1_000, 500, 2_000, -1_500],
    [512, -256, 768, 384],
];

struct ImpulseFixture(PathBuf);

impl ImpulseFixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock after UNIX epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "sotf-aud134-loaded-convolution-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("create unique convolution fixture directory");
        write_pcm16_four_channel_wav(&directory.join("true-stereo.wav"), &IMPULSE);
        Self(directory)
    }

    fn path(&self) -> PathBuf {
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
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * 8).to_le_bytes());
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

fn descriptor() -> PluginDescriptor {
    let path = PathBuf::from(
        std::env::var_os("SOTF_TEST_CONVOLUTION_VST3_PLUGIN")
            .expect("SOTF_TEST_CONVOLUTION_VST3_PLUGIN must point to the built .vst3 bundle"),
    );
    PluginDescriptor {
        id: CONVOLUTION_CLASS_ID.into(),
        name: "SOTF: Convolution".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format: PluginFormat::Vst3,
        path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn plugin_state(path: &Path, mix: f32, true_stereo: bool) -> Vec<u8> {
    let resource = serde_json::json!({
        "version": 1,
        "path": path.to_string_lossy(),
    });
    let state = serde_json::json!({
        "version": "1.0.0",
        "params": {
            "mix": {"f32": mix},
            "gain_db": {"f32": 0.0},
            "use_nupc": {"bool": true},
            "zero_latency_head": {"bool": false},
            "head_taps": {"i32": 128},
            "true_stereo": {"bool": true_stereo},
        },
        "fields": {
            IR_RESOURCE_FIELD: serde_json::to_string(&resource).unwrap(),
        },
    });
    serde_json::to_vec(&state).expect("serialize complete NIH plugin state")
}

fn external_state(descriptor: PluginDescriptor, opaque_state: Vec<u8>) -> ExternalPluginState {
    ExternalPluginState::new(
        descriptor,
        ExternalPluginSandboxMode::InProcess,
        opaque_state,
    )
}

fn signal(frames: usize) -> Vec<[f32; 2]> {
    (0..frames)
        .map(|frame| {
            let t = frame as f32;
            [0.13 * (0.17 * t).sin(), 0.09 * (0.11 * t).cos()]
        })
        .collect()
}

fn interleave(frames: &[[f32; 2]]) -> Vec<f32> {
    frames
        .iter()
        .flat_map(|frame| frame.iter().copied())
        .collect()
}

fn try_render(plugin: &mut ExternalPlugin, input: &[[f32; 2]]) -> Result<Vec<f32>, String> {
    let input = interleave(input);
    let frames = input.len() / 2;
    let mut output = vec![f32::NAN; input.len()];
    let context = ProcessContext::new(SAMPLE_RATE, frames);
    assert_eq!(plugin.process(&input, &mut output, &context)?, frames);
    Ok(output)
}

fn render(plugin: &mut ExternalPlugin, input: &[[f32; 2]]) -> Vec<f32> {
    try_render(plugin, input).expect("render native VST3 audio")
}

fn expected_output(input: &[[f32; 2]], latency: usize, total_frames: usize, mix: f32) -> Vec<f64> {
    let mut wet = vec![[0.0_f64; 2]; input.len() + IMPULSE.len() - 1];
    for (input_frame, [left, right]) in input.iter().copied().enumerate() {
        for (tap, [ll, lr, rl, rr]) in IMPULSE.iter().copied().enumerate() {
            let output_frame = input_frame + tap;
            wet[output_frame][0] += f64::from(left) * f64::from(ll) / 32_768.0;
            wet[output_frame][0] += f64::from(right) * f64::from(rl) / 32_768.0;
            wet[output_frame][1] += f64::from(left) * f64::from(lr) / 32_768.0;
            wet[output_frame][1] += f64::from(right) * f64::from(rr) / 32_768.0;
        }
    }

    let mut expected = vec![0.0_f64; total_frames * 2];
    for frame in 0..total_frames {
        if frame < latency {
            continue;
        }
        let source_frame = frame - latency;
        let dry = input.get(source_frame).copied().unwrap_or([0.0; 2]);
        let convolved = wet.get(source_frame).copied().unwrap_or([0.0; 2]);
        for channel in 0..2 {
            expected[frame * 2 + channel] = (1.0 - f64::from(mix)) * f64::from(dry[channel])
                + f64::from(mix) * convolved[channel];
        }
    }
    expected
}

fn assert_matches(actual: &[f32], expected: &[f64], context: &str) {
    assert_eq!(actual.len(), expected.len(), "{context} sample count");
    assert!(
        actual.iter().all(|sample| sample.is_finite()),
        "{context}: actual output contains a non-finite sample"
    );
    assert!(
        expected.iter().all(|sample| sample.is_finite()),
        "{context}: direct reference contains a non-finite sample"
    );
    let mut max_error = 0.0_f64;
    let mut max_index = 0;
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        let error = (f64::from(*actual) - expected).abs();
        if error > max_error {
            max_error = error;
            max_index = index;
        }
    }
    let rms_error = (actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| (f64::from(*actual) - expected).powi(2))
        .sum::<f64>()
        / actual.len() as f64)
        .sqrt();
    assert!(
        max_error <= 1.0e-5 && rms_error <= 1.0e-6,
        "{context}: peak error {max_error} at interleaved sample {max_index}, RMS error {rms_error}"
    );
}

fn assert_differs(actual: &[f32], reference: &[f64], minimum_rms: f64, context: &str) {
    assert_eq!(actual.len(), reference.len(), "{context} sample count");
    assert!(actual.iter().all(|sample| sample.is_finite()));
    let rms_difference = (actual
        .iter()
        .zip(reference)
        .map(|(actual, reference)| (f64::from(*actual) - reference).powi(2))
        .sum::<f64>()
        / actual.len() as f64)
        .sqrt();
    assert!(
        rms_difference > minimum_rms,
        "{context}: RMS difference {rms_difference} did not exceed {minimum_rms}"
    );
}

fn expected_dry_output(
    input: &[[f32; 2]],
    latency: usize,
    total_frames: usize,
    mix: f32,
) -> Vec<f64> {
    let mut expected = vec![0.0; total_frames * 2];
    for frame in latency..total_frames {
        if let Some(source) = input.get(frame - latency) {
            for channel in 0..2 {
                expected[frame * 2 + channel] = (1.0 - f64::from(mix)) * f64::from(source[channel]);
            }
        }
    }
    expected
}

fn assert_saved_resource(plugin: &ExternalPlugin, expected_path: &Path, expected_mix: f32) {
    let saved = plugin.save_opaque_state().expect("save VST3 state");
    let saved: serde_json::Value = serde_json::from_slice(&saved).expect("saved NIH state JSON");
    assert_eq!(
        saved["params"]["mix"]["f32"]
            .as_f64()
            .map(|value| value as f32)
            .map(f32::to_bits),
        Some(expected_mix.to_bits())
    );
    assert_eq!(saved["params"]["true_stereo"]["bool"].as_bool(), Some(true));
    let resource: serde_json::Value = serde_json::from_str(
        saved["fields"][IR_RESOURCE_FIELD]
            .as_str()
            .expect("serialized IR resource field"),
    )
    .expect("serialized IR resource JSON");
    assert_eq!(resource["path"], expected_path.to_string_lossy().as_ref());
}

#[test]
#[ignore = "requires SOTF_TEST_CONVOLUTION_VST3_PLUGIN to point to the built Convolution VST3 bundle"]
fn native_vst3_convolution_bundle_loads_state_audio_and_preserves_old_graph_on_missing_ir() {
    let fixture = ImpulseFixture::new();
    let descriptor = descriptor();
    let valid_bytes = plugin_state(&fixture.path(), MIX, true);
    let valid_state = external_state(descriptor.clone(), valid_bytes.clone());

    let mut rendered = ExternalPlugin::from_placeholder_state(&valid_state, SAMPLE_RATE)
        .expect("load the exported Convolution VST3 bundle and restore its IR state");
    assert_eq!(rendered.hosting_backend(), ExternalHostingBackend::Vst3);
    assert_eq!(rendered.input_channels(), 2);
    assert_eq!(rendered.output_channels(), 2);
    rendered.refresh_control_thread_metadata();
    let latency = rendered.latency_samples();
    assert_eq!(
        latency, 1_024,
        "fixture requests the normal convolution latency"
    );
    let expected_tail = latency + IMPULSE.len() - 1;
    assert_eq!(
        rendered.tail_length(),
        TailLength::Finite(expected_tail as u64)
    );

    let input = signal(193);
    let mut full_input = input.clone();
    full_input.resize(input.len() + expected_tail, [0.0; 2]);
    let output = render(&mut rendered, &full_input);
    let expected = expected_output(&input, latency, full_input.len(), MIX);
    assert_matches(&output, &expected, "restored full true-stereo response");
    assert_saved_resource(&rendered, &fixture.path(), MIX);

    // Two identically populated instances distinguish preservation of the live
    // processor history from a cold reconstruction after a rejected restore.
    let mut live = ExternalPlugin::from_placeholder_state(&valid_state, SAMPLE_RATE)
        .expect("load populated subject");
    let mut control = ExternalPlugin::from_placeholder_state(&valid_state, SAMPLE_RATE)
        .expect("load populated control twin");
    let replacement_bytes = plugin_state(&fixture.path(), REPLACEMENT_MIX, true);
    live.load_opaque_state(&replacement_bytes)
        .expect("successfully restore a valid replacement state on the live VST3");
    control
        .load_opaque_state(&replacement_bytes)
        .expect("successfully restore the same replacement state on the control VST3");
    live.refresh_control_thread_metadata();
    control.refresh_control_thread_metadata();
    assert_eq!(live.latency_samples(), latency);
    assert_eq!(live.tail_length(), TailLength::Finite(expected_tail as u64));
    assert_saved_resource(&live, &fixture.path(), REPLACEMENT_MIX);

    let replacement_input = signal(193);
    let mut replacement_full_input = replacement_input.clone();
    replacement_full_input.resize(replacement_input.len() + expected_tail, [0.0; 2]);
    let replacement_output = render(&mut live, &replacement_full_input);
    let replacement_control_output = render(&mut control, &replacement_full_input);
    let replacement_expected = expected_output(
        &replacement_input,
        latency,
        replacement_full_input.len(),
        REPLACEMENT_MIX,
    );
    assert_matches(
        &replacement_output,
        &replacement_expected,
        "successfully restored in-place VST3 state",
    );
    assert_matches(
        &replacement_control_output,
        &replacement_expected,
        "successfully restored control VST3 state",
    );

    let prefix = signal(latency + 173);
    assert_eq!(render(&mut live, &prefix), render(&mut control, &prefix));

    let missing_path = fixture.0.join("missing-ir.wav");
    let invalid_bytes = plugin_state(&missing_path, 0.91, false);
    fs::remove_file(fixture.path()).expect("remove source IR after both processors loaded it");
    let restore_error = live
        .load_opaque_state(&invalid_bytes)
        .expect_err("restore must reject a missing external IR");
    let tail_after_failed_restore = live.tail_length();
    assert_saved_resource(&live, &fixture.0.join("true-stereo.wav"), REPLACEMENT_MIX);

    let continuation = signal(97);
    let continuation_control = render(&mut control, &continuation);
    let continuation_live = try_render(&mut live, &continuation).unwrap_or_else(|error| {
        panic!(
            "failed missing-IR restore stopped processing; tail metadata is {tail_after_failed_restore:?}: {error}"
        )
    });
    assert_eq!(
        continuation_live, continuation_control,
        "failed missing-IR restore must retain the populated convolution history"
    );
    assert!(
        continuation_live.iter().any(|sample| sample.abs() > 1.0e-5),
        "populated continuation must contain nonzero audio beyond plugin latency"
    );
    assert_eq!(
        tail_after_failed_restore,
        TailLength::Finite(expected_tail as u64),
        "failed missing-IR restore must preserve tail metadata after successful continuation; restore error: {restore_error}"
    );

    let tail_before_empty_restore = live.tail_length();
    let empty_restore_error = live
        .load_opaque_state(&[])
        .expect_err("the native VST3 empty state stream must be rejected");
    assert_saved_resource(&live, &fixture.0.join("true-stereo.wav"), REPLACEMENT_MIX);
    assert_eq!(
        live.tail_length(),
        tail_before_empty_restore,
        "rejected empty state must preserve the populated plugin tail; restore error: {empty_restore_error}"
    );

    let empty_state_continuation = signal(97);
    let empty_state_control = render(&mut control, &empty_state_continuation);
    let empty_state_live = try_render(&mut live, &empty_state_continuation).unwrap_or_else(|error| {
        panic!(
            "rejected empty state stopped processing with original IR deleted; tail is {:?}: {error}",
            live.tail_length()
        )
    });
    assert_eq!(
        empty_state_live, empty_state_control,
        "rejected empty state must preserve populated audio history"
    );
    assert!(
        empty_state_live.iter().any(|sample| sample.abs() > 1.0e-5),
        "empty-state continuation must contain nonzero audio after the plugin latency"
    );

    const RESET_MIX: f32 = 0.23;
    let mix_id = live
        .parameters()
        .into_iter()
        .find(|parameter| parameter.name.eq_ignore_ascii_case("Mix"))
        .expect("VST3 exposes the Mix control")
        .id;
    live.set_parameter(mix_id, ParameterValue::Float(RESET_MIX))
        .expect("set a new realtime mix before VST3 reactivation");
    let mix_event = signal(64);
    let mix_event_output = render(&mut live, &mix_event);
    assert!(mix_event_output.iter().all(|sample| sample.is_finite()));
    assert_saved_resource(&live, &fixture.0.join("true-stereo.wav"), RESET_MIX);

    live.reset_checked().unwrap();
    assert_saved_resource(&live, &fixture.0.join("true-stereo.wav"), RESET_MIX);
    let mut after_failure = signal(193);
    after_failure.resize(after_failure.len() + expected_tail, [0.0; 2]);
    let output_after_failure = render(&mut live, &after_failure);
    let expected_after_failure = expected_output(
        &after_failure[..193],
        latency,
        after_failure.len(),
        RESET_MIX,
    );
    assert_matches(
        &output_after_failure,
        &expected_after_failure,
        "old IR remains prepared after failed missing-file restore",
    );
    let dry_reference = expected_dry_output(
        &after_failure[..193],
        latency,
        after_failure.len(),
        RESET_MIX,
    );
    assert_differs(
        &output_after_failure,
        &dry_reference,
        1.0e-3,
        "retained IR response must differ from dry-only sensitivity control",
    );
}
