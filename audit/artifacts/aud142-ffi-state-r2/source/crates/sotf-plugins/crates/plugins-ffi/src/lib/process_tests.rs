//! Fixed-frame C audio callback contract regressions.

use crate::{
    ParameterMap, PluginError, PluginHandle, plugin_create, plugin_destroy, plugin_get_last_error,
    plugin_process,
};
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, PluginInfo, ProcessContext};
use std::ffi::{CStr, CString};

#[derive(Clone, Copy)]
enum Outcome {
    Complete,
    Frames(usize),
    Failure,
}

struct FrameCountPlugin {
    input_channels: usize,
    output_channels: usize,
    outcome: Outcome,
}

impl Plugin for FrameCountPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Frame count test", "1", "SOTF")
    }
    fn input_channels(&self) -> usize {
        self.input_channels
    }
    fn output_channels(&self) -> usize {
        self.output_channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(String::new())
    }
    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        _input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        match self.outcome {
            Outcome::Complete => {
                output.fill(0.25);
                Ok(context.num_frames)
            }
            Outcome::Frames(frames) => {
                // Deliberately leave stale samples behind a written prefix.
                let written = self.output_channels.min(output.len());
                output[..written].fill(0.125);
                Ok(frames)
            }
            Outcome::Failure => {
                let written = self.output_channels.min(output.len());
                output[..written].fill(0.125);
                Err(String::new())
            }
        }
    }
}

struct OwnedHandle(*mut PluginHandle);

#[test]
fn string_choices_preserve_live_defaults_and_reject_structural_changes() {
    for (name, key, default, output_channels) in [
        (c"AAE", c"room_preset", 1.0 / 3.0, 6),
        (c"DeEsser", c"mode", 1.0, 2),
    ] {
        let handle = OwnedHandle(plugin_create(
            name.as_ptr(),
            c"{}".as_ptr(),
            48_000,
            2,
            output_channels,
        ));
        assert!(!handle.0.is_null());
        assert_eq!(
            crate::plugin_get_parameter(handle.0, key.as_ptr()),
            default,
            "{} live default",
            name.to_str().unwrap()
        );
        assert_eq!(
            crate::plugin_set_parameter(handle.0, key.as_ptr(), default),
            PluginError::Success as i32
        );
        assert_eq!(
            crate::plugin_set_parameter(handle.0, key.as_ptr(), 0.0),
            PluginError::InvalidParameter as i32
        );
        assert_eq!(crate::plugin_get_parameter(handle.0, key.as_ptr()), default);
    }
}

#[test]
fn expanded_eq_band_orders_reach_the_dsp_as_integers() {
    let config =
        c"{\"filters\":[{\"filter_type\":\"peak\",\"freq\":1000.0,\"q\":1.5,\"db_gain\":3.0}]}";
    let handle = OwnedHandle(plugin_create(c"EQ".as_ptr(), config.as_ptr(), 48_000, 2, 2));
    assert!(!handle.0.is_null());
    for order in [4, 6, 8, 2] {
        let normalized = f64::from(order - 2) / 6.0;
        assert_eq!(
            crate::plugin_set_parameter(handle.0, c"band_0_order".as_ptr(), normalized),
            PluginError::Success as i32
        );
        assert_eq!(
            crate::plugin_get_parameter(handle.0, c"band_0_order".as_ptr()),
            normalized,
            "order {order}"
        );
        // SAFETY: The owned handle is valid and no callback is running.
        let plugin = unsafe { &(*handle.0).plugin };
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("band_0_order")),
            Some(ParameterValue::Int(order))
        );
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn injected_handle(input_channels: usize, output_channels: usize, outcome: Outcome) -> OwnedHandle {
    let plugin = Box::new(FrameCountPlugin {
        input_channels,
        output_channels,
        outcome,
    });
    let parameter_map = ParameterMap::from_plugin(&*plugin, "Gain");
    OwnedHandle(Box::into_raw(Box::new(PluginHandle {
        plugin,
        plugin_type: "Gain".into(),
        config_json: "{}".into(),
        parameter_map,
        retired_parameter_maps: Vec::new(),
        sample_rate: 48_000,
        max_callback_frames: 256,
        input_channels,
        output_channels,
        midi_output_events: Vec::new(),
        note_expression_output_events: Vec::new(),
    })))
}

#[test]
fn incomplete_and_overlong_successes_are_errors_and_silence_the_whole_destination() {
    let frames = 17;
    for (input_channels, output_channels) in [(1, 1), (1, 2), (2, 1), (6, 2), (2, 6)] {
        for outcome in [
            Outcome::Frames(0),
            Outcome::Frames(frames - 1),
            Outcome::Frames(frames + 1),
            Outcome::Frames(usize::MAX),
            Outcome::Failure,
        ] {
            let handle = injected_handle(input_channels, output_channels, outcome);
            let input = vec![0.5; frames * input_channels];
            let mut output = vec![9.0; frames * output_channels + 2];
            let status = std::cell::Cell::new(PluginError::Success as i32);
            let mut process = || {
                status.set(plugin_process(
                    handle.0,
                    input.as_ptr(),
                    output.as_mut_ptr().wrapping_add(1),
                    frames,
                ))
            };
            #[cfg(debug_assertions)]
            sotf_host::test_utils::assert_no_allocs("FFI frame-count failure", &mut process);
            #[cfg(not(debug_assertions))]
            process();
            assert_eq!(status.get(), PluginError::ProcessingFailed as i32);
            assert!(
                output[1..output.len() - 1]
                    .iter()
                    .all(|sample| *sample == 0.0)
            );
            assert_eq!(output[0], 9.0);
            assert_eq!(output[output.len() - 1], 9.0);
            let error = plugin_get_last_error();
            assert!(!error.is_null());
            // SAFETY: The error points to the static diagnostic from this call.
            let error = unsafe { CStr::from_ptr(error) }.to_str().unwrap();
            assert!(error.contains("frame count") || error.contains("processing failed"));
        }
    }
}

#[test]
fn complete_outputs_preserve_samples_for_independent_bus_widths() {
    for (input_channels, output_channels) in [(1, 1), (1, 2), (2, 1), (6, 2), (2, 6)] {
        for frames in [0, 1, 17, 256] {
            let handle = injected_handle(input_channels, output_channels, Outcome::Complete);
            let input = vec![0.5; frames * input_channels];
            let mut output = vec![9.0; frames * output_channels + 2];
            assert_eq!(
                plugin_process(
                    handle.0,
                    input.as_ptr(),
                    output.as_mut_ptr().wrapping_add(1),
                    frames
                ),
                PluginError::Success as i32
            );
            assert!(
                output[1..output.len() - 1]
                    .iter()
                    .all(|sample| *sample == 0.25)
            );
            assert_eq!(output[0], 9.0);
            assert_eq!(output[output.len() - 1], 9.0);
        }
    }
}

#[test]
fn normal_plugins_fill_fixed_frames_for_supported_layouts() {
    for (plugin_type, input_channels, output_channels) in [
        ("Gain", 1, 1),
        ("Gain", 2, 2),
        ("MonoToStereo", 1, 2),
        ("Downmix", 6, 2),
        ("Upmixer", 2, 6),
        ("AEC", 2, 1),
        ("BandSplit", 2, 4),
        ("BandMerge", 4, 2),
    ] {
        let name = CString::new(plugin_type).unwrap();
        let handle = OwnedHandle(plugin_create(
            name.as_ptr(),
            c"{}".as_ptr(),
            48_000,
            input_channels,
            output_channels,
        ));
        assert!(!handle.0.is_null(), "{plugin_type}");
        for frames in [1, 7, 127, 256] {
            let input = vec![0.0; frames * input_channels];
            let mut output = vec![f32::NAN; frames * output_channels];
            assert_eq!(
                plugin_process(handle.0, input.as_ptr(), output.as_mut_ptr(), frames),
                PluginError::Success as i32,
                "{plugin_type}: {frames} frames"
            );
            assert!(
                output.iter().all(|sample| sample.is_finite()),
                "{plugin_type}"
            );
        }
    }
}

#[test]
fn rejected_state_update_preserves_earlier_parameters() {
    let handle = OwnedHandle(plugin_create(
        c"Gate".as_ptr(),
        c"{}".as_ptr(),
        48_000,
        2,
        2,
    ));
    assert!(!handle.0.is_null());
    let before = crate::plugin_get_parameter(handle.0, c"attack".as_ptr());
    let invalid = br#"{"attack":10.0,"threshold":"invalid"}"#;
    assert_eq!(
        crate::plugin_load_state(handle.0, invalid.as_ptr(), invalid.len()),
        PluginError::InvalidConfig as i32
    );
    assert_eq!(
        crate::plugin_get_parameter(handle.0, c"attack".as_ptr()),
        before,
        "failed restoration must preserve the original attack parameter"
    );
}

#[test]
fn successful_scalar_ffi_automation_has_no_cold_thread_allocations() {
    let eq_config =
        c"{\"filters\":[{\"filter_type\":\"peak\",\"freq\":1000.0,\"q\":1.5,\"db_gain\":3.0}]}";
    for (name, config, parameter) in [
        (c"Gain", c"{}", c"gain_db"),
        (c"Saturation", c"{}", c"drive"),
        (c"Limiter", c"{}", c"threshold"),
        (c"EQ", eq_config, c"band_0_gain"),
        (c"EQ", eq_config, c"band_0_freq"),
        (c"EQ", eq_config, c"band_0_q"),
    ] {
        let handle = OwnedHandle(plugin_create(name.as_ptr(), config.as_ptr(), 48_000, 2, 2));
        assert!(!handle.0.is_null());
        // Transfer unique ownership to a fresh callback thread; the originating
        // thread makes no further handle calls. Plugin is Send, and the C-string
        // metadata remains owned by the handle until destruction on that thread.
        let address = handle.0 as usize;
        std::mem::forget(handle);
        std::thread::spawn(move || {
            let handle = OwnedHandle(address as *mut PluginHandle);
            // The shared Q schema permits notch Q up to 40; this fixture is a
            // peak filter whose documented runtime maximum is 20.
            let values = if parameter == c"band_0_q" {
                [0.1, 0.2, 0.3, 0.4]
            } else {
                [0.2, 0.4, 0.6, 0.8]
            };
            let automate = || {
                for normalized in values.into_iter().cycle().take(128) {
                    assert_eq!(
                        crate::plugin_set_parameter(handle.0, parameter.as_ptr(), normalized),
                        PluginError::Success as i32,
                        "{} {} at {normalized}",
                        name.to_str().unwrap(),
                        parameter.to_str().unwrap()
                    );
                    let actual = crate::plugin_get_parameter(handle.0, parameter.as_ptr());
                    assert!(
                        (actual - normalized).abs() < 1e-6,
                        "{}: {actual} versus {normalized}",
                        parameter.to_str().unwrap()
                    );
                }
            };
            #[cfg(debug_assertions)]
            sotf_host::test_utils::assert_no_allocs("FFI scalar automation", automate);
            #[cfg(not(debug_assertions))]
            automate();
        })
        .join()
        .unwrap();
    }
}
