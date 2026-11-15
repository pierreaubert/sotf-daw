//! DAW-engine sweep playback for capture takes.
//!
//! Implements `sotf_capture`'s [`SweepPlayback`] over [`AudioEngineManager`].
//! Routes mono stimuli to a selected hardware channel, preserving the
//! capture caller's virtual-output policy and diagnostic log prefix.

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
    stream_width: Option<u16>,
}

impl EnginePlayback {
    /// Idle backend; playback starts with [`SweepPlayback::start`].
    pub fn new(allow_virtual_output: bool, log_tag: &'static str) -> Self {
        Self {
            manager: None,
            allow_virtual_output,
            log_tag,
            stream_width: None,
        }
    }

    /// Declare the layout/sink width stream indices are meant
    /// against (see [`SweepPlayback::set_stream_width`]); builder
    /// form for the inline `&mut EnginePlayback::new(..)` call sites.
    #[must_use]
    pub fn with_stream_width(mut self, width: Option<u16>) -> Self {
        self.stream_width = width;
        self
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
        let supported_configs: Vec<_> = output_device
            .supported_output_configs()
            .map_err(|e| format!("Failed to get supported output configs: {e}"))?
            .collect();
        let hardware_channels = supported_configs
            .iter()
            .map(|config| config.channels() as usize)
            .max()
            .unwrap_or_else(|| {
                output_device
                    .default_output_config()
                    .map(|cfg| cfg.channels() as usize)
                    .unwrap_or(2)
            });

        // Effective stream width (sotf#258): the caller's declared
        // layout width when set; otherwise, on software PCMs whose
        // enumerated maximum is fictional (PipeWire/Pulse/plug report
        // every count up to 64), the device default width; on native
        // hardware the enumerated maximum as before. Opening at an
        // arbitrary enumerated width lets the server reassign channel
        // positions by count.
        let width_is_fictional = sotf_capture::devices::has_software_pcm_signature(
            &supported_configs
                .iter()
                .map(|config| config.channels())
                .collect::<Vec<_>>(),
            supported_configs
                .iter()
                .map(|config| config.max_sample_rate())
                .max()
                .unwrap_or(0),
        );
        let stream_width = match self.stream_width {
            Some(width) => usize::from(width),
            None if width_is_fictional => output_device
                .default_output_config()
                .map(|cfg| cfg.channels() as usize)
                .unwrap_or(2),
            None => hardware_channels,
        }
        .max(usize::from(channel) + 1);
        if stream_width > hardware_channels && !width_is_fictional {
            return Err(format!(
                "stream width {stream_width} exceeds hardware channel count {hardware_channels}"
            ));
        }

        log::info!(
            "[{tag}] Hardware output channels: {hardware_channels}, stream width: {stream_width}"
        );

        log::info!(
            "[{tag}] Routing mono input (channel 0) to hardware output channel {channel} (0-indexed)"
        );

        let route = sweep_route(channel, hardware_channels, stream_width, sweep_rate)?;
        let plugins = vec![route];

        let actual_output_rate = crate::manager::select_output_sample_rate_for_channels(
            sweep_rate,
            device,
            stream_width,
        );
        if actual_output_rate != sweep_rate {
            log::warn!(
                "[{tag}] OUTPUT SAMPLE RATE MISMATCH: engine will use {actual_output_rate}Hz but sweep is at {sweep_rate}Hz (engine will resample)"
            );
        }

        manager
            .start_playback(device.map(str::to_string), plugins, stream_width)
            .map_err(|e| e.to_string())?;
        self.manager = Some(manager);
        Ok(())
    }

    fn set_stream_width(&mut self, width: Option<u16>) {
        self.stream_width = width;
    }

    fn is_finished(&mut self) -> bool {
        let manager = self
            .manager
            .as_mut()
            .expect("EnginePlayback::start must precede polling");
        manager.try_recv_event();
        manager.get_state() == StreamingState::Idle
    }

    fn check_health(&self) -> Result<(), String> {
        // Without this override the trait default reports healthy
        // forever: an output stream that died in the playback thread
        // (busy hw: PCM under PipeWire, unplugged DAC) looked exactly
        // like a finished sweep, and the capture failed later with a
        // misleading "no signal lock" (sotf#247).
        let Some(manager) = &self.manager else {
            return Ok(());
        };
        if manager.get_state() == StreamingState::Error {
            let detail = manager
                .get_engine_state()
                .last_error
                .filter(|error| !error.is_empty())
                .unwrap_or_else(|| "unknown engine error".to_string());
            return Err(sotf_capture::signal_recorder::actionable_capture_error(
                &format!("[{}] Playback failed", self.log_tag),
                &detail,
            ));
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        if let Some(mut manager) = self.manager.take() {
            manager.stop().map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}

// Build the route matrix at the *opened stream width*. It used to
// span only channel+1 logical channels and rely on playback padding;
// but the frame writer treats a 2-channel logical frame as stereo
// program material and upmixes it across a wider stream, and a
// stream negotiated from the enumerated maximum gets its positions
// reassigned by count on server PCMs (sotf#258). Devices wider than
// the matrix plugin's 128-channel limit keep the truncated matrix
// and the frame writer's index-preserving zero pad.
fn sweep_route(
    channel: u16,
    hardware_channels: usize,
    stream_width: usize,
    sample_rate: u32,
) -> Result<PluginConfig, String> {
    check_output_channel(channel, hardware_channels)?;
    // Matrix plugin limit (see the create_plugin validation below).
    const MATRIX_MAX_CHANNELS: usize = 128;
    let output_channels = if stream_width <= MATRIX_MAX_CHANNELS {
        stream_width.max(usize::from(channel) + 1)
    } else {
        usize::from(channel) + 1
    };
    let mut matrix = vec![0.0_f32; output_channels];
    matrix[usize::from(channel)] = 1.0;
    let config = PluginConfig::new(
        "matrix",
        serde_json::json!({
            "input_channels": 1,
            "output_channels": output_channels,
            "matrix": matrix,
        }),
    );
    // The general engine can skip invalid plugins. Capture must fail before
    // starting instead of silently playing its stimulus on the wrong channel.
    sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, sample_rate)
        .map_err(|error| format!("Cannot route sweep to output channel {channel}: {error}"))?;
    Ok(config)
}

/// Measurement playback backend chosen per output device.
///
/// Server/virtual sinks (PipeWire/Pulse through ALSA) get the native
/// PipeWire backend when compiled in — it declares explicit channel
/// positions, so routing survives sinks whose position order is not
/// ALSA's. Everything else uses the engine backend as before. Call
/// sites construct this instead of [`EnginePlayback`] directly so
/// the choice cannot drift between frontends.
pub enum MeasurementPlayback {
    /// DAW-engine backend (cpal/ALSA sink).
    Engine(EnginePlayback),
    /// Native PipeWire backend (Linux, `pipewire-io` feature).
    #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
    PipeWire(sotf_capture::signal_recorder::PipeWirePlayback),
}

impl std::fmt::Debug for MeasurementPlayback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(backend) => f.debug_tuple("Engine").field(backend).finish(),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => f.debug_tuple("PipeWire").field(backend).finish(),
        }
    }
}

impl MeasurementPlayback {
    /// Pick the backend for `device` (display name or stable id as
    /// passed to [`SweepPlayback::start`]; `None` = system default).
    pub fn new_for_device(
        allow_virtual_output: bool,
        log_tag: &'static str,
        device: Option<&str>,
    ) -> Self {
        #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
        {
            if sotf_capture::signal_recorder::pipewire_backend_applies(device) {
                log::info!("[{log_tag}] Using native PipeWire playback for device {device:?}");
                return Self::PipeWire(sotf_capture::signal_recorder::PipeWirePlayback::new());
            }
        }
        let _ = device;
        Self::Engine(EnginePlayback::new(allow_virtual_output, log_tag))
    }

    /// Builder form of [`SweepPlayback::set_stream_width`].
    #[must_use]
    pub fn with_stream_width(mut self, width: Option<u16>) -> Self {
        self.set_stream_width(width);
        self
    }
}

impl SweepPlayback for MeasurementPlayback {
    fn start(
        &mut self,
        wav: &Path,
        device: Option<&str>,
        channel: u16,
        sweep_rate: u32,
    ) -> Result<(), String> {
        match self {
            Self::Engine(backend) => backend.start(wav, device, channel, sweep_rate),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => backend.start(wav, device, channel, sweep_rate),
        }
    }

    fn set_stream_width(&mut self, width: Option<u16>) {
        match self {
            Self::Engine(backend) => backend.set_stream_width(width),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => backend.set_stream_width(width),
        }
    }

    fn is_finished(&mut self) -> bool {
        match self {
            Self::Engine(backend) => backend.is_finished(),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => backend.is_finished(),
        }
    }

    fn check_health(&self) -> Result<(), String> {
        match self {
            Self::Engine(backend) => backend.check_health(),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => backend.check_health(),
        }
    }

    fn stop(&mut self) -> Result<(), String> {
        match self {
            Self::Engine(backend) => backend.stop(),
            #[cfg(all(target_os = "linux", target_env = "gnu", feature = "pipewire-io"))]
            Self::PipeWire(backend) => backend.stop(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sweep_route;
    use sotf_plugins::ProcessContext;

    #[test]
    fn sweep_routes_on_wide_devices_preserve_selected_channel() {
        for channel in [0_u16, 1, 3, 127] {
            let config = sweep_route(channel, 130, 130, 48_000).unwrap();
            let mut plugin =
                sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000)
                    .unwrap();
            plugin.initialize(48_000.0).unwrap();
            let channels = plugin.output_channels();
            let input = [0.25_f32, -0.5, 0.75];
            let mut output = vec![f32::NAN; channels * input.len()];
            plugin
                .process(
                    &input,
                    &mut output,
                    &ProcessContext::new(48_000, input.len()),
                )
                .unwrap();
            for (frame, expected) in output.chunks_exact(channels).zip(input) {
                for (index, sample) in frame.iter().enumerate() {
                    assert_eq!(
                        *sample,
                        if index == usize::from(channel) {
                            expected
                        } else {
                            0.0
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn sweep_route_spans_the_opened_stream_width() {
        // A matrix truncated to channel+1 makes the frame writer see
        // a 2-channel logical frame for channel 1 and upmix it as
        // stereo program across the wide stream (sotf#258 family).
        // The matrix must span the opened width instead.
        let config = sweep_route(1, 8, 8, 48_000).unwrap();
        let plugin =
            sotf_plugins::create_plugin(&config.plugin_type, &config.parameters, 1, 48_000)
                .unwrap();
        assert_eq!(plugin.output_channels(), 8);
    }

    #[test]
    fn unsupported_sweep_routes_fail_before_playback() {
        assert!(sweep_route(2, 2, 2, 48_000).is_err());
        let error = sweep_route(128, 130, 130, 48_000).unwrap_err();
        assert!(
            error.contains("Cannot route sweep to output channel 128"),
            "{error}"
        );
        assert!(
            error.contains("Matrix channel counts must be <= 128"),
            "{error}"
        );
    }
}
