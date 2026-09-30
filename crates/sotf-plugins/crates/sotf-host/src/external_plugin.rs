use crate::error::PluginError;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginInfo, PluginResult, ProcessContext,
    TailLength,
};
use crate::serialization::{PluginPreset, SerializablePlugin};
use std::collections::HashMap;

/// Reserved engine-config parameter used to correlate a hosted worker with
/// the persisted player plugin instance that created it.
pub const EXTERNAL_PLUGIN_INSTANCE_ID_PARAMETER: &str = "_sotf_instance_id";
#[cfg(all(feature = "external-plugin-au", target_os = "macos"))]
mod au_backend;
#[cfg(feature = "external-plugin-clap")]
mod clap_backend;
mod external_hosting_backend;
mod external_plugin_state;
mod format;
mod load;
mod misc;
mod native_backend;
mod plugin;
mod plugin_descriptor;
mod plugin_descriptor_probe_cache;
mod plugin_format;
mod plugin_scan_summary;
mod plugin_scanner;
#[cfg(test)]
mod tests;
mod types;
#[cfg(feature = "external-plugin-vst3")]
mod vst3_backend;

pub use external_hosting_backend::*;
pub use external_plugin_state::*;
pub use misc::*;
pub use plugin::*;
pub use plugin_descriptor::*;
pub use plugin_descriptor_probe_cache::*;
pub use plugin_format::*;
pub use plugin_scan_summary::*;
pub use plugin_scanner::*;
pub use types::*;

use external_hosting_backend::try_load_dynamic_backend;
use native_backend::{NativeAmbisonicsControls, NativeExternalPluginBackend, NativePluginMetadata};

pub struct ExternalPlugin {
    descriptor: PluginDescriptor,
    discovery_descriptor: PluginDescriptor,
    audio_setup: Option<NativePluginAudioSetup>,
    input_channels: usize,
    output_channels: usize,
    sample_rate: u32,
    max_block_frames: usize,
    parameters: Vec<Parameter>,
    hosting_backend: ExternalHostingBackend,
    restore_error: Option<String>,
    opaque_state: Vec<u8>,
    native_backend: Option<Box<dyn NativeExternalPluginBackend>>,
}

impl ExternalPlugin {
    pub const DEFAULT_MAX_BLOCK_FRAMES: usize = 8192;
    /// Create a new external plugin wrapper from a descriptor.
    ///
    /// Native backend selection is feature-gated by format:
    /// - CLAP: `external-plugin-clap`
    /// - VST3: `external-plugin-vst3`
    /// - AU: `external-plugin-au`
    ///
    /// A missing format backend is a construction error. A runnable graph must
    /// never silently replace an external processor with dry passthrough.
    pub fn new(descriptor: &PluginDescriptor, sample_rate: u32) -> Result<Self, String> {
        Self::new_with_max_block_frames(descriptor, sample_rate, Self::DEFAULT_MAX_BLOCK_FRAMES)
    }

    pub fn new_with_max_block_frames(
        descriptor: &PluginDescriptor,
        sample_rate: u32,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        Self::new_with_optional_audio_setup_and_max_block_frames(
            descriptor,
            None,
            sample_rate,
            max_block_frames,
        )
    }

    /// Creates an external plugin with an explicit per-instance audio setup.
    ///
    /// # Errors
    /// Returns an error if the descriptor, setup or native plugin cannot be loaded.
    pub fn new_with_audio_setup(
        descriptor: &PluginDescriptor,
        audio_setup: NativePluginAudioSetup,
        sample_rate: u32,
    ) -> Result<Self, String> {
        Self::new_with_audio_setup_and_max_block_frames(
            descriptor,
            audio_setup,
            sample_rate,
            Self::DEFAULT_MAX_BLOCK_FRAMES,
        )
    }

    /// Creates an external plugin with an explicit setup and block capacity.
    ///
    /// # Errors
    /// Returns an error if the descriptor, setup or native plugin cannot be loaded.
    pub fn new_with_audio_setup_and_max_block_frames(
        descriptor: &PluginDescriptor,
        audio_setup: NativePluginAudioSetup,
        sample_rate: u32,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        Self::new_with_optional_audio_setup_and_max_block_frames(
            descriptor,
            Some(audio_setup),
            sample_rate,
            max_block_frames,
        )
    }

    fn new_with_optional_audio_setup_and_max_block_frames(
        descriptor: &PluginDescriptor,
        audio_setup: Option<NativePluginAudioSetup>,
        sample_rate: u32,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be positive".into());
        }
        if max_block_frames == 0 {
            return Err("maximum block frame count must be positive".into());
        }
        if max_block_frames > i32::MAX as usize {
            return Err("maximum block frame count exceeds native plugin ABI limits".into());
        }

        if descriptor.audio_outputs == 0 {
            descriptor.validate_for_native_probe()?;
        } else {
            descriptor.validate()?;
        }
        let hosting_plan = plan_external_plugin_hosting(descriptor);
        if hosting_plan.backend == ExternalHostingBackend::Passthrough {
            return Err(hosting_plan.reason.unwrap_or_else(|| {
                format!(
                    "external plugin '{}' has no available native backend",
                    descriptor.name
                )
            }));
        }
        let backend_audio_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            descriptor,
            audio_setup.as_ref(),
        )?;
        let persisted_audio_setup = audio_setup.clone().or_else(|| {
            matches!(
                backend_audio_setup.as_ref(),
                Some(NativePluginAudioSetup::BandSplit { .. })
            )
            .then(|| backend_audio_setup.clone())
            .flatten()
        });
        let native_backend = try_load_dynamic_backend(
            descriptor,
            hosting_plan.backend,
            sample_rate,
            max_block_frames,
            backend_audio_setup.as_ref(),
        )?;
        let native_backend = native_backend.ok_or_else(|| {
            format!(
                "external plugin '{}' did not create a native backend",
                descriptor.name
            )
        })?;
        let mut resolved_descriptor = descriptor.clone();
        let metadata = native_backend.metadata();
        resolved_descriptor.id.clone_from(&metadata.id);
        resolved_descriptor.name.clone_from(&metadata.name);
        resolved_descriptor.vendor.clone_from(&metadata.vendor);
        resolved_descriptor.version.clone_from(&metadata.version);
        resolved_descriptor.audio_inputs = metadata.input_channels;
        resolved_descriptor.audio_outputs = metadata.output_channels;
        let (input_channels, output_channels) = (metadata.input_channels, metadata.output_channels);
        let parameters = native_backend.parameters();

        let plugin = Self {
            descriptor: resolved_descriptor,
            discovery_descriptor: descriptor.clone(),
            audio_setup: persisted_audio_setup,
            input_channels,
            output_channels,
            sample_rate,
            max_block_frames,
            parameters,
            hosting_backend: hosting_plan.backend,
            restore_error: None,
            opaque_state: Vec::new(),
            native_backend: Some(native_backend),
        };
        if let Some(setup) = backend_audio_setup.as_ref() {
            plugin.validate_native_audio_setup_parameters(setup)?;
        }
        Ok(plugin)
    }

    /// Get the plugin descriptor.
    pub fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    /// Returns scanner metadata without this instance's selected channel widths.
    pub fn discovery_descriptor(&self) -> &PluginDescriptor {
        &self.discovery_descriptor
    }

    /// Returns the persisted per-instance setup, if one was explicitly selected.
    pub fn audio_setup(&self) -> Option<&NativePluginAudioSetup> {
        self.audio_setup.as_ref()
    }

    /// Deliberately changes the selected layout of a recognized native
    /// Ambisonics instance.
    ///
    /// This is a control-thread operation. It reconstructs and validates a
    /// candidate using the current native state, renegotiates the candidate
    /// while deactivated, and commits only after typed layout, native controls,
    /// exposed parameter values, and a newly saved state agree. Any error
    /// leaves the current instance and its setup untouched.
    pub fn reconfigure_audio_setup(
        &mut self,
        new_setup: NativePluginAudioSetup,
    ) -> Result<(), String> {
        new_setup.validate_for_descriptor(&self.discovery_descriptor)?;
        if matches!(new_setup, NativePluginAudioSetup::BandSplit { .. }) {
            return self.reconfigure_band_split_audio_setup(new_setup);
        }
        let current_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
        )?
        .ok_or_else(|| {
            format!(
                "external plugin '{}' has no recognized native Ambisonics setup",
                self.discovery_descriptor.name
            )
        })?;
        if current_setup == new_setup && self.audio_setup.as_ref() == Some(&new_setup) {
            return Ok(());
        }

        let active_backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no active native instance to reconfigure",
                self.discovery_descriptor.name
            )
        })?;
        let previous_controls = active_backend.ambisonics_controls()?.ok_or_else(|| {
            format!(
                "external plugin '{}' does not expose the recognized Ambisonics state schema",
                self.discovery_descriptor.name
            )
        })?;
        Self::validate_backend_audio_setup_parameters(
            &**active_backend,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &current_setup,
        )?;
        let exposed_parameters = Self::snapshot_exposed_parameters(
            &**active_backend,
            &self.parameters,
            &self.descriptor.name,
        )?;
        let current_state = active_backend
            .save_state()?
            .ok_or_else(|| {
                format!(
                    "external plugin '{}' cannot change layout because native state is not serializable",
                    self.descriptor.name
                )
            })?;

        // Restore the exact recognized plugin state on a disposable instance
        // at the old tuple first. This preserves hidden controls and private
        // persisted fields without interpreting the format's opaque bytes.
        let mut candidate = self.replacement_backend_for_state(
            &self.discovery_descriptor,
            Some(&current_setup),
            &current_state,
        )?;
        Self::validate_ambisonics_controls(
            &*candidate,
            &self.discovery_descriptor,
            &self.descriptor.name,
            previous_controls,
        )?;
        Self::validate_exposed_parameters(&*candidate, &exposed_parameters, &self.descriptor.name)?;

        candidate.reconfigure_ambisonics_audio_setup(&new_setup)?;
        Self::validate_backend_audio_setup_parameters(
            &*candidate,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &new_setup,
        )?;
        Self::validate_ambisonics_controls(
            &*candidate,
            &self.discovery_descriptor,
            &self.descriptor.name,
            previous_controls,
        )?;
        Self::validate_exposed_parameters(&*candidate, &exposed_parameters, &self.descriptor.name)?;

        let new_state = candidate.save_state()?.ok_or_else(|| {
            format!(
                "external plugin '{}' cannot commit layout because the candidate state is not serializable",
                self.descriptor.name
            )
        })?;
        // Prove the new opaque state restores under the new typed tuple before
        // replacing the active instance. This also catches wrappers that save
        // a layout different from the one they negotiated.
        let verifier = self.replacement_backend_for_state(
            &self.discovery_descriptor,
            Some(&new_setup),
            &new_state,
        )?;
        Self::validate_backend_audio_setup_parameters(
            &*verifier,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &new_setup,
        )?;
        Self::validate_ambisonics_controls(
            &*verifier,
            &self.discovery_descriptor,
            &self.descriptor.name,
            previous_controls,
        )?;
        Self::validate_exposed_parameters(&*verifier, &exposed_parameters, &self.descriptor.name)?;

        self.audio_setup = Some(new_setup);
        self.commit_native_backend(candidate, new_state);
        Ok(())
    }

    fn reconfigure_band_split_audio_setup(
        &mut self,
        new_setup: NativePluginAudioSetup,
    ) -> Result<(), String> {
        let current_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
        )?
        .ok_or_else(|| {
            format!(
                "external plugin '{}' has no recognized native BandSplit setup",
                self.discovery_descriptor.name
            )
        })?;
        if !matches!(current_setup, NativePluginAudioSetup::BandSplit { .. }) {
            return Err(format!(
                "external plugin '{}' cannot change from a non-BandSplit setup to BandSplit",
                self.discovery_descriptor.name
            ));
        }
        if current_setup == new_setup && self.audio_setup.as_ref() == Some(&new_setup) {
            return Ok(());
        }

        let active_backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no active native instance to reconfigure",
                self.discovery_descriptor.name
            )
        })?;
        Self::validate_backend_audio_setup_parameters(
            &**active_backend,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &current_setup,
        )?;
        let exposed_parameters = Self::snapshot_exposed_parameters(
            &**active_backend,
            &self.parameters,
            &self.descriptor.name,
        )?;
        let current_state = active_backend
            .save_state()?
            .ok_or_else(|| {
                format!(
                    "external plugin '{}' cannot change BandSplit layout because native state is not serializable",
                    self.descriptor.name
                )
            })?;

        let mut candidate = self.replacement_backend_for_state(
            &self.discovery_descriptor,
            Some(&current_setup),
            &current_state,
        )?;
        Self::validate_backend_audio_setup_parameters(
            &*candidate,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &current_setup,
        )?;
        Self::validate_exposed_parameters(&*candidate, &exposed_parameters, &self.descriptor.name)?;

        candidate.reconfigure_band_split_audio_setup(&new_setup)?;
        Self::validate_backend_audio_setup_parameters(
            &*candidate,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &new_setup,
        )?;
        Self::validate_exposed_parameters(&*candidate, &exposed_parameters, &self.descriptor.name)?;

        let new_state = candidate.save_state()?.ok_or_else(|| {
            format!(
                "external plugin '{}' cannot commit BandSplit layout because candidate state is not serializable",
                self.descriptor.name
            )
        })?;
        let verifier = self.replacement_backend_for_state(
            &self.discovery_descriptor,
            Some(&new_setup),
            &new_state,
        )?;
        Self::validate_backend_audio_setup_parameters(
            &*verifier,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &new_setup,
        )?;
        Self::validate_exposed_parameters(&*verifier, &exposed_parameters, &self.descriptor.name)?;

        self.audio_setup = Some(new_setup);
        self.commit_native_backend(candidate, new_state);
        Ok(())
    }

    pub fn hosting_backend(&self) -> ExternalHostingBackend {
        self.hosting_backend
    }

    pub fn hosting_plan(&self) -> ExternalPluginHostingPlan {
        plan_external_plugin_hosting(&self.descriptor)
    }

    pub fn restore_error(&self) -> Option<&str> {
        self.restore_error.as_deref()
    }

    /// Serialize descriptor and placeholder state for project/preset storage.
    pub fn placeholder_state(&self) -> ExternalPluginState {
        let mut state = ExternalPluginState::new(
            self.discovery_descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            self.opaque_state.clone(),
        );
        state.audio_setup.clone_from(&self.audio_setup);
        state
    }

    /// Recreate an external plugin wrapper from a serialized placeholder state.
    pub fn from_placeholder_state(
        state: &ExternalPluginState,
        sample_rate: u32,
    ) -> Result<Self, String> {
        Self::from_placeholder_state_with_max_block_frames(
            state,
            sample_rate,
            Self::DEFAULT_MAX_BLOCK_FRAMES,
        )
    }

    pub fn from_placeholder_state_with_max_block_frames(
        state: &ExternalPluginState,
        sample_rate: u32,
        max_block_frames: usize,
    ) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("sample rate must be positive".into());
        }
        state.validate()?;
        if state.sandbox_mode != ExternalPluginSandboxMode::InProcess {
            return Err(format!(
                "External plugin state sandbox mode {:?} cannot restore in-process plugin",
                state.sandbox_mode
            ));
        }
        let restore_audio_setup =
            if state.audio_setup.is_none() && !state.opaque_state.is_empty() {
                NativePluginAudioSetup::legacy_band_split_default(&state.descriptor)
            } else {
                None
            }
            .or_else(|| state.audio_setup.clone());
        let backend_audio_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &state.descriptor,
            restore_audio_setup.as_ref(),
        )?;
        let mut plugin = Self::new_with_optional_audio_setup_and_max_block_frames(
            &state.descriptor,
            restore_audio_setup,
            sample_rate,
            max_block_frames,
        )?;
        if !state.opaque_state.is_empty() {
            let backend = plugin.native_backend.as_mut().ok_or_else(|| {
                format!(
                    "external plugin '{}' has no native backend for state restore",
                    state.descriptor.name
                )
            })?;
            backend.load_state(&state.opaque_state).map_err(|error| {
                format!(
                    "failed to restore external plugin '{}': {error}",
                    state.descriptor.name
                )
            })?;
            if let Some(setup) = backend_audio_setup.as_ref() {
                Self::validate_backend_audio_setup_parameters(
                    &**backend,
                    &plugin.discovery_descriptor,
                    &plugin.descriptor.name,
                    setup,
                )?;
            }
        }
        plugin.opaque_state = state.opaque_state.clone();
        Ok(plugin)
    }

    fn validate_native_audio_setup_parameters(
        &self,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        let backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no native backend for structural readback",
                self.descriptor.name
            )
        })?;
        Self::validate_backend_audio_setup_parameters(
            &**backend,
            &self.discovery_descriptor,
            &self.descriptor.name,
            setup,
        )
    }

    fn validate_backend_audio_setup_parameters(
        backend: &dyn NativeExternalPluginBackend,
        discovery_descriptor: &PluginDescriptor,
        plugin_name: &str,
        setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        setup.validate_for_descriptor(discovery_descriptor)?;
        match setup {
            NativePluginAudioSetup::Ambisonics {
                order,
                target_layout,
            } => {
                let expected = (
                    i32::from(*order),
                    target_layout.plugin_parameter_choice_index(),
                );
                let actual = backend
                    .ambisonics_layout_parameters()?
                    .ok_or_else(|| {
                        format!(
                            "external plugin '{plugin_name}' does not support native Ambisonics structural readback"
                        )
                    })?;
                if actual != expected {
                    return Err(format!(
                        "external plugin '{plugin_name}' audio setup conflicts with native structural parameters: expected order/layout {expected:?}, got {actual:?}"
                    ));
                }
            }
            NativePluginAudioSetup::BandSplit {
                num_bands,
                output_layout,
            } => {
                let expected_bands = i32::from(*num_bands);
                let actual = backend
                    .band_split_layout_parameters()?
                    .ok_or_else(|| {
                        format!(
                            "external plugin '{plugin_name}' does not support native BandSplit structural readback"
                        )
                    })?;
                if actual != (expected_bands, *output_layout) {
                    return Err(format!(
                        "external plugin '{plugin_name}' audio setup conflicts with native BandSplit structural parameters: expected {expected_bands} bands in {output_layout:?} layout, got {actual:?}"
                    ));
                }
            }
        }
        let (expected_inputs, expected_outputs) = setup.channel_counts()?;
        let metadata = backend.metadata();
        if (metadata.input_channels, metadata.output_channels)
            != (expected_inputs, expected_outputs)
        {
            return Err(format!(
                "external plugin '{plugin_name}' negotiated {}→{} channels for structural setup requiring {expected_inputs}→{expected_outputs}",
                metadata.input_channels, metadata.output_channels
            ));
        }
        Ok(())
    }

    fn validate_ambisonics_controls(
        backend: &dyn NativeExternalPluginBackend,
        discovery_descriptor: &PluginDescriptor,
        plugin_name: &str,
        expected: NativeAmbisonicsControls,
    ) -> Result<(), String> {
        NativePluginAudioSetup::for_descriptor_or_legacy_default(discovery_descriptor, None)?
            .ok_or_else(|| {
                format!(
                    "external plugin '{plugin_name}' does not have the recognized Ambisonics parameter schema"
                )
            })?;
        let actual = backend.ambisonics_controls()?.ok_or_else(|| {
            format!("external plugin '{plugin_name}' does not expose Ambisonics control readback")
        })?;
        if actual != expected {
            return Err(format!(
                "external plugin '{plugin_name}' changed persistent Ambisonics controls during layout reconfiguration: expected {expected:?}, got {actual:?}"
            ));
        }
        Ok(())
    }

    fn snapshot_exposed_parameters(
        backend: &dyn NativeExternalPluginBackend,
        parameters: &[Parameter],
        plugin_name: &str,
    ) -> Result<Vec<(ParameterId, ParameterValue)>, String> {
        parameters
            .iter()
            .map(|parameter| {
                let value = backend.get_parameter(&parameter.id).ok_or_else(|| {
                    format!(
                        "external plugin '{plugin_name}' cannot read exposed parameter '{}' before layout reconfiguration",
                        parameter.id
                    )
                })?;
                Ok((parameter.id.clone(), value))
            })
            .collect()
    }

    fn validate_exposed_parameters(
        backend: &dyn NativeExternalPluginBackend,
        expected: &[(ParameterId, ParameterValue)],
        plugin_name: &str,
    ) -> Result<(), String> {
        let actual_parameters = backend.parameters();
        if actual_parameters.len() != expected.len() {
            return Err(format!(
                "external plugin '{plugin_name}' changed the exposed parameter count from {} to {} during layout reconfiguration",
                expected.len(),
                actual_parameters.len()
            ));
        }
        for (id, expected_value) in expected {
            if !actual_parameters
                .iter()
                .any(|parameter| parameter.id.as_str() == id.as_str())
            {
                return Err(format!(
                    "external plugin '{plugin_name}' dropped exposed parameter '{id}' during layout reconfiguration"
                ));
            }
            let actual_value = backend.get_parameter(id).ok_or_else(|| {
                format!(
                    "external plugin '{plugin_name}' cannot verify exposed parameter '{id}' after layout reconfiguration"
                )
            })?;
            if &actual_value != expected_value {
                return Err(format!(
                    "external plugin '{plugin_name}' changed exposed parameter '{id}' from {expected_value} to {actual_value} during layout reconfiguration"
                ));
            }
        }
        Ok(())
    }

    fn resolved_descriptor(
        discovery_descriptor: &PluginDescriptor,
        metadata: &NativePluginMetadata,
    ) -> PluginDescriptor {
        let mut descriptor = discovery_descriptor.clone();
        descriptor.id.clone_from(&metadata.id);
        descriptor.name.clone_from(&metadata.name);
        descriptor.vendor.clone_from(&metadata.vendor);
        descriptor.version.clone_from(&metadata.version);
        descriptor.audio_inputs = metadata.input_channels;
        descriptor.audio_outputs = metadata.output_channels;
        descriptor
    }

    fn commit_native_backend(
        &mut self,
        backend: Box<dyn NativeExternalPluginBackend>,
        opaque_state: Vec<u8>,
    ) {
        let metadata = backend.metadata().clone();
        self.descriptor = Self::resolved_descriptor(&self.discovery_descriptor, &metadata);
        self.input_channels = metadata.input_channels;
        self.output_channels = metadata.output_channels;
        self.parameters = backend.parameters();
        self.native_backend = Some(backend);
        self.opaque_state = opaque_state;
    }

    fn replacement_backend_for_state(
        &self,
        descriptor: &PluginDescriptor,
        audio_setup: Option<&NativePluginAudioSetup>,
        opaque_state: &[u8],
    ) -> Result<Box<dyn NativeExternalPluginBackend>, String> {
        let effective_setup =
            NativePluginAudioSetup::for_descriptor_or_legacy_default(descriptor, audio_setup)?;
        let mut backend = try_load_dynamic_backend(
            descriptor,
            self.hosting_backend,
            self.sample_rate,
            self.max_block_frames,
            effective_setup.as_ref(),
        )?
        .ok_or_else(|| {
            format!(
                "external plugin '{}' has no available native backend",
                descriptor.name
            )
        })?;
        if !opaque_state.is_empty() {
            backend.load_state(opaque_state)?;
        }
        if let Some(setup) = effective_setup.as_ref() {
            Self::validate_backend_audio_setup_parameters(
                &*backend,
                descriptor,
                &descriptor.name,
                setup,
            )?;
        }
        Ok(backend)
    }

    pub fn to_placeholder_preset(
        &self,
        name: impl Into<String>,
    ) -> Result<PluginPreset, PluginError> {
        let mut preset = PluginPreset::new(
            name.into(),
            EXTERNAL_PLUGIN_PRESET_ID.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        );
        let opaque_state = match self.native_backend.as_ref() {
            Some(backend) => backend.save_state().map_err(|error| {
                PluginError::InvalidConfiguration(format!(
                    "failed to save external plugin '{}': {error}",
                    self.descriptor.name
                ))
            })?,
            None => None,
        }
        .unwrap_or_else(|| self.opaque_state.clone());
        let mut state = ExternalPluginState::new(
            self.discovery_descriptor.clone(),
            ExternalPluginSandboxMode::InProcess,
            opaque_state,
        );
        state.audio_setup.clone_from(&self.audio_setup);
        preset.set_external_plugin_state(&state)?;
        Ok(preset)
    }

    fn expected_input_len(&self, ctx: &ProcessContext) -> usize {
        ctx.num_frames.saturating_mul(self.input_channels)
    }

    fn expected_output_len(&self, ctx: &ProcessContext) -> usize {
        ctx.num_frames.saturating_mul(self.output_channels)
    }

    fn process_native(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        let backend = self.native_backend.as_mut().ok_or_else(|| {
            format!(
                "external plugin '{}' selected {:?} hosting without a native instance",
                self.descriptor.name, self.hosting_backend
            )
        })?;
        backend.process(
            input,
            output,
            self.input_channels,
            self.output_channels,
            ctx,
        )?;
        Ok(ctx.num_frames)
    }
}

impl Plugin for ExternalPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new(
            &self.descriptor.name,
            &self.descriptor.version,
            &self.descriptor.vendor,
        )
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::External
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::boundary(PluginCostClass::External, self.latency_samples())
    }

    fn latency_samples(&self) -> usize {
        self.native_backend
            .as_ref()
            .map_or(0, |backend| backend.latency_samples())
    }

    fn tail_length(&self) -> TailLength {
        self.native_backend
            .as_ref()
            .map_or(TailLength::Unknown, |backend| backend.tail_length())
    }

    fn refresh_control_thread_metadata(&mut self) {
        if let Some(backend) = self.native_backend.as_mut() {
            let _ = backend.refresh_tail_length();
        }
    }

    fn reset(&mut self) {
        let _ = self.reset_checked();
    }

    fn reset_checked(&mut self) -> PluginResult<()> {
        if let Some(backend) = self.native_backend.as_mut() {
            backend.reset()?;
        }
        Ok(())
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.native_backend
            .as_ref()
            .is_some_and(|backend| backend.guarantees_identity_frame_geometry())
    }

    fn input_channels(&self) -> usize {
        self.input_channels
    }

    fn output_channels(&self) -> usize {
        self.output_channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        let parameter = self.parameters.iter().find(|parameter| parameter.id == id);
        let Some(parameter) = parameter else {
            return Err(format!(
                "parameter '{id}' is not exposed by external plugin '{}'",
                self.descriptor.name
            ));
        };
        parameter.validate(&value)?;
        self.native_backend
            .as_mut()
            .ok_or_else(|| {
                format!(
                    "parameter '{id}' cannot be changed because external plugin '{}' is unavailable",
                    self.descriptor.name
                )
            })?
            .set_parameter(&id, &value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.native_backend.as_ref()?.get_parameter(id)
    }

    fn save_opaque_state(&self) -> PluginResult<Vec<u8>> {
        self.native_backend
            .as_ref()
            .ok_or_else(|| "external plugin has no native backend".to_string())?
            .save_state()
            .map(|state| state.unwrap_or_default())
    }

    fn load_opaque_state(&mut self, state: &[u8]) -> PluginResult<()> {
        let effective_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
        )?;
        if effective_setup.is_none() {
            self.native_backend
                .as_mut()
                .ok_or_else(|| "external plugin has no native backend".to_string())?
                .load_state(state)?;
            self.opaque_state = state.to_vec();
            return Ok(());
        }

        if state.is_empty() {
            self.opaque_state.clear();
            return Ok(());
        }

        let replacement = self.replacement_backend_for_state(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
            state,
        )?;
        self.commit_native_backend(replacement, state.to_vec());
        Ok(())
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> PluginResult<usize> {
        let expected_input = self.expected_input_len(ctx);
        let expected_output = self.expected_output_len(ctx);

        if input.len() < expected_input {
            return Err(format!(
                "external plugin '{}' received {} input samples but expected {expected_input} ({} channels x {} frames)",
                self.descriptor.name,
                input.len(),
                self.input_channels,
                ctx.num_frames
            ));
        }
        if output.len() < expected_output {
            return Err(format!(
                "external plugin '{}' received {} output samples but expected {expected_output} ({} channels x {} frames)",
                self.descriptor.name,
                output.len(),
                self.output_channels,
                ctx.num_frames
            ));
        }

        match self.hosting_backend {
            ExternalHostingBackend::Passthrough => Err(format!(
                "external plugin '{}' cannot process without a native backend",
                self.descriptor.name
            )),
            ExternalHostingBackend::Clap
            | ExternalHostingBackend::Vst3
            | ExternalHostingBackend::AudioUnit => self.process_native(input, output, ctx),
        }
    }
}

impl SerializablePlugin for ExternalPlugin {
    fn serialize(&self) -> Result<PluginPreset, PluginError> {
        self.to_placeholder_preset(self.descriptor.name.clone())
    }

    fn deserialize(&mut self, preset: &PluginPreset) -> Result<(), PluginError> {
        if !preset.is_compatible(EXTERNAL_PLUGIN_PRESET_ID) {
            return Err(PluginError::InvalidConfiguration(format!(
                "external plugin preset expected plugin_id '{}', got '{}'",
                EXTERNAL_PLUGIN_PRESET_ID, preset.plugin_id
            )));
        }

        self.parameters_from_map(&preset.parameters)?;

        let state = preset.external_plugin_state()?.ok_or_else(|| {
            PluginError::InvalidConfiguration(
                "external plugin preset is missing external plugin state".to_string(),
            )
        })?;
        if state.sandbox_mode != ExternalPluginSandboxMode::InProcess {
            return Err(PluginError::InvalidConfiguration(format!(
                "external plugin preset sandbox mode {:?} cannot restore in-process plugin",
                state.sandbox_mode
            )));
        }
        if state.format != self.discovery_descriptor.format
            || state.plugin_id != self.discovery_descriptor.id
            || state.plugin_path != self.discovery_descriptor.path
            || state.descriptor.audio_inputs != self.discovery_descriptor.audio_inputs
            || state.descriptor.audio_outputs != self.discovery_descriptor.audio_outputs
            || state.descriptor.is_instrument != self.discovery_descriptor.is_instrument
        {
            return Err(PluginError::InvalidConfiguration(format!(
                "external plugin preset targets '{}' at {}, not '{}' at {}",
                state.plugin_id,
                state.plugin_path.display(),
                self.discovery_descriptor.id,
                self.discovery_descriptor.path.display()
            )));
        }

        state
            .validate()
            .map_err(PluginError::InvalidConfiguration)?;
        let replacement = self
            .replacement_backend_for_state(
                &state.descriptor,
                state.audio_setup.as_ref(),
                &state.opaque_state,
            )
            .map_err(PluginError::InvalidConfiguration)?;

        // Commit only after construction, opaque restore, and structural
        // readback all succeeded on the replacement instance.
        self.discovery_descriptor = state.descriptor.clone();
        self.audio_setup.clone_from(&state.audio_setup);
        self.commit_native_backend(replacement, state.opaque_state);

        Ok(())
    }

    fn parameters_to_map(&self) -> HashMap<String, ParameterValue> {
        HashMap::new()
    }

    fn parameters_from_map(
        &mut self,
        params: &HashMap<String, ParameterValue>,
    ) -> Result<(), PluginError> {
        if params.is_empty() {
            Ok(())
        } else {
            Err(PluginError::InvalidConfiguration(
                "external plugin placeholder presets do not store host-side parameters".to_string(),
            ))
        }
    }
}
