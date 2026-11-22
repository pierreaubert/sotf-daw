//! Verifies allocation-free updates of prepared filters and bypass.

// Rust guideline compliant 2026-10-09

#![cfg(debug_assertions)]

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_host::test_utils::{CountingAlloc, assert_no_allocs_or_deallocs};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{SurroundBassMode, UpmixerPlugin, UpmixerPluginParams};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn prepared_filter_targets_and_bypass_do_not_allocate() {
    for (surround, bass_mode) in [
        (false, SurroundBassMode::Off),
        (true, SurroundBassMode::Off),
        (true, SurroundBassMode::Derived),
        (true, SurroundBassMode::Managed),
    ] {
        let mut params = UpmixerPluginParams::default();
        params.core.fft_size = 512;
        params.core.speaker_config = "5.1.4".into();
        if surround {
            params.multichannel.input_layout = Some(
                ChannelLayout::from_speaker_config(get_speaker_config("5.0").unwrap()).unwrap(),
            );
            params.multichannel.bass_mode = bass_mode;
        }
        let mut plugin = UpmixerPlugin::try_from_params(params).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let frames = 257;
        let input = vec![0.05; frames * plugin.input_channels()];
        let mut output = vec![0.0; frames * plugin.output_channels()];
        let context = ProcessContext::new(48_000, frames);
        let controls = [
            (
                "lfe_cutoff_hz",
                ParameterValue::Float(80.0),
                ParameterValue::Float(140.0),
            ),
            (
                "bandpass_hz",
                ParameterValue::Float(200.0),
                ParameterValue::Float(300.0),
            ),
            (
                "height_hf_cap_hz",
                ParameterValue::Float(8_000.0),
                ParameterValue::Float(18_000.0),
            ),
            (
                "bypass_all_processing",
                ParameterValue::Bool(true),
                ParameterValue::Bool(false),
            ),
        ];
        for (name, first, second) in controls {
            let id = ParameterId::from(name);
            assert_no_allocs_or_deallocs(name, || {
                plugin.set_parameter(id.clone(), first).unwrap();
                plugin.process(&input, &mut output, &context).unwrap();
                plugin.set_parameter(id.clone(), second.clone()).unwrap();
                plugin.process(&input, &mut output, &context).unwrap();
            });
            let cached = plugin
                .parameters()
                .into_iter()
                .find(|parameter| parameter.id == id)
                .unwrap();
            assert_eq!(cached.default_value, second, "surround={surround} {name}");
            assert_eq!(
                plugin.get_parameter(&id),
                Some(second),
                "surround={surround} {name}"
            );
            assert!(output.iter().all(|sample| sample.is_finite()));
            for parameter in plugin.parameters() {
                assert_eq!(
                    plugin.get_parameter(&parameter.id),
                    Some(parameter.default_value),
                    "surround={surround} {name} affected {}",
                    parameter.id,
                );
            }
        }
    }
}
