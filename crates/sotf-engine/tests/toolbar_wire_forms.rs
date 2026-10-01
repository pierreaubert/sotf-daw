//! Toolbar wire-form audit for every plugin choice parameter.
//!
//! Regression coverage for the `auto_gain_position: integer 2 vs string`
//! failure class. The systemwide toolbar sends `ParamSpec` choice controls
//! as integer indices, while several factory parameter structs declare the
//! same field as a string label. Any such mismatch breaks Add Plugin and
//! parameter edits from the UI.
//!
//! For every factory-supported plugin type this test:
//! 1. builds the exact default parameters the toolbar offers on Add Plugin
//!    (`PluginSettings::default_for(..).to_plugin_config(..)`),
//! 2. requires baseline factory creation to succeed,
//! 3. checks direct JSON forms where they are unambiguous, and sends
//!    Crossover band-count indices through the settings accessor because its
//!    stored counts share integer values with toolbar choice indices.

use sotf_audio::{PluginSettings, PluginType};
use sotf_plugins::factory::{create_plugin, supported_plugin_types};
use sotf_plugins::param_specs::ParamType;

const SAMPLE_RATE: u32 = 48_000;

/// Input channel count per plugin type: what the toolbar sends on Add Plugin
/// in a stereo system (the daemon's current channels, stereo here), except
/// for plugins with a fixed input geometry.
fn channels_for_type(plugin_type: &str) -> usize {
    match plugin_type {
        "mono_to_stereo" => 1,
        // Fixed-geometry plugins: audit with the channel count the toolbar
        // would actually carry for their required layout.
        "binaural_decoder" => 6,
        "ambisonics_decoder" => 4,
        _ => 2,
    }
}

/// Plugin types that cannot be instantiated from defaults alone, or that the
/// daemon deliberately hides from the toolbar Add list
/// (`handle_get_available_plugins` exclusion set: infrastructure and meter
/// plugins, not user-insertable effects). Skipped with a report, never
/// failed:
/// - `external`/`external_plugin` need live discovered resources;
/// - `fletcher_munson` is filtered out of the toolbar Add list and its
///   settings conversion is legacy-dead (old presets deserialize through
///   `FletcherMunsonCompat` directly, covered by the plugin's own tests).
/// - `resampler` is infrastructure excluded from the Add list; it has no
///   engine `PluginType`/settings mapping by design.
fn needs_live_resources(plugin_type: &str) -> bool {
    matches!(
        plugin_type,
        "external" | "external_plugin" | "fletcher_munson" | "resampler"
    )
}

/// Engine keys the plugin layout hides from the toolbar UI (the renderer
/// skips them entirely). The toolbar can never send these — e.g. compressor
/// legacy detector fields that remain serializable but have no DSP effect
/// and are rejected outright when explicitly present — so the wire-form
/// audit skips them instead of probing values the factory must reject.
fn layout_hidden_keys(settings: &PluginSettings) -> std::collections::HashSet<&'static str> {
    let Some(layout) = settings.layout() else {
        return std::collections::HashSet::new();
    };
    let specs = settings.param_specs();
    layout
        .config
        .iter()
        .chain(layout.output.iter())
        .chain(layout.main.iter().flat_map(|group| group.controls.iter()))
        .chain(layout.tabs.iter().flat_map(|tab| tab.controls.iter()))
        .filter(|control| control.hidden)
        .filter_map(|control| specs.get(control.param_index))
        .map(|spec| spec.engine_key)
        .collect()
}

#[test]
fn toolbar_choice_forms_survive_factory_create_for_all_plugins() {
    let mut failures = Vec::new();
    let mut audited_types = 0;
    let mut audited_fields = 0;

    for plugin_type in supported_plugin_types() {
        if needs_live_resources(plugin_type) {
            eprintln!("skipping {plugin_type}: needs live discovered resources");
            continue;
        }
        let Some(settings_type) = PluginType::from_wire_name(plugin_type) else {
            failures.push(format!("{plugin_type}: no PluginType for factory type"));
            continue;
        };
        let settings = match PluginSettings::default_for(&settings_type) {
            Ok(settings) => settings,
            Err(error) => {
                failures.push(format!("{plugin_type}: no default settings: {error}"));
                continue;
            }
        };
        let defaults = settings.to_plugin_config(f64::from(SAMPLE_RATE)).parameters;
        let channels = channels_for_type(plugin_type);

        if let Err(error) = create_plugin(plugin_type, &defaults, channels, SAMPLE_RATE) {
            failures.push(format!(
                "{plugin_type}: baseline defaults rejected: {error}"
            ));
            continue;
        }
        audited_types += 1;

        let hidden_keys = layout_hidden_keys(&settings);
        for (spec_index, spec) in settings.param_specs().iter().enumerate() {
            let ParamType::Choice { labels, .. } = spec.param_type else {
                continue;
            };
            if hidden_keys.contains(spec.engine_key) {
                eprintln!(
                    "skipping {plugin_type}.{}: hidden from toolbar layout",
                    spec.engine_key
                );
                continue;
            }
            audited_fields += 1;

            if plugin_type == "crossover" && spec.engine_key == "topology" {
                let Some(mode_index) = settings
                    .param_specs()
                    .iter()
                    .position(|candidate| candidate.engine_key == "mode")
                else {
                    failures.push("crossover: missing mode choice for topology audit".into());
                    continue;
                };
                for (index, label) in labels.iter().enumerate() {
                    let mut selected = settings.clone();
                    if let PluginSettings::Crossover {
                        channel_frequencies_hz,
                        channel_modes,
                        ..
                    } = &mut selected
                    {
                        *channel_frequencies_hz = Some(vec![500.0, 1_000.0]);
                        *channel_modes = Some(vec!["lowpass".into(), "highpass".into()]);
                    }
                    selected.set_param_value(spec_index, index as f64);
                    selected.set_param_value(mode_index, 2.0);
                    let config = selected.to_plugin_config(f64::from(SAMPLE_RATE));
                    let expected_topology = if index == 0 { "bands" } else { "per_channel" };
                    if config.parameters["topology"] != serde_json::json!(expected_topology) {
                        failures.push(format!(
                            "crossover.topology choice {index} ({label:?}) serialized as {}, expected {expected_topology:?}",
                            config.parameters["topology"]
                        ));
                        continue;
                    }
                    let expected_width = if index == 0 { 4 } else { 2 };
                    match create_plugin(
                        plugin_type,
                        &config.parameters,
                        channels,
                        SAMPLE_RATE,
                    ) {
                        Ok(plugin) if plugin.output_channels() == expected_width => {}
                        Ok(plugin) => failures.push(format!(
                            "crossover.topology choice {index} ({label:?}) produced {} output channels; expected {expected_width}",
                            plugin.output_channels(),
                        )),
                        Err(error) => failures.push(format!(
                            "crossover.topology choice {index} ({label:?}) rejected: {error}"
                        )),
                    }
                }
                continue;
            }

            // Crossover's stored `band_count` is the actual number of bands
            // (2, 3, or 4), while the toolbar sends its choice index (0, 1,
            // or 2). Exercise the real settings accessor that translates
            // between those representations instead of making the factory
            // guess whether a JSON integer is a count or an index.
            if plugin_type == "crossover" && spec.engine_key == "band_count" {
                let Some(mode_index) = settings
                    .param_specs()
                    .iter()
                    .position(|candidate| candidate.engine_key == "mode")
                else {
                    failures.push("crossover: missing mode choice for count-width audit".into());
                    continue;
                };
                for (index, label) in labels.iter().enumerate() {
                    let mut selected = settings.clone();
                    selected.set_param_value(spec_index, index as f64);
                    selected.set_param_value(mode_index, 2.0);
                    let config = selected.to_plugin_config(f64::from(SAMPLE_RATE));
                    let expected_count = index + 2;
                    if config.parameters["band_count"] != serde_json::json!(expected_count) {
                        failures.push(format!(
                            "crossover.band_count choice {index} ({label:?}) serialized as {}, expected {expected_count}",
                            config.parameters["band_count"]
                        ));
                        continue;
                    }
                    match create_plugin(
                        plugin_type,
                        &config.parameters,
                        channels,
                        SAMPLE_RATE,
                    ) {
                        Ok(plugin) if plugin.output_channels() == 2 * expected_count => {}
                        Ok(plugin) => failures.push(format!(
                            "crossover.band_count choice {index} ({label:?}) produced {} output channels; expected {}",
                            plugin.output_channels(),
                            2 * expected_count
                        )),
                        Err(error) => failures.push(format!(
                            "crossover.band_count choice {index} ({label:?}) rejected: {error}"
                        )),
                    }
                }
                continue;
            }

            // The toolbar sends whatever JSON type the daemon defaults carry
            // for this field, so report it: a failure only breaks the UI
            // when the rejected form matches the default's type.
            let default_form = match &defaults[spec.engine_key] {
                serde_json::Value::Number(_) => "int-default",
                serde_json::Value::String(_) => "string-default",
                serde_json::Value::Null => "absent-default",
                other => {
                    failures.push(format!(
                        "{plugin_type}.{}: unexpected default shape {other:?}",
                        spec.engine_key
                    ));
                    continue;
                }
            };
            for (index, label) in labels.iter().enumerate() {
                // Loudness Auto mode legitimately requires measured SPL
                // calibration; probe it in its valid configuration so the
                // audit tests the wire form, not the validation itself.
                let calibrated = plugin_type == "loudness_compensation"
                    && spec.engine_key == "mode"
                    && index == 2;
                let mut int_form = defaults.clone();
                int_form[spec.engine_key] = serde_json::json!(index);
                if calibrated {
                    int_form["auto_calibrated"] = serde_json::json!(true);
                }
                if let Err(error) = create_plugin(plugin_type, &int_form, channels, SAMPLE_RATE) {
                    failures.push(format!(
                        "{plugin_type}.{} [{default_form}]: index {index} rejected: {error}",
                        spec.engine_key
                    ));
                }
                let mut label_form = defaults.clone();
                label_form[spec.engine_key] = serde_json::json!(label);
                if calibrated {
                    label_form["auto_calibrated"] = serde_json::json!(true);
                }
                if let Err(error) = create_plugin(plugin_type, &label_form, channels, SAMPLE_RATE) {
                    failures.push(format!(
                        "{plugin_type}.{} [{default_form}]: label {label:?} rejected: {error}",
                        spec.engine_key
                    ));
                }
            }
        }
    }

    assert!(audited_types > 0, "audit covered no plugin types at all");
    assert!(
        failures.is_empty(),
        "{} toolbar wire-form failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("audited {audited_types} plugin types, {audited_fields} choice fields");
}
