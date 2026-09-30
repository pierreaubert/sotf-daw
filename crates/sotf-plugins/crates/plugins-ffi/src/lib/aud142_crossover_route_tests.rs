//! Public C-ABI compatibility checks for Crossover runtime metadata.

// Rust guideline compliant 2026-02-21
use crate::{
    ParameterInfo, PluginError, PluginHandle, plugin_create, plugin_destroy, plugin_get_last_error,
    plugin_get_parameter, plugin_get_parameter_choice_label, plugin_get_parameter_count,
    plugin_get_parameter_info, plugin_set_parameter,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

struct CrossoverHandle(*mut PluginHandle);

impl CrossoverHandle {
    fn new(config: &str, output_channels: usize) -> Self {
        let plugin_type = CString::new("Crossover").expect("static plugin type has no NUL");
        let config = CString::new(config).expect("test config has no NUL");
        let handle = plugin_create(
            plugin_type.as_ptr(),
            config.as_ptr(),
            48_000,
            2,
            output_channels,
        );
        assert!(
            !handle.is_null(),
            "Crossover creation failed: {}",
            c_string(plugin_get_last_error())
        );
        Self(handle)
    }

    fn count(&self) -> usize {
        usize::try_from(plugin_get_parameter_count(self.0)).expect("parameter count is nonnegative")
    }

    fn info(&self, index: usize) -> &ParameterInfo {
        let info = plugin_get_parameter_info(self.0, index);
        assert!(!info.is_null(), "parameter info at index {index} is null");
        // SAFETY: `plugin_get_parameter_info` returns a pointer owned by this live handle.
        unsafe { &*info }
    }

    fn id(&self, index: usize) -> String {
        c_string(self.info(index).id)
    }

    fn index_of(&self, id: &str) -> usize {
        (0..self.count())
            .find(|index| self.id(*index) == id)
            .unwrap_or_else(|| panic!("parameter id {id} is missing"))
    }

    fn choice(&self, parameter_index: usize, choice_index: usize) -> Option<String> {
        let label = plugin_get_parameter_choice_label(self.0, parameter_index, choice_index);
        (!label.is_null()).then(|| c_string(label))
    }

    fn get(&self, id: &str) -> f64 {
        let id = CString::new(id).expect("test parameter id has no NUL");
        plugin_get_parameter(self.0, id.as_ptr())
    }

    fn set(&mut self, id: &str, normalized: f64) -> i32 {
        let id = CString::new(id).expect("test parameter id has no NUL");
        plugin_set_parameter(self.0, id.as_ptr(), normalized)
    }
}

impl Drop for CrossoverHandle {
    fn drop(&mut self) {
        plugin_destroy(self.0);
    }
}

fn c_string(value: *const c_char) -> String {
    assert!(!value.is_null(), "expected a non-null C string");
    // SAFETY: the FFI returns a NUL-terminated string valid while the handle lives,
    // or a process-static choice label. Every call here occurs before handle drop.
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .expect("FFI string is UTF-8")
        .to_owned()
}

fn assert_ids(handle: &CrossoverHandle, expected: &[&str]) {
    let actual: Vec<_> = (0..handle.count()).map(|index| handle.id(index)).collect();
    assert_eq!(actual, expected);
}

fn assert_frequency_metadata(handle: &CrossoverHandle, id: &str) {
    let info = handle.info(handle.index_of(id));
    assert_eq!(info.min_value, 20.0, "{id} minimum");
    assert_eq!(info.max_value, 20_000.0, "{id} maximum");
    assert_eq!(info.steps, 0, "{id} remains continuous");
    assert!(
        !info.logarithmic,
        "{id} retains its linear normalized scale"
    );
}

fn assert_linear_frequency_probes(handle: &mut CrossoverHandle, id: &str) {
    assert_frequency_metadata(handle, id);
    for normalized in [0.25, 0.5, 0.75] {
        assert_eq!(
            handle.set(id, normalized),
            PluginError::Success as i32,
            "{id}"
        );
        assert!(
            (handle.get(id) - normalized).abs() < 1.0e-5,
            "{id} normalized value at {normalized}"
        );
    }
}

#[test]
fn runtime_metadata_keeps_topology_specific_ids_and_adds_string_choices() {
    let mut two_way =
        CrossoverHandle::new(r#"{"type":"LR24","frequency":1000.0,"output":"both"}"#, 4);
    assert_ids(&two_way, &["type", "frequency", "mode"]);
    let type_index = two_way.index_of("type");
    assert_eq!(two_way.info(type_index).min_value, 0.0);
    assert_eq!(two_way.info(type_index).max_value, 12.0);
    assert_eq!(two_way.info(type_index).default_value, 0.0);
    assert_eq!(two_way.info(type_index).steps, 12);
    for (index, family) in sotf_plugins::param_specs::crossover::CROSSOVER_TYPES
        .iter()
        .enumerate()
    {
        assert_eq!(two_way.choice(type_index, index).as_deref(), Some(*family));
    }
    assert_eq!(two_way.choice(type_index, 13), None);
    let mode_index = two_way.index_of("mode");
    assert_eq!(two_way.info(mode_index).steps, 2);
    assert_eq!(two_way.choice(mode_index, 0).as_deref(), Some("Lowpass"));
    assert_eq!(two_way.choice(mode_index, 1).as_deref(), Some("Highpass"));
    assert_eq!(two_way.choice(mode_index, 2).as_deref(), Some("Both"));
    assert!((two_way.get("mode") - 1.0).abs() < 1.0e-12);
    assert_linear_frequency_probes(&mut two_way, "frequency");
    let old_type_value = two_way.get("type");
    assert_ne!(two_way.set("type", 0.0), PluginError::Success as i32);
    assert_eq!(
        two_way.get("type"),
        old_type_value,
        "type remains structural"
    );

    let mut four_way = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":1000.0,"output":"both","extra_frequencies":[1800.0,19000.0]}"#,
        8,
    );
    assert_ids(
        &four_way,
        &["type", "frequency", "mode", "frequency_2", "frequency_3"],
    );
    assert_linear_frequency_probes(&mut four_way, "frequency_2");

    let mut four_way_last_cutoff = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":1000.0,"output":"both","extra_frequencies":[4000.0,19000.0]}"#,
        8,
    );
    assert_ids(
        &four_way_last_cutoff,
        &["type", "frequency", "mode", "frequency_2", "frequency_3"],
    );
    assert_linear_frequency_probes(&mut four_way_last_cutoff, "frequency_3");

    let fir = CrossoverHandle::new(
        r#"{"type":"LinearPhase","frequency":1000.0,"output":"both","extra_frequencies":[1800.0,19000.0],"fir_taps":1025}"#,
        8,
    );
    assert_ids(
        &fir,
        &[
            "type",
            "frequency",
            "mode",
            "frequency_2",
            "frequency_3",
            "fir_taps",
        ],
    );
    assert_frequency_metadata(&fir, "frequency");

    let per_channel = CrossoverHandle::new(
        r#"{"type":"LR24","frequency":1000.0,"output":"both","channel_frequencies_hz":[900.0,2100.0],"channel_modes":["lowpass","highpass"]}"#,
        2,
    );
    assert_ids(
        &per_channel,
        &[
            "type",
            "channel_frequency_0",
            "channel_mode_0",
            "channel_frequency_1",
            "channel_mode_1",
        ],
    );
    assert!((0..per_channel.count()).all(|index| per_channel.id(index) != "mode"));
    let channel_mode_index = per_channel.index_of("channel_mode_0");
    assert_eq!(per_channel.info(channel_mode_index).steps, 3);
    for (choice_index, label) in ["Lowpass", "Highpass", "Mute", "Passthrough"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            per_channel
                .choice(channel_mode_index, choice_index)
                .as_deref(),
            Some(label)
        );
    }
    assert_frequency_metadata(&per_channel, "channel_frequency_0");
    assert!(
        (per_channel.get("channel_mode_1") - 1.0 / 3.0).abs() < 1.0e-12,
        "canonical lowercase `highpass` maps to the displayed Highpass choice"
    );
}

#[test]
fn every_canonical_family_has_the_expected_normalized_runtime_value() {
    for (index, family) in sotf_plugins::param_specs::crossover::CROSSOVER_TYPES
        .iter()
        .enumerate()
    {
        let config = format!(r#"{{"type":{family:?},"frequency":1000.0,"output":"both"}}"#);
        let handle = CrossoverHandle::new(&config, 4);
        let mut expected_ids = vec!["type", "frequency", "mode"];
        if *family == "LinearPhase" {
            expected_ids.push("fir_taps");
        }
        assert_ids(&handle, &expected_ids);
        let type_index = handle.index_of("type");
        assert_eq!(
            handle.choice(type_index, index).as_deref(),
            Some(*family),
            "choice label for {family}"
        );
        assert_eq!(
            handle.info(type_index).default_value,
            index as f64,
            "runtime enum index for {family}"
        );
        assert!(
            (handle.get("type") - index as f64 / 12.0).abs() < 1.0e-12,
            "normalized runtime enum value for {family}"
        );
    }
}
