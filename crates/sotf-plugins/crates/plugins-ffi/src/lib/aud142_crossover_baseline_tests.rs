//! Captures the current FFI runtime fallback for representative Crossover instances.

// Rust guideline compliant 2026-02-21
use crate::parameter_map::ParameterMap;
use sotf_host::parameters::ParameterId;
use sotf_host::plugin::Plugin;
use sotf_plugins::plugin_crossover::{CrossoverPlugin, CrossoverPluginParams, PerChannelOpMode};
use std::ffi::CStr;

fn cases() -> Vec<(&'static str, CrossoverPlugin, Vec<&'static str>)> {
    vec![
        (
            "lr24_two_way",
            CrossoverPlugin::new(2, "LR24", 1_000.0, "both").unwrap(),
            vec!["type", "frequency", "mode"],
        ),
        (
            "lr24_four_way",
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap(),
            vec!["type", "frequency", "mode", "frequency_2", "frequency_3"],
        ),
        (
            "fir_four_way",
            CrossoverPlugin::from_params(
                2,
                &CrossoverPluginParams {
                    crossover_type: "FIR".into(),
                    frequency: 700.0,
                    extra_frequencies: vec![1_800.0, 5_000.0],
                    output: "both".into(),
                    fir_taps: Some(1_025),
                    channel_frequencies_hz: Vec::new(),
                    channel_modes: Some(Vec::new()),
                    topology: None,
                    band_count: None,
                },
            )
            .unwrap(),
            vec![
                "type",
                "frequency",
                "mode",
                "frequency_2",
                "frequency_3",
                "fir_taps",
            ],
        ),
        (
            "lr24_per_channel",
            CrossoverPlugin::new_per_channel(
                "LR24",
                vec![900.0, 2_100.0],
                vec![PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
            )
            .unwrap(),
            vec![
                "type",
                "channel_frequency_0",
                "channel_mode_0",
                "channel_frequency_1",
                "channel_mode_1",
            ],
        ),
    ]
}

#[test]
fn current_crossover_ffi_metadata_order_and_values() {
    let mut report = String::new();
    for (case, plugin, expected_ids) in cases() {
        let parameters = plugin.parameters();
        let map = ParameterMap::from_plugin(&plugin, "Crossover");
        assert_eq!(map.count(), parameters.len(), "{case}");
        let ids: Vec<_> = (0..map.count())
            .map(|index| map.param_id_at(index).unwrap().to_owned())
            .collect();
        assert_eq!(ids, expected_ids, "{case}");
        report.push_str(&format!("case={case};count={}\n", map.count()));

        for index in 0..map.count() {
            let info = map.get_info(index).unwrap();
            // SAFETY: `ParameterMap` owns each leaked CString until it is dropped.
            let id = unsafe { CStr::from_ptr(info.id) }.to_str().unwrap();
            // SAFETY: `ParameterMap` owns each leaked CString until it is dropped.
            let name = unsafe { CStr::from_ptr(info.name) }.to_str().unwrap();
            // SAFETY: `ParameterMap` owns each leaked CString until it is dropped.
            let unit = unsafe { CStr::from_ptr(info.unit) }.to_str().unwrap();
            let raw = plugin.get_parameter(&ParameterId::from(id));
            let normalized = map.get_normalized(&plugin, id);
            report.push_str(&format!(
                "index={index};id={id};name={name};unit={unit};min={};max={};default={};steps={};logarithmic={};raw={raw:?};normalized={normalized:?}\n",
                info.min_value,
                info.max_value,
                info.default_value,
                info.steps,
                info.logarithmic,
            ));
        }
    }
    println!("AUD142_FFI_BASELINE_BEGIN\n{report}AUD142_FFI_BASELINE_END");
}
