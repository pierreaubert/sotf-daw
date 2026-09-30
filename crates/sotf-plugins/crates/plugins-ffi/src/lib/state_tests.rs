//! Transactional restoration through both C state entry points.

use crate::*;
use sotf_host::parameters::{ParameterId, ParameterValue};
use std::ffi::{CStr, CString};

struct Handle(*mut PluginHandle);

impl Handle {
    fn new(kind: &str, config: &str, inputs: usize, outputs: usize) -> Self {
        let kind = CString::new(kind).unwrap();
        let config = CString::new(config).unwrap();
        let handle = plugin_create(kind.as_ptr(), config.as_ptr(), 48_000, inputs, outputs);
        assert!(
            !handle.is_null(),
            "{}: {}",
            kind.to_str().unwrap(),
            last_error()
        );
        Self(handle)
    }

    fn inner(&self) -> &PluginHandle {
        // SAFETY: This guard owns a live handle, accessed only on this thread.
        unsafe { &*self.0 }
    }

    fn set(&mut self, name: &str, value: ParameterValue) {
        // SAFETY: Exclusive access to this guard excludes processing.
        unsafe { &mut *self.0 }
            .plugin
            .set_parameter(ParameterId::from(name), value)
            .unwrap();
    }

    fn state(&self) -> Vec<u8> {
        let mut len = 0;
        let state = plugin_save_state(self.0, &mut len);
        assert!(!state.is_null());
        // SAFETY: The FFI owns exactly len initialized bytes until freed below.
        let saved = unsafe { std::slice::from_raw_parts(state, len) }.to_vec();
        plugin_free_state(state, len);
        saved
    }

    fn load(&mut self, state: &[u8], document: bool) -> i32 {
        if document {
            let plugin_type = self.inner().plugin_type.clone();
            let bytes = serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "ut_type": "org.spinorama.sotf.plugin-preset",
                "plugin_type": plugin_type,
                "state": state,
            }))
            .unwrap();
            plugin_import_preset_json(self.0, bytes.as_ptr(), bytes.len())
        } else {
            plugin_load_state(self.0, state.as_ptr(), state.len())
        }
    }

    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let frames = input.len() / self.inner().input_channels;
        let mut output = vec![f32::NAN; frames * self.inner().output_channels];
        assert_eq!(
            plugin_process(self.0, input.as_ptr(), output.as_mut_ptr(), frames),
            0
        );
        assert!(output.iter().all(|value| value.is_finite()));
        output
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn last_error() -> String {
    let error = plugin_get_last_error();
    if error.is_null() {
        return String::new();
    }
    // SAFETY: The current thread owns this valid diagnostic until its next call.
    unsafe { CStr::from_ptr(error) }
        .to_string_lossy()
        .into_owned()
}

fn write_pcm16_ir(channels: &[&[i16]]) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    assert!(!channels.is_empty());
    let frames = channels[0].len();
    assert!(channels.iter().all(|channel| channel.len() == frames));
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "sotf-ffi-true-stereo-{}-{}.wav",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let channel_count = channels.len() as u16;
    let data_size = (frames * channels.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_size as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&channel_count.to_le_bytes());
    bytes.extend_from_slice(&48_000_u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000_u32 * u32::from(channel_count) * 2).to_le_bytes());
    bytes.extend_from_slice(&(channel_count * 2).to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_size.to_le_bytes());
    for frame in 0..frames {
        for channel in channels {
            bytes.extend_from_slice(&channel[frame].to_le_bytes());
        }
    }
    std::fs::write(&path, bytes).unwrap();
    path
}

fn write_true_stereo_ir() -> std::path::PathBuf {
    write_pcm16_ir(&[
        &[8192_i16, 0, 0],
        &[-4096_i16, 0, 0],
        &[6144_i16, 0, 0],
        &[8192_i16, 2048, 0],
    ])
}

fn write_stereo_ir_with_tail() -> std::path::PathBuf {
    let mut left = vec![0_i16; 2_048];
    let mut right = vec![0_i16; 2_048];
    left[0] = 8_192;
    left[1_500] = 4_096;
    right[0] = -4_096;
    right[1_700] = 2_048;
    write_pcm16_ir(&[&left, &right])
}

#[test]
fn failed_presets_preserve_config_storage_parameters_and_processing_history() {
    for document in [false, true] {
        for invalid in [
            br#"{"attack":10.0,"threshold":"invalid"}"#.as_slice(),
            br#"{"attack":10.0,"threshold":null}"#.as_slice(),
        ] {
            let config = r#"{"attack_ms":2.0,"release_ms":200.0,"threshold_db":-25.0}"#;
            let mut handle = Handle::new("Gate", config, 2, 2);
            let mut reference = Handle::new("Gate", config, 2, 2);
            let warmup = vec![0.3; 1024];
            assert_eq!(handle.process(&warmup), reference.process(&warmup));
            let saved = handle.state();
            let instance = std::ptr::from_ref(&*handle.inner().plugin).cast::<()>();
            let metadata = plugin_get_parameter_info(handle.0, 0);
            assert_eq!(
                handle.load(invalid, document),
                PluginError::InvalidConfig as i32
            );
            assert_eq!(handle.state(), saved);
            assert_eq!(handle.inner().config_json, config);
            assert_eq!(
                std::ptr::from_ref(&*handle.inner().plugin).cast::<()>(),
                instance
            );
            assert_eq!(plugin_get_parameter_info(handle.0, 0), metadata);
            // The next block must continue the old envelope and detector state.
            assert_eq!(
                handle.process(&vec![0.002; 2048]),
                reference.process(&vec![0.002; 2048])
            );
        }
    }
}

#[test]
fn partial_restore_preserves_automated_values_and_metadata_storage() {
    for document in [false, true] {
        let mut handle = Handle::new("Gate", "{}", 2, 2);
        handle.set("release", ParameterValue::Float(123.0));
        let metadata = plugin_get_parameter_info(handle.0, 0);
        let state = br#"{"attack":10.0,"threshold":-20.0,"future_parameter":42}"#;
        assert_eq!(handle.load(state, document), 0, "{}", last_error());
        let saved: serde_json::Value = serde_json::from_slice(&handle.state()).unwrap();
        assert_eq!(saved["attack"], 10.0);
        assert_eq!(saved["threshold"], -20.0);
        assert_eq!(saved["release"], 123.0);
        assert_eq!(plugin_get_parameter_info(handle.0, 0), metadata);
        assert_eq!(handle.inner().config_json, "{}");
        assert_eq!(
            (
                handle.inner().input_channels,
                handle.inner().output_channels
            ),
            (2, 2)
        );
    }
}

#[test]
fn restore_preserves_constructor_only_matrix_routing_and_asymmetric_layout() {
    let config = r#"{"input_channels":2,"output_channels":3,"matrix":[1.0,0.0,0.0,1.0,0.25,-0.5]}"#;
    for document in [false, true] {
        let mut handle = Handle::new("Matrix", config, 2, 3);
        let mut reference = Handle::new("Matrix", config, 2, 3);
        assert_eq!(handle.load(b"{}", document), 0, "{}", last_error());
        assert_eq!(handle.inner().config_json, config);
        let input = [0.2, 0.4, -0.7, 0.1];
        assert_eq!(handle.process(&input), reference.process(&input));
        assert_eq!(handle.process(&[0.2, 0.4]), vec![0.2, 0.4, -0.15]);
    }
}

#[test]
fn native_default_states_roundtrip_through_transactional_restore() {
    let mut failures = Vec::new();
    for kind in plugins_bridge::factory::available_plugin_types() {
        let (inputs, outputs) = match *kind {
            "MonoToStereo" => (1, 2),
            "AmbisonicsDecoder" => (4, 6),
            "Upmixer" | "AAE" => (2, 6),
            "BandSplit" => (2, 4),
            "BandMerge" => (4, 2),
            "AEC" | "Beamformer" => (2, 1),
            _ => (2, 2),
        };
        let c_kind = CString::new(*kind).unwrap();
        let pointer = plugin_create(c_kind.as_ptr(), c"{}".as_ptr(), 48_000, inputs, outputs);
        if pointer.is_null() {
            failures.push(format!("{kind}: creation failed: {}", last_error()));
            continue;
        }
        let mut handle = Handle(pointer);
        let saved = handle.state();
        let status = handle.load(&saved, false);
        if status != 0 {
            failures.push(format!("{kind}: {}", last_error()));
        } else if handle.state() != saved {
            failures.push(format!("{kind}: restored state differs from saved state"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn restored_saturation_selects_oversampling_before_initialization() {
    for document in [false, true] {
        let config =
            r#"{"oversampling":"2x","drive":3.0,"mix":0.7,"_sotf_max_callback_frames":256}"#;
        let mut handle = Handle::new("Saturation", config, 2, 2);
        for choice in ["4x", "Off", "2x"] {
            let state = serde_json::to_vec(&serde_json::json!({"oversampling":choice})).unwrap();
            assert_eq!(handle.load(&state, document), 0, "{}", last_error());
            let expected_config = serde_json::json!({"oversampling":choice,"drive":3.0,"mix":0.7,"_sotf_max_callback_frames":256}).to_string();
            let mut reference = Handle::new("Saturation", &expected_config, 2, 2);
            assert_eq!(
                handle.inner().plugin.latency_samples(),
                reference.inner().plugin.latency_samples()
            );
            assert_eq!(handle.inner().config_json, config);
            for block in 0..8 {
                let input: Vec<f32> = (0..512)
                    .map(|i| ((i + block * 512) as f32 * 0.31).sin() * 0.2)
                    .collect();
                assert_eq!(
                    handle.process(&input),
                    reference.process(&input),
                    "{choice}"
                );
            }
        }
    }
}

#[test]
fn rejected_plugin_setter_preserves_prior_instance_and_oversampling() {
    for document in [false, true] {
        let mut handle = Handle::new("Saturation", r#"{"oversampling":"4x"}"#, 2, 2);
        let before = handle.state();
        let latency = handle.inner().plugin.latency_samples();
        let instance = std::ptr::from_ref(&*handle.inner().plugin).cast::<()>();
        let invalid = br#"{"drive":7.0,"oversampling":"invalid"}"#;
        assert_eq!(
            handle.load(invalid, document),
            PluginError::InvalidConfig as i32
        );
        assert_eq!(handle.state(), before);
        assert_eq!(handle.inner().plugin.latency_samples(), latency);
        assert_eq!(
            std::ptr::from_ref(&*handle.inner().plugin).cast::<()>(),
            instance
        );
        assert!(last_error().contains("oversampling"));
    }
}

#[test]
fn crossfeed_saved_custom_values_override_preset_action() {
    for document in [false, true] {
        let mut source = Handle::new("Crossfeed", "{}", 2, 2);
        source.set("preset", ParameterValue::Int(2));
        source.set("mode", ParameterValue::Int(0));
        source.set("bauer_fcut_hz", ParameterValue::Float(900.0));
        let saved = source.state();
        let mut target = Handle::new("Crossfeed", "{}", 2, 2);
        assert_eq!(target.load(&saved, document), 0, "{}", last_error());
        assert_eq!(target.state(), saved);

        // A changed selector with no explicit overrides retains its action.
        let state = br#"{"preset":3}"#;
        assert_eq!(target.load(state, document), 0, "{}", last_error());
        let mut expected = Handle::new("Crossfeed", "{}", 2, 2);
        expected.set("preset", ParameterValue::Int(3));
        assert_eq!(target.state(), expected.state());
    }
}

#[test]
fn partial_restore_retains_external_convolution_ir_and_processing_options() {
    use std::io::Write;
    struct IrFile(std::path::PathBuf);
    impl Drop for IrFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let file = IrFile(std::env::temp_dir().join(format!(
        "sotf-ffi-restore-{}-{stamp}.wav",
        std::process::id()
    )));
    // Mono PCM16 at the host clock: h = [1/4, 0, -1/8, 1/16].
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&44u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&96_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&8u32.to_le_bytes());
    for sample in [8192i16, 0, -4096, 2048] {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file.0)
        .unwrap()
        .write_all(&wav)
        .unwrap();

    let config = serde_json::json!({"ir_file":file.0,"use_nupc":true,"zero_latency_head":true,"head_taps":32}).to_string();
    for document in [false, true] {
        let mut handle = Handle::new("Convolution", &config, 1, 1);
        assert_eq!(
            handle.load(br#"{"mix":0.5}"#, document),
            0,
            "{}",
            last_error()
        );
        assert_eq!(handle.inner().config_json, config);
        assert_eq!(handle.inner().plugin.latency_samples(), 0);
        // Reset clears the IR startup crossfade and snaps the mix smoother to
        // the restored target, permitting an exact impulse-response oracle.
        assert_eq!(plugin_reset(handle.0), 0);
        let output = handle.process(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let expected = [0.625, 0.0, -0.0625, 0.03125, 0.0, 0.0, 0.0, 0.0];
        for (sample, expected) in output.iter().zip(expected) {
            assert!(
                (sample - expected).abs() < 1e-6,
                "got {sample}, expected {expected}"
            );
        }
    }
}

#[test]
fn advertised_analog_plugins_and_aliases_expose_typed_parameters() {
    for (kind, scalar) in [
        ("AnalogEQ", "low_gain"),
        ("analog_eq", "low_gain"),
        ("AnalogLimiter", "threshold"),
        ("analog_limiter", "threshold"),
        ("AnalogCompressor", "threshold"),
        ("analog_compressor", "threshold"),
    ] {
        let handle = Handle::new(kind, "{}", 2, 2);
        let plugin = &*handle.inner().plugin;
        assert!(matches!(
            plugin.get_parameter(&ParameterId::from("analog_model")),
            Some(ParameterValue::String(_))
        ));
        assert!(matches!(
            plugin.get_parameter(&ParameterId::from(scalar)),
            Some(ParameterValue::Float(_))
        ));
        if matches!(kind, "AnalogLimiter" | "analog_limiter") {
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("soft")),
                Some(ParameterValue::Bool(false))
            );
        }
        let count = usize::try_from(plugin_get_parameter_count(handle.0)).unwrap();
        assert!(count > 0);
        for index in 0..count {
            let info = plugin_get_parameter_info(handle.0, index);
            assert!(!info.is_null());
            // SAFETY: The metadata pointer belongs to this still-live handle.
            let info = unsafe { &*info };
            let value = plugin_get_parameter(handle.0, info.id);
            assert!(
                value.is_finite() && (0.0..=1.0).contains(&value),
                "{kind}: {index} = {value}"
            );
        }
    }
}

#[test]
fn gate_modes_roundtrip_through_both_c_state_apis_and_obey_gain_caps() {
    let config = r#"{"threshold_db":-30.0,"ratio":3.0,"attack_ms":0.1,"hold_ms":0.0,"release_ms":10.0,"range_db":6.0}"#;
    for document in [false, true] {
        let mut handle = Handle::new("Gate", config, 2, 2);
        for (mode, signed_cap) in [(1, 6.0_f64), (2, -6.0), (0, 0.0)] {
            let state = serde_json::to_vec(&serde_json::json!({"mode": mode, "max_boost_db": 6.0}))
                .unwrap();
            assert_eq!(handle.load(&state, document), 0, "{}", last_error());
            assert_eq!(
                plugin_get_parameter(handle.0, c"mode".as_ptr()),
                mode as f64 / 2.0
            );
            assert_eq!(
                plugin_get_parameter(handle.0, c"max_boost_db".as_ptr()),
                0.25
            );
            let saved = handle.state();
            let values: serde_json::Value = serde_json::from_slice(&saved).unwrap();
            assert_eq!(values["mode"], mode);
            assert_eq!(values["max_boost_db"], 6.0);
            assert_eq!(handle.load(&saved, document), 0, "{}", last_error());
            let mut last = Vec::new();
            for frames in [1, 17, 257, 63].into_iter().cycle().take(128) {
                let input: Vec<_> = (0..frames).flat_map(|_| [0.1, -0.1]).collect();
                last = handle.process(&input);
            }
            // The DSP uses a scalar fast-power approximation bounded to 0.02 dB.
            let expected = 0.1 * 10.0_f64.powf(signed_cap / 20.0);
            for sample in last.as_chunks::<2>().0 {
                assert!(
                    (20.0 * (f64::from(sample[0]) / expected).log10()).abs() < 0.02,
                    "mode {mode}"
                );
                assert!(
                    (20.0 * (-f64::from(sample[1]) / expected).log10()).abs() < 0.02,
                    "mode {mode}"
                );
            }
            let unchanged = handle.state();
            assert_ne!(
                plugin_set_parameter(handle.0, c"mode".as_ptr(), ((mode + 1) % 3) as f64 / 2.0),
                0,
                "changing mode after activation requires state reconstruction"
            );
            assert_eq!(handle.state(), unchanged);
        }
    }
}

#[test]
fn gate_c_external_key_layout_processes_and_restores_both_state_formats() {
    for document in [false, true] {
        for (mode, sign) in [("Upward", 1.0), ("Duck", -1.0)] {
            let config = serde_json::json!({"mode":mode,"sidechain_external":true,
                "threshold_db":-30.0,"ratio":3.0,"attack_ms":0.1,"_sotf_max_callback_frames":17003,
                "release_ms":10.0,"hold_ms":0.0,"range_db":6.0,"max_boost_db":6.0})
            .to_string();
            let mut handle = Handle::new("Gate", &config, 4, 2);
            let saved = handle.state();
            assert_eq!(handle.load(&saved, document), 0, "{}", last_error());
            assert_eq!(
                (
                    handle.inner().input_channels,
                    handle.inner().output_channels
                ),
                (4, 2)
            );
            let input: Vec<_> = (0..17003).flat_map(|_| [0.004, -0.002, 0.1, 0.0]).collect();
            let output = handle.process(&input);
            assert_eq!(output.len(), 34006);
            let gain = 10.0_f64.powf(sign * 6.0 / 20.0);
            for frame in output[output.len() - 126..].as_chunks::<2>().0 {
                for (actual, program) in frame.iter().zip([0.004, -0.002]) {
                    assert!((20.0 * (f64::from(*actual) / (program * gain)).log10()).abs() < 0.02);
                }
            }
        }
    }
    for (inputs, outputs, external) in [(4, 4, true), (2, 2, true), (4, 2, false)] {
        let config =
            CString::new(serde_json::json!({"sidechain_external":external}).to_string()).unwrap();
        assert!(
            plugin_create(c"Gate".as_ptr(), config.as_ptr(), 48_000, inputs, outputs).is_null()
        );
        assert!(last_error().contains("input channels"));
    }
}

fn assert_convolution_left_impulse(handle: &mut Handle, expected: [f32; 2]) {
    assert_eq!(plugin_reset(handle.0), 0);
    let mut input = vec![0.0_f32; 1_100 * 2];
    input[0] = 1.0;
    let output = handle.process(&input);
    for (actual, expected) in output[1_024 * 2..1_024 * 2 + 2].iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "Convolution output was {actual}, expected {expected}"
        );
    }
}

#[test]
fn true_stereo_convolution_rebuilds_from_state_and_preserves_failed_restore_history() {
    struct IrFile(std::path::PathBuf);
    impl Drop for IrFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let true_ir = IrFile(write_true_stereo_ir());
    let stereo_ir = IrFile(write_stereo_ir_with_tail());
    let config = |ir_file: &std::path::Path, true_stereo: bool, mix: f32| {
        serde_json::json!({
            "ir_file": ir_file,
            "mix": mix,
            "gain_db": 0.0,
            "use_nupc": false,
            "zero_latency_head": false,
            "true_stereo": true_stereo,
        })
        .to_string()
    };
    let true_config = config(&true_ir.0, true, 1.0);
    let false_config = config(&true_ir.0, false, 1.0);

    for document in [false, true] {
        let true_source = Handle::new("Convolution", &true_config, 2, 2);
        let false_source = Handle::new("Convolution", &false_config, 2, 2);
        let true_state = true_source.state();
        let false_state = false_source.state();
        assert_eq!(
            true_source
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(true))
        );

        // The FFI state contains the structural choice. A fresh legacy-config
        // handle must build the replacement with four-path routing before it
        // replays ordinary parameters.
        let mut restored = Handle::new("Convolution", &false_config, 2, 2);
        assert_eq!(restored.load(&true_state, document), 0, "{}", last_error());
        assert_eq!(
            restored
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(true))
        );
        let restored_config: serde_json::Value =
            serde_json::from_str(&restored.inner().config_json).unwrap();
        assert_eq!(restored_config["true_stereo"], true);
        assert_eq!(
            restored_config["ir_file"],
            true_ir.0.to_string_lossy().as_ref()
        );
        assert_convolution_left_impulse(&mut restored, [0.25, -0.125]);

        // Both transitions reconstruct on the control thread and remain
        // serializable through either the state API or preset-document API.
        assert_eq!(restored.load(&false_state, document), 0, "{}", last_error());
        assert_eq!(
            restored
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(false))
        );
        assert_convolution_left_impulse(&mut restored, [0.25, 0.0]);
        assert_eq!(restored.load(&true_state, document), 0, "{}", last_error());
        assert_convolution_left_impulse(&mut restored, [0.25, -0.125]);

        // Restoring a compatible true-stereo preset also replaces the old IR
        // before constructing the new route. The old handle starts with a
        // two-channel IR, which cannot be used to construct true-stereo mode.
        let legacy_two_channel_config = config(&stereo_ir.0, false, 1.0);
        let mut restored_from_legacy = Handle::new("Convolution", &legacy_two_channel_config, 2, 2);
        assert_eq!(
            restored_from_legacy.load(&true_state, document),
            0,
            "{}",
            last_error()
        );
        let restored_from_legacy_config: serde_json::Value =
            serde_json::from_str(&restored_from_legacy.inner().config_json).unwrap();
        assert_eq!(restored_from_legacy_config["true_stereo"], true);
        assert_eq!(
            restored_from_legacy_config["ir_file"],
            true_ir.0.to_string_lossy().as_ref()
        );
        assert_convolution_left_impulse(&mut restored_from_legacy, [0.25, -0.125]);

        // A partial state that carries only routing reconstructs the same IR
        // and retains its existing mix value.
        let partial_config = config(&true_ir.0, false, 0.5);
        let mut partial = Handle::new("Convolution", &partial_config, 2, 2);
        let partial_mode = br#"{"true_stereo":true}"#;
        assert_eq!(partial.load(partial_mode, document), 0, "{}", last_error());
        assert_eq!(
            partial
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(true))
        );
        assert_eq!(
            partial
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("mix")),
            Some(ParameterValue::Float(0.5))
        );
        let partial_restored_config: serde_json::Value =
            serde_json::from_str(&partial.inner().config_json).unwrap();
        assert_eq!(
            partial_restored_config["ir_file"],
            true_ir.0.to_string_lossy().as_ref()
        );
        assert_convolution_left_impulse(&mut partial, [0.625, -0.0625]);

        // Invalid field types fail before publication and retain the in-flight
        // convolution in the currently installed true-stereo processor.
        let mut invalid_type = Handle::new("Convolution", &true_config, 2, 2);
        let mut type_reference = Handle::new("Convolution", &true_config, 2, 2);
        let prefix = [1.0_f32, 0.0];
        assert_eq!(
            invalid_type.process(&prefix),
            type_reference.process(&prefix)
        );
        let state_before_invalid = invalid_type.state();
        let config_before_invalid = invalid_type.inner().config_json.clone();
        let instance_before_invalid =
            std::ptr::from_ref(&*invalid_type.inner().plugin).cast::<()>();
        assert_ne!(
            plugin_set_parameter(invalid_type.0, c"true_stereo".as_ptr(), 0.0),
            0,
            "structural routing must be changed through state reconstruction, not scalar automation"
        );
        assert_eq!(invalid_type.state(), state_before_invalid);
        assert_eq!(invalid_type.inner().config_json, config_before_invalid);
        assert_eq!(
            std::ptr::from_ref(&*invalid_type.inner().plugin).cast::<()>(),
            instance_before_invalid
        );
        assert_eq!(
            invalid_type.load(br#"{"true_stereo":"yes"}"#, document),
            PluginError::InvalidConfig as i32
        );
        assert_eq!(invalid_type.state(), state_before_invalid);
        assert_eq!(invalid_type.inner().config_json, config_before_invalid);
        assert_eq!(
            std::ptr::from_ref(&*invalid_type.inner().plugin).cast::<()>(),
            instance_before_invalid
        );
        let continuation = vec![0.0_f32; 1_100 * 2];
        let output = invalid_type.process(&continuation);
        assert_eq!(output, type_reference.process(&continuation));
        assert!((output[1_023 * 2 + 1] + 0.125).abs() < 1e-6);

        // A valid true-stereo preset cannot be installed against an incompatible
        // two-channel IR. Rejection must preserve the old plugin and its live
        // long-IR history rather than half-publishing the new mode.
        let legacy_config = config(&stereo_ir.0, false, 1.0);
        let mut incompatible = Handle::new("Convolution", &legacy_config, 2, 2);
        let mut incompatible_reference = Handle::new("Convolution", &legacy_config, 2, 2);
        let mut warmup = vec![0.0_f32; 1_550 * 2];
        warmup[0] = 1.0;
        assert_eq!(
            incompatible.process(&warmup),
            incompatible_reference.process(&warmup)
        );
        let state_before_width_error = incompatible.state();
        let instance_before_width_error =
            std::ptr::from_ref(&*incompatible.inner().plugin).cast::<()>();
        let mut incompatible_state: serde_json::Value =
            serde_json::from_slice(&true_state).unwrap();
        incompatible_state["ir_file"] =
            serde_json::Value::String(stereo_ir.0.to_string_lossy().into_owned());
        let incompatible_state = serde_json::to_vec(&incompatible_state).unwrap();
        assert_eq!(
            incompatible.load(&incompatible_state, document),
            PluginError::InvalidConfig as i32
        );
        assert_eq!(incompatible.state(), state_before_width_error);
        assert_eq!(incompatible.inner().config_json, legacy_config);
        assert_eq!(
            std::ptr::from_ref(&*incompatible.inner().plugin).cast::<()>(),
            instance_before_width_error
        );
        let continuation = vec![0.0_f32; 1_400 * 2];
        let output = incompatible.process(&continuation);
        assert_eq!(output, incompatible_reference.process(&continuation));
        assert!(output.iter().any(|sample| sample.abs() > 1e-6));
    }
}

#[test]
fn bandsplit_same_width_restore_rebuilds_ordered_cutoffs_and_structural_controls() {
    let legacy_config = r#"{"frequencies":[1000.0,2000.0,3000.0],"type":"LR24","recombination_mode":"legacy_cascade","num_bands":4}"#;
    let compensated_config = r#"{"frequencies":[2000.0,3000.0,4000.0],"type":"LR48","recombination_mode":"phase_compensated","num_bands":4}"#;

    for document in [false, true] {
        let mut restored = Handle::new("BandSplit", legacy_config, 2, 8);
        restored.set("band_0_gain_db", ParameterValue::Float(-6.0));
        let partial_state = br#"{"frequency":2000.0,"frequency_2":3000.0,"frequency_3":4000.0,"type":1,"recombination_mode":1}"#;

        // This cutoff shift would fail if the first new value were replayed
        // before the existing cutoffs. Rebuild from the merged vector and
        // preserve omitted gain controls from the live state.
        assert_eq!(
            restored.load(partial_state, document),
            0,
            "{}",
            last_error()
        );
        let merged_config: serde_json::Value =
            serde_json::from_str(&restored.inner().config_json).unwrap();
        assert_eq!(
            merged_config["frequencies"],
            serde_json::json!([2000.0, 3000.0, 4000.0])
        );
        assert_eq!(merged_config["num_bands"], 4);
        assert_eq!(merged_config["type"], "LR48");
        assert_eq!(merged_config["recombination_mode"], "phase_compensated");
        assert_eq!(
            restored
                .inner()
                .plugin
                .get_parameter(&ParameterId::from("band_0_gain_db")),
            Some(ParameterValue::Float(-6.0))
        );

        let mut reference = Handle::new("BandSplit", compensated_config, 2, 8);
        reference.set("band_0_gain_db", ParameterValue::Float(-6.0));
        // Restored replacements initialize their parameter smoothers at the
        // saved targets. Reset the post-create reference to the same startup
        // contract instead of comparing against a fresh 20 ms gain ramp.
        assert_eq!(plugin_reset(reference.0), 0);
        let restored_state: serde_json::Value = serde_json::from_slice(&restored.state()).unwrap();
        let reference_state: serde_json::Value =
            serde_json::from_slice(&reference.state()).unwrap();
        assert_eq!(
            restored_state, reference_state,
            "restored plugin parameters differ"
        );
        let input: Vec<f32> = (0..1024)
            .flat_map(|frame| {
                let phase = frame as f32 * 0.071;
                [phase.sin() * 0.4, (phase * 0.73).cos() * 0.3]
            })
            .collect();
        let actual = restored.process(&input);
        let expected = reference.process(&input);
        assert_eq!(actual.len(), expected.len());
        assert!(actual.iter().all(|sample| sample.is_finite()));
        assert!(expected.iter().all(|sample| sample.is_finite()));
        assert!(
            actual == expected,
            "restored output differs from fresh reference across {} samples",
            actual.len()
        );

        let partial_config = r#"{"frequencies":[1000.0,2000.0,3000.0],"type":"LR48","recombination_mode":"phase_compensated","num_bands":4}"#;
        let mut partial = Handle::new("BandSplit", partial_config, 2, 8);
        partial.set("band_1_gain_db", ParameterValue::Float(3.0));
        assert_eq!(
            partial.load(br#"{"frequency":1500.0}"#, document),
            0,
            "{}",
            last_error()
        );
        let partial_state: serde_json::Value = serde_json::from_slice(&partial.state()).unwrap();
        assert_eq!(partial_state["frequency"], 1500.0);
        assert_eq!(partial_state["frequency_2"], 2000.0);
        assert_eq!(partial_state["frequency_3"], 3000.0);
        assert_eq!(partial_state["type"], 1);
        assert_eq!(partial_state["recombination_mode"], 1);
        assert_eq!(partial_state["band_1_gain_db"], 3.0);

        let partial_reference_config = r#"{"frequencies":[1500.0,2000.0,3000.0],"type":"LR48","recombination_mode":"phase_compensated","num_bands":4}"#;
        let mut partial_reference = Handle::new("BandSplit", partial_reference_config, 2, 8);
        partial_reference.set("band_1_gain_db", ParameterValue::Float(3.0));
        assert_eq!(plugin_reset(partial_reference.0), 0);
        assert_eq!(partial.process(&input), partial_reference.process(&input));
    }
}

#[test]
fn bandsplit_crossover_type_alias_config_restores_through_state_and_preset() {
    let alias_config = r#"{"frequencies":[1000.0,2000.0,3000.0],"crossover_type":"LR24","recombination_mode":"legacy_cascade","num_bands":4}"#;
    let target_config = r#"{"frequencies":[2000.0,3000.0,4000.0],"type":"LR48","recombination_mode":"phase_compensated","num_bands":4}"#;
    let restore = br#"{"frequency":2000.0,"frequency_2":3000.0,"frequency_3":4000.0,"type":1,"recombination_mode":1}"#;

    for document in [false, true] {
        let mut restored = Handle::new("BandSplit", alias_config, 2, 8);
        assert_eq!(restored.load(restore, document), 0, "{}", last_error());

        let mut reference = Handle::new("BandSplit", target_config, 2, 8);
        assert_eq!(plugin_reset(reference.0), 0);
        let input: Vec<f32> = (0..1024)
            .flat_map(|frame| {
                let phase = frame as f32 * 0.059;
                [phase.sin() * 0.35, (phase * 0.81).cos() * 0.28]
            })
            .collect();
        let actual = restored.process(&input);
        let expected = reference.process(&input);
        assert_eq!(actual.len(), expected.len());
        assert!(actual.iter().all(|sample| sample.is_finite()));
        assert!(expected.iter().all(|sample| sample.is_finite()));
        assert!(
            actual == expected,
            "alias-config restored output differs from fresh reference across {} samples",
            actual.len()
        );
    }
}

#[test]
fn bandsplit_invalid_and_width_changing_restores_keep_live_history() {
    let config = r#"{"frequencies":[1000.0,2000.0,3000.0],"type":"LR24","recombination_mode":"phase_compensated","num_bands":4}"#;

    for document in [false, true] {
        let mut handle = Handle::new("BandSplit", config, 2, 8);
        let mut reference = Handle::new("BandSplit", config, 2, 8);
        let warmup: Vec<f32> = (0..8192)
            .map(|sample| {
                let frame = (sample / 2) as f32;
                if sample % 2 == 0 {
                    (frame * 0.053).sin() * 0.5
                } else {
                    (frame * 0.037).cos() * 0.35
                }
            })
            .collect();
        assert_eq!(handle.process(&warmup), reference.process(&warmup));

        for invalid in [
            br#"{"frequency":3500.0}"#.as_slice(),
            br#"{"num_bands":1}"#.as_slice(),
        ] {
            let saved_state = handle.state();
            let saved_config = handle.inner().config_json.clone();
            let instance = std::ptr::from_ref(&*handle.inner().plugin).cast::<()>();
            assert_eq!(
                handle.load(invalid, document),
                PluginError::InvalidConfig as i32
            );
            assert_eq!(handle.state(), saved_state);
            assert_eq!(handle.inner().config_json, saved_config);
            assert_eq!(
                std::ptr::from_ref(&*handle.inner().plugin).cast::<()>(),
                instance
            );
            let continuation: Vec<f32> = (0..2048)
                .map(|sample| {
                    let frame = (sample / 2) as f32;
                    if sample % 2 == 0 {
                        (frame * 0.091).sin() * 0.4
                    } else {
                        (frame * 0.067).cos() * 0.3
                    }
                })
                .collect();
            assert_eq!(
                handle.process(&continuation),
                reference.process(&continuation)
            );
        }
    }
}
