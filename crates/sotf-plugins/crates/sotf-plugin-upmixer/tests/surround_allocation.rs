//! Checks heap activity during surround playback and drain.

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
fn surround_processing_and_drain_do_not_allocate_or_free() {
    for (input_id, output_id) in [
        ("5.0", "5.1.4"),
        ("5.0", "5.1.6"),
        ("5.1", "5.1.6"),
        ("5.1", "7.1.4"),
        ("7.1", "9.1.6"),
    ] {
        for mode in [
            SurroundBassMode::Off,
            SurroundBassMode::Derived,
            SurroundBassMode::Managed,
        ] {
            let input_layout =
                ChannelLayout::from_speaker_config(get_speaker_config(input_id).unwrap()).unwrap();
            let input_channels = input_layout.channels.len();
            let mut params = UpmixerPluginParams::default();
            params.core.fft_size = 512;
            params.core.speaker_config = output_id.into();
            params.multichannel.input_layout = Some(input_layout);
            params.multichannel.bass_mode = mode;
            params.multichannel.combine_input_lfe = true;
            let mut plugin = UpmixerPlugin::try_from_params(params).unwrap();
            plugin.initialize(48_000.0).unwrap();
            let output_channels = plugin.output_channels();
            let frames = 257;
            let input: Vec<f32> = (0..frames * input_channels)
                .map(|index| ((index * 37 % 101) as f32 - 50.0) / 100.0)
                .collect();
            let mut output = vec![0.0; frames * output_channels];
            let context = ProcessContext::new(48_000, frames);
            let gain_id = ParameterId::from("height_gain");
            let bypass_id = ParameterId::from("bypass_heights");
            let surround_bypass_id = ParameterId::from("bypass_surround");
            let analysis_controls = [
                (
                    ParameterId::from("height_transient_reduction"),
                    ParameterValue::Float(0.4),
                ),
                (
                    ParameterId::from("height_direct_leak"),
                    ParameterValue::Float(0.1),
                ),
                (
                    ParameterId::from("ambient_boost"),
                    ParameterValue::Float(1.1),
                ),
                (
                    ParameterId::from("multi_source_threshold"),
                    ParameterValue::Float(0.4),
                ),
                (
                    ParameterId::from("safety_cap_db"),
                    ParameterValue::Float(2.0),
                ),
            ];
            assert_no_allocs_or_deallocs("surround first process and automation", || {
                plugin.process(&input, &mut output, &context).unwrap();
                plugin
                    .set_parameter(gain_id.clone(), ParameterValue::Float(0.5))
                    .unwrap();
                plugin
                    .set_parameter(bypass_id.clone(), ParameterValue::Bool(true))
                    .unwrap();
                plugin
                    .set_parameter(bypass_id.clone(), ParameterValue::Bool(false))
                    .unwrap();
                plugin
                    .set_parameter(surround_bypass_id.clone(), ParameterValue::Bool(true))
                    .unwrap();
                plugin
                    .set_parameter(surround_bypass_id.clone(), ParameterValue::Bool(false))
                    .unwrap();
                for (id, value) in &analysis_controls {
                    plugin.set_parameter(id.clone(), value.clone()).unwrap();
                }
                for _ in 0..20 {
                    plugin.process(&input, &mut output, &context).unwrap();
                }
            });
            let mut tail = vec![0.0; plugin.drain_output_frames_max() * output_channels];
            assert_no_allocs_or_deallocs("surround drain and reset", || {
                loop {
                    if plugin.drain(&mut tail, &context).unwrap().complete {
                        break;
                    }
                }
                plugin.reset();
            });
        }
    }
}
