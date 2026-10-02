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
mod native_crossover_layout;
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
pub use native_crossover_layout::NativeCrossoverStructure;
pub use plugin::*;
pub use plugin_descriptor::*;
pub use plugin_descriptor_probe_cache::*;
pub use plugin_format::*;
pub use plugin_scan_summary::*;
pub use plugin_scanner::*;
pub use types::*;

use external_hosting_backend::try_load_dynamic_backend;
use native_backend::{
    NativeAmbisonicsControls, NativeExternalPluginBackend, NativePluginMetadata,
    native_parameter_id,
};

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
        // A fresh backend selects its audio ports and buses while loading, but
        // CLAP/VST3 structural controls are applied by the same control-thread
        // route used for deliberate reconfiguration. Apply them to this new,
        // still-disposable instance before exposing metadata or audio. State
        // restoration uses `replacement_backend_for_state()` instead, where
        // opaque state is loaded first and checked against the persisted setup.
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
        if matches!(new_setup, NativePluginAudioSetup::Crossover { .. }) {
            return self.reconfigure_crossover_audio_setup(new_setup);
        }
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

        let renegotiate_result = candidate
            .reconfigure_ambisonics_audio_setup(&new_setup)
            .and_then(|()| {
                Self::validate_backend_audio_setup_parameters(
                    &*candidate,
                    &self.discovery_descriptor,
                    &self.descriptor.name,
                    &new_setup,
                )
            });
        if let Err(renegotiate_error) = renegotiate_result {
            if !matches!(
                current_setup,
                NativePluginAudioSetup::AmbisonicsCustom { .. }
            ) && !matches!(new_setup, NativePluginAudioSetup::AmbisonicsCustom { .. })
            {
                return Err(renegotiate_error);
            }
            // A custom-boundary crossing cannot renegotiate from the old
            // bytes alone: the candidate holds the previous tuple while the
            // new setup needs different recognized fields. Rewrite only the
            // recognized fields, then restore the state directly at the
            // explicitly requested valid setup (Crossover-style recovery).
            let repaired_state = ambisonics_state_with_setup(
                &current_state,
                self.discovery_descriptor.format,
                &new_setup,
            )
            .map_err(|repair_error| {
                format!(
                    "external plugin '{}' could not prepare Ambisonics recovery state after candidate failure ({renegotiate_error}): {repair_error}",
                    self.descriptor.name
                )
            })?;
            candidate = self
                .replacement_backend_for_state(
                    &self.discovery_descriptor,
                    Some(&new_setup),
                    &repaired_state,
                )
                .map_err(|recovery_error| {
                    format!(
                        "external plugin '{}' failed Ambisonics recovery at the requested setup after candidate failure ({renegotiate_error}): {recovery_error}",
                        self.descriptor.name
                    )
                })?;
            Self::validate_backend_audio_setup_parameters(
                &*candidate,
                &self.discovery_descriptor,
                &self.descriptor.name,
                &new_setup,
            )?;
        }
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

    fn reconfigure_crossover_audio_setup(
        &mut self,
        new_setup: NativePluginAudioSetup,
    ) -> Result<(), String> {
        let current_setup = self.current_crossover_audio_setup()?;
        let active_backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no active native instance to reconfigure",
                self.discovery_descriptor.name
            )
        })?;
        let active_structure = active_backend
            .crossover_layout_parameters()?
            .ok_or_else(|| {
                format!(
                    "external plugin '{}' does not expose the recognized Crossover state schema",
                    self.descriptor.name
                )
            })?;
        let NativePluginAudioSetup::Crossover {
            input_layout,
            output_layout,
            ..
        } = &current_setup
        else {
            return Err(format!(
                "external plugin '{}' has a non-Crossover typed setup",
                self.descriptor.name
            ));
        };
        // Native structural parameters can change before the host accepts a restart.
        // Keep the named layouts, but describe the saved state using its actual controls.
        let active_setup = NativePluginAudioSetup::Crossover {
            input_layout: *input_layout,
            num_bands: active_structure.num_bands,
            topology: active_structure.topology,
            mode: active_structure.mode,
            output_layout: *output_layout,
        };
        let native_structure_matches_setup = active_setup == current_setup;
        if native_structure_matches_setup {
            Self::validate_backend_audio_setup_parameters(
                &**active_backend,
                &self.discovery_descriptor,
                &self.descriptor.name,
                &current_setup,
            )?;
        }
        if native_structure_matches_setup
            && current_setup == new_setup
            && self.audio_setup.as_ref() == Some(&new_setup)
        {
            return Ok(());
        }

        let crossover_structural_ids =
            crossover_structural_parameter_ids(self.discovery_descriptor.format)?;
        let exposed_parameters = Self::snapshot_exposed_parameters_excluding(
            &**active_backend,
            &self.parameters,
            &self.descriptor.name,
            &crossover_structural_ids,
        )?;
        let current_state = active_backend
            .save_state()?
            .ok_or_else(|| {
                format!(
                    "external plugin '{}' cannot change Crossover layout because native state is not serializable",
                    self.descriptor.name
                )
            })?;

        let candidate_result = (|| {
            let mut candidate = self.replacement_backend_for_state(
                &self.discovery_descriptor,
                Some(&active_setup),
                &current_state,
            )?;
            Self::validate_exposed_parameter_subset(
                &*candidate,
                &exposed_parameters,
                &self.descriptor.name,
            )?;

            candidate.reconfigure_crossover_audio_setup(&new_setup)?;
            Self::validate_backend_audio_setup_parameters(
                &*candidate,
                &self.discovery_descriptor,
                &self.descriptor.name,
                &new_setup,
            )?;
            Self::validate_exposed_parameter_subset(
                &*candidate,
                &exposed_parameters,
                &self.descriptor.name,
            )?;
            Ok(candidate)
        })();
        let candidate = match candidate_result {
            Ok(candidate) => candidate,
            Err(candidate_error) if !native_structure_matches_setup => {
                // A structurally invalid state, such as FIR plus PerChannel,
                // may fail before a candidate can be reconfigured. Rewrite
                // only the recognized structure fields, then restore the
                // state directly at the explicitly requested valid setup.
                let repaired_state = crossover_state_with_setup(
                    &current_state,
                    self.discovery_descriptor.format,
                    &new_setup,
                )
                .map_err(|repair_error| {
                    format!(
                        "external plugin '{}' could not prepare Crossover recovery state after candidate failure ({candidate_error}): {repair_error}",
                        self.descriptor.name
                    )
                })?;
                let candidate = self
                    .replacement_backend_for_state(
                        &self.discovery_descriptor,
                        Some(&new_setup),
                        &repaired_state,
                    )
                    .map_err(|recovery_error| {
                        format!(
                            "external plugin '{}' could not restore the requested Crossover setup after native structure drift (candidate failure: {candidate_error}; repaired-state failure: {recovery_error})",
                            self.descriptor.name
                        )
                    })?;
                Self::validate_exposed_parameter_subset(
                    &*candidate,
                    &exposed_parameters,
                    &self.descriptor.name,
                )?;
                candidate
            }
            Err(error) => return Err(error),
        };

        let new_state = candidate.save_state()?.ok_or_else(|| {
            format!(
                "external plugin '{}' cannot commit Crossover layout because candidate state is not serializable",
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
        Self::validate_exposed_parameter_subset(
            &*verifier,
            &exposed_parameters,
            &self.descriptor.name,
        )?;

        self.audio_setup = Some(new_setup);
        self.commit_native_backend(candidate, new_state);
        Ok(())
    }

    fn current_crossover_audio_setup(&self) -> Result<NativePluginAudioSetup, String> {
        if let Some(setup @ NativePluginAudioSetup::Crossover { .. }) = self.audio_setup.as_ref() {
            return Ok(setup.clone());
        }
        let backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no active native instance for Crossover layout readback",
                self.discovery_descriptor.name
            )
        })?;
        let structure = backend.crossover_layout_parameters()?.ok_or_else(|| {
            format!(
                "external plugin '{}' does not expose the recognized Crossover state schema",
                self.discovery_descriptor.name
            )
        })?;
        let input_layout = NativeCrossoverInputLayout::unique_for_channel_count(
            backend.metadata().input_channels,
        )
        .ok_or_else(|| {
            format!(
                "external plugin '{}' has {} input channels but no persisted named Crossover layout; an explicit layout is required",
                self.discovery_descriptor.name,
                backend.metadata().input_channels
            )
        })?;
        let output_layout = match self.discovery_descriptor.format {
            PluginFormat::Clap => NativeCrossoverOutputLayout::ClapPacked,
            PluginFormat::Vst3 => NativeCrossoverOutputLayout::Vst3Buses,
            PluginFormat::AudioUnit => {
                return Err("native Crossover setup is unavailable for Audio Unit".into());
            }
        };
        let setup = NativePluginAudioSetup::Crossover {
            input_layout,
            num_bands: structure.num_bands,
            topology: structure.topology,
            mode: structure.mode,
            output_layout,
        };
        Self::validate_backend_audio_setup_parameters(
            &**backend,
            &self.discovery_descriptor,
            &self.descriptor.name,
            &setup,
        )?;
        Ok(setup)
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
            if let Some(NativePluginAudioSetup::AmbisonicsCustom { custom, .. }) =
                backend_audio_setup.as_ref()
            {
                validate_ambisonics_custom_agreement(
                    &state.opaque_state,
                    state.descriptor.format,
                    &state.descriptor.name,
                    custom,
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
            NativePluginAudioSetup::AmbisonicsCustom { order, .. } => {
                let expected = (
                    i32::from(*order),
                    AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX,
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
            NativePluginAudioSetup::Crossover {
                num_bands,
                topology,
                mode,
                ..
            } => {
                let expected = NativeCrossoverStructure {
                    mode: *mode,
                    topology: *topology,
                    num_bands: *num_bands,
                };
                let actual = backend
                    .crossover_layout_parameters()?
                    .ok_or_else(|| {
                        format!(
                            "external plugin '{plugin_name}' does not support native Crossover structural readback"
                        )
                    })?;
                if actual != expected {
                    return Err(format!(
                        "external plugin '{plugin_name}' audio setup conflicts with native Crossover structural parameters: expected {expected:?}, got {actual:?}"
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
        Self::snapshot_exposed_parameters_excluding(backend, parameters, plugin_name, &[])
    }

    fn snapshot_exposed_parameters_excluding(
        backend: &dyn NativeExternalPluginBackend,
        parameters: &[Parameter],
        plugin_name: &str,
        excluded_ids: &[ParameterId],
    ) -> Result<Vec<(ParameterId, ParameterValue)>, String> {
        parameters
            .iter()
            .filter(|parameter| !excluded_ids.contains(&parameter.id))
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

    fn restore_exposed_parameters(
        backend: &mut dyn NativeExternalPluginBackend,
        expected: &[(ParameterId, ParameterValue)],
        structural_ids: &[ParameterId],
        plugin_name: &str,
    ) -> Result<(), String> {
        let actual_parameters = backend.parameters();
        if actual_parameters.len() != expected.len() {
            return Err(format!(
                "external plugin '{plugin_name}' changed the exposed parameter count from {} to {} during sample-rate reinitialization",
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
                    "external plugin '{plugin_name}' dropped exposed parameter '{id}' during sample-rate reinitialization"
                ));
            }

            let actual_value = backend.get_parameter(id).ok_or_else(|| {
                format!(
                    "external plugin '{plugin_name}' cannot verify exposed parameter '{id}' during sample-rate reinitialization"
                )
            })?;
            if &actual_value == expected_value {
                continue;
            }
            if structural_ids.contains(id) {
                return Err(format!(
                    "external plugin '{plugin_name}' has a pending structural change for '{id}' that conflicts with its restored native audio setup"
                ));
            }

            // Native state callbacks capture committed values. Preserve ordinary
            // control events that were queued after the last process callback.
            backend.set_parameter(id, expected_value)?;
            let restored_value = backend.get_parameter(id).ok_or_else(|| {
                format!(
                    "external plugin '{plugin_name}' cannot verify restored exposed parameter '{id}' during sample-rate reinitialization"
                )
            })?;
            if &restored_value != expected_value {
                return Err(format!(
                    "external plugin '{plugin_name}' did not preserve exposed parameter '{id}' during sample-rate reinitialization: expected {expected_value}, got {restored_value}"
                ));
            }
        }

        Self::validate_exposed_parameters(backend, expected, plugin_name)
    }

    fn validate_exposed_parameter_subset(
        backend: &dyn NativeExternalPluginBackend,
        expected: &[(ParameterId, ParameterValue)],
        plugin_name: &str,
    ) -> Result<(), String> {
        let actual_parameters = backend.parameters();
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
        self.replacement_backend_for_state_at_rate(
            descriptor,
            audio_setup,
            opaque_state,
            self.sample_rate,
        )
    }

    fn replacement_backend_for_state_at_rate(
        &self,
        descriptor: &PluginDescriptor,
        audio_setup: Option<&NativePluginAudioSetup>,
        opaque_state: &[u8],
        sample_rate: u32,
    ) -> Result<Box<dyn NativeExternalPluginBackend>, String> {
        let effective_setup =
            NativePluginAudioSetup::for_descriptor_or_legacy_default(descriptor, audio_setup)?;
        let mut backend = try_load_dynamic_backend(
            descriptor,
            self.hosting_backend,
            sample_rate,
            self.max_block_frames,
            effective_setup.as_ref(),
        )?
        .ok_or_else(|| {
            format!(
                "external plugin '{}' has no available native backend",
                descriptor.name
            )
        })?;
        let restore_empty_vst3_state =
            effective_setup.is_none() && self.hosting_backend == ExternalHostingBackend::Vst3;
        if !opaque_state.is_empty() || restore_empty_vst3_state {
            // A no-setup VST3 empty-state restore is still a native callback:
            // it can fail after mutating or suspending the candidate. Preserve
            // its result while keeping the installed backend detached from it.
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
        if let Some(NativePluginAudioSetup::AmbisonicsCustom { custom, .. }) =
            effective_setup.as_ref()
            && !opaque_state.is_empty()
        {
            validate_ambisonics_custom_agreement(
                opaque_state,
                descriptor.format,
                &descriptor.name,
                custom,
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

/// Returns the host-facing IDs of native Crossover controls that are changed
/// by an explicit audio-setup request.
fn crossover_structural_parameter_ids(format: PluginFormat) -> Result<Vec<ParameterId>, String> {
    let format_prefix = match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => {
            return Err("native Crossover setup is unavailable for Audio Unit".into());
        }
    };

    Ok(["mode", "topology", "band_count"]
        .into_iter()
        .map(|key| ParameterId::from(format!("{format_prefix}.{}", native_parameter_id(key))))
        .collect())
}

fn audio_setup_structural_parameter_ids(
    format: PluginFormat,
    setup: Option<&NativePluginAudioSetup>,
) -> Result<Vec<ParameterId>, String> {
    let keys: &[&str] = match setup {
        Some(NativePluginAudioSetup::Ambisonics { .. }) => &["order", "target_layout"],
        Some(NativePluginAudioSetup::AmbisonicsCustom { .. }) => &["order", "target_layout"],
        Some(NativePluginAudioSetup::BandSplit { .. }) => &["num_bands"],
        Some(NativePluginAudioSetup::Crossover { .. }) => {
            return crossover_structural_parameter_ids(format);
        }
        None => return Ok(Vec::new()),
    };
    let format_prefix = match format {
        PluginFormat::Clap => "clap",
        PluginFormat::Vst3 => "vst3",
        PluginFormat::AudioUnit => {
            return Err("native audio setup is unavailable for Audio Unit".into());
        }
    };

    Ok(keys
        .iter()
        .map(|key| ParameterId::from(format!("{format_prefix}.{}", native_parameter_id(key))))
        .collect())
}

const CLAP_STATE_LENGTH_PREFIX_BYTES: usize = 8;

fn crossover_state_with_setup(
    opaque_state: &[u8],
    format: PluginFormat,
    setup: &NativePluginAudioSetup,
) -> Result<Vec<u8>, String> {
    let NativePluginAudioSetup::Crossover {
        num_bands,
        topology,
        mode,
        ..
    } = setup
    else {
        return Err("Crossover state repair requires a Crossover audio setup".into());
    };
    let (clap_prefixed, payload) = match format {
        PluginFormat::Clap => {
            let length_bytes = opaque_state
                .get(..CLAP_STATE_LENGTH_PREFIX_BYTES)
                .ok_or_else(|| "CLAP Crossover state is missing its length prefix".to_string())?;
            let length_bytes: [u8; CLAP_STATE_LENGTH_PREFIX_BYTES] = length_bytes
                .try_into()
                .map_err(|_| "CLAP Crossover state has an invalid length prefix".to_string())?;
            let payload = &opaque_state[CLAP_STATE_LENGTH_PREFIX_BYTES..];
            let expected_length = usize::try_from(u64::from_le_bytes(length_bytes))
                .map_err(|_| "CLAP Crossover state length does not fit in memory".to_string())?;
            if payload.len() != expected_length {
                return Err(format!(
                    "CLAP Crossover state length prefix describes {expected_length} bytes but contains {}",
                    payload.len()
                ));
            }
            (true, payload)
        }
        PluginFormat::Vst3 => (false, opaque_state),
        PluginFormat::AudioUnit => {
            return Err("native Crossover state repair is unavailable for Audio Unit".into());
        }
    };

    let mut state: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| format!("failed to parse native Crossover state: {error}"))?;
    let parameters = state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "native Crossover state has no parameter object".to_string())?;
    let mode_index = match mode {
        NativeCrossoverMode::Lowpass => 0,
        NativeCrossoverMode::Highpass => 1,
        NativeCrossoverMode::Both => 2,
    };
    let topology_index = match topology {
        NativeCrossoverTopology::Bands => 0,
        NativeCrossoverTopology::PerChannel => 1,
    };
    // These keys and choice indices are the recognized native Crossover state
    // schema; all other serialized fields remain untouched by the repair.
    parameters.insert("mode".into(), serde_json::json!({"i32": mode_index}));
    parameters.insert(
        "topology".into(),
        serde_json::json!({"i32": topology_index}),
    );
    parameters.insert(
        "band_count".into(),
        serde_json::json!({"i32": i32::from(num_bands.saturating_sub(2))}),
    );
    let payload = serde_json::to_vec(&state)
        .map_err(|error| format!("failed to encode repaired Crossover state: {error}"))?;
    if clap_prefixed {
        let payload_length = u64::try_from(payload.len())
            .map_err(|_| "repaired CLAP Crossover state is too large".to_string())?;
        let mut repaired = Vec::with_capacity(CLAP_STATE_LENGTH_PREFIX_BYTES + payload.len());
        repaired.extend_from_slice(&payload_length.to_le_bytes());
        repaired.extend_from_slice(&payload);
        Ok(repaired)
    } else {
        Ok(payload)
    }
}

/// NIH opaque-state field carrying custom Ambisonics geometry.
///
/// Part of the recognized native Ambisonics schema, like the hidden
/// `order`/`target_layout` parameters: the NIH wrapper writes the DSP
/// custom-layout JSON here when target 8 is active, and the host
/// validates it against the typed setup geometry. Shared with the NIH
/// plugin crate so both sides name one field.
pub const AMBISONICS_CUSTOM_STATE_FIELD: &str = "sotf_ambisonics_custom";

/// Splits a native Ambisonics state blob into its JSON payload.
///
/// Returns whether the blob carried the CLAP length prefix alongside
/// the payload bytes. Mirrors the Crossover repair framing so both
/// formats keep one recognized parse path.
fn native_ambisonics_state_payload(
    opaque_state: &[u8],
    format: PluginFormat,
) -> Result<(bool, &[u8]), String> {
    match format {
        PluginFormat::Clap => {
            let length_bytes = opaque_state
                .get(..CLAP_STATE_LENGTH_PREFIX_BYTES)
                .ok_or_else(|| "CLAP Ambisonics state is missing its length prefix".to_string())?;
            let length_bytes: [u8; CLAP_STATE_LENGTH_PREFIX_BYTES] = length_bytes
                .try_into()
                .map_err(|_| "CLAP Ambisonics state has an invalid length prefix".to_string())?;
            let payload = &opaque_state[CLAP_STATE_LENGTH_PREFIX_BYTES..];
            let expected_length = usize::try_from(u64::from_le_bytes(length_bytes))
                .map_err(|_| "CLAP Ambisonics state length does not fit in memory".to_string())?;
            if payload.len() != expected_length {
                return Err(format!(
                    "CLAP Ambisonics state length prefix describes {expected_length} bytes but contains {}",
                    payload.len()
                ));
            }
            Ok((true, payload))
        }
        PluginFormat::Vst3 => Ok((false, opaque_state)),
        PluginFormat::AudioUnit => {
            Err("native Ambisonics state is unavailable for Audio Unit".into())
        }
    }
}

/// Reads the embedded custom geometry from a native Ambisonics blob.
///
/// Returns the parsed custom-layout JSON value carried in the
/// recognized opaque-state field.
fn ambisonics_custom_geometry_from_opaque_state(
    opaque_state: &[u8],
    format: PluginFormat,
) -> Result<serde_json::Value, String> {
    let (_, payload) = native_ambisonics_state_payload(opaque_state, format)?;
    let state: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| format!("failed to parse native Ambisonics state: {error}"))?;
    let field = state
        .get("fields")
        .and_then(|fields| fields.get(AMBISONICS_CUSTOM_STATE_FIELD))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            "native Ambisonics state carries no custom geometry for the selected custom setup"
                .to_string()
        })?;
    serde_json::from_str(field)
        .map_err(|error| format!("failed to parse native Ambisonics custom geometry: {error}"))
}

/// Validates typed custom geometry against the native state field.
///
/// The opaque custom geometry must equal the selected setup geometry;
/// anything else is a wrong-instance restore, never a silent fallback.
fn validate_ambisonics_custom_agreement(
    opaque_state: &[u8],
    format: PluginFormat,
    plugin_name: &str,
    expected: &NativeAmbisonicsCustomGeometry,
) -> Result<(), String> {
    let actual = ambisonics_custom_geometry_from_opaque_state(opaque_state, format)?;
    let expected_value = serde_json::to_value(expected)
        .map_err(|error| format!("failed to encode selected custom Ambisonics geometry: {error}"))?;
    if actual != expected_value {
        return Err(format!(
            "external plugin '{plugin_name}' custom Ambisonics geometry in native state does not match the selected setup"
        ));
    }
    Ok(())
}

/// Rewrites recognized Ambisonics fields for an explicit setup change.
///
/// Sets the structural `order`/`target_layout` parameters and installs
/// (or removes) the custom geometry field, mirroring the Crossover
/// state repair. Scalar controls and all other fields stay untouched.
fn ambisonics_state_with_setup(
    opaque_state: &[u8],
    format: PluginFormat,
    setup: &NativePluginAudioSetup,
) -> Result<Vec<u8>, String> {
    let (order, target_index, custom_json) = match setup {
        NativePluginAudioSetup::Ambisonics {
            order,
            target_layout,
        } => (
            *order,
            target_layout.plugin_parameter_choice_index(),
            None,
        ),
        NativePluginAudioSetup::AmbisonicsCustom { order, custom } => {
            let custom_json = serde_json::to_string(custom).map_err(|error| {
                format!("failed to encode selected custom Ambisonics geometry: {error}")
            })?;
            (*order, AMBISONICS_CUSTOM_TARGET_CHOICE_INDEX, Some(custom_json))
        }
        _ => return Err("Ambisonics state repair requires an Ambisonics audio setup".into()),
    };
    let (clap_prefixed, payload) = native_ambisonics_state_payload(opaque_state, format)?;
    let mut state: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| format!("failed to parse native Ambisonics state: {error}"))?;
    let parameters = state
        .get_mut("params")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "native Ambisonics state has no parameter object".to_string())?;
    // These keys and choice indices are the recognized native Ambisonics
    // state schema; all other serialized fields remain untouched.
    parameters.insert("order".into(), serde_json::json!({"i32": i32::from(order)}));
    parameters.insert(
        "target_layout".into(),
        serde_json::json!({"i32": target_index}),
    );
    match custom_json {
        Some(custom_json) => {
            if let Some(fields) = state
                .get_mut("fields")
                .and_then(serde_json::Value::as_object_mut)
            {
                fields.insert(
                    AMBISONICS_CUSTOM_STATE_FIELD.into(),
                    serde_json::Value::String(custom_json),
                );
            } else {
                let mut fresh = serde_json::Map::new();
                fresh.insert(
                    AMBISONICS_CUSTOM_STATE_FIELD.into(),
                    serde_json::Value::String(custom_json),
                );
                state
                    .as_object_mut()
                    .ok_or_else(|| "native Ambisonics state has no top-level object".to_string())?
                    .insert("fields".into(), serde_json::Value::Object(fresh));
            }
        }
        None => {
            if let Some(fields) = state
                .get_mut("fields")
                .and_then(serde_json::Value::as_object_mut)
            {
                fields.remove(AMBISONICS_CUSTOM_STATE_FIELD);
            }
        }
    }
    let payload = serde_json::to_vec(&state)
        .map_err(|error| format!("failed to encode repaired Ambisonics state: {error}"))?;
    if clap_prefixed {
        let payload_length = u64::try_from(payload.len())
            .map_err(|_| "repaired CLAP Ambisonics state is too large".to_string())?;
        let mut repaired = Vec::with_capacity(CLAP_STATE_LENGTH_PREFIX_BYTES + payload.len());
        repaired.extend_from_slice(&payload_length.to_le_bytes());
        repaired.extend_from_slice(&payload);
        Ok(repaired)
    } else {
        Ok(payload)
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

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("sample rate must be positive".into());
        }
        if sample_rate == self.sample_rate {
            return Ok(());
        }

        let effective_audio_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
        )?;
        let structural_parameter_ids = audio_setup_structural_parameter_ids(
            self.discovery_descriptor.format,
            effective_audio_setup.as_ref(),
        )?;

        let active_backend = self.native_backend.as_ref().ok_or_else(|| {
            format!(
                "external plugin '{}' has no active native instance to reinitialize",
                self.descriptor.name
            )
        })?;
        let expected_channels = (
            active_backend.metadata().input_channels,
            active_backend.metadata().output_channels,
        );
        let exposed_parameters = Self::snapshot_exposed_parameters(
            &**active_backend,
            &self.parameters,
            &self.descriptor.name,
        )?;
        let current_state = active_backend
            .save_state()?
            .ok_or_else(|| {
                format!(
                    "external plugin '{}' cannot change sample rate because native state is not serializable",
                    self.descriptor.name
                )
            })?;

        let mut candidate = self.replacement_backend_for_state_at_rate(
            &self.discovery_descriptor,
            self.audio_setup.as_ref(),
            &current_state,
            sample_rate,
        )?;
        let candidate_metadata = candidate.metadata();
        if (
            candidate_metadata.input_channels,
            candidate_metadata.output_channels,
        ) != expected_channels
        {
            return Err(format!(
                "external plugin '{}' changed channel geometry from {}→{} to {}→{} while changing sample rate",
                self.descriptor.name,
                expected_channels.0,
                expected_channels.1,
                candidate_metadata.input_channels,
                candidate_metadata.output_channels
            ));
        }
        Self::restore_exposed_parameters(
            &mut *candidate,
            &exposed_parameters,
            &structural_parameter_ids,
            &self.descriptor.name,
        )?;
        if let Some(setup) = effective_audio_setup.as_ref() {
            Self::validate_backend_audio_setup_parameters(
                &*candidate,
                &self.discovery_descriptor,
                &self.descriptor.name,
                setup,
            )?;
        }
        Self::validate_exposed_parameters(&*candidate, &exposed_parameters, &self.descriptor.name)?;

        self.commit_native_backend(candidate, current_state);
        self.sample_rate = sample_rate;
        Ok(())
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
            if self.hosting_backend == ExternalHostingBackend::Vst3 {
                // VST3 state callbacks may deactivate the component before
                // validating state, including an empty stream. Stage every
                // attempt on a detached instance so rejection cannot mutate
                // the installed processor or its tail/history state.
                let replacement = self.replacement_backend_for_state(
                    &self.discovery_descriptor,
                    self.audio_setup.as_ref(),
                    state,
                )?;
                self.commit_native_backend(replacement, state.to_vec());
                return Ok(());
            }

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

#[cfg(test)]
mod ambisonics_custom_state_tests {
    use super::{
        AMBISONICS_CUSTOM_STATE_FIELD, NativeAmbisonicsCustomGeometry,
        NativeAmbisonicsCustomSpeaker, NativeAmbisonicsTargetLayout, NativePluginAudioSetup,
        PluginFormat, ambisonics_state_with_setup, audio_setup_structural_parameter_ids,
        validate_ambisonics_custom_agreement,
    };

    fn speaker(
        label: &str,
        azimuth_deg: f32,
        elevation_deg: f32,
        is_lfe: bool,
    ) -> NativeAmbisonicsCustomSpeaker {
        NativeAmbisonicsCustomSpeaker {
            label: label.to_owned(),
            azimuth_deg,
            elevation_deg,
            is_lfe,
        }
    }

    fn custom_geometry() -> NativeAmbisonicsCustomGeometry {
        NativeAmbisonicsCustomGeometry {
            name: "moved-lfe".to_owned(),
            speakers: vec![
                speaker("FL", 30.0, 0.0, false),
                speaker("FR", -30.0, 0.0, false),
                speaker("C", 0.0, 0.0, false),
                speaker("SL", 90.0, 0.0, false),
                speaker("SR", -90.0, 0.0, false),
                speaker("LFE", 0.0, 0.0, true),
            ],
        }
    }

    fn custom_setup() -> NativePluginAudioSetup {
        NativePluginAudioSetup::AmbisonicsCustom {
            order: 7,
            custom: custom_geometry(),
        }
    }

    fn named_setup() -> NativePluginAudioSetup {
        NativePluginAudioSetup::Ambisonics {
            order: 7,
            target_layout: NativeAmbisonicsTargetLayout::SevenOneFour,
        }
    }

    fn opaque_blob(format: PluginFormat, payload: serde_json::Value) -> Vec<u8> {
        let bytes = serde_json::to_vec(&payload).unwrap();
        match format {
            PluginFormat::Clap => {
                let length = u64::try_from(bytes.len()).unwrap();
                let mut blob = length.to_le_bytes().to_vec();
                blob.extend_from_slice(&bytes);
                blob
            }
            _ => bytes,
        }
    }

    fn payload_json(blob: &[u8], format: PluginFormat) -> serde_json::Value {
        let payload = match format {
            PluginFormat::Clap => {
                assert!(blob.len() > 8);
                let length = u64::from_le_bytes(blob[..8].try_into().unwrap());
                let length = usize::try_from(length).unwrap();
                assert_eq!(length, blob.len() - 8);
                &blob[8..]
            }
            _ => blob,
        };
        serde_json::from_slice(payload).unwrap()
    }

    fn named_payload() -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "params": {
                "order": {"i32": 7},
                "target_layout": {"i32": 5},
                "sentinel": 1
            },
            "fields": {}
        })
    }

    fn custom_payload() -> serde_json::Value {
        let mut payload = named_payload();
        payload["params"]["target_layout"] = serde_json::json!({"i32": 8});
        let fields = payload
            .get_mut("fields")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap();
        fields.insert(
            AMBISONICS_CUSTOM_STATE_FIELD.to_owned(),
            serde_json::Value::String(serde_json::to_string(&custom_geometry()).unwrap()),
        );
        payload
    }

    #[test]
    fn custom_agreement_accepts_matching_geometry_on_both_formats() {
        for format in [PluginFormat::Clap, PluginFormat::Vst3] {
            let blob = opaque_blob(format, custom_payload());
            validate_ambisonics_custom_agreement(&blob, format, "ambisonics", &custom_geometry())
                .unwrap();
        }
    }

    #[test]
    fn custom_agreement_rejects_mismatch_missing_and_malformed_field() {
        let mut other = custom_geometry();
        other.speakers.swap(0, 1);
        let mut payload = custom_payload();
        let fields = payload
            .get_mut("fields")
            .and_then(serde_json::Value::as_object_mut)
            .unwrap();
        fields.insert(
            AMBISONICS_CUSTOM_STATE_FIELD.to_owned(),
            serde_json::Value::String(serde_json::to_string(&other).unwrap()),
        );
        let error = validate_ambisonics_custom_agreement(
            &opaque_blob(PluginFormat::Vst3, payload),
            PluginFormat::Vst3,
            "ambisonics",
            &custom_geometry(),
        )
        .unwrap_err();
        assert!(
            error.contains("does not match the selected setup"),
            "unexpected: {error}"
        );
        let error = validate_ambisonics_custom_agreement(
            &opaque_blob(PluginFormat::Vst3, named_payload()),
            PluginFormat::Vst3,
            "ambisonics",
            &custom_geometry(),
        )
        .unwrap_err();
        assert!(
            error.contains("carries no custom geometry"),
            "unexpected: {error}"
        );
        let mut broken = custom_payload();
        broken["fields"][AMBISONICS_CUSTOM_STATE_FIELD] =
            serde_json::Value::String("not json".to_owned());
        let error = validate_ambisonics_custom_agreement(
            &opaque_blob(PluginFormat::Vst3, broken),
            PluginFormat::Vst3,
            "ambisonics",
            &custom_geometry(),
        )
        .unwrap_err();
        assert!(
            error.contains("failed to parse native Ambisonics custom geometry"),
            "unexpected: {error}"
        );
    }

    #[test]
    fn custom_agreement_rejects_broken_clap_framing() {
        let payload = serde_json::to_vec(&custom_payload()).unwrap();
        let error = validate_ambisonics_custom_agreement(
            &payload[..4],
            PluginFormat::Clap,
            "ambisonics",
            &custom_geometry(),
        )
        .unwrap_err();
        assert!(
            error.contains("missing its length prefix"),
            "unexpected: {error}"
        );
        let length = u64::try_from(payload.len() + 1).unwrap();
        let mut wrong_length = length.to_le_bytes().to_vec();
        wrong_length.extend_from_slice(&payload);
        let error = validate_ambisonics_custom_agreement(
            &wrong_length,
            PluginFormat::Clap,
            "ambisonics",
            &custom_geometry(),
        )
        .unwrap_err();
        assert!(error.contains("describes"), "unexpected: {error}");
    }

    #[test]
    fn custom_state_rewrite_installs_and_removes_geometry() {
        for format in [PluginFormat::Clap, PluginFormat::Vst3] {
            let rewritten = ambisonics_state_with_setup(
                &opaque_blob(format, named_payload()),
                format,
                &custom_setup(),
            )
            .unwrap();
            let value = payload_json(&rewritten, format);
            assert_eq!(value["params"]["order"], serde_json::json!({"i32": 7}));
            assert_eq!(
                value["params"]["target_layout"],
                serde_json::json!({"i32": 8})
            );
            assert_eq!(value["params"]["sentinel"], serde_json::json!(1));
            let field = value["fields"][AMBISONICS_CUSTOM_STATE_FIELD]
                .as_str()
                .unwrap();
            let decoded: NativeAmbisonicsCustomGeometry =
                serde_json::from_str(field).unwrap();
            assert_eq!(decoded, custom_geometry());
            let rewritten =
                ambisonics_state_with_setup(&rewritten, format, &named_setup()).unwrap();
            let value = payload_json(&rewritten, format);
            assert_eq!(
                value["params"]["target_layout"],
                serde_json::json!({"i32": 5})
            );
            assert!(
                value["fields"]
                    .get(AMBISONICS_CUSTOM_STATE_FIELD)
                    .is_none()
            );
            assert_eq!(value["params"]["sentinel"], serde_json::json!(1));
        }
    }

    #[test]
    fn custom_structural_ids_match_named() {
        let custom = custom_setup();
        let named = named_setup();
        for format in [PluginFormat::Clap, PluginFormat::Vst3] {
            let custom_ids =
                audio_setup_structural_parameter_ids(format, Some(&custom)).unwrap();
            let named_ids = audio_setup_structural_parameter_ids(format, Some(&named)).unwrap();
            assert_eq!(custom_ids, named_ids);
            assert_eq!(custom_ids.len(), 2);
        }
    }
}

#[cfg(test)]
mod crossover_structural_parameter_id_tests {
    use super::{PluginFormat, crossover_structural_parameter_ids};

    #[test]
    fn structural_controls_use_the_exposed_format_specific_numeric_ids() {
        let clap_ids = crossover_structural_parameter_ids(PluginFormat::Clap).unwrap();
        assert_eq!(
            clap_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            ["clap.3357091", "clap.1196016239", "clap.645889669"]
        );

        let vst3_ids = crossover_structural_parameter_ids(PluginFormat::Vst3).unwrap();
        assert_eq!(
            vst3_ids.iter().map(|id| id.as_str()).collect::<Vec<_>>(),
            ["vst3.3357091", "vst3.1196016239", "vst3.645889669"]
        );
    }

    #[test]
    fn crossover_structural_ids_are_unavailable_for_audio_unit() {
        assert!(crossover_structural_parameter_ids(PluginFormat::AudioUnit).is_err());
    }
}
