//! Host-driven engine for DAWs and other external-clock embedders.
//!
//! Unlike [`super::AudioEngine`], this type owns no device and spawns no audio
//! pipeline threads. The host calls [`EmbeddedAudioEngine::process_at`] from its
//! render callback and therefore remains the sole owner of timing and I/O.

use super::processing_thread::build_plugin_host_with_policy;
use crate::{EngineConfig, PluginBuildDiagnostic};
use sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqControlHandle;
use sotf_plugins::plugin_linear_phase_eq::{
    BandConfig, CommitRefusal, LinearPhaseEqPlugin, LiveFilterSnapshot, PreparedBandUpdate,
};
use sotf_plugins::{Host, ParameterEventSender, ParameterValue, Plugin, PluginHost};
use std::sync::Arc;

/// Allocation-free-after-build, externally-clocked plugin engine.
pub struct EmbeddedAudioEngine {
    host: PluginHost,
    input_sample_rate: u32,
    max_block_frames: usize,
}

impl EmbeddedAudioEngine {
    /// Build the configured graph without opening an audio device.
    ///
    /// Non-fatal skipped-plugin diagnostics are returned alongside the engine.
    pub fn new(
        config: &EngineConfig,
    ) -> Result<(Self, Vec<PluginBuildDiagnostic>), PluginBuildDiagnostic> {
        config.validate().map_err(PluginBuildDiagnostic::host)?;
        let (host, diagnostics) = build_plugin_host_with_policy(
            &config.plugins,
            config.output_sample_rate,
            config.input_channels,
            config.oversampling_policy,
        )?;
        Ok((
            Self {
                host,
                input_sample_rate: config.output_sample_rate,
                max_block_frames: config.frame_size,
            },
            diagnostics,
        ))
    }

    /// Process one host-owned render block at an absolute sample position.
    ///
    /// `input` and `output` are interleaved. The caller must size `output` for
    /// `output_frames_for_input(input_frames) * output_channels()` samples.
    /// No device, sleeping, locks, or internal clock are involved.
    pub fn process_at(
        &mut self,
        sample_position: u64,
        input: &[f32],
        output: &mut [f32],
    ) -> Result<usize, String> {
        let input_channels = self.host.input_channels();
        if input_channels == 0 || !input.len().is_multiple_of(input_channels) {
            return Err(format!(
                "embedded input has {} samples, not a whole number of {}-channel frames",
                input.len(),
                input_channels
            ));
        }
        let input_frames = input.len() / input_channels;
        if input_frames > self.max_block_frames {
            return Err(format!(
                "embedded block has {input_frames} frames, exceeding configured maximum {}",
                self.max_block_frames
            ));
        }
        let required = self
            .host
            .output_frames_for_input(input_frames)
            .checked_mul(self.host.output_channels())
            .ok_or("embedded output size overflow")?;
        if output.len() < required {
            return Err(format!(
                "embedded output buffer too small: need {required} samples, got {}",
                output.len()
            ));
        }
        self.host.set_playback_position(sample_position)?;
        self.host.process(input, &mut output[..required])
    }

    /// Queue sample-accurate automation within the next render block.
    pub fn set_plugin_parameter_at(
        &mut self,
        plugin_index: usize,
        param_id: &str,
        value: ParameterValue,
        sample_offset: usize,
    ) -> Result<(), String> {
        self.host
            .validate_automatable_plugin_parameter(plugin_index, param_id, &value)?;
        self.host
            .set_plugin_parameter_at(plugin_index, param_id, value, sample_offset)
    }

    /// Move the bounded automation producer to a control thread.
    ///
    /// Once taken, automation should be queued through the returned sender;
    /// the render callback consumes it without locking.
    pub fn take_parameter_event_sender(&mut self) -> Option<ParameterEventSender> {
        self.host.take_parameter_event_sender()
    }

    /// Clone the shared linear-phase EQ control handle, if present.
    ///
    /// Control thread only. The `Arc` routes worker prepared updates and
    /// retired-bank reclamation without `&mut` engine access; audio never
    /// locks. Returns `None` for an out-of-range index or a non-EQ plugin.
    pub fn linear_phase_eq_handle(
        &self,
        plugin_index: usize,
    ) -> Option<Arc<LinearPhaseEqControlHandle>> {
        let data: Arc<dyn std::any::Any + Send + Sync> =
            Host::get_plugin_data(&self.host, plugin_index)?;
        Arc::downcast::<LinearPhaseEqControlHandle>(data).ok()
    }

    /// Capture a linear-phase EQ live snapshot for worker preparation.
    ///
    /// Control thread only; allocates the snapshot. Hand the snapshot plus a
    /// band edit to `LinearPhaseEqPlugin::prepare_band_update` on a worker,
    /// then deliver with `linear_phase_eq_submit` or the shared handle.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range index or a non-EQ plugin.
    pub fn linear_phase_eq_snapshot(
        &self,
        plugin_index: usize,
    ) -> Result<LiveFilterSnapshot, String> {
        let plugin: &dyn Plugin = self
            .host
            .get_plugin(plugin_index)
            .ok_or_else(|| format!("plugin index {plugin_index} out of bounds"))?;
        let wrapper = plugin
            .as_any()
            .and_then(|any| {
                any.downcast_ref::<
                    sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqDynamicPlugin,
                >()
            })
            .ok_or_else(|| format!("plugin index {plugin_index} is not a linear-phase EQ"))?;
        Ok(wrapper.snapshot_for_update())
    }

    /// Queue a worker-prepared EQ update for the next render quanta.
    ///
    /// Control thread only; nonblocking and bounded. On a full queue the
    /// payload drops on the calling thread with a loud error; live config
    /// and history stay untouched.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range index, a non-EQ plugin, or a
    /// full/busy queue.
    pub fn linear_phase_eq_submit(
        &self,
        plugin_index: usize,
        prepared: PreparedBandUpdate,
    ) -> Result<(), String> {
        let handle = self
            .linear_phase_eq_handle(plugin_index)
            .ok_or_else(|| format!("plugin index {plugin_index} is not a linear-phase EQ"))?;
        handle.try_submit(prepared)
    }

    /// Snapshot, prepare and queue one EQ band edit on control.
    ///
    /// Convenience for control callers without a dedicated worker. Uses the
    /// existing DSP preparation API, so validation matches the worker path.
    ///
    /// # Errors
    ///
    /// Returns snapshot, preparation (index, placement, range) or full-queue
    /// errors. All errors leave live state untouched.
    pub fn linear_phase_eq_request(
        &self,
        plugin_index: usize,
        band_index: usize,
        new_band: BandConfig,
    ) -> Result<(), String> {
        let base = self.linear_phase_eq_snapshot(plugin_index)?;
        let prepared = LinearPhaseEqPlugin::prepare_band_update(&base, band_index, new_band)?;
        self.linear_phase_eq_submit(plugin_index, prepared)
    }

    /// Drop reclaimable EQ retired banks on the calling thread.
    ///
    /// Control thread only. Returns the number reclaimed (zero when unavailable
    /// or contended; retry later). Call between renders after blends complete.
    pub fn linear_phase_eq_reclaim(&self, plugin_index: usize) -> usize {
        self.linear_phase_eq_handle(plugin_index)
            .map(|handle| handle.try_reclaim())
            .unwrap_or(0)
    }

    /// Report the last EQ audio commit refusal, if any.
    ///
    /// Control thread only. Reads the wrapper record through the existing
    /// `get_plugin` plus `as_any` `&self` pattern without locking. Returns
    /// `None` when no refusal is recorded or the plugin is unavailable.
    pub fn linear_phase_eq_last_refusal(&self, plugin_index: usize) -> Option<CommitRefusal> {
        let plugin: &dyn Plugin = self.host.get_plugin(plugin_index)?;
        plugin
            .as_any()
            .and_then(|any| {
                any.downcast_ref::<
                    sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqDynamicPlugin,
                >()
            })
            .and_then(|wrapper| wrapper.last_refusal())
    }

    /// Count retained EQ audio-slot updates (`0..=2`).
    ///
    /// Control thread only, same `&self` lookup as `linear_phase_eq_snapshot`.
    /// Returns zero when the plugin is unavailable. Use with
    /// `linear_phase_eq_last_refusal` to observe a wedged head, then recover
    /// with `linear_phase_eq_request_cancel`.
    pub fn linear_phase_eq_pending_len(&self, plugin_index: usize) -> usize {
        let Some(plugin) = self.host.get_plugin(plugin_index) else {
            return 0;
        };
        plugin
            .as_any()
            .and_then(|any| {
                any.downcast_ref::<
                    sotf_plugins::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqDynamicPlugin,
                >()
            })
            .map(|wrapper| wrapper.pending_len())
            .unwrap_or(0)
    }

    /// Request eviction of the EQ audio head slot.
    ///
    /// Lock-free control call recording one coalescing cancel generation on
    /// the shared handle. Audio evicts at most one head payload per quantum
    /// into a bounded outbox without dropping; observe completion via
    /// `linear_phase_eq_pending_len` plus `linear_phase_eq_reclaim_cancelled`
    /// count. Returns false when the plugin is unavailable.
    pub fn linear_phase_eq_request_cancel(&self, plugin_index: usize) -> bool {
        match self.linear_phase_eq_handle(plugin_index) {
            Some(handle) => {
                handle.request_cancel();
                true
            }
            None => false,
        }
    }

    /// Drop cancelled EQ head payloads on the calling thread.
    ///
    /// Control thread only. Returns the number destroyed off audio (zero when
    /// unavailable, empty, or contended; retry later).
    pub fn linear_phase_eq_reclaim_cancelled(&self, plugin_index: usize) -> usize {
        self.linear_phase_eq_handle(plugin_index)
            .map(|handle| handle.try_reclaim_cancelled())
            .unwrap_or(0)
    }

    /// Read an accepted linear-phase EQ band gain in dB.
    ///
    /// Control thread only; reports the committed live config, never a queued
    /// edit whose render has not changed.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range plugin or band index, a non-EQ
    /// plugin, or an unreadable control.
    pub fn linear_phase_eq_band_gain(
        &self,
        plugin_index: usize,
        band_index: usize,
    ) -> Result<f32, String> {
        use sotf_plugins::{ParameterId, ParameterValue};
        let plugin: &dyn Plugin = self
            .host
            .get_plugin(plugin_index)
            .ok_or_else(|| format!("plugin index {plugin_index} out of bounds"))?;
        let id = ParameterId::from(format!("band_{band_index}_gain").as_str());
        match plugin.get_parameter(&id) {
            Some(ParameterValue::Float(gain)) => Ok(gain),
            other => Err(format!(
                "plugin index {plugin_index} band {band_index} gain unreadable: {other:?}"
            )),
        }
    }

    pub fn input_channels(&self) -> usize {
        self.host.input_channels()
    }

    pub fn output_channels(&self) -> usize {
        self.host.output_channels()
    }

    pub fn output_sample_rate(&self) -> Result<u32, String> {
        self.host.output_sample_rate_native(self.input_sample_rate)
    }

    pub fn output_frames_for_input(&self, input_frames: usize) -> usize {
        self.host.output_frames_for_input(input_frames)
    }

    pub fn latency_samples(&self) -> usize {
        self.host.total_latency_samples()
    }

    /// Reset plugin history after a transport discontinuity or loop jump.
    pub fn reset_transport(&mut self, sample_position: u64) {
        self.host.reset();
        self.host
            .set_playback_position(sample_position)
            .expect("host reset reopens sink lifecycle before repositioning");
    }

    /// Maximum input block size accepted without growing host scratch storage.
    pub fn max_block_frames(&self) -> usize {
        self.max_block_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginConfig;

    #[test]
    fn external_clock_and_sample_accurate_automation_drive_rendering() {
        let config = EngineConfig {
            frame_size: 8,
            plugins: vec![PluginConfig {
                plugin_type: "gain".to_string(),
                parameters: serde_json::json!({ "gain_db": 0.0 }),
            }],
            ..EngineConfig::default()
        };
        let (mut engine, diagnostics) = EmbeddedAudioEngine::new(&config).unwrap();
        assert!(diagnostics.is_empty());
        engine
            .set_plugin_parameter_at(0, "gain_db", ParameterValue::Float(-6.0), 4)
            .unwrap();

        let input = vec![1.0; 16];
        let mut output = vec![0.0; 16];
        assert_eq!(engine.process_at(96_000, &input, &mut output).unwrap(), 8);
        assert!(
            output[..8]
                .iter()
                .all(|sample| (*sample - 1.0).abs() < 1e-5)
        );
        // The gain plugin smooths realtime changes, so it intentionally does
        // not jump to the final -6 dB value. The externally scheduled boundary
        // still guarantees the first four stereo frames are untouched and the
        // following segment begins moving toward the new value.
        assert!(output[8..].iter().any(|sample| *sample < 0.9999));
    }
}
