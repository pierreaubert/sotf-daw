use super::external_plugin_state::{NativeBandSplitOutputLayout, NativePluginAudioSetup};
use super::native_crossover_layout::NativeCrossoverStructure;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{ProcessContext, TailLength};

#[derive(Debug, Clone)]
pub(super) struct NativePluginMetadata {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) vendor: String,
    pub(super) version: String,
    pub(super) input_channels: usize,
    pub(super) output_channels: usize,
}

/// Non-layout controls in the exact SOTF Ambisonics parameter schema.
///
/// These controls are intentionally hidden from generic automation, but they
/// are persisted plugin state and must survive a deliberate bus-layout change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeAmbisonicsControls {
    pub(super) max_re_weighting: bool,
    pub(super) dual_band: bool,
    pub(super) algorithm: i32,
}

/// Format-specific native instance owned by [`super::ExternalPlugin`].
///
/// Implementations must allocate all audio buffers during construction and
/// perform no allocation, locking, logging, or filesystem work in `process`.
pub(super) trait NativeExternalPluginBackend: Send {
    fn metadata(&self) -> &NativePluginMetadata;

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: &ParameterId, _value: &ParameterValue) -> Result<(), String> {
        Err(format!("native external parameter '{id}' is not exposed"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    /// Reads hidden structural controls for the supported Ambisonics plugin.
    ///
    /// These controls intentionally do not appear in the host automation list.
    /// `None` means this backend does not expose the native readback route.
    fn ambisonics_layout_parameters(&self) -> Result<Option<(i32, i32)>, String> {
        Ok(None)
    }

    /// Reads hidden, persistent Ambisonics controls from the native ABI.
    /// `None` means this is not the recognized SOTF Ambisonics plugin.
    fn ambisonics_controls(&self) -> Result<Option<NativeAmbisonicsControls>, String> {
        Ok(None)
    }

    /// Reads the selected band count and native bus representation for the
    /// exact supported BandSplit plugin. `None` means the schema is absent.
    fn band_split_layout_parameters(
        &self,
    ) -> Result<Option<(i32, NativeBandSplitOutputLayout)>, String> {
        Ok(None)
    }

    /// Reads structural state for the exact supported SOTF Crossover plugin.
    /// `None` means this backend does not expose the recognized schema.
    fn crossover_layout_parameters(&self) -> Result<Option<NativeCrossoverStructure>, String> {
        Ok(None)
    }

    /// Renegotiates a recognized Ambisonics instance while it is deactivated.
    /// The caller only invokes this on a disposable replacement candidate.
    fn reconfigure_ambisonics_audio_setup(
        &mut self,
        _setup: &super::external_plugin_state::NativePluginAudioSetup,
    ) -> Result<(), String> {
        Err("native backend does not support deliberate Ambisonics reconfiguration".into())
    }

    /// Selects a deliberate BandSplit route on a disposable candidate instance.
    fn reconfigure_band_split_audio_setup(
        &mut self,
        _setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        Err("native backend does not support deliberate BandSplit reconfiguration".into())
    }

    /// Selects a deliberate Crossover layout on a disposable candidate.
    fn reconfigure_crossover_audio_setup(
        &mut self,
        _setup: &NativePluginAudioSetup,
    ) -> Result<(), String> {
        Err("native backend does not support deliberate Crossover reconfiguration".into())
    }

    /// Reports queued parameter delivery without native calls or allocation.
    ///
    /// The owner uses this to invalidate editor state after processor delivery;
    /// snapshot capture remains on the serialized control thread.
    fn has_pending_parameter_updates(&self) -> bool {
        false
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        input_channels: usize,
        output_channels: usize,
        context: &ProcessContext,
    ) -> Result<(), String>;

    fn save_state(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }

    fn load_state(&mut self, _state: &[u8]) -> Result<(), String> {
        Ok(())
    }

    /// Clear native processing history without changing persisted parameters.
    fn reset(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn latency_samples(&self) -> usize {
        0
    }

    /// Native callback APIs process a fixed frame count. Backends opt in only
    /// when their format wrapper verifies that contract.
    fn guarantees_identity_frame_geometry(&self) -> bool {
        false
    }

    /// Conservative native zero-input response bound.
    fn tail_length(&self) -> TailLength {
        TailLength::Unknown
    }

    /// Refresh metadata with format-specific thread restrictions. The caller
    /// must be the serialized plugin control thread, never an audio callback.
    fn refresh_tail_length(&mut self) -> TailLength {
        self.tail_length()
    }
}

pub(super) fn native_parameter_id(id: &str) -> u32 {
    let mut hash = 0_u32;
    for byte in id.bytes() {
        hash = hash.wrapping_mul(31).wrapping_add(u32::from(byte));
    }

    // NIH-plug clears this bit because VST3 reserves it for host parameters.
    hash & !(1 << 31)
}
