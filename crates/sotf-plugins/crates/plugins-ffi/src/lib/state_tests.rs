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
            let bytes = serde_json::to_vec(&serde_json::json!({"state": state})).unwrap();
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
