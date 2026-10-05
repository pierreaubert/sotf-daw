//! Reproduce CLAP parameter text conversions reported by release validation.

use clap_sys::ext::params::{CLAP_EXT_PARAMS, clap_param_info, clap_plugin_params};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use nih_plug::prelude::*;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::Arc;

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn no_request(_: *const clap_host) {}

fn assert_roundtrip<P: Plugin + ClapPlugin>(
    name: &str,
    initial: f64,
    max_ulps: Option<u32>,
) -> String {
    let host = Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF parameter text test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    });
    // SAFETY: the boxed host remains live for the wrapper and all synchronous callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<P>::new(&*host) };
    let plugin: *const clap_plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: the live plugin owns the extension and writes into valid fixed-size buffers.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
            .cast::<clap_plugin_params>();
        assert!(!params.is_null());
        let mut id = None;
        for index in 0..((*params).count.unwrap())(plugin) {
            let mut info: clap_param_info = std::mem::zeroed();
            assert!(((*params).get_info.unwrap())(plugin, index, &mut info));
            if CStr::from_ptr(info.name.as_ptr()).to_bytes() == name.as_bytes() {
                id = Some(info.id);
                break;
            }
        }
        let id = id.unwrap_or_else(|| panic!("missing CLAP parameter {name}"));
        let to_text = |value| {
            let mut display = [0 as c_char; 128];
            assert!(((*params).value_to_text.unwrap())(
                plugin,
                id,
                value,
                display.as_mut_ptr(),
                display.len() as u32,
            ));
            CStr::from_ptr(display.as_ptr())
                .to_string_lossy()
                .into_owned()
        };
        let first = to_text(initial);
        let display = CString::new(first.as_str()).unwrap();
        let mut parsed = f64::NAN;
        assert!(((*params).text_to_value.unwrap())(
            plugin,
            id,
            display.as_ptr(),
            &mut parsed,
        ));
        assert!(parsed.is_finite());
        assert!((0.0..=1.0).contains(&parsed));
        assert!(
            (parsed - initial).abs() <= 8.0 * f64::from(f32::EPSILON),
            "{name}: normalized value changed too far: {initial} -> {parsed}"
        );
        if let Some(max_ulps) = max_ulps {
            // All twelve validator failures in run #559 moved one normalized f32 ULP.
            let distance = (initial as f32)
                .to_bits()
                .abs_diff((parsed as f32).to_bits());
            assert!(
                distance <= max_ulps,
                "{name}: normalized value moved {distance} f32 ULPs"
            );
        }
        assert_eq!(first, to_text(parsed), "{name}: CLAP display text changed");
        first
    }
}

#[cfg(feature = "aec")]
#[test]
fn aec_step_size_text_is_stable() {
    for value in [0.0, f32::EPSILON as f64, 0.25, 0.5, 0.75, 1.0] {
        assert_roundtrip::<crate::plugin::SotfAEC>("Step Size", value, None);
    }
    assert_roundtrip::<crate::plugin::SotfAEC>("Step Size", 0.26262626262626265, Some(8));
}

#[cfg(feature = "crossfeed")]
#[test]
fn crossfeed_frequency_text_is_stable() {
    for value in [0.0, f32::EPSILON as f64, 0.25, 0.5, 0.75, 1.0] {
        assert_roundtrip::<crate::plugin::SotfCrossfeed>("MB Mid/High Freq", value, None);
    }
    assert_roundtrip::<crate::plugin::SotfCrossfeed>(
        "MB Mid/High Freq",
        0.8686868686868687,
        Some(8),
    );
}

#[cfg(feature = "upmixer")]
#[test]
fn upmixer_lfo_rate_text_is_stable() {
    for value in [0.0, f32::EPSILON as f64, 0.25, 0.5, 0.75, 1.0] {
        assert_roundtrip::<crate::plugin::SotfUpmixer>("Decor LFO Rate", value, None);
    }
    assert_roundtrip::<crate::plugin::SotfUpmixer>("Decor LFO Rate", 0.43529411764705883, Some(8));
}

struct TextParams {
    unit: FloatParam,
    custom: FloatParam,
    mode: IntParam,
    enabled: BoolParam,
}

// SAFETY: each pointer references a field owned by the Arc held by TextPlugin.
unsafe impl Params for TextParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        vec![
            (
                "unit".into(),
                ParamPtr::FloatParam(&self.unit as *const _),
                String::new(),
            ),
            (
                "custom".into(),
                ParamPtr::FloatParam(&self.custom as *const _),
                String::new(),
            ),
            (
                "mode".into(),
                ParamPtr::IntParam(&self.mode as *const _),
                String::new(),
            ),
            (
                "enabled".into(),
                ParamPtr::BoolParam(&self.enabled as *const _),
                String::new(),
            ),
        ]
    }
}

struct TextPlugin {
    params: Arc<TextParams>,
}

impl Default for TextPlugin {
    fn default() -> Self {
        let range = FloatRange::Linear {
            min: 20.0,
            max: 20_000.0,
        };
        Self {
            params: Arc::new(TextParams {
                unit: FloatParam::new("Unit Frequency", 440.0, range).with_unit(" Hz"),
                custom: FloatParam::new("Custom Frequency", 440.0, range)
                    .with_unit(" Hz")
                    .with_value_to_string(Arc::new(|value| format!("{value:.1}")))
                    .with_string_to_value(Arc::new(|text| {
                        text.trim_end_matches(" Hz").parse::<f32>().ok()
                    })),
                mode: IntParam::new("Mode", 1, IntRange::Linear { min: 0, max: 3 }),
                enabled: BoolParam::new("Enabled", true),
            }),
        }
    }
}

impl Plugin for TextPlugin {
    const NAME: &'static str = "SOTF CLAP Text Probe";
    const VENDOR: &'static str = "SOTF";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "1.0.0";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout::const_default()];
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        _: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for TextPlugin {
    const CLAP_ID: &'static str = "org.spinorama.sotf.text-probe";
    const CLAP_DESCRIPTION: Option<&'static str> = None;
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect];
}

#[test]
fn clap_unit_and_custom_formatter_roundtrip_through_extension() {
    let normalized = f64::from(
        FloatRange::Linear {
            min: 20.0,
            max: 20_000.0,
        }
        .normalize(440.0),
    );
    let unit_text = assert_roundtrip::<TextPlugin>("Unit Frequency", normalized, Some(8));
    assert!(unit_text.ends_with(" Hz"));
    let custom_text = assert_roundtrip::<TextPlugin>("Custom Frequency", normalized, Some(8));
    assert_eq!(custom_text, "440.0 Hz");
}

fn assert_parser_result(name: &str, text: &str, expected: f64) {
    let host = Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF parser test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    });
    // SAFETY: the host and wrapper outlive all synchronous CLAP callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<TextPlugin>::new(&*host) };
    let plugin: *const clap_plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: callback pointers and parameter info belong to the live wrapper.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let params = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_PARAMS.as_ptr())
            .cast::<clap_plugin_params>();
        assert!(!params.is_null());
        let mut id = None;
        for index in 0..((*params).count.unwrap())(plugin) {
            let mut info: clap_param_info = std::mem::zeroed();
            assert!(((*params).get_info.unwrap())(plugin, index, &mut info));
            if CStr::from_ptr(info.name.as_ptr()).to_bytes() == name.as_bytes() {
                id = Some(info.id);
                break;
            }
        }
        let id = id.unwrap_or_else(|| panic!("missing CLAP parameter {name}"));
        let input = CString::new(text).unwrap();
        let mut parsed = f64::NAN;
        assert!(((*params).text_to_value.unwrap())(
            plugin,
            id,
            input.as_ptr(),
            &mut parsed,
        ));
        assert_eq!(parsed, expected, "{name}: parser changed for {text:?}");
    }
}

#[test]
fn clap_noncanonical_text_and_stepped_params_keep_parser_values() {
    let params = TextPlugin::default();
    let expected = params
        .params
        .unit
        .string_to_normalized_value(" 440.05 Hz ")
        .unwrap();
    assert_parser_result("Unit Frequency", " 440.05 Hz ", f64::from(expected));
    let mode = params.params.mode.string_to_normalized_value("2").unwrap();
    assert_parser_result("Mode", "2", f64::from(mode) * 3.0);
    assert_parser_result("Enabled", "On", 1.0);
}
