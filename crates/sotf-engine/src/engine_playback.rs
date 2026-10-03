//! DAW-engine sweep playback for capture takes.
//!
//! Implements `sotf_capture`'s [`SweepPlayback`] over [`AudioEngineManager`].
//! The playback sequence below moved verbatim from
//! `signal_recorder::record` (single-channel variant, including its
//! virtual-output allowance and rate-mismatch warning); only the log tag
//! is parameterized so multi-channel callers keep their own prefix.

use crate::engine::PluginConfig;
use crate::{AudioEngineManager, StreamingState};
use sotf_capture::signal_recorder::SweepPlayback;
use sotf_capture::signal_recorder::check_output_channel;
use std::path::Path;

/// Engine-backed sweep playback with pre-extraction behavior.
///
/// `allow_virtual_output` preserves the historical single/multi
/// asymmetry: single-channel capture allowed virtual outputs
/// (loopback capture through BlackHole-style devices) while
/// multi-channel capture did not. `log_tag` keeps the original
/// `[record_and_analyze]` / `[record_and_analyze_multi]` prefixes.
pub struct EnginePlayback {
    manager: Option<AudioEngineManager>,
    allow_virtual_output: bool,
    log_tag: &'static str,
}

impl EnginePlayback {
    /// Idle backend; playback starts with [`SweepPlayback::start`].
    pub fn new(allow_virtual_output: bool, log_tag: &'static str) -> Self {
        Self {
            manager: None,
            allow_virtual_output,
            log_tag,
        }
    }
}

impl std::fmt::Debug for EnginePlayback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnginePlayback")
            .field("active", &self.manager.is_some())
            .field("allow_virtual_output", &self.allow_virtual_output)
            .field("log_tag", &self.log_tag)
            .finish()
    }
}

impl SweepPlayback for EnginePlayback {
    fn start(
        &mut self,
        wav: &Path,
        device: Option<&str>,
        channel: u16,
        sweep_rate: u32,
    ) -> Result<(), String> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let tag = self.log_tag;
        let mut manager = AudioEngineManager::new();
        if self.allow_virtual_output {
            // Allow virtual output devices (BlackHole, loopback) -
            // recording intentionally sends signal through a loopback or
            // to real speakers for mic capture.
            manager.set_allow_virtual_output(true);
        }
        manager
            .load_file(wav)
            .map_err(|e| format!("Failed to load file: {e}"))?;

        let host = cpal::default_host();
        let output_device = if let Some(dev_name) = device {
            log::info!("[{tag}] Looking for output device: {dev_name}");
            sotf_capture::devices::find_device(&host, dev_name, false)?
        } else {
            log::debug!("[{tag}] Using default output device");
            host.default_output_device()
                .ok_or_else(|| "No default output device available".to_string())?
        };

        log::info!(
            "[{tag}] Output device: {}",
            output_device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "Unknown Device".to_string())
        );

        // Hardware channel count keeps the original fallible query (not
        // the shared helper's default fallback) to preserve the error here.
        let hardware_channels = output_device
            .supported_output_configs()
            .map_err(|e| format!("Failed to get supported output configs: {e}"))?
            .map(|config| config.channels() as usize)
            .max()
            .unwrap_or_else(|| {
                output_device
                    .default_output_config()
                    .map(|cfg| cfg.channels() as usize)
                    .unwrap_or(2)
            });

        log::info!("[{tag}] Hardware output channels: {hardware_channels}");

        check_output_channel(channel, hardware_channels)?;

        log::info!(
            "[{tag}] Routing mono input (channel 0) to hardware output channel {channel} (0-indexed)"
        );

        // Matrix: 1 input x hardware_channels outputs, all zeros except
        // position [output_channel] = 1.0.
        let mut matrix = vec![0.0_f32; hardware_channels];
        matrix[channel as usize] = 1.0;

        let matrix_params = serde_json::json!({
            "input_channels": 1,
            "output_channels": hardware_channels,
            "matrix": matrix,
        });

        let plugins = vec![PluginConfig::new("matrix", matrix_params)];

        log::info!(
            "[{tag}] Matrix: 1 input -> {hardware_channels} outputs, channel {channel} active (rest silent)"
        );

        let actual_output_rate = crate::manager::select_output_sample_rate_for_channels(
            sweep_rate,
            device,
            hardware_channels,
        );
        if actual_output_rate != sweep_rate {
            log::warn!(
                "[{tag}] OUTPUT SAMPLE RATE MISMATCH: engine will use {actual_output_rate}Hz but sweep is at {sweep_rate}Hz (engine will resample)"
            );
        }

        manager
            .start_playback(device.map(str::to_string), plugins, hardware_channels)
            .map_err(|e| e.to_string())?;
        self.manager = Some(manager);
        Ok(())
    }

    fn is_finished(&mut self) -> bool {
        let manager = self
            .manager
            .as_mut()
            .expect("EnginePlayback::start must precede polling");
        manager.try_recv_event();
        manager.get_state() == StreamingState::Idle
    }

    fn stop(&mut self) -> Result<(), String> {
        if let Some(mut manager) = self.manager.take() {
            manager.stop().map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}
