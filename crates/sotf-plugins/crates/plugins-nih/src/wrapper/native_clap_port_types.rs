//! Compare the two CLAP audio-port extensions through their real C callbacks.

use clap_sys::ext::audio_ports::{
    CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports,
};
use clap_sys::ext::audio_ports_config::{
    CLAP_EXT_AUDIO_PORTS_CONFIG, clap_audio_ports_config, clap_plugin_audio_ports_config,
};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use nih_plug::prelude::{ClapPlugin, Plugin};
use std::ffi::{CStr, c_char, c_void};

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn no_request(_: *const clap_host) {}

fn test_host() -> Box<clap_host> {
    Box::new(clap_host {
        clap_version: clap_sys::version::CLAP_VERSION,
        host_data: std::ptr::null_mut(),
        name: c"SOTF port type test".as_ptr(),
        vendor: c"SOTF".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    })
}

fn assert_port_types<P: Plugin + ClapPlugin>(
    cases: &[(u32, u32, u32, Option<&[u8]>, Option<&[u8]>)],
) {
    let host = test_host();
    // SAFETY: the host and wrapper live through all synchronous CLAP callbacks.
    let wrapper = unsafe { nih_plug::wrapper::clap::Wrapper::<P>::new(&*host) };
    let plugin: *const clap_plugin = wrapper.clap_plugin.as_ptr();
    // SAFETY: extension pointers and selected layouts belong to the live wrapper.
    unsafe {
        assert!(((*plugin).init.unwrap())(plugin));
        let configs =
            ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
                .cast::<clap_plugin_audio_ports_config>();
        let ports = ((*plugin).get_extension.unwrap())(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr())
            .cast::<clap_plugin_audio_ports>();
        assert!(!configs.is_null() && !ports.is_null());

        for &(index, input_channels, output_channels, input_type, output_type) in cases {
            let mut config: clap_audio_ports_config = std::mem::zeroed();
            assert!(((*configs).get.unwrap())(plugin, index, &mut config));
            assert_eq!(config.main_input_channel_count, input_channels);
            assert_eq!(config.main_output_channel_count, output_channels);
            let port_type =
                |ptr: *const c_char| (!ptr.is_null()).then(|| CStr::from_ptr(ptr).to_bytes());
            assert_eq!(port_type(config.main_input_port_type), input_type);
            assert_eq!(port_type(config.main_output_port_type), output_type);
            assert!(((*configs).select.unwrap())(plugin, config.id));

            let mut input: clap_audio_port_info = std::mem::zeroed();
            let mut output: clap_audio_port_info = std::mem::zeroed();
            assert!(((*ports).get.unwrap())(plugin, 0, true, &mut input));
            assert!(((*ports).get.unwrap())(plugin, 0, false, &mut output));
            assert_eq!(input.channel_count, input_channels);
            assert_eq!(output.channel_count, output_channels);
            assert_eq!(port_type(input.port_type), input_type);
            assert_eq!(port_type(output.port_type), output_type);
        }
    }
}

#[cfg(feature = "eq")]
#[test]
fn eq_stereo_and_quad_report_consistent_port_types() {
    assert_port_types::<crate::plugin::SotfEQ>(&[
        (0, 2, 2, Some(b"stereo"), Some(b"stereo")),
        (2, 4, 4, Some(b"surround"), Some(b"surround")),
    ]);
}

#[cfg(feature = "crossover")]
#[test]
fn crossover_stereo_and_quad_report_consistent_port_types() {
    assert_port_types::<crate::plugin::SotfCrossover>(&[
        (0, 2, 2, Some(b"stereo"), Some(b"stereo")),
        (8, 4, 4, Some(b"surround"), None),
    ]);
}
