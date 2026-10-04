#![cfg(all(feature = "external-plugin-clap", feature = "external-plugin-vst3"))]

//! Loaded-artifact baseline for the current Crossover export. These tests deliberately
//! record the pre-route stereo fallback; the multichannel matrix belongs in the follow-up.
use serde_json::Value;
use sotf_host::external_plugin::{
    ExternalPlugin, PluginDescriptor, PluginFormat, PluginScanStatus,
};
use sotf_host::plugin::{Plugin, ProcessContext};
use std::path::PathBuf;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 257;
const GUARD: usize = 8;
const CANARY: f32 = f32::from_bits(0x4f12_3456);
const BLOCKS: [usize; 4] = [1, 63, 127, 66];
const CLAP_ID: &str = "org.spinorama.sotf.crossover";
const VST3_CLASS_ID: &str = "536F746643726F73736F766572303031";

#[test]
#[ignore = "requires SOTF_TEST_CROSSOVER_CLAP_PLUGIN to point to the captured .clap library"]
fn captured_clap_crossover_baseline_loads_as_stereo() {
    verify_stereo_baseline(
        PluginFormat::Clap,
        CLAP_ID,
        "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
    );
}

#[test]
#[ignore = "requires SOTF_TEST_CROSSOVER_VST3_PLUGIN to point to the captured .vst3 bundle"]
fn captured_vst3_crossover_baseline_loads_as_stereo() {
    verify_stereo_baseline(
        PluginFormat::Vst3,
        VST3_CLASS_ID,
        "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
    );
}

#[test]
#[ignore = "requires captured Crossover CLAP and VST3 artifacts in the two SOTF_TEST_* variables"]
fn captured_crossover_baseline_records_legacy_modes_and_both_width_refusal() {
    for (format, plugin_id, library_env) in [
        (
            PluginFormat::Clap,
            CLAP_ID,
            "SOTF_TEST_CROSSOVER_CLAP_PLUGIN",
        ),
        (
            PluginFormat::Vst3,
            VST3_CLASS_ID,
            "SOTF_TEST_CROSSOVER_VST3_PLUGIN",
        ),
    ] {
        verify_stateful_stereo_baseline(format, plugin_id, library_env);
    }
}

fn verify_stereo_baseline(format: PluginFormat, plugin_id: &str, library_env: &str) {
    let descriptor = descriptor(format, plugin_id, library_env);
    let mut plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("load captured {format:?} Crossover baseline: {error}"));
    assert_eq!(plugin.input_channels(), 2);
    assert_eq!(plugin.output_channels(), 2);
    assert_eq!(plugin.discovery_descriptor(), &descriptor);

    let output = render_partitioned(&mut plugin, &distinct_stereo_input(), 2);
    assert!(output.iter().any(|sample| sample.abs() > 1.0e-7));
}

fn verify_stateful_stereo_baseline(format: PluginFormat, plugin_id: &str, library_env: &str) {
    let descriptor = descriptor(format, plugin_id, library_env);
    let input = distinct_stereo_input();
    let seed = ExternalPlugin::new(&descriptor, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("load captured {format:?} Crossover seed: {error}"));
    let current_state = seed
        .save_opaque_state()
        .unwrap_or_else(|error| panic!("save {format:?} Crossover baseline: {error}"));
    let parsed = parse_native_state(&current_state, format);
    let saved_ids = parsed
        .get("params")
        .and_then(Value::as_object)
        .expect("native Crossover state contains a parameter map")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    eprintln!("{format:?} captured Crossover native state IDs: {saved_ids:?}");
    for id in ["frequency", "family", "mode", "band_count", "topology"] {
        assert!(
            saved_ids.iter().any(|saved_id| saved_id == id),
            "missing saved ID {id}"
        );
    }
    assert_eq!(native_choice(&current_state, format, "mode"), Some(0));
    assert_eq!(native_choice(&current_state, format, "family"), Some(0));

    let mut default_plugin = ExternalPlugin::new(&descriptor, SAMPLE_RATE).unwrap();
    let default_audio = render_partitioned(&mut default_plugin, &input, 2);
    assert_waveform_matches(
        &default_audio,
        &render_public_crossover(&input, "lowpass", 1000.0, 2),
        "captured default LR24 lowpass",
    );
    capture_baseline(
        format,
        "default-lowpass",
        &current_state,
        &input,
        &default_audio,
    );

    let mut high_state = current_state.clone();
    set_native_choice(&mut high_state, format, "mode", 1);
    let mut highpass = ExternalPlugin::new(&descriptor, SAMPLE_RATE).unwrap();
    highpass
        .load_opaque_state(&high_state)
        .unwrap_or_else(|error| panic!("load {format:?} highpass baseline state: {error}"));
    assert_eq!(highpass.input_channels(), 2);
    assert_eq!(highpass.output_channels(), 2);
    assert_eq!(
        native_choice(&highpass.save_opaque_state().unwrap(), format, "mode"),
        Some(1)
    );
    let high_audio = render_partitioned(&mut highpass, &input, 2);
    assert_waveform_matches(
        &high_audio,
        &render_public_crossover(&input, "highpass", 1000.0, 2),
        "captured highpass full vector",
    );
    capture_baseline(format, "highpass", &high_state, &input, &high_audio);

    // AUD142's pre-route wrapper may accept the structural Both state, but it still
    // discovers/returns only the generic stereo output. This records the current
    // limitation; follow-up routing must negotiate 4/6/8 channels or refuse transactionally.
    let mut both_state = current_state.clone();
    set_native_choice(&mut both_state, format, "mode", 2);
    let mut both = ExternalPlugin::new(&descriptor, SAMPLE_RATE).unwrap();
    let both_outcome = match both.load_opaque_state(&both_state) {
        Ok(()) => {
            let message = format!(
                "{format:?} Both baseline: native state accepted, host reports {} output channels; two-band Both requires 4",
                both.output_channels()
            );
            eprintln!("{message}");
            message
        }
        Err(error) => {
            let message = format!("{format:?} Both baseline rejected before routing: {error}");
            eprintln!("{message}");
            message
        }
    };
    capture_both_refusal(format, &both_state, &both_outcome);
    assert_ne!(
        both.output_channels(),
        4,
        "captured baseline must expose the missing two-band Both route"
    );
    assert_eq!(
        render_public_crossover(&input, "both", 1000.0, 4).len(),
        FRAMES * 4,
        "independent public DSP composition confirms the required Both width"
    );

    // This is a synthetic missing-field compatibility case: retain Frequency from the
    // captured state and remove the later fields. It is not an archived historical preset.
    let mut frequency_only_state = current_state;
    edit_native_state(&mut frequency_only_state, format, |state| {
        let params = state
            .get_mut("params")
            .and_then(Value::as_object_mut)
            .expect("Crossover legacy state parameter map");
        params.retain(|id, _| id == "frequency");
        params.insert("frequency".into(), serde_json::json!({"f32": 1800.0}));
        if let Some(fields) = state.get_mut("fields").and_then(Value::as_object_mut) {
            fields.clear();
        }
    });
    let mut legacy = ExternalPlugin::new(&descriptor, SAMPLE_RATE).unwrap();
    legacy
        .load_opaque_state(&frequency_only_state)
        .unwrap_or_else(|error| {
            panic!("restore frequency-only {format:?} Crossover compatibility state: {error}")
        });
    assert_eq!(
        native_choice(&legacy.save_opaque_state().unwrap(), format, "mode"),
        Some(0)
    );
    let legacy_audio = render_partitioned(&mut legacy, &input, 2);
    assert_waveform_matches(
        &legacy_audio,
        &render_public_crossover(&input, "lowpass", 1800.0, 2),
        "frequency-only missing-field state defaults to lowpass",
    );
    let restored_frequency_only = legacy.save_opaque_state().unwrap();
    capture_baseline(
        format,
        "frequency-only-restored-lowpass",
        &restored_frequency_only,
        &input,
        &legacy_audio,
    );
    capture_baseline(
        format,
        "frequency-only-input-state",
        &frequency_only_state,
        &input,
        &[],
    );
}

fn descriptor(format: PluginFormat, id: &str, library_env: &str) -> PluginDescriptor {
    PluginDescriptor {
        id: id.into(),
        name: "SOTF: Crossover".into(),
        vendor: "SOTF".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        format,
        path: PathBuf::from(
            std::env::var_os(library_env).unwrap_or_else(|| panic!("{library_env} must be set")),
        ),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["audio-effect".into()],
        scan_status: PluginScanStatus::Loadable,
    }
}

fn parse_native_state(opaque: &[u8], format: PluginFormat) -> Value {
    let payload = if format == PluginFormat::Clap {
        let prefix: [u8; 8] = opaque
            .get(..8)
            .expect("CLAP native state has a length prefix")
            .try_into()
            .unwrap();
        let length = usize::try_from(u64::from_le_bytes(prefix)).unwrap();
        let payload = opaque.get(8..).unwrap();
        assert_eq!(payload.len(), length);
        payload
    } else {
        opaque
    };
    serde_json::from_slice(payload).expect("native state is NIH JSON")
}

fn edit_native_state(opaque: &mut Vec<u8>, format: PluginFormat, edit: impl FnOnce(&mut Value)) {
    let mut state = parse_native_state(opaque, format);
    edit(&mut state);
    let payload = serde_json::to_vec(&state).unwrap();
    opaque.clear();
    if format == PluginFormat::Clap {
        opaque.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    }
    opaque.extend_from_slice(&payload);
}

fn native_choice(opaque: &[u8], format: PluginFormat, id: &str) -> Option<i64> {
    parse_native_state(opaque, format)
        .get("params")?
        .get(id)?
        .get("i32")?
        .as_i64()
}

fn set_native_choice(opaque: &mut Vec<u8>, format: PluginFormat, id: &str, choice: i32) {
    edit_native_state(opaque, format, |state| {
        state["params"][id] = serde_json::json!({"i32": choice});
    });
}

fn render_public_crossover(
    input: &[f32],
    output: &str,
    frequency: f64,
    channels: usize,
) -> Vec<f32> {
    let config = serde_json::json!({
        "type": "LR24",
        "frequency": frequency,
        "output": output,
    });
    let mut reference = sotf_plugins::create_plugin("Crossover", &config, 2, SAMPLE_RATE)
        .unwrap_or_else(|error| panic!("construct public {output} Crossover: {error}"));
    reference
        .initialize(f64::from(SAMPLE_RATE))
        .unwrap_or_else(|error| panic!("initialize public {output} Crossover: {error}"));
    render_partitioned(reference.as_mut(), input, channels)
}

fn render_partitioned(plugin: &mut dyn Plugin, input: &[f32], channels: usize) -> Vec<f32> {
    assert_eq!(input.len(), FRAMES * 2);
    let output_len = FRAMES * channels;
    let output_start = GUARD;
    let output_end = output_start + output_len;
    let mut guarded = vec![CANARY; output_len + GUARD * 2];
    guarded[output_start..output_end].fill(f32::NAN);
    let mut frame_offset = 0;
    for block_frames in BLOCKS {
        let input_start = frame_offset * 2;
        let input_end = input_start + block_frames * 2;
        let output_block_start = output_start + frame_offset * channels;
        let output_block_end = output_block_start + block_frames * channels;
        assert_eq!(
            plugin
                .process(
                    &input[input_start..input_end],
                    &mut guarded[output_block_start..output_block_end],
                    &ProcessContext::new(SAMPLE_RATE, block_frames),
                )
                .unwrap_or_else(|error| panic!("process Crossover reference/host: {error}")),
            block_frames
        );
        frame_offset += block_frames;
    }
    assert_eq!(frame_offset, FRAMES);
    assert!(
        guarded[..output_start]
            .iter()
            .chain(&guarded[output_end..])
            .all(|sample| sample.to_bits() == CANARY.to_bits())
    );
    let output = guarded[output_start..output_end].to_vec();
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn assert_waveform_matches(actual: &[f32], expected: &[f32], route: &str) {
    assert_eq!(actual.len(), expected.len(), "{route} length");
    let max_error = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert!(max_error <= 2.0e-5, "{route} max sample error {max_error}");
}

fn capture_baseline(
    format: PluginFormat,
    label: &str,
    opaque_state: &[u8],
    input: &[f32],
    output: &[f32],
) {
    let Some(root) = std::env::var_os("SOTF_CROSSOVER_CAPTURE_DIR") else {
        return;
    };
    let format_name = match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => unreachable!("the capture fixture only covers CLAP/VST3"),
    };
    let directory = PathBuf::from(root).join(format_name);
    std::fs::create_dir_all(&directory).expect("create Crossover baseline capture directory");
    std::fs::write(directory.join(format!("{label}.state.bin")), opaque_state)
        .expect("write exact loaded native state bytes");
    if !output.is_empty() {
        let bytes = output
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        std::fs::write(directory.join(format!("{label}.output.f32le")), bytes)
            .expect("write exact full-vector loaded output");
    }
    let input_bytes = input
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    std::fs::write(directory.join("input.f32le"), input_bytes)
        .expect("write deterministic full-vector input");
    std::fs::write(
        directory.join("capture.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "format": format_name,
            "sample_rate": SAMPLE_RATE,
            "frames": FRAMES,
            "input_channels": 2,
            "partition_frames": BLOCKS,
            "f32_encoding": "little-endian",
            "fixture_scope": "immutable exported baseline; frequency-only case is synthesized by removing later fields",
        }))
        .expect("serialize capture description"),
    )
    .expect("write capture description");
}

fn capture_both_refusal(format: PluginFormat, opaque_state: &[u8], outcome: &str) {
    let Some(root) = std::env::var_os("SOTF_CROSSOVER_CAPTURE_DIR") else {
        return;
    };
    let format_name = match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => unreachable!("the capture fixture only covers CLAP/VST3"),
    };
    let directory = PathBuf::from(root).join(format_name);
    std::fs::create_dir_all(&directory).expect("create Crossover baseline capture directory");
    std::fs::write(
        directory.join("both-two-band-request.state.bin"),
        opaque_state,
    )
    .expect("write Both state that reproduces the missing baseline route");
    std::fs::write(directory.join("both-baseline-result.txt"), outcome)
        .expect("write exact Both load result");
}

fn distinct_stereo_input() -> Vec<f32> {
    let mut input = vec![0.0; FRAMES * 2];
    for frame in 0..FRAMES {
        let phase = frame as f32 * 0.13;
        input[frame * 2] = phase.sin() * 0.4 + (phase * 0.07).cos() * 0.1;
        input[frame * 2 + 1] = (phase * 0.83).sin() * 0.3 + (phase * 0.11).cos() * 0.2;
    }
    input
}
