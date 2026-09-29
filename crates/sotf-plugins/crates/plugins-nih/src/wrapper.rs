//! Macro-based wrapper that generates nih-plug Plugin implementations for SOTF plugins.

#[doc(hidden)]
pub mod transport;

#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_sample_accurate {
    ("Gain") => {
        true
    };
    ("Gate") => {
        true
    };
    ("EQ") => {
        true
    };
    ("LinearPhaseEQ") => {
        true
    };
    ($other:literal) => {
        false
    };
}

/// DSP channel counts for the fixed layouts exported by the NIH binaries.
#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_io_channels {
    ("MonoToStereo", $channels:literal) => {
        (1usize, 2usize)
    };
    ("AmbisonicsDecoder", $channels:literal) => {
        (4usize, 6usize)
    };
    ("Upmixer", $channels:literal) => {
        (2usize, 6usize)
    };
    ("AAE", $channels:literal) => {
        (2usize, 6usize)
    };
    ("BandSplit", $channels:literal) => {
        (2usize, 4usize)
    };
    ("BandMerge", $channels:literal) => {
        (4usize, 2usize)
    };
    ("AEC", $channels:literal) => {
        (2usize, 1usize)
    };
    ("Beamformer", $channels:literal) => {
        (2usize, 1usize)
    };
    ($other:literal, $channels:literal) => {
        ($channels as usize, $channels as usize)
    };
}

/// Default DSP input/output counts for the packaged plugin layouts.
pub fn plugin_io_channels(plugin_type: &str) -> (usize, usize) {
    match plugin_type {
        "MonoToStereo" => (1, 2),
        "AmbisonicsDecoder" => (4, 6),
        "Upmixer" | "AAE" => (2, 6),
        "BandSplit" => (2, 4),
        "BandMerge" => (4, 2),
        "AEC" | "Beamformer" => (2, 1),
        _ => (2, 2),
    }
}

/// Append Gate's key bus without renumbering existing CLAP configurations.
#[doc(hidden)]
#[macro_export]
macro_rules! sotf_nih_layouts {
    ("Gate", $default:expr) => {{
        const DEFAULT: nih_plug::prelude::AudioIOLayout = $default;
        &[
            DEFAULT,
            nih_plug::prelude::AudioIOLayout {
                aux_input_ports: &[nih_plug::audio_setup::new_nonzero_u32(2)],
                names: nih_plug::audio_setup::PortNames {
                    layout: Some("Stereo + Sidechain"),
                    aux_inputs: &["Sidechain"],
                    ..DEFAULT.names
                },
                ..DEFAULT
            },
        ]
    }};
    ($other:literal, $default:expr) => {
        &[$default]
    };
}

/// Factory channel argument. BandMerge historically takes the output width.
pub fn plugin_constructor_channels(plugin_type: &str) -> usize {
    match plugin_type {
        "MonoToStereo" => 1,
        "AmbisonicsDecoder" => 4,
        _ => 2,
    }
}

/// Generate a complete nih-plug plugin struct from SOTF plugin metadata.
///
/// This macro creates a struct that:
/// - Implements `nih_plug::Plugin`, `Vst3Plugin`, and `ClapPlugin`
/// - Creates the SOTF plugin via `plugins_bridge::create_plugin()`
/// - Handles interleave/deinterleave for buffer conversion
/// - Syncs nih-plug parameters to SOTF plugin parameters
#[macro_export]
macro_rules! sotf_nih_plugin {
    (
        $struct_name:ident,
        plugin_type: $plugin_type:tt,
        name: $name:literal,
        clap_id: $clap_id:literal,
        vst3_class_id: $vst3_id:expr,
        channels: $channels:literal
    ) => {
        pub struct $struct_name {
            params: std::sync::Arc<$crate::params::DynamicParams>,
            inner: Option<Box<dyn sotf_host::plugin::Plugin>>,
            bridge: $crate::PluginBridgeWrapper,
            interleaved_in: Vec<f32>,
            interleaved_out: Vec<f32>,
            max_frames: usize,
            sample_rate: u32,
            structural_fingerprint: u64,
            transport: $crate::wrapper::transport::TransportTracker,
        }

        impl Default for $struct_name {
            fn default() -> Self {
                let specs = $crate::wrapper::get_param_specs($plugin_type);
                let bridge_inner = plugins_bridge::param_bridge::ParamBridge::new(specs);

                // Build param infos for DynamicParams
                let mut infos = Vec::new();
                for i in 0..bridge_inner.count() {
                    if let Some(info) = bridge_inner.info(i) {
                        infos.push(info);
                    }
                }

                // If no ParamSpec params, or when LinearPhaseEQ needs its
                // dynamic band schema in addition to global specs, inspect a
                // temporary uninitialized instance on the control thread.
                if (infos.is_empty() || matches!($plugin_type, "LinearPhaseEQ" | "EQ"))
                    && let Ok(plugin) = plugins_bridge::create_plugin(
                        $plugin_type,
                        $crate::wrapper::plugin_constructor_channels($plugin_type),
                        48000,
                        &$crate::wrapper::default_plugin_config($plugin_type),
                    )
                {
                    for param in plugin.parameters() {
                        if infos.iter().any(|info| info.id == param.id.as_str()) {
                            continue;
                        }
                        if let Some(info) = $crate::wrapper::bridged_info_from_parameter(&param) {
                            infos.push(info);
                        }
                    }
                }

                // Expose pre-migration ids to DAW hosts; internal sync and
                // construction translate back to canonical keys.
                for info in &mut infos {
                    info.id = $crate::wrapper::legacy_external_param_id($plugin_type, &info.id)
                        .to_string();
                }

                Self {
                    params: $crate::params::DynamicParams::from_infos(&infos),
                    inner: None,
                    bridge: $crate::PluginBridgeWrapper::new(bridge_inner),
                    interleaved_in: Vec::new(),
                    interleaved_out: Vec::new(),
                    max_frames: 0,
                    sample_rate: 48000,
                    structural_fingerprint: 0,
                    transport: $crate::wrapper::transport::TransportTracker::default(),
                }
            }
        }

        impl nih_plug::prelude::Plugin for $struct_name {
            const NAME: &'static str = $name;
            const VENDOR: &'static str = "SOTF / Spinorama";
            const URL: &'static str = "https://spinorama.org";
            const EMAIL: &'static str = "";
            const VERSION: &'static str = env!("CARGO_PKG_VERSION");
            // nih-plug splits process buffers at host automation boundaries
            // when this is enabled. The per-slice sync below therefore stamps
            // AsyncTimelinePlugin events at the exact absolute frame instead
            // of collapsing automation to the original callback start.
            const SAMPLE_ACCURATE_AUTOMATION: bool =
                $crate::sotf_nih_sample_accurate!($plugin_type);
            fn tail_length(&self) -> Option<u32> {
                Some($crate::wrapper::native_tail_samples(
                    self.inner.as_ref().map_or(sotf_host::TailLength::Unknown, |plugin| plugin.tail_length()),
                ))
            }
            const AUDIO_IO_LAYOUTS: &'static [nih_plug::prelude::AudioIOLayout] =
                $crate::sotf_nih_layouts!($plugin_type, nih_plug::prelude::AudioIOLayout {
                    main_input_channels: std::num::NonZeroU32::new({
                        let (inputs, outputs) =
                            $crate::sotf_nih_io_channels!($plugin_type, $channels);
                        if inputs < outputs {
                            inputs as u32
                        } else {
                            outputs as u32
                        }
                    }),
                    main_output_channels: std::num::NonZeroU32::new(
                        $crate::sotf_nih_io_channels!($plugin_type, $channels).1 as u32,
                    ),
                    // NIH's main Buffer contains only output channels. Put
                    // additional input channels on a separate bus so none are lost.
                    aux_input_ports: {
                        const IO: (usize, usize) =
                            $crate::sotf_nih_io_channels!($plugin_type, $channels);
                        if IO.0 > IO.1 {
                            &[nih_plug::audio_setup::new_nonzero_u32((IO.0 - IO.1) as u32)]
                        } else {
                            &[]
                        }
                    },
                    aux_output_ports: &[],
                    ..nih_plug::prelude::AudioIOLayout::const_default()
                });

            type SysExMessage = ();
            type BackgroundTask = ();

            fn params(&self) -> std::sync::Arc<dyn nih_plug::prelude::Params> {
                self.params.clone()
            }

            fn initialize(
                &mut self,
                audio_io_layout: &nih_plug::prelude::AudioIOLayout,
                buffer_config: &nih_plug::prelude::BufferConfig,
                context: &mut impl nih_plug::prelude::InitContext<Self>,
            ) -> bool {
                if !Self::AUDIO_IO_LAYOUTS.contains(audio_io_layout) {
                    return false;
                }
                self.sample_rate = buffer_config.sample_rate as u32;
                let max_frames = buffer_config.max_buffer_size as usize;

                match $crate::params::configuration::create_plugin(
                    $plugin_type,
                    self.sample_rate,
                    &self.params,
                ) {
                    Ok(mut plugin) => {
                        let input_channels = plugin.input_channels();
                        let output_channels = plugin.output_channels();
                        let main_inputs = audio_io_layout.main_input_channels
                            .map_or(0, |channels| channels.get() as usize);
                        let total_inputs = main_inputs + audio_io_layout.aux_input_ports
                            .iter().map(|channels| channels.get() as usize).sum::<usize>();
                        // Gate's internal detector ignores the optional key bus.
                        // External detection requires that the host selected it.
                        let ignores_key_bus = matches!($plugin_type, "Gate")
                            && input_channels == main_inputs;
                        if (input_channels != total_inputs && !ignores_key_bus)
                            || Some(output_channels as u32)
                                != audio_io_layout.main_output_channels.map(|channels| channels.get())
                        {
                            log::error!(
                                "{} DSP channels do not match the declared host layout",
                                $plugin_type
                            );
                            return false;
                        }

                        plugin = match plugins_bridge::prepare_standalone_plugin(plugin, max_frames) {
                            Ok(plugin) => plugin,
                            Err(error) => {
                                log::error!("Failed to prepare {}: {error}", $plugin_type);
                                return false;
                            }
                        };
                        if matches!($plugin_type, "LinearPhaseEQ") {
                            plugin = match sotf_host::AsyncTimelinePlugin::new(
                                plugin,
                                self.sample_rate,
                                max_frames,
                            ) {
                                Ok(adapter) => Box::new(adapter),
                                Err(e) => {
                                    log::error!(
                                        "Failed to initialize {} adapter: {e}",
                                        $plugin_type
                                    );
                                    return false;
                                }
                            };
                        } else if let Err(e) = plugin.initialize(self.sample_rate) {
                            log::error!("Failed to initialize {}: {e}", $plugin_type);
                            return false;
                        }

                        // Validate the complete saved state on the control thread.
                        // Realtime values may depend on structural settings, such
                        // as the limiter requiring a fully wet mix in ISP mode.
                        if let Err(error) = self.params.sync_to_plugin(plugin.as_mut()) {
                            log::error!("Failed to restore {} parameters: {error}", $plugin_type);
                            return false;
                        }

                        let latency = match u32::try_from(plugin.latency_samples()) {
                            Ok(latency) => latency,
                            Err(_) => {
                                log::error!("{} latency does not fit the host ABI", $plugin_type);
                                return false;
                            }
                        };
                        context.set_latency_samples(latency);

                        self.interleaved_in = vec![0.0; max_frames * input_channels];
                        self.interleaved_out = vec![0.0; max_frames * output_channels];
                        self.max_frames = max_frames;
                        self.structural_fingerprint = self.params.structural_fingerprint();
                        self.inner = Some(plugin);
                        self.transport = $crate::wrapper::transport::TransportTracker::default();
                        true
                    }
                    Err(e) => {
                        log::error!("Failed to create {}: {e}", $plugin_type);
                        false
                    }
                }
            }

            fn process(
                &mut self,
                buffer: &mut nih_plug::prelude::Buffer,
                aux: &mut nih_plug::prelude::AuxiliaryBuffers,
                context: &mut impl nih_plug::prelude::ProcessContext<Self>,
            ) -> nih_plug::prelude::ProcessStatus {
                self.process_with_transport(buffer, aux, context.transport().into())
            }

            fn reset(&mut self) {
                self.transport = $crate::wrapper::transport::TransportTracker::default();
                if let Some(plugin) = self.inner.as_mut() {
                    plugin.reset();
                }
            }
        }

        impl $struct_name {
            fn process_with_transport(
                &mut self,
                buffer: &mut nih_plug::prelude::Buffer,
                aux: &mut nih_plug::prelude::AuxiliaryBuffers,
                transport: $crate::wrapper::transport::NativeTransport,
            ) -> nih_plug::prelude::ProcessStatus {
                let plugin = match self.inner.as_mut() {
                    Some(p) => p,
                    None => return nih_plug::prelude::ProcessStatus::Error("Not initialized"),
                };

                let num_frames = buffer.samples();
                let num_channels = buffer.channels();
                let input_channels = plugin.input_channels();
                let output_channels = plugin.output_channels();
                let expected_channels = output_channels;

                // A misbehaving host may hand us a block larger than the
                // negotiated `max_buffer_size` or with an unexpected channel
                // count. The scratch vectors are sized from `initialize()`,
                // so reject the block with silence instead of indexing them
                // out of bounds (mirrors the FFI crate's BufferTooSmall path).
                let needed = match $crate::wrapper::check_host_block(
                    num_frames,
                    num_channels,
                    self.max_frames,
                    expected_channels,
                ) {
                    Some(needed) => needed,
                    None => {
                        for channel in buffer.as_slice() {
                            channel.fill(0.0);
                        }
                        return nih_plug::prelude::ProcessStatus::Error(
                            "Host block exceeds the negotiated maximum",
                        );
                    }
                };
                if num_frames
                    .checked_mul(input_channels)
                    .is_none_or(|samples| samples > self.interleaved_in.len())
                    || needed > self.interleaved_out.len()
                    || (input_channels > output_channels
                        && (aux.inputs.len() != 1
                            || aux.inputs[0].channels() != input_channels - output_channels
                            || aux.inputs[0].samples() != num_frames))
                {
                    for channel in buffer.as_slice() {
                        channel.fill(0.0);
                    }
                    return nih_plug::prelude::ProcessStatus::Error(
                        "Host block exceeds the negotiated maximum",
                    );
                }

                if self.params.structural_fingerprint() != self.structural_fingerprint {
                        // Structural state is hidden/non-automatable and is
                        // reconstructed by initialize(). If a host restores it
                        // while active, fail silent and request lifecycle
                        // reactivation instead of silently diverging or rebuilding
                        // and destroying plugin resources on the render thread.
                        for channel in buffer.as_slice() {
                            channel.fill(0.0);
                        }
                        return nih_plug::prelude::ProcessStatus::Error(
                            "Structural parameter state changed; reactivate plugin",
                        );
                }

                // Sync nih-plug params → SOTF plugin
                if self
                    .bridge
                    .sync_params_to_plugin(&self.params, plugin.as_mut())
                    .is_err()
                {
                    for channel in buffer.as_slice() {
                        channel.fill(0.0);
                    }
                    return nih_plug::prelude::ProcessStatus::Error("Parameter update failed");
                }

                // Interleave main inputs and any additional input bus.
                let channel_slices = buffer.as_slice();
                for frame in 0..num_frames {
                    for ch in 0..input_channels.min(output_channels) {
                        self.interleaved_in[frame * input_channels + ch] =
                            channel_slices[ch][frame];
                    }
                    if input_channels > output_channels {
                        for ch in 0..input_channels - output_channels {
                            self.interleaved_in[frame * input_channels + output_channels + ch] =
                                aux.inputs[0].as_slice_immutable()[ch][frame];
                        }
                    }
                }

                // Process
                let ctx = self.transport.context(transport, self.sample_rate, num_frames);
                let produced = plugin.process(
                    &self.interleaved_in[..num_frames * input_channels],
                    &mut self.interleaved_out[..num_frames * output_channels],
                    &ctx,
                );
                if !matches!(produced, Ok(frames) if frames == num_frames) {
                    for channel in buffer.as_slice() {
                        channel.fill(0.0);
                    }
                    return nih_plug::prelude::ProcessStatus::Error("Processing failed or returned an incomplete block");
                }

                // Deinterleave output
                let channel_slices = buffer.as_slice();
                for frame in 0..num_frames {
                    for ch in 0..num_channels {
                        channel_slices[ch][frame] = self.interleaved_out[frame * num_channels + ch];
                    }
                }

                $crate::wrapper::process_status_for_tail(plugin.tail_length())
            }
        }

        impl nih_plug::prelude::Vst3Plugin for $struct_name {
            const VST3_CLASS_ID: [u8; 16] = $vst3_id;
            const VST3_SUBCATEGORIES: &'static [nih_plug::prelude::Vst3SubCategory] =
                &[nih_plug::prelude::Vst3SubCategory::Fx];

            fn default_audio_io_layout() -> nih_plug::prelude::AudioIOLayout {
                // VST3 discovers buses from its initial layout. CLAP retains
                // its legacy stereo config ID and explicitly selects keys.
                let layouts = <Self as nih_plug::prelude::Plugin>::AUDIO_IO_LAYOUTS;
                layouts[if matches!($plugin_type, "Gate") { 1 } else { 0 }]
            }
        }

        impl nih_plug::prelude::ClapPlugin for $struct_name {
            const CLAP_ID: &'static str = $clap_id;
            const CLAP_FEATURES: &'static [nih_plug::prelude::ClapFeature] =
                &[nih_plug::prelude::ClapFeature::AudioEffect];
            const CLAP_DESCRIPTION: Option<&'static str> = None;
            const CLAP_MANUAL_URL: Option<&'static str> = None;
            const CLAP_SUPPORT_URL: Option<&'static str> = None;
        }
    };
}

/// Convert an output-clock tail bound to the common native representation.
#[doc(hidden)]
pub fn native_tail_samples(tail: sotf_host::TailLength) -> u32 {
    match tail {
        sotf_host::TailLength::Finite(frames) if frames < i32::MAX as u64 => frames as u32,
        _ => u32::MAX,
    }
}

/// Keep processing until the declared zero-input response can be emitted.
#[doc(hidden)]
pub fn process_status_for_tail(tail: sotf_host::TailLength) -> nih_plug::prelude::ProcessStatus {
    match native_tail_samples(tail) {
        0 => nih_plug::prelude::ProcessStatus::Normal,
        u32::MAX => nih_plug::prelude::ProcessStatus::KeepAlive,
        frames => nih_plug::prelude::ProcessStatus::Tail(frames),
    }
}

/// Stable DAW parameter count. Neutral peaking bands preserve pass-through audio
/// while making every supported EQ band available before the host scans params.
pub const NIH_EQ_BANDS: usize = 20;

/// Default construction used for parameter discovery and host initialization.
pub fn default_plugin_config(plugin_type: &str) -> String {
    match plugin_type {
        "EQ" => eq_config_json(|_| None),
        "LinearPhaseEQ" => r#"{"num_filters":10}"#.to_string(),
        _ => "{}".to_string(),
    }
}

/// Restore the fixed EQ band schema from DAW state before starting audio.
pub fn eq_config_json(
    value: impl Fn(&str) -> Option<sotf_host::parameters::ParameterValue>,
) -> String {
    use sotf_host::parameters::ParameterValue;
    let types = [
        "Peak",
        "Lowshelf",
        "Highshelf",
        "Lowpass",
        "Highpass",
        "Bandpass",
        "Notch",
        "AllPass",
    ];
    let filters: Vec<_> = (0..NIH_EQ_BANDS)
        .map(|band| {
            let float = |field, default| {
                value(&format!("band_{band}_{field}"))
                    .and_then(|value| value.as_float())
                    .unwrap_or(default)
            };
            let integer = |field, default| {
                value(&format!("band_{band}_{field}"))
                    .and_then(|value| value.as_int())
                    .unwrap_or(default)
            };
            serde_json::json!({
                "filter_type": types.get(integer("filter_type", 0) as usize).unwrap_or(&"Peak"),
                "freq": float("freq", 1000.0),
                "q": float("q", 1.0),
                "db_gain": float("gain", 0.0),
                "order": integer("order", 2),
            })
        })
        .collect();
    let enabled = matches!(value("auto_gain_enabled"), Some(ParameterValue::Bool(true)));
    serde_json::json!({ "filters": filters, "auto_gain": { "enabled": enabled } }).to_string()
}

/// Apply structural controls on the control thread. Their host representation
/// remains the historical choice index while the DSP accepts an actual factor.
pub(crate) fn apply_eq_structural(
    plugin: &mut dyn sotf_host::plugin::Plugin,
    value: impl Fn(&str) -> Option<sotf_host::parameters::ParameterValue>,
) -> Result<(), String> {
    use sotf_host::parameters::{ParameterId, ParameterValue};
    for id in ["max_filters", "oversampling", "topology"] {
        if let Some(mut current) = value(id) {
            if id == "oversampling" {
                current = ParameterValue::Int(match current.as_int().unwrap_or(0) {
                    1 => 2,
                    2 => 4,
                    _ => 1,
                });
            }
            plugin.set_parameter(ParameterId::from(id), current)?;
        }
    }
    Ok(())
}

/// Convert runtime plugin metadata to NIH's format without erasing integer,
/// boolean, or structural/realtime semantics.
pub fn bridged_info_from_parameter(
    parameter: &sotf_host::parameters::Parameter,
) -> Option<plugins_bridge::param_bridge::BridgedParamInfo> {
    use plugins_bridge::param_bridge::BridgedParamKind;
    use sotf_host::param_specs::UpdateMode;
    use sotf_host::parameters::ParameterValue;

    let (min_value, max_value, default_value, steps) = match (
        &parameter.min_value,
        &parameter.max_value,
        &parameter.default_value,
    ) {
        (
            Some(ParameterValue::Float(min)),
            Some(ParameterValue::Float(max)),
            ParameterValue::Float(default),
        ) => (*min as f64, *max as f64, *default as f64, 0),
        (
            Some(ParameterValue::Int(min)),
            Some(ParameterValue::Int(max)),
            ParameterValue::Int(default),
        ) => (
            *min as f64,
            *max as f64,
            *default as f64,
            u32::try_from(i64::from(*max) - i64::from(*min) + 1).ok()?,
        ),
        (None, None, ParameterValue::Bool(default)) => {
            (0.0, 1.0, if *default { 1.0 } else { 0.0 }, 1)
        }
        _ => return None,
    };
    Some(plugins_bridge::param_bridge::BridgedParamInfo {
        id: parameter.id.to_string(),
        name: parameter.name.clone(),
        unit: parameter.unit.clone(),
        min_value,
        max_value,
        default_value,
        kind: match parameter.default_value {
            ParameterValue::Float(_) => BridgedParamKind::Float,
            ParameterValue::Int(_) => BridgedParamKind::Int,
            ParameterValue::Bool(_) => BridgedParamKind::Bool,
            ParameterValue::String(_) => return None,
        },
        steps,
        logarithmic: parameter.logarithmic,
        realtime: parameter.update_mode == UpdateMode::Realtime,
        group: parameter.group.clone(),
    })
}

/// Validate a host-supplied block against the bounds negotiated in `initialize()`.
///
/// Returns the required interleaved sample count when `num_frames` fits within
/// `max_frames` and `num_channels` matches the plugin's declared layout;
/// returns `None` otherwise (oversized block, channel mismatch, or sample
/// count overflow). Callers must fill the host buffer with silence and return
/// `ProcessStatus::Error` on `None`, mirroring the FFI crate's
/// `BufferTooSmall` behavior.
pub fn check_host_block(
    num_frames: usize,
    num_channels: usize,
    max_frames: usize,
    expected_channels: usize,
) -> Option<usize> {
    if num_channels != expected_channels || num_frames > max_frames {
        return None;
    }
    num_frames.checked_mul(num_channels)
}

/// DAW-facing parameter id for a canonical spec key.
///
/// CLAP/VST3 hosts persist parameter ids across sessions, so the four
/// choice parameters renamed to `*_index`/`mode`/`preset`/`type` keep their
/// pre-migration ids on this boundary. The internal engine, toolbar, and
/// factory all use the canonical keys; the NIH map translates back when
/// building host-visible state (see `linear_phase_eq_config_json`).
/// Scoped per plugin type so untouched plugins sharing a name (e.g. the
/// upmixer's own `fft_size`) are unaffected.
pub fn legacy_external_param_id<'a>(
    plugin_type: &str,
    canonical: &'a str,
) -> std::borrow::Cow<'a, str> {
    use std::borrow::Cow;
    let linear_phase_eq = matches!(plugin_type, "LinearPhaseEQ" | "linear_phase_eq");
    let crossfeed = matches!(plugin_type, "Crossfeed" | "crossfeed");
    let spectral = matches!(plugin_type, "SpectralCompressor" | "spectral_compressor");
    let band_split = matches!(plugin_type, "BandSplit" | "band_split");
    match canonical {
        "fir_length_index" if linear_phase_eq => Cow::Borrowed("fir_length"),
        "phase_mode_index" if linear_phase_eq => Cow::Borrowed("phase_mode"),
        "mode" if crossfeed => Cow::Borrowed("crossfeed_mode"),
        "preset" if crossfeed => Cow::Borrowed("crossfeed_preset"),
        "fft_size_index" if spectral => Cow::Borrowed("fft_size"),
        "type" if band_split => Cow::Borrowed("crossover_type"),
        _ => Cow::Borrowed(canonical),
    }
}

/// Get ParamSpec array for a plugin type.
pub fn get_param_specs(plugin_type: &str) -> &'static [sotf_host::param_specs::ParamSpec] {
    use sotf_plugins::param_specs::*;

    match plugin_type {
        "EQ" => eq::GLOBAL_PARAMS,
        "Compressor" => compressor::PARAMS,
        "Limiter" => limiter::PARAMS,
        "Gate" => gate::PARAMS,
        "Gain" => gain::PARAMS,
        "Saturation" => saturation::PARAMS,
        "Delay" => delay::PARAMS,
        "Expander" => expander::PARAMS,
        "Crossfeed" => crossfeed::PARAMS,
        "FletcherMunson" => loudness_compensation::PARAMS,
        "LoudnessCompensation" => loudness_compensation::PARAMS,
        "MultibandCompressor" => multiband_compressor::GLOBAL_PARAMS,
        "MultibandExpander" => multiband_expander::GLOBAL_PARAMS,
        "Upmixer" => upmixer::PARAMS,
        "AAE" => aae::PARAMS,
        "XTC" => xtc::PARAMS,
        "Binaural" => binaural::PARAMS,
        "Denoiser" => denoiser::PARAMS,
        "SpeechDenoiser" => speech_denoiser::PARAMS,
        "HissReducer" => hiss_reducer::PARAMS,
        "Declick" => declick::PARAMS,
        "BandSplit" => band_split::PARAMS,
        "BandMerge" => band_merge::PARAMS,
        "AEC" => aec::PARAMS,
        "Beamformer" => beamformer::PARAMS,
        "LinearPhaseEQ" => linear_phase_eq::PARAMS,
        "SpectralCompressor" => spectral_compressor::PARAMS,
        "AmbisonicsDecoder" => ambisonics::PARAMS,
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::check_host_block;

    #[test]
    fn fitting_block_returns_interleaved_sample_count() {
        assert_eq!(check_host_block(64, 2, 128, 2), Some(128));
        assert_eq!(check_host_block(128, 2, 128, 2), Some(256));
        assert_eq!(check_host_block(0, 2, 128, 2), Some(0));
    }

    #[test]
    fn oversized_block_is_rejected() {
        assert_eq!(check_host_block(129, 2, 128, 2), None);
        assert_eq!(check_host_block(1024, 2, 128, 2), None);
    }

    #[test]
    fn channel_mismatch_is_rejected() {
        assert_eq!(check_host_block(64, 1, 128, 2), None);
        assert_eq!(check_host_block(64, 4, 128, 2), None);
    }

    #[test]
    fn overflowing_sample_count_is_rejected() {
        assert_eq!(
            check_host_block(usize::MAX, usize::MAX, usize::MAX, usize::MAX),
            None
        );
    }
}

#[cfg(test)]
#[path = "wrapper_tests.rs"]
mod process_tests;
