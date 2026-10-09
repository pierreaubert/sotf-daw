use super::ExternalPlugin;
use super::external_hosting_backend::plan_external_plugin_hosting;
use super::external_hosting_backend::select_hosting_backend;
use super::external_plugin_state::ExternalPluginState;
use super::misc::EXTERNAL_PLUGIN_PRESET_ID;
use super::native_backend::{NativeExternalPluginBackend, NativePluginMetadata};
use super::plugin::plugin_format_capabilities;
use super::plugin_descriptor::PluginDescriptor;
use super::plugin_format::PluginFormat;
use super::plugin_scan_summary::PluginScanSummary;
use super::plugin_scanner::PluginScanner;
use super::types::ExternalHostingBackend;
use super::types::ExternalPluginSandboxMode;
use super::types::PluginScanStatus;
use super::types::PluginScanStatusMode;
use crate::assert_no_allocs;
use crate::error::PluginError;
use crate::parameters::{ParameterId, ParameterValue};
use crate::plugin::{Plugin, ProcessContext};
use crate::serialization::{PluginPreset, SerializablePlugin};
use std::fs;
use std::path::{Path, PathBuf};

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

fn unavailable_test_plugin(descriptor: &PluginDescriptor, sample_rate: u32) -> ExternalPlugin {
    ExternalPlugin {
        descriptor: descriptor.clone(),
        discovery_descriptor: descriptor.clone(),
        audio_setup: None,
        input_channels: descriptor.audio_inputs,
        output_channels: descriptor.audio_outputs.max(1),
        sample_rate: f64::from(sample_rate),
        max_block_frames: ExternalPlugin::DEFAULT_MAX_BLOCK_FRAMES,
        parameters: Vec::new(),
        hosting_backend: ExternalHostingBackend::Passthrough,
        restore_error: Some("intentional non-runnable test placeholder".to_string()),
        opaque_state: Vec::new(),
        native_backend: None,
        plugin_instance_id: None,
        editor_data: None,
        editor_dirty: false,
    }
}

struct NegotiatedMaxBackend {
    metadata: NativePluginMetadata,
    max_block_frames: usize,
}

struct EditorReadbackBackend {
    read_only: bool,
    metadata: NativePluginMetadata,
    value: f32,
    control_reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl NativeExternalPluginBackend for EditorReadbackBackend {
    fn metadata(&self) -> &NativePluginMetadata {
        &self.metadata
    }

    fn parameters(&self) -> Vec<crate::parameters::Parameter> {
        ["value", "complement", "unavailable"]
            .into_iter()
            .map(|id| {
                let mut parameter = crate::parameters::Parameter::new_float(id, id, 1.0, 0.0, 4.0);
                parameter.read_only = self.read_only && id == "value";
                parameter
            })
            .collect()
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.control_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match id.as_str() {
            "value" => Some(ParameterValue::Float(self.value)),
            "complement" => Some(ParameterValue::Float(4.0 - self.value)),
            _ => None,
        }
    }

    fn set_parameter(&mut self, id: &ParameterId, value: &ParameterValue) -> Result<(), String> {
        match (id.as_str(), value) {
            ("value", ParameterValue::Float(value)) if *value != 3.0 => {
                self.value = (value * 4.0).round() / 4.0;
                Ok(())
            }
            _ => Err("intentional native rejection".into()),
        }
    }

    fn save_state(&self) -> Result<Option<Vec<u8>>, String> {
        self.control_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.value == 3.5 {
            Err("intentional state capture failure".into())
        } else {
            Ok(Some(self.value.to_le_bytes().to_vec()))
        }
    }

    fn reset(&mut self) -> Result<(), String> {
        self.value = 2.0;
        Ok(())
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _: usize,
        _: usize,
        context: &ProcessContext,
    ) -> Result<(), String> {
        let samples = context.num_frames * 2;
        output[..samples].copy_from_slice(&input[..samples]);
        Ok(())
    }
}

fn editor_readback_plugin() -> (
    ExternalPlugin,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    editor_readback_plugin_with_permissions(false)
}

fn editor_readback_plugin_with_permissions(
    read_only: bool,
) -> (
    ExternalPlugin,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let descriptor = PluginDescriptor {
        id: "test.editor-readback".into(),
        name: "Editor Readback".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: PathBuf::from("/native/editor-readback.clap"),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Loadable,
    };
    let mut plugin = unavailable_test_plugin(&descriptor, 48_000);
    let control_reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let backend = EditorReadbackBackend {
        read_only,
        metadata: NativePluginMetadata {
            id: descriptor.id.clone(),
            name: descriptor.name.clone(),
            vendor: descriptor.vendor.clone(),
            version: descriptor.version.clone(),
            input_channels: 2,
            output_channels: 2,
        },
        value: 2.5,
        control_reads: control_reads.clone(),
    };
    plugin.parameters = backend.parameters();
    plugin.native_backend = Some(Box::new(backend));
    plugin.hosting_backend = ExternalHostingBackend::Clap;
    plugin.restore_error = None;
    (plugin, control_reads)
}

fn editor_snapshot(plugin: &ExternalPlugin) -> std::sync::Arc<super::ExternalPluginEditorData> {
    plugin.get_data().unwrap().downcast().unwrap()
}

#[test]
fn in_process_editor_uses_native_values_coupled_readback_and_immutable_snapshots() {
    let (mut plugin, _) = editor_readback_plugin();
    assert!(plugin.get_data().is_none());
    plugin.bind_editor_instance(17);
    let initial = editor_snapshot(&plugin);
    assert_eq!(initial.plugin_instance_id, Some(17));
    assert_eq!(
        initial.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(2.5)
    );
    assert!(
        !initial
            .parameter_values
            .contains_key(&ParameterId::from("unavailable"))
    );
    assert_eq!(
        initial.native_state.as_ref().unwrap().opaque_state,
        2.5_f32.to_le_bytes()
    );
    plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(1.13))
        .unwrap();
    assert!(plugin.get_data().is_none());
    plugin.refresh_control_thread_metadata();
    let accepted = editor_snapshot(&plugin);
    assert_eq!(
        accepted.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(1.25)
    );
    assert_eq!(
        accepted.parameter_values[&ParameterId::from("complement")],
        ParameterValue::Float(2.75)
    );
    assert_eq!(
        accepted.native_state.as_ref().unwrap().opaque_state,
        1.25_f32.to_le_bytes()
    );
    assert_eq!(
        initial.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(2.5)
    );
    assert_eq!(
        initial.native_state.as_ref().unwrap().opaque_state,
        2.5_f32.to_le_bytes()
    );
    assert!(
        plugin
            .set_parameter(ParameterId::from("value"), ParameterValue::Float(3.0))
            .is_err()
    );
    assert!(std::sync::Arc::ptr_eq(&accepted, &editor_snapshot(&plugin)));
    plugin.reset_checked().unwrap();
    assert!(plugin.get_data().is_none());
    plugin.refresh_control_thread_metadata();
    let reset = editor_snapshot(&plugin);
    assert_eq!(
        reset.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(2.0)
    );
    assert_eq!(
        reset.native_state.as_ref().unwrap().opaque_state,
        2.0_f32.to_le_bytes()
    );
    plugin.bind_editor_instance(99);
    assert_eq!(editor_snapshot(&plugin).plugin_instance_id, Some(99));
    assert_eq!(reset.plugin_instance_id, Some(17));
}

#[test]
fn in_process_editor_state_capture_failure_is_visible_without_overwriting_saved_state() {
    let (mut plugin, _) = editor_readback_plugin();
    plugin.bind_editor_instance(17);
    let before = plugin.placeholder_state();
    plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(3.5))
        .unwrap();
    assert!(plugin.get_data().is_none());
    plugin.refresh_control_thread_metadata();
    let failed = editor_snapshot(&plugin);
    assert_eq!(
        failed.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(3.5)
    );
    assert_eq!(
        failed.native_state.as_ref().unwrap_err(),
        "intentional state capture failure"
    );
    assert_eq!(plugin.placeholder_state(), before);
    plugin.reset_checked().unwrap();
    plugin.refresh_control_thread_metadata();
    assert!(editor_snapshot(&plugin).native_state.is_ok());
}

#[test]
fn in_process_editor_get_data_and_process_do_no_native_readback_or_allocation() {
    let (mut plugin, control_reads) = editor_readback_plugin();
    plugin.bind_editor_instance(17);
    let reads = control_reads.load(std::sync::atomic::Ordering::Relaxed);
    let input = [0.25_f32; 32];
    let mut output = [0.0_f32; 32];
    let context = ProcessContext::new(48_000, 16);
    assert_no_allocs("external cached editor and process", || {
        for _ in 0..8 {
            assert!(plugin.get_data().is_some());
            plugin.process(&input, &mut output, &context).unwrap();
        }
    });
    assert_eq!(
        control_reads.load(std::sync::atomic::Ordering::Relaxed),
        reads
    );
    assert_eq!(output, input);
    let id = ParameterId::from("value");
    assert_no_allocs("external realtime setter defers editor capture", || {
        plugin
            .set_parameter(id.clone(), ParameterValue::Float(1.0))
            .unwrap();
        assert!(plugin.get_data().is_none());
    });
    assert_eq!(
        control_reads.load(std::sync::atomic::Ordering::Relaxed),
        reads
    );
    plugin.refresh_control_thread_metadata();
    assert_eq!(
        editor_snapshot(&plugin).parameter_values[&id],
        ParameterValue::Float(1.0)
    );
}

#[test]
fn in_process_editor_host_immediate_refreshes_but_queued_events_do_not_capture_state() {
    use crate::host::{DawHost, Host};
    let (mut plugin, control_reads) = editor_readback_plugin();
    plugin.bind_editor_instance(17);
    let mut host = DawHost::new(2, 48_000);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.build().unwrap();
    host.set_plugin_parameter_immediate(0, "value", ParameterValue::Float(1.13))
        .unwrap();
    let accepted = host
        .get_plugin_data(0)
        .unwrap()
        .downcast::<super::ExternalPluginEditorData>()
        .unwrap();
    assert_eq!(
        accepted.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(1.25)
    );
    let input = [0.25_f32; 32];
    let mut output = [0.0_f32; 32];
    host.process(&input, &mut output).unwrap();
    let reads = control_reads.load(std::sync::atomic::Ordering::Relaxed);
    host.set_plugin_parameter(0, "value", ParameterValue::Float(2.13))
        .unwrap();
    assert_no_allocs(
        "external queued parameter defers native editor capture",
        || {
            host.process(&input, &mut output).unwrap();
        },
    );
    assert_eq!(
        control_reads.load(std::sync::atomic::Ordering::Relaxed),
        reads
    );
    assert!(
        host.get_plugin_data(0).is_none(),
        "queued edits must not publish stale editor state"
    );
    host.set_plugin_parameter_immediate(0, "value", ParameterValue::Float(2.13))
        .unwrap();
    let refreshed = host
        .get_plugin_data(0)
        .unwrap()
        .downcast::<super::ExternalPluginEditorData>()
        .unwrap();
    assert_eq!(
        refreshed.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(2.25)
    );
    assert_eq!(
        refreshed.parameter_values[&ParameterId::from("complement")],
        ParameterValue::Float(1.75)
    );
    assert_eq!(
        accepted.parameter_values[&ParameterId::from("value")],
        ParameterValue::Float(1.25)
    );
}

#[cfg(all(feature = "external-plugin-au", target_os = "macos"))]
fn local_au_editor_descriptor() -> PluginDescriptor {
    PluginDescriptor {
        id: std::env::var("SOTF_QA_AUDIO_UNIT_ID").expect("SOTF_QA_AUDIO_UNIT_ID is required"),
        path: PathBuf::from(
            std::env::var("SOTF_QA_AUDIO_UNIT_PATH").expect("SOTF_QA_AUDIO_UNIT_PATH is required"),
        ),
        name: "Local AU qualification".into(),
        vendor: "Local".into(),
        version: "1".into(),
        format: PluginFormat::AudioUnit,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Loadable,
    }
}

#[cfg(feature = "external-plugin-vst3")]
#[test]
#[ignore = "requires an explicitly selected local SOTF VST3 bundle"]
fn in_process_editor_local_vst3_toggle_and_restore() {
    let path =
        PathBuf::from(std::env::var("SOTF_QA_VST3_PATH").expect("SOTF_QA_VST3_PATH is required"));
    let mut scanner = PluginScanner::new();
    scanner.scan_path(&path, Some(PluginFormat::Vst3)).unwrap();
    let descriptor = scanner
        .list()
        .first()
        .expect("local VST3 was not discovered")
        .clone();
    assert_eq!(descriptor.path, path);
    // Filesystem discovery deliberately has no bus metadata. The actual native
    // probe supplies the descriptor required for a persistent editor instance.
    let probe = ExternalPlugin::new(&descriptor, 48_000).unwrap();
    let descriptor = probe.descriptor().clone();
    drop(probe);
    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).unwrap();
    plugin.bind_editor_instance(17);
    let before = editor_snapshot(&plugin);
    assert!(before.native_state.is_ok());
    let (id, original) = before
        .parameters
        .iter()
        .find_map(|parameter| {
            if parameter.read_only {
                return None;
            }
            match before.parameter_values.get(&parameter.id)? {
                ParameterValue::Bool(value) => Some((parameter.id.clone(), *value)),
                _ => None,
            }
        })
        .expect("selected local VST3 has no writable toggle");
    plugin
        .set_parameter(id.clone(), ParameterValue::Bool(!original))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&id),
        Some(ParameterValue::Bool(!original))
    );
    assert_eq!(before.parameter_values[&id], ParameterValue::Bool(original));

    let (numeric_id, numeric_target) = before
        .parameters
        .iter()
        .find_map(|parameter| {
            if parameter.read_only {
                return None;
            }
            match (
                before.parameter_values.get(&parameter.id)?,
                parameter.min_value.as_ref()?,
                parameter.max_value.as_ref()?,
            ) {
                (
                    ParameterValue::Float(current),
                    ParameterValue::Float(min),
                    ParameterValue::Float(max),
                ) if min.is_finite() && max.is_finite() && min < max => {
                    let candidate = min * 0.75 + max * 0.25;
                    let target = if candidate == *current {
                        min * 0.25 + max * 0.75
                    } else {
                        candidate
                    };
                    Some((parameter.id.clone(), ParameterValue::Float(target)))
                }
                _ => None,
            }
        })
        .expect("local VST3 exposes no writable bounded float");
    plugin
        .set_parameter(numeric_id.clone(), numeric_target)
        .unwrap();
    assert_ne!(
        plugin.get_parameter(&numeric_id),
        before.parameter_values.get(&numeric_id).cloned()
    );

    // Capture immediately, before processing, as the production control route
    // does. The envelope must preserve accepted edits while paused.
    plugin.refresh_control_thread_metadata();
    let immediate = editor_snapshot(&plugin);
    let saved_immediate = immediate.native_state.as_ref().unwrap();
    let mut immediate_restore =
        ExternalPlugin::from_placeholder_state(saved_immediate, 48_000).unwrap();
    immediate_restore.bind_editor_instance(19);
    let immediate_data = editor_snapshot(&immediate_restore);
    for parameter in immediate
        .parameters
        .iter()
        .filter(|parameter| !parameter.read_only)
    {
        assert_eq!(
            immediate_data.parameter_values.get(&parameter.id),
            immediate.parameter_values.get(&parameter.id),
            "immediate VST3 restore differs for {}",
            parameter.name
        );
    }
    drop(immediate_restore);

    // Deliver the queued controller edit to the native processor before saving
    // processor state. This is an ephemeral silent instance, not the audio route.
    let context = ProcessContext::new(48_000, 64);
    let input = vec![0.0; plugin.input_channels() * 64];
    let mut output = vec![0.0; plugin.output_channels() * 64];
    assert_no_allocs("local VST3 pending processor delivery", || {
        plugin.process(&input, &mut output, &context).unwrap();
    });
    assert!(
        plugin.get_data().is_none(),
        "processor delivery must invalidate cached native state"
    );
    assert!(output.iter().all(|value| value.is_finite()));
    plugin.refresh_control_thread_metadata();
    let after = editor_snapshot(&plugin);
    assert_eq!(after.parameter_values[&id], ParameterValue::Bool(!original));
    let saved = after.native_state.as_ref().unwrap();
    assert!(!saved.opaque_state.is_empty());
    assert_ne!(
        saved.opaque_state, saved_immediate.opaque_state,
        "delivered edits must recapture actual processor state"
    );
    let mut restored = ExternalPlugin::from_placeholder_state(saved, 48_000).unwrap();
    restored.bind_editor_instance(18);
    let restored_data = editor_snapshot(&restored);
    for parameter in after
        .parameters
        .iter()
        .filter(|parameter| !parameter.read_only)
    {
        assert_eq!(
            restored_data.parameter_values.get(&parameter.id),
            after.parameter_values.get(&parameter.id),
            "native VST3 restore differs for {}",
            parameter.name
        );
    }
    assert_no_allocs("local VST3 cached editor readback", || {
        for _ in 0..8 {
            let snapshot = editor_snapshot(&plugin);
            assert_eq!(
                snapshot.parameter_values[&id],
                ParameterValue::Bool(!original)
            );
        }
    });
    eprintln!(
        "Local VST3: {} parameters; toggle {}: {} -> {}; native state restored",
        after.parameters.len(),
        id,
        original,
        !original
    );
}

#[cfg(all(feature = "external-plugin-au", target_os = "macos"))]
#[test]
#[ignore = "requires an explicitly selected local Audio Unit"]
fn in_process_editor_local_au_edit_and_restore() {
    let descriptor = local_au_editor_descriptor();
    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).unwrap();
    plugin.bind_editor_instance(17);
    let before = editor_snapshot(&plugin);
    assert!(before.native_state.is_ok());
    assert!(
        !before.parameters.is_empty(),
        "local AU exposes no parameter definitions"
    );
    let readonly_count = before
        .parameters
        .iter()
        .filter(|parameter| parameter.read_only)
        .count();
    if let Ok(expected) = std::env::var("SOTF_QA_AU_EXPECT_READONLY_COUNT") {
        assert_eq!(
            readonly_count,
            expected.parse::<usize>().unwrap(),
            "native readable meter discovery changed"
        );
    }
    for parameter in before
        .parameters
        .iter()
        .filter(|parameter| parameter.read_only)
    {
        let actual = before
            .parameter_values
            .get(&parameter.id)
            .expect("readable native meter has no actual value")
            .clone();
        let error = plugin
            .set_parameter(parameter.id.clone(), actual)
            .unwrap_err();
        assert!(error.contains("read-only"), "{}", parameter.name);
        assert!(std::sync::Arc::ptr_eq(&before, &editor_snapshot(&plugin)));
    }
    let (id, target) = before
        .parameters
        .iter()
        .find_map(|parameter| {
            if parameter.read_only {
                return None;
            }
            let current = before.parameter_values.get(&parameter.id)?;
            let target = match (current, &parameter.min_value, &parameter.max_value) {
                (
                    ParameterValue::Float(current),
                    Some(ParameterValue::Float(min)),
                    Some(ParameterValue::Float(max)),
                ) if min.is_finite() && max.is_finite() && min < max => {
                    let candidate = min * 0.75 + max * 0.25;
                    ParameterValue::Float(if candidate == *current {
                        min * 0.25 + max * 0.75
                    } else {
                        candidate
                    })
                }
                (
                    ParameterValue::Int(current),
                    Some(ParameterValue::Int(min)),
                    Some(ParameterValue::Int(max)),
                ) if min < max => ParameterValue::Int(if current == min { *max } else { *min }),
                (ParameterValue::Bool(current), _, _) => ParameterValue::Bool(!current),
                _ => return None,
            };
            Some((parameter.id.clone(), target))
        })
        .expect("local AU exposes no editable numeric or boolean parameter");
    let original = before.parameter_values[&id].clone();
    plugin.set_parameter(id.clone(), target).unwrap();
    plugin.refresh_control_thread_metadata();
    let after = editor_snapshot(&plugin);
    let actual = plugin.get_parameter(&id).unwrap();
    assert_eq!(after.parameter_values[&id], actual);
    assert_ne!(actual, original, "native AU did not accept a changed value");
    assert_eq!(before.parameter_values[&id], original);
    let saved = after.native_state.as_ref().unwrap();
    assert!(
        !saved.opaque_state.is_empty(),
        "local AU does not persist native state"
    );
    let mut restored = ExternalPlugin::from_placeholder_state(saved, 48_000).unwrap();
    restored.bind_editor_instance(18);
    let restored_data = editor_snapshot(&restored);
    assert_eq!(restored_data.plugin_instance_id, Some(18));
    assert_eq!(restored_data.parameter_values, after.parameter_values);
    assert_no_allocs("local AU cached editor readback", || {
        for _ in 0..8 {
            assert!(restored.get_data().is_some());
        }
    });
    eprintln!(
        "Local AU {}: {} parameters, {readonly_count} readable read-only values (writes rejected without invalidating snapshot); edited {id}: {original:?} -> {actual:?}; restored all actual values",
        descriptor.id,
        after.parameters.len()
    );
}

impl NativeExternalPluginBackend for NegotiatedMaxBackend {
    fn metadata(&self) -> &NativePluginMetadata {
        &self.metadata
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        input_channels: usize,
        output_channels: usize,
        context: &ProcessContext,
    ) -> Result<(), String> {
        if context.num_frames > self.max_block_frames {
            return Err("negotiated maximum exceeded".into());
        }
        let input_samples = context.num_frames * input_channels;
        let output_samples = context.num_frames * output_channels;
        output[..output_samples].fill(0.0);
        let copied = input_samples.min(output_samples);
        output[..copied].copy_from_slice(&input[..copied]);
        Ok(())
    }
}

#[test]
fn negotiated_maximum_native_block_is_allocation_free() {
    let max_block_frames = 8_192;
    let descriptor = PluginDescriptor {
        id: "test.negotiated-max".into(),
        name: "Negotiated Max".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: PathBuf::from("/tmp/negotiated-max.clap"),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };
    let metadata = NativePluginMetadata {
        id: descriptor.id.clone(),
        name: descriptor.name.clone(),
        vendor: descriptor.vendor.clone(),
        version: descriptor.version.clone(),
        input_channels: 2,
        output_channels: 2,
    };
    let mut plugin = ExternalPlugin {
        discovery_descriptor: descriptor.clone(),
        descriptor,
        audio_setup: None,
        input_channels: 2,
        output_channels: 2,
        sample_rate: 48_000.0,
        max_block_frames,
        parameters: Vec::new(),
        hosting_backend: ExternalHostingBackend::Clap,
        restore_error: None,
        opaque_state: Vec::new(),
        native_backend: Some(Box::new(NegotiatedMaxBackend {
            metadata,
            max_block_frames,
        })),
        plugin_instance_id: None,
        editor_data: None,
        editor_dirty: false,
    };
    let input = vec![0.25_f32; max_block_frames * 2];
    let mut output = vec![0.0_f32; max_block_frames * 2];
    let context = ProcessContext::new(48_000, max_block_frames);
    plugin.process(&input, &mut output, &context).unwrap();
    assert_no_allocs("external native negotiated maximum", || {
        for _ in 0..8 {
            plugin.process(&input, &mut output, &context).unwrap();
        }
    });
    assert_eq!(output, input);
}

#[test]
fn test_plugin_scanner_search_paths() {
    // Verify search paths are non-empty for at least one format
    let paths = PluginScanner::search_paths(PluginFormat::Clap);
    assert!(!paths.is_empty(), "Should have CLAP search paths");
}

#[test]
fn test_plugin_scanner_scan_nonexistent() {
    let mut scanner = PluginScanner::new();
    scanner.scan_directory(Path::new("/nonexistent/path"), PluginFormat::Clap);
    assert!(scanner.plugins.is_empty());
}

#[test]
fn test_plugin_scanner_scan_path_single_bundle() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("scan-path-single.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let mut scanner = PluginScanner::new();

    scanner.scan_path(&plugin_path, None).unwrap();

    assert_eq!(scanner.plugins.len(), 1);
    assert_eq!(scanner.plugins[0].format, PluginFormat::Clap);
    assert_eq!(scanner.plugins[0].name, "scan-path-single");
    assert_eq!(scanner.plugins[0].audio_inputs, 0);
    assert_eq!(scanner.plugins[0].audio_outputs, 0);
}

#[test]
fn test_plugin_scanner_scan_path_directory_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("recursive-test.vst3"), b"stub plugin").unwrap();
    let mut scanner = PluginScanner::new();

    scanner.scan_path(dir.path(), None).unwrap();

    assert_eq!(scanner.plugins.len(), 1);
    assert_eq!(scanner.plugins[0].format, PluginFormat::Vst3);
    assert_eq!(scanner.plugins[0].name, "recursive-test");
}

#[test]
fn test_plugin_scanner_scan_path_rejects_format_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let plugin_path = dir.path().join("mismatch.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let mut scanner = PluginScanner::new();

    let err = scanner
        .scan_path(&plugin_path, Some(PluginFormat::Vst3))
        .unwrap_err();

    assert!(err.contains("not Vst3"));
    assert!(scanner.plugins.is_empty());
}

#[test]
fn test_external_plugin_non_runnable_placeholder_rejects_processing() {
    let mut tmp_path = env::temp_dir();
    tmp_path.push(format!(
        "sotf-external-plugin-fake-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp_path).unwrap();
    let plugin_path = tmp_path.join("fake.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();

    let desc = PluginDescriptor {
        id: "test.plugin".into(),
        name: "Test Plugin".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };

    let mut plugin = unavailable_test_plugin(&desc, 48_000);
    let input = vec![0.5f32; 2048];
    let mut output = vec![0.0f32; 2048];
    let ctx = ProcessContext::new(48000, 1024);

    let error = plugin.process(&input, &mut output, &ctx).unwrap_err();
    assert!(error.contains("cannot process without a native backend"));

    fs::remove_file(plugin_path).unwrap();
    fs::remove_dir_all(tmp_path).unwrap();
}

#[test]
fn test_external_plugin_new_never_silently_bypasses_unavailable_backend() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unavailable.clap");
    fs::write(&path, b"not a native plugin").unwrap();
    let descriptor = PluginDescriptor {
        id: "test.unavailable".into(),
        name: "Unavailable".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };

    let error = match ExternalPlugin::new(&descriptor, 48_000) {
        Ok(_) => panic!("unavailable backend must not become a runnable bypass"),
        Err(error) => error,
    };
    assert!(!error.is_empty());
}

#[test]
fn test_external_plugin_rejects_invalid_negotiated_block_contract() {
    let descriptor = PluginDescriptor {
        id: "test.block-contract".into(),
        name: "Block Contract".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: PathBuf::from("/tmp/block-contract.clap"),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };

    let error = match ExternalPlugin::new_with_max_block_frames(&descriptor, 48_000, 0) {
        Ok(_) => panic!("zero-sized block contract must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("maximum block frame count must be positive"));
}

#[test]
fn test_external_plugin_scan_recursive_and_dedup() {
    let root = env::temp_dir().join(format!(
        "sotf-external-plugin-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let nested = root.join("nested");
    let plugin_file = nested.join("my-plugin.clap");

    fs::create_dir_all(&nested).unwrap();
    fs::write(&plugin_file, b"stub").unwrap();

    let mut scanner = PluginScanner::new();
    scanner.scan_directory(&root, PluginFormat::Clap);
    assert_eq!(scanner.plugins.len(), 1);
    assert_eq!(scanner.plugins[0].name, "my-plugin");
    assert_eq!(
        scanner.plugins[0].scan_status,
        PluginFormat::Clap.build_scan_status()
    );
    scanner.scan_directory(&root, PluginFormat::Clap);
    assert_eq!(scanner.plugins.len(), 1);

    fs::remove_file(&plugin_file).unwrap();
    fs::remove_dir_all(&nested).unwrap();
    fs::remove_dir_all(&root).unwrap_or(());
}

#[test]
fn test_external_plugin_scanner_can_preserve_discovered_status() {
    let root = env::temp_dir().join(format!(
        "sotf-external-plugin-discovered-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let plugin_file = root.join("raw-discovery.clap");

    fs::create_dir_all(&root).unwrap();
    fs::write(&plugin_file, b"stub").unwrap();

    let mut scanner = PluginScanner::with_scan_status_mode(PluginScanStatusMode::DiscoveryOnly);
    scanner.scan_directory(&root, PluginFormat::Clap);

    assert_eq!(scanner.plugins.len(), 1);
    assert_eq!(scanner.plugins[0].scan_status, PluginScanStatus::Discovered);

    fs::remove_file(&plugin_file).unwrap();
    fs::remove_dir_all(&root).unwrap_or(());
}

#[test]
fn test_external_plugin_scan_summary_counts_statuses() {
    let descriptor = |id: &str, status: PluginScanStatus| PluginDescriptor {
        id: id.into(),
        name: id.into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: PathBuf::from(format!("/tmp/{id}.clap")),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: status,
    };
    let mut scanner = PluginScanner::new();
    scanner.plugins.push(descriptor(
        "discovered.plugin",
        PluginScanStatus::Discovered,
    ));
    scanner
        .plugins
        .push(descriptor("loadable.plugin", PluginScanStatus::Loadable));
    scanner.plugins.push(descriptor(
        "unsupported.plugin",
        PluginScanStatus::UnsupportedByBuild,
    ));

    let summary = scanner.summary();

    assert_eq!(
        summary,
        PluginScanSummary {
            total: 3,
            discovered: 1,
            loadable: 1,
            unsupported_by_build: 1,
        }
    );
}

#[test]
fn test_external_plugin_capability_matrix_reports_build_support() {
    let matrix = plugin_format_capabilities();
    assert_eq!(matrix.len(), 3);
    let clap = matrix
        .iter()
        .find(|capability| capability.format == PluginFormat::Clap)
        .unwrap();
    assert_eq!(clap.feature, "external-plugin-clap");
    assert_eq!(clap.scan_status, PluginFormat::Clap.build_scan_status());
    assert_eq!(clap.backend, select_hosting_backend(PluginFormat::Clap));
    assert_eq!(
        clap.native_backend_available,
        clap.backend != ExternalHostingBackend::Passthrough
    );
    if clap.native_backend_available {
        assert_eq!(clap.reason, None);
    } else {
        assert!(
            clap.reason
                .as_deref()
                .unwrap()
                .contains("unsupported-by-build")
        );
    }
}

#[test]
fn test_external_plugin_hosting_plan_reports_feature_gate() {
    let desc = PluginDescriptor {
        id: "planned.plugin".into(),
        name: "Planned Plugin".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: PathBuf::from("/tmp/planned-plugin.clap"),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };

    let plan = plan_external_plugin_hosting(&desc);

    assert_eq!(plan.format, PluginFormat::Clap);
    assert_eq!(plan.feature, "external-plugin-clap");
    assert_eq!(plan.scan_status, PluginFormat::Clap.build_scan_status());
    assert_eq!(plan.backend, select_hosting_backend(PluginFormat::Clap));
    if plan.backend == ExternalHostingBackend::Passthrough {
        assert!(!plan.native_backend_available);
        assert!(
            plan.reason
                .as_deref()
                .unwrap()
                .contains("cannot be added to a runnable graph")
        );
    } else {
        assert!(plan.native_backend_available);
        assert_eq!(plan.reason, None);
    }
}

#[test]
fn test_external_plugin_set_parameter_unknown() {
    let mut tmp_path = env::temp_dir();
    tmp_path.push(format!(
        "sotf-external-plugin-setparam-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp_path).unwrap();
    let plugin_path = tmp_path.join("fake.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let desc = PluginDescriptor {
        id: "test.plugin".into(),
        name: "Test Plugin".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };

    let mut plugin = unavailable_test_plugin(&desc, 48_000);
    let result = plugin.set_parameter(ParameterId::from("unknown"), ParameterValue::Float(1.0));
    assert!(result.is_err());

    fs::remove_file(plugin_path).unwrap();
    fs::remove_dir_all(tmp_path).unwrap();
}

#[test]
fn test_external_plugin_placeholder_state_round_trips() {
    let tmp_path = env::temp_dir().join(format!(
        "sotf-external-plugin-state-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp_path).unwrap();
    let plugin_path = tmp_path.join("state-test.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let desc = PluginDescriptor {
        id: "test.state".into(),
        name: "State Test".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["state".into()],
        scan_status: PluginScanStatus::Discovered,
    };
    let plugin = unavailable_test_plugin(&desc, 48_000);
    let mut state = plugin.placeholder_state();
    state.opaque_state = vec![1, 2, 3, 4];

    let json = serde_json::to_string(&state).unwrap();
    let decoded: ExternalPluginState = serde_json::from_str(&json).unwrap();
    let restore_error = match ExternalPlugin::from_placeholder_state(&decoded, 48_000) {
        Ok(_) => panic!("stub plugin must not restore as a runnable processor"),
        Err(error) => error,
    };

    assert_eq!(decoded, state);
    assert!(!restore_error.is_empty());
    assert_eq!(decoded.sandbox_mode, ExternalPluginSandboxMode::InProcess);
    assert_eq!(decoded.opaque_state, vec![1, 2, 3, 4]);

    let mut incompatible = decoded;
    incompatible.sandbox_mode = ExternalPluginSandboxMode::Isolated;
    let error = ExternalPlugin::from_placeholder_state(&incompatible, 48_000)
        .err()
        .expect("isolated state must not restore in process");
    assert!(error.contains("cannot restore in-process plugin"));

    fs::remove_file(plugin_path).unwrap();
    fs::remove_dir_all(tmp_path).unwrap();
}

#[test]
fn test_external_plugin_placeholder_state_rejects_missing_plugin() {
    let missing_path = env::temp_dir().join(format!(
        "sotf-external-plugin-missing-{}.clap",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let desc = PluginDescriptor {
        id: "test.missing".into(),
        name: "Missing Test".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: missing_path,
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["state".into()],
        scan_status: PluginScanStatus::Discovered,
    };
    let state = ExternalPluginState::new(
        desc.clone(),
        ExternalPluginSandboxMode::InProcess,
        vec![1, 2, 3],
    );

    let error = match ExternalPlugin::from_placeholder_state(&state, 48_000) {
        Ok(_) => panic!("missing plugin must not restore as a runnable processor"),
        Err(error) => error,
    };
    assert!(
        error.contains("plugin path does not exist") || error.contains("native hosting feature")
    );
}

#[test]
fn test_external_plugin_serializable_preset_round_trips_placeholder_state() {
    let tmp_path = env::temp_dir().join(format!(
        "sotf-external-plugin-serializable-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp_path).unwrap();
    let plugin_path = tmp_path.join("serializable.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let desc = PluginDescriptor {
        id: "test.serializable".into(),
        name: "Serializable Test".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec!["state".into()],
        scan_status: PluginScanStatus::Discovered,
    };
    let mut plugin = unavailable_test_plugin(&desc, 48_000);

    let preset = SerializablePlugin::serialize(&plugin).unwrap();
    let restored_state = preset.external_plugin_state().unwrap().unwrap();

    assert_eq!(preset.plugin_id, EXTERNAL_PLUGIN_PRESET_ID);
    assert_eq!(restored_state.descriptor, desc);
    assert_eq!(
        restored_state.sandbox_mode,
        ExternalPluginSandboxMode::InProcess
    );
    assert!(restored_state.opaque_state.is_empty());
    assert!(SerializablePlugin::deserialize(&mut plugin, &preset).is_err());

    let mut isolated_state = restored_state;
    isolated_state.sandbox_mode = ExternalPluginSandboxMode::Isolated;
    let mut incompatible_preset = preset.clone();
    incompatible_preset
        .set_external_plugin_state(&isolated_state)
        .unwrap();
    let error = SerializablePlugin::deserialize(&mut plugin, &incompatible_preset)
        .expect_err("isolated preset must not restore into in-process plugin");
    assert!(
        error
            .to_string()
            .contains("cannot restore in-process plugin")
    );

    fs::remove_file(plugin_path).unwrap();
    fs::remove_dir_all(tmp_path).unwrap();
}

#[test]
fn test_external_plugin_deserialize_rejects_different_descriptor() {
    let tmp_path = env::temp_dir().join(format!(
        "sotf-external-plugin-mismatch-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&tmp_path).unwrap();
    let plugin_path = tmp_path.join("mismatch.clap");
    fs::write(&plugin_path, b"stub plugin").unwrap();
    let desc = PluginDescriptor {
        id: "test.mismatch".into(),
        name: "Mismatch Test".into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        format: PluginFormat::Clap,
        path: plugin_path.clone(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: vec![],
        scan_status: PluginScanStatus::Discovered,
    };
    let mut plugin = unavailable_test_plugin(&desc, 48_000);
    let mut state = plugin.placeholder_state();
    state.plugin_id = "other.plugin".into();
    state.descriptor.id = "other.plugin".into();

    let mut preset = PluginPreset::new(
        "Other".into(),
        EXTERNAL_PLUGIN_PRESET_ID.into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    preset.set_external_plugin_state(&state).unwrap();

    assert!(matches!(
        SerializablePlugin::deserialize(&mut plugin, &preset),
        Err(PluginError::InvalidConfiguration(_))
    ));

    fs::remove_file(plugin_path).unwrap();
    fs::remove_dir_all(tmp_path).unwrap();
}

#[test]
fn test_plugin_format_extension() {
    assert_eq!(PluginFormat::Clap.extension(), "clap");
    assert_eq!(PluginFormat::Vst3.extension(), "vst3");
    assert_eq!(PluginFormat::AudioUnit.extension(), "component");
}

#[test]
fn test_external_plugin_backend_selection_is_feature_gated() {
    assert_eq!(
        select_hosting_backend(PluginFormat::Clap),
        if cfg!(feature = "external-plugin-clap") {
            ExternalHostingBackend::Clap
        } else {
            ExternalHostingBackend::Passthrough
        }
    );
    assert_eq!(
        select_hosting_backend(PluginFormat::Vst3),
        if cfg!(feature = "external-plugin-vst3") {
            ExternalHostingBackend::Vst3
        } else {
            ExternalHostingBackend::Passthrough
        }
    );
    assert_eq!(
        select_hosting_backend(PluginFormat::AudioUnit),
        if cfg!(feature = "external-plugin-au") {
            ExternalHostingBackend::AudioUnit
        } else {
            ExternalHostingBackend::Passthrough
        }
    );
}

#[test]
fn in_process_read_only_edit_preserves_snapshot_and_never_calls_native_setter() {
    let (mut plugin, reads) = editor_readback_plugin_with_permissions(true);
    plugin.bind_editor_instance(17);
    let before = editor_snapshot(&plugin);
    let count = reads.load(std::sync::atomic::Ordering::Relaxed);
    let error = plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(1.25))
        .unwrap_err();
    assert!(error.contains("read-only"));
    assert!(std::sync::Arc::ptr_eq(&before, &editor_snapshot(&plugin)));
    assert_eq!(reads.load(std::sync::atomic::Ordering::Relaxed), count);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("value")),
        Some(ParameterValue::Float(2.5))
    );
}

#[cfg(all(feature = "external-plugin-au", target_os = "macos"))]
#[test]
#[ignore = "requires a local Audio Unit with native indexed value strings"]
fn local_au_native_choices_edit_and_restore() {
    let descriptor = local_au_editor_descriptor();
    let mut plugin = ExternalPlugin::new(&descriptor, 48_000).unwrap();
    plugin.bind_editor_instance(17);
    let before = editor_snapshot(&plugin);
    let parameter = before
        .parameters
        .iter()
        .find(|parameter| !parameter.read_only && parameter.choices.len() > 1)
        .unwrap_or_else(|| {
            panic!(
                "{} exposes no labeled writable choice: {:?}",
                descriptor.id,
                before
                    .parameters
                    .iter()
                    .map(|parameter| (
                        &parameter.id,
                        &parameter.name,
                        &parameter.choices,
                        parameter.step,
                        parameter.read_only
                    ))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(parameter.step, Some(1.0));
    let current = before.parameter_values.get(&parameter.id).unwrap();
    let choice = parameter
        .choices
        .iter()
        .find(|choice| &choice.value != current)
        .unwrap();
    let id = parameter.id.clone();
    let target = choice.value.clone();
    let label = choice.label.clone();
    plugin.set_parameter(id.clone(), target.clone()).unwrap();
    plugin.refresh_control_thread_metadata();
    let after = editor_snapshot(&plugin);
    assert_eq!(after.parameter_values[&id], target);
    assert_eq!(
        after
            .parameters
            .iter()
            .find(|parameter| parameter.id == id)
            .unwrap()
            .choices
            .iter()
            .find(|choice| choice.value == target)
            .unwrap()
            .label,
        label
    );
    let native = after.native_state.as_ref().unwrap();
    assert!(!native.opaque_state.is_empty());
    let mut restored = ExternalPlugin::from_placeholder_state(native, 48_000).unwrap();
    restored.bind_editor_instance(18);
    let restored_data = editor_snapshot(&restored);
    for parameter in after
        .parameters
        .iter()
        .filter(|parameter| !parameter.read_only)
    {
        assert_eq!(
            restored_data.parameter_values.get(&parameter.id),
            after.parameter_values.get(&parameter.id),
            "{}",
            parameter.name
        );
    }
    assert_eq!(
        serde_json::to_value(&after.parameters).unwrap(),
        serde_json::to_value(&restored_data.parameters).unwrap()
    );
    assert_no_allocs("local AU choice cached editor data", || {
        for _ in 0..8 {
            assert!(restored.get_data().is_some());
        }
    });
    eprintln!(
        "Local AU {}: {} parameters, {} labeled choices on {id}; selected {label:?} ({target:?}), restored all writable values and metadata",
        descriptor.id,
        after.parameters.len(),
        parameter.choices.len()
    );
}
