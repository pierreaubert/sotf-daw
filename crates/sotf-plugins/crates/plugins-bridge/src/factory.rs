//! Universal plugin factory for all SOTF audio plugins.

use sotf_host::parametric_in_place_plugin::ParametricInPlacePluginAdapter;
use sotf_host::plugin::Plugin;
use sotf_host::{ParameterId, ParameterValue, ParametricPlugin, ParametricPluginAdapter};

fn create_nested_plugin(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: u32,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin(plugin_type, channels, sample_rate, &parameters.to_string())
}

/// Create a plugin instance from a type name, channel count, sample rate, and JSON config.
///
/// The `sample_rate` is used for plugins that need it at construction time (EQ, XTC, Convolution).
/// Most plugins receive sample rate later via `initialize()`.
///
/// The `config_json` should be a JSON object matching the plugin's `Params` struct,
/// or `"{}"` / `"null"` for default parameters.
pub fn create_plugin(
    plugin_type: &str,
    channels: usize,
    sample_rate: u32,
    config_json: &str,
) -> Result<Box<dyn Plugin>, String> {
    match plugin_type {
        // ============================================================
        // Wave 1: Core effects
        // ============================================================
        "EQ" | "eq" => {
            let json: serde_json::Value = serde_json::from_str(config_json)
                .map_err(|error| format!("Invalid EQ config: {error}"))?;
            let params: sotf_plugin_eq::EqPluginParams = parse_params(config_json)?;
            let mut plugin = sotf_plugin_eq::EqPlugin::from_params(channels, sample_rate, params)?;
            if let Some(tdf2) = json.get("tdf2").and_then(serde_json::Value::as_bool) {
                plugin.parametric_set_parameter(
                    ParameterId::from("tdf2"),
                    ParameterValue::Bool(tdf2),
                )?;
            }
            if let Some(topology) = json.get("topology").and_then(serde_json::Value::as_f64) {
                plugin.parametric_set_parameter(
                    ParameterId::from("topology"),
                    ParameterValue::Int(topology as i32),
                )?;
            }
            if let Some(oversampling) = json.get("oversampling").and_then(serde_json::Value::as_f64)
            {
                plugin.parametric_set_parameter(
                    ParameterId::from("oversampling"),
                    ParameterValue::Int(oversampling as i32),
                )?;
            }
            Ok(plugin.into_boxed_plugin())
        }

        "Compressor" | "compressor" => {
            // Route to multiband compressor in single-band mode (num_bands=1).
            let mut params: sotf_plugin_multiband_compressor::MultibandCompressorPluginParams =
                parse_params(config_json)?;
            params.num_bands = 1;
            let plugin =
                sotf_plugin_multiband_compressor::MultibandCompressorPlugin::try_from_params(
                    channels,
                    params,
                    sample_rate,
                )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Limiter" | "limiter" => {
            let params: sotf_plugin_limiter::LimiterPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_limiter::LimiterPlugin::from_params(channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Gate" | "gate" => {
            let params: sotf_plugin_gate::GatePluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_gate::GatePlugin::from_params(channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Gain" | "gain" => {
            let params: sotf_plugin_gain::GainPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_gain::GainPlugin::from_params(channels, params)?;
            Ok(Box::new(ParametricPluginAdapter::new(plugin)))
        }

        "Delay" | "delay" => {
            let params: sotf_plugin_delay::DelayPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_delay::DelayPlugin::from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        // ============================================================
        // Wave 2: Extended effects
        // ============================================================
        "Expander" | "expander" => {
            // Route to multiband expander in single-band mode (num_bands=1).
            let mut params: sotf_plugin_multiband_expander::MultibandExpanderPluginParams =
                parse_params(config_json)?;
            params.num_bands = 1;
            let plugin = sotf_plugin_multiband_expander::MultibandExpanderPlugin::from_params(
                channels, params,
            );
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "DeEsser" | "de_esser" => {
            let params: sotf_plugin_de_esser::DeEsserPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_de_esser::DeEsserPlugin::try_from_params_at_sample_rate(
                channels,
                params,
                sample_rate,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "DynamicEQ" | "dynamic_eq" => {
            let params: sotf_plugin_dynamic_eq::DynamicEqPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_dynamic_eq::DynamicEqPlugin::try_from_params_at_sample_rate(
                channels,
                params,
                sample_rate,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Crossfeed" | "crossfeed" => {
            let params: sotf_plugin_crossfeed::CrossfeedPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_crossfeed::CrossfeedPlugin::new(params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "MultibandCompressor" | "multiband_compressor" => {
            let params: sotf_plugin_multiband_compressor::MultibandCompressorPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_multiband_compressor::MultibandCompressorPlugin::try_from_params(
                    channels,
                    params,
                    sample_rate,
                )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "MultibandExpander" | "multiband_expander" => {
            let params: sotf_plugin_multiband_expander::MultibandExpanderPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_multiband_expander::MultibandExpanderPlugin::from_params(
                channels, params,
            );
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Convolution" | "convolution" => {
            let config: serde_json::Value =
                if config_json.trim().is_empty() || matches!(config_json.trim(), "null" | "{}") {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(config_json)
                        .map_err(|error| format!("Invalid Convolution config: {error}"))?
                };
            let true_stereo = match config.get("true_stereo") {
                None => false,
                Some(serde_json::Value::Bool(enabled)) => *enabled,
                Some(_) => return Err("Convolution true_stereo must be a boolean".into()),
            };
            let defaults = sotf_plugin_convolution::ConvolutionPluginParams::default();
            let params: sotf_plugin_convolution::ConvolutionPluginParams =
                parse_params_with_defaults(
                    config_json,
                    [
                        ("ir_file", serde_json::json!(defaults.ir_file)),
                        ("mix", serde_json::json!(defaults.mix)),
                        ("gain_db", serde_json::json!(defaults.gain_db)),
                    ],
                )?;
            let plugin = sotf_plugin_convolution::ConvolutionPlugin::from_params_with_routing(
                channels,
                sample_rate,
                params,
                true_stereo,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "FletcherMunson" | "fletcher_munson" => {
            // Backward compat: route to LoudnessCompensation in Auto mode (mode=2)
            let fm_params: sotf_plugin_loudness_compensation::FletcherMunsonCompat =
                parse_params(config_json)?;
            let lc_params = fm_params.into_loudness_compensation_params();
            let plugin =
                sotf_plugin_loudness_compensation::LoudnessCompensationPlugin::from_params(
                    channels, lc_params,
                )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "LoudnessCompensation" | "loudness_compensation" => {
            let params: sotf_plugin_loudness_compensation::LoudnessCompensationPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_loudness_compensation::LoudnessCompensationPlugin::from_params(
                    channels, params,
                )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "ChannelMuteSolo" | "channel_mute_solo" => {
            let params: sotf_plugin_channel_mute_solo::ChannelMuteSoloParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_channel_mute_solo::ChannelMuteSoloPlugin::try_from_params(
                channels, params,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Downmix" | "downmix" => {
            let mut params: sotf_plugin_downmix::DownmixPluginParams = parse_params(config_json)?;
            params.input_channels = channels;
            let plugin = sotf_plugin_downmix::DownmixPlugin::try_from_params(params)?;
            Ok(Box::new(plugin))
        }

        // ============================================================
        // Wave 3: Spatial / specialized
        // ============================================================
        "Upmixer" | "upmixer" => {
            let params: sotf_plugin_upmixer::UpmixerPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_upmixer::UpmixerPlugin::from_params(params);
            Ok(Box::new(plugin))
        }

        "AAE" | "aae" | "active_acoustic_enhancement" => {
            let params: sotf_plugin_aae::params::AaePluginParams = parse_params(config_json)?;
            let mut plugin = sotf_plugin_aae::AaePlugin::try_from_params(params)?;
            plugin.initialize(sample_rate)?;
            Ok(Box::new(plugin))
        }

        "XTC" | "xtc" => {
            let params: sotf_plugin_xtc::XtcPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_xtc::XtcPlugin::from_params(params, sample_rate)?;
            Ok(Box::new(plugin))
        }

        "Binaural" | "binaural" => {
            let params: sotf_plugin_binaural::BinauralDecoderParams = parse_params_with_defaults(
                config_json,
                [("input_channels", serde_json::json!(channels))],
            )?;
            if params.input_channels != channels {
                return Err(format!(
                    "Binaural is configured for {} input channels, got {channels}",
                    params.input_channels
                ));
            }
            let plugin = sotf_plugin_binaural::BinauralDecoderPlugin::try_from_params(params)?;
            Ok(Box::new(plugin))
        }

        "Matrix" | "matrix" => {
            #[derive(serde::Deserialize, Default)]
            struct MatrixConfig {
                input_channels: Option<usize>,
                output_channels: Option<usize>,
                input_channel_map: Option<Vec<usize>>,
                output_channel_map: Option<Vec<usize>>,
                matrix: Option<Vec<f32>>,
            }
            let config: MatrixConfig = parse_params(config_json)?;
            let MatrixConfig {
                input_channels,
                output_channels,
                input_channel_map,
                output_channel_map,
                matrix,
            } = config;
            let plugin = match (
                input_channel_map,
                output_channel_map,
                matrix,
            ) {
                (Some(input_map), Some(output_map), Some(matrix)) =>
                    sotf_plugin_matrix::MatrixPlugin::with_sparse_mapping(input_map, output_map, matrix)?,
                (None, None, Some(matrix)) => {
                    let input = input_channels.unwrap_or(channels);
                    let output = output_channels.unwrap_or(channels);
                    sotf_plugin_matrix::MatrixPlugin::with_matrix(input, output, matrix)?
                }
                (None, None, None) if input_channels.is_none() && output_channels.is_none() =>
                    sotf_plugin_matrix::MatrixPlugin::new(channels, channels),
                _ => return Err("Matrix requires both channel maps and a matrix, or a full matrix configuration".into()),
            };
            Ok(Box::new(plugin))
        }

        "MonoToStereo" | "mono_to_stereo" => {
            let params: sotf_plugin_mono_to_stereo::MonoToStereoPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_mono_to_stereo::MonoToStereoPlugin::try_from_params(channels, params)?;
            Ok(Box::new(plugin))
        }

        "PND" | "pnd" => {
            let params: sotf_plugin_pnd::PndPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_pnd::PndPlugin::try_from_params(channels, params)?;
            Ok(Box::new(plugin))
        }

        "Denoiser" | "denoiser" => {
            let params: sotf_plugin_denoiser::DenoiserPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_denoiser::DenoiserPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "SpeechDenoiser" | "speech_denoiser" | "RNNoise" | "rnnoise" => {
            let params: sotf_plugin_speech_denoiser::SpeechDenoiserPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_speech_denoiser::SpeechDenoiserPlugin::try_from_params(
                channels, params,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "HissReducer" | "hiss_reducer" | "Hiss" | "hiss" => {
            let params: sotf_plugin_hiss_reducer::HissReducerPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_hiss_reducer::HissReducerPlugin::from_params(channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Declick" | "declick" | "TransientRepair" | "transient_repair" => {
            let params: sotf_plugin_declick::DeclickPluginParams = parse_params(config_json)?;
            let plugin =
                sotf_plugin_declick::DeclickPlugin::from_params(channels, sample_rate, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "ABCompare" | "ab_compare" => {
            let params: sotf_plugin_ab_compare::ABComparePluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_ab_compare::ABComparePlugin::from_params_with_factory(
                channels,
                sample_rate,
                params,
                create_nested_plugin,
            )?;
            Ok(Box::new(plugin))
        }

        "Crossover" | "crossover" => {
            let specs = sotf_plugin_crossover::params::PARAMS;
            let crossover_type =
                sotf_plugin_crossover::params::CROSSOVER_TYPES[specs[0].default_usize()];
            // The construction schema calls the runtime `mode` parameter `output`.
            let output = ["lowpass", "highpass", "both"][specs[2].default_usize()];
            let params: sotf_plugin_crossover::CrossoverPluginParams = parse_params_with_defaults(
                config_json,
                [
                    ("type", serde_json::json!(crossover_type)),
                    ("frequency", serde_json::json!(specs[1].default_f64())),
                    ("output", serde_json::json!(output)),
                ],
            )?;
            let plugin = sotf_plugin_crossover::CrossoverPlugin::from_params(channels, &params)?;
            Ok(Box::new(plugin))
        }

        "StereoImager" | "stereo_imager" => {
            let params: sotf_plugin_stereo_imager::StereoImagerPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_stereo_imager::StereoImagerPlugin::from_params(channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "TransientShaper" | "transient_shaper" => {
            let params: sotf_plugin_transient_shaper::TransientShaperPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_transient_shaper::TransientShaperPlugin::try_from_params(
                channels, params,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Saturation" | "saturation" => {
            let params: sotf_plugin_saturation::SaturationPluginParams = parse_params(config_json)?;
            let plugin =
                sotf_plugin_saturation::SaturationPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "AnalogCompressor" | "analog_compressor" => {
            let params: sotf_plugin_analog_compressor::AnalogCompressorPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_analog_compressor::AnalogCompressorPlugin::try_from_params(
                channels, params,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "AnalogEQ" | "analog_eq" => {
            let params: sotf_plugin_analog_eq::AnalogEqPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_analog_eq::AnalogEqPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "AnalogLimiter" | "analog_limiter" => {
            let params: sotf_plugin_analog_limiter::AnalogLimiterPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_analog_limiter::AnalogLimiterPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "LinearPhaseEQ" | "linear_phase_eq" => {
            let params: sotf_plugin_linear_phase_eq::LinearPhaseEqPluginParams =
                parse_params(config_json)?;
            let plugin = sotf_plugin_linear_phase_eq::LinearPhaseEqPlugin::from_params(
                channels,
                sample_rate,
                params,
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "SpectralCompressor" | "spectral_compressor" => {
            let params: sotf_plugin_spectral_compressor::SpectralCompressorPluginParams =
                parse_params(config_json)?;
            let plugin =
                sotf_plugin_spectral_compressor::SpectralCompressorPlugin::try_from_params(
                    channels, params,
                )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "Dither" | "dither" => {
            let plugin = sotf_plugin_dither::DitherPlugin::new(channels);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "AmbisonicsDecoder" | "ambisonics_decoder" => {
            let config: sotf_plugin_ambisonics::AmbisonicsDecoderConfig =
                parse_params(config_json)?;
            let mut plugin = sotf_plugin_ambisonics::AmbisonicsDecoderPlugin::new(&config)?;
            if plugin.input_channels() != channels {
                return Err(format!(
                    "Order-{} ambisonics requires {} input channels, got {channels}",
                    config.order,
                    plugin.input_channels()
                ));
            }
            plugin.initialize(sample_rate)?;
            Ok(Box::new(plugin))
        }

        "BandSplit" | "band_split" => {
            let params: sotf_plugin_band_split::BandSplitPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_band_split::BandSplitPlugin::from_params(channels, &params)?;
            Ok(Box::new(plugin))
        }

        "BandMerge" | "band_merge" => {
            let params: sotf_plugin_band_merge::BandMergePluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_band_merge::BandMergePlugin::from_params(channels, &params)?;
            Ok(Box::new(plugin))
        }

        "AEC" | "aec" => {
            if channels != 2 {
                return Err(format!(
                    "AEC requires 2 input channels (microphone + reference), got {channels}"
                ));
            }
            let params: sotf_plugin_aec::AecPluginParams = parse_params(config_json)?;
            let plugin = sotf_plugin_aec::AecPlugin::from_params(sample_rate, params)?;
            Ok(Box::new(plugin))
        }

        "Beamformer" | "beamformer" => {
            let params: sotf_plugin_beamformer::BeamformerPluginParams = parse_params(config_json)?;
            if params.num_mics != channels {
                return Err(format!(
                    "Beamformer is configured for {} microphones, got {channels} input channels",
                    params.num_mics
                ));
            }
            let plugin =
                sotf_plugin_beamformer::BeamformerPlugin::from_params(sample_rate, params)?;
            Ok(Box::new(plugin))
        }

        "SpectrumAnalyzer" | "spectrum_analyzer" => {
            let config: sotf_host::SpectrumConfig = parse_params(config_json)?;
            let plugin = sotf_host::SpectrumAnalyzerPlugin::with_config_at_sample_rate(
                channels,
                sample_rate,
                config,
            )?;
            Ok(Box::new(plugin))
        }

        "LoudnessMonitor" | "loudness_monitor" => {
            #[derive(serde::Deserialize)]
            struct LoudnessConfig {
                enabled: bool,
                #[serde(default)]
                integrated_mode: sotf_host::IntegratedLoudnessMode,
                channel_layout: Option<sotf_host::speaker_config::ChannelLayout>,
                speaker_config: Option<String>,
            }
            let config: LoudnessConfig =
                parse_params_with_defaults(config_json, [("enabled", serde_json::json!(true))])?;
            let speaker_layout = config
                .speaker_config
                .map(|id| {
                    let speakers = sotf_host::speaker_config::get_speaker_config(&id)
                        .ok_or_else(|| format!("Unknown loudness speaker_config: {id}"))?;
                    sotf_host::speaker_config::ChannelLayout::from_speaker_config(speakers)
                })
                .transpose()?;
            let layout = match (config.channel_layout, speaker_layout) {
                (Some(layout), Some(speaker_layout)) if layout != speaker_layout => {
                    return Err("loudness channel_layout conflicts with speaker_config".into());
                }
                (Some(layout), _) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            };
            let mut plugin = if let Some(layout) = layout {
                sotf_host::LoudnessMonitorPlugin::new_with_layout(channels, layout)?
            } else {
                sotf_host::LoudnessMonitorPlugin::new(channels)?
            }
            .with_integrated_mode(config.integrated_mode)?;
            plugin.set_parameter(
                ParameterId::from("enabled"),
                ParameterValue::Bool(config.enabled),
            )?;
            Ok(Box::new(plugin))
        }

        _ => Err(format!("Unknown plugin type: {plugin_type}")),
    }
}

/// List all available plugin types.
pub fn available_plugin_types() -> &'static [&'static str] {
    &[
        "EQ",
        "Compressor",
        "Limiter",
        "Gate",
        "Gain",
        "Delay",
        "Expander",
        "Crossfeed",
        "MultibandCompressor",
        "MultibandExpander",
        "Convolution",
        "FletcherMunson",
        "LoudnessCompensation",
        "ChannelMuteSolo",
        "Downmix",
        "Upmixer",
        "AAE",
        "XTC",
        "Binaural",
        "Matrix",
        "MonoToStereo",
        "PND",
        "Denoiser",
        "SpeechDenoiser",
        "HissReducer",
        "Declick",
        "ABCompare",
        "Crossover",
        "StereoImager",
        "TransientShaper",
        "DynamicEQ",
        "Saturation",
        "AnalogEQ",
        "AnalogLimiter",
        "AnalogCompressor",
        "LinearPhaseEQ",
        "SpectralCompressor",
        "Dither",
        "AmbisonicsDecoder",
        "BandSplit",
        "BandMerge",
        "AEC",
        "Beamformer",
        "SpectrumAnalyzer",
        "LoudnessMonitor",
        "DeEsser",
    ]
}

// Fill absent construction fields without replacing explicit values, including
// invalid values that must still reach deserialization or plugin validation.
fn parse_params_with_defaults<T: serde::de::DeserializeOwned>(
    config_json: &str,
    defaults: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
) -> Result<T, String> {
    let mut config: serde_json::Map<String, serde_json::Value> = parse_params(config_json)?;
    for (key, value) in defaults {
        config.entry(key).or_insert(value);
    }
    serde_json::from_value(serde_json::Value::Object(config))
        .map_err(|error| format!("Failed to parse plugin config: {error}"))
}

fn parse_params<T: serde::de::DeserializeOwned>(config_json: &str) -> Result<T, String> {
    let trimmed = config_json.trim();
    if trimmed.is_empty() || trimmed == "null" || trimmed == "{}" {
        // Try to deserialize from empty object for types with serde defaults
        serde_json::from_str::<T>("{}").map_err(|e| format!("Default params failed: {e}"))
    } else {
        serde_json::from_str::<T>(config_json)
            .map_err(|e| format!("Failed to parse plugin config: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::plugin::ProcessContext;
    use std::path::PathBuf;

    fn write_four_path_ir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "sotf-bridge-true-stereo-{}-{}.wav",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let paths = [
            [8192_i16, 0, 0],
            [-4096_i16, 0, 0],
            [6144_i16, 0, 0],
            [8192_i16, 2048, 0],
        ];
        let channels = paths.len() as u16;
        let frames = paths[0].len();
        let data_size = (frames * paths.len() * 2) as u32;
        let mut bytes = Vec::with_capacity(44 + data_size as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_size).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * channels as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_size.to_le_bytes());
        for frame in 0..frames {
            for path in &paths {
                bytes.extend_from_slice(&path[frame].to_le_bytes());
            }
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn wrapper_defaults_and_aliases_initialize_and_process() {
        for plugin_type in [
            "Convolution",
            "convolution",
            "Binaural",
            "binaural",
            "Crossover",
            "crossover",
            "LoudnessMonitor",
            "loudness_monitor",
        ] {
            for config in ["", "null", "{}", " { } "] {
                let mut plugin = create_plugin(plugin_type, 2, 44_100, config)
                    .unwrap_or_else(|error| panic!("{plugin_type} {config:?}: {error}"));
                assert_eq!(plugin.input_channels(), 2, "{plugin_type}");
                assert_eq!(plugin.output_channels(), 2, "{plugin_type}");
                plugin.initialize(44_100).unwrap();
                let input = vec![0.125; 128 * 2];
                let mut output = vec![f32::NAN; input.len()];
                assert_eq!(
                    plugin
                        .process(&input, &mut output, &ProcessContext::new(44_100, 128))
                        .unwrap(),
                    128
                );
                assert!(
                    output.iter().all(|sample| sample.is_finite()),
                    "{plugin_type}"
                );
            }
        }
        assert!(available_plugin_types().contains(&"LoudnessMonitor"));
    }

    #[test]
    fn constructor_defaults_preserve_explicit_settings() {
        let convolution =
            create_plugin("Convolution", 3, 96_000, r#"{"mix":0.25,"gain_db":-6.0}"#).unwrap();
        assert_eq!(convolution.input_channels(), 3);
        assert_eq!(
            convolution.get_parameter(&ParameterId::from("mix")),
            Some(ParameterValue::Float(0.25))
        );
        assert_eq!(
            convolution.get_parameter(&ParameterId::from("gain_db")),
            Some(ParameterValue::Float(-6.0))
        );

        let crossover = create_plugin(
            "Crossover",
            3,
            48_000,
            r#"{"frequency":750.0,"output":"both"}"#,
        )
        .unwrap();
        assert_eq!(crossover.input_channels(), 3);
        assert_eq!(crossover.output_channels(), 6);
        assert_eq!(
            crossover.get_parameter(&ParameterId::from("frequency")),
            Some(ParameterValue::Float(750.0))
        );

        for config in ["{}", r#"{"input_channels":6}"#] {
            let binaural = create_plugin("Binaural", 6, 48_000, config).unwrap();
            assert_eq!(binaural.input_channels(), 6);
            assert_eq!(binaural.output_channels(), 2);
        }
        let mut loudness = create_plugin(
            "LoudnessMonitor",
            6,
            48_000,
            r#"{"speaker_config":"5.1","integrated_mode":"whole_program","enabled":false}"#,
        )
        .unwrap();
        loudness.initialize(48_000).unwrap();
        assert_eq!(
            loudness.get_parameter(&ParameterId::from("enabled")),
            Some(ParameterValue::Bool(false))
        );
        let input = vec![0.125; 128 * 6];
        let mut output = vec![f32::NAN; input.len()];
        loudness
            .process(&input, &mut output, &ProcessContext::new(48_000, 128))
            .unwrap();
        assert_eq!(output, input);
        let data = loudness.get_data().expect("loudness data");
        let data = data.downcast_ref::<sotf_host::LoudnessData>().unwrap();
        assert!(!data.measurement_enabled);
    }

    #[test]
    fn constructor_defaults_do_not_hide_invalid_explicit_settings() {
        for (plugin_type, config) in [
            ("Convolution", r#"{"ir_file":null}"#),
            ("Convolution", r#"{"mix":2.0}"#),
            ("Convolution", r#"{"gain_db":100.0}"#),
            (
                "Convolution",
                r#"{"ir_file":"/nonexistent/sotf-bridge-test.wav"}"#,
            ),
            ("Binaural", r#"{"input_channels":null}"#),
            ("Binaural", r#"{"input_channels":6}"#),
            ("Crossover", r#"{"type":"invalid"}"#),
            ("Crossover", r#"{"frequency":0.0}"#),
            ("Crossover", r#"{"output":"invalid"}"#),
            ("LoudnessMonitor", r#"{"enabled":"false"}"#),
            ("LoudnessMonitor", r#"{"integrated_mode":"invalid"}"#),
            ("LoudnessMonitor", r#"{"speaker_config":"5.1"}"#),
            ("LoudnessMonitor", r#"{"speaker_config":"invalid"}"#),
        ] {
            assert!(
                create_plugin(plugin_type, 2, 48_000, config).is_err(),
                "{plugin_type}: {config}"
            );
        }
        for plugin_type in ["Convolution", "Binaural", "Crossover", "LoudnessMonitor"] {
            for config in ["[]", "true", "1", "{"] {
                assert!(
                    create_plugin(plugin_type, 2, 48_000, config).is_err(),
                    "{plugin_type}: {config}"
                );
            }
        }
    }

    #[test]
    fn convolution_true_stereo_json_reaches_audible_bridge_route_and_requires_bool() {
        let ir_path = write_four_path_ir();
        let true_config = serde_json::json!({
            "ir_file": ir_path.to_string_lossy(),
            "mix": 1.0,
            "gain_db": 0.0,
            "true_stereo": true,
        })
        .to_string();
        let mut plugin = create_plugin("Convolution", 2, 48_000, &true_config).unwrap();
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(true))
        );
        plugin.initialize(48_000).unwrap();
        assert_eq!(plugin.latency_samples(), 1_024);
        let mut input = vec![0.0_f32; 1_050 * 2];
        input[0] = 0.5;
        input[20 * 2 + 1] = 0.25;
        let mut output = vec![f32::NAN; input.len()];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 1_050))
            .unwrap();
        assert!((output[1_024 * 2] - 0.5 * (8192.0 / 32768.0)).abs() < 2e-5);
        assert!((output[1_024 * 2 + 1] - 0.5 * (-4096.0 / 32768.0)).abs() < 2e-5);
        assert!((output[1_044 * 2] - 0.25 * (6144.0 / 32768.0)).abs() < 2e-5);
        assert!((output[1_044 * 2 + 1] - 0.25 * (8192.0 / 32768.0)).abs() < 2e-5);

        let default_plugin = create_plugin(
            "Convolution",
            2,
            48_000,
            &serde_json::json!({ "ir_file": ir_path.to_string_lossy() }).to_string(),
        )
        .unwrap();
        assert_eq!(
            default_plugin.get_parameter(&ParameterId::from("true_stereo")),
            Some(ParameterValue::Bool(false))
        );

        for value in [
            serde_json::json!("true"),
            serde_json::Value::Null,
            serde_json::json!(1),
        ] {
            let config = serde_json::json!({
                "ir_file": ir_path.to_string_lossy(),
                "true_stereo": value,
            })
            .to_string();
            let error = match create_plugin("Convolution", 2, 48_000, &config) {
                Ok(_) => panic!("nonboolean true_stereo must fail: {value}"),
                Err(error) => error,
            };
            assert!(error.contains("true_stereo must be a boolean"), "{error}");
        }
        let wrong_width = match create_plugin("Convolution", 3, 48_000, &true_config) {
            Ok(_) => panic!("true-stereo convolution must reject three plugin channels"),
            Err(error) => error,
        };
        assert!(
            wrong_width.contains("exactly 2 plugin channels"),
            "{wrong_width}"
        );
        let _ = std::fs::remove_file(ir_path);
    }

    #[test]
    fn test_create_wave1_plugins() {
        let wave1 = ["EQ", "Compressor", "Limiter", "Gate", "Gain", "Delay"];
        for plugin_type in &wave1 {
            let result = create_plugin(plugin_type, 2, 48000, "{}");
            assert!(
                result.is_ok(),
                "Failed to create {plugin_type}: {:?}",
                result.err()
            );
            let mut plugin = result.unwrap();
            assert!(plugin.initialize(48000).is_ok());
        }
    }

    #[test]
    fn test_create_wave1_lowercase() {
        let wave1 = ["eq", "compressor", "limiter", "gate", "gain", "delay"];
        for plugin_type in &wave1 {
            let result = create_plugin(plugin_type, 2, 48000, "{}");
            assert!(
                result.is_ok(),
                "Failed to create {plugin_type}: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn test_unknown_plugin() {
        let result = create_plugin("NonExistent", 2, 48000, "{}");
        assert!(result.is_err());
    }

    #[test]
    fn ab_compare_bridge_builds_initial_path_with_authoritative_factory() {
        let config = serde_json::json!({
            "path_a": {
                "type": "Plugin",
                "plugin_type": "expander",
                "parameters": {}
            },
            "path_b": {
                "type": "Rack",
                "plugins": [{"plugin_type": "hiss_reducer", "parameters": {}}]
            },
            "auto_gain_enabled": false
        })
        .to_string();
        let mut plugin = create_plugin("ABCompare", 2, 48_000, &config)
            .expect("bridge factory must be available during initial nested path construction");
        plugin.initialize(48_000).unwrap();
        let input = vec![0.0_f32; 256 * 2];
        let mut output = vec![1.0_f32; input.len()];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 256))
                .unwrap(),
            256
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn matrix_factory_preserves_configured_nonsquare_routing() {
        let config = r#"{"input_channels":2,"output_channels":1,"matrix":[0.5,0.5]}"#;
        let plugin = create_plugin("Matrix", 2, 48_000, config).unwrap();
        assert_eq!(plugin.input_channels(), 2);
        assert_eq!(plugin.output_channels(), 1);
    }

    #[test]
    fn test_create_newly_wired_plugins() {
        for plugin_type in ["BandSplit", "BandMerge", "AEC", "Beamformer"] {
            let result = create_plugin(plugin_type, 2, 48000, "{}");
            assert!(
                result.is_ok(),
                "Failed to create {plugin_type}: {:?}",
                result.err()
            );
            let mut plugin = result.unwrap();
            assert!(plugin.initialize(48000).is_ok());
        }
    }

    #[test]
    fn beamformer_bridge_returns_errors_for_malformed_state() {
        for config in [
            r#"{"num_mics":1}"#,
            r#"{"num_mics":2,"mic_spacing_cm":100.0}"#,
            r#"{"num_mics":2,"steer_angle_deg":200.0}"#,
            r#"{"num_mics":2,"beamformer_type":"unknown"}"#,
        ] {
            assert!(
                create_plugin("Beamformer", 2, 48_000, config).is_err(),
                "{config}"
            );
        }
        assert!(create_plugin("Beamformer", 4, 48_000, r#"{"num_mics":2}"#).is_err());
        assert!(
            create_plugin(
                "Beamformer",
                2,
                48_000,
                r#"{"num_mics":2,"beamformer_type":"GSC"}"#,
            )
            .is_ok()
        );
    }

    #[test]
    fn aec_bridge_enforces_canonical_bus_layout_and_ranges() {
        assert!(create_plugin("AEC", 1, 48_000, "{}").is_err());
        assert!(create_plugin("AEC", 3, 48_000, "{}").is_err());
        assert!(create_plugin("AEC", 2, 48_000, r#"{"step_size":1.2}"#).is_err());
        let plugin = create_plugin(
            "AEC",
            2,
            48_000,
            r#"{"echo_tail_ms":100.0,"step_size":0.4,"post_filter_enabled":false}"#,
        )
        .expect("valid canonical AEC state");
        assert_eq!(plugin.input_channels(), 2);
        assert_eq!(plugin.output_channels(), 1);
    }

    #[test]
    fn ambisonics_bridge_enforces_every_order_width() {
        for (order, channels, layout) in [(1, 4, "5.1"), (2, 9, "7.1.4"), (3, 16, "9.1.6")] {
            let config = serde_json::json!({"order": order, "target_layout": layout}).to_string();
            let plugin = create_plugin("AmbisonicsDecoder", channels, 48_000, &config).unwrap();
            assert_eq!(plugin.input_channels(), channels);
            assert!(create_plugin("AmbisonicsDecoder", channels - 1, 48_000, &config).is_err());
        }
    }

    fn aud135_ordered_input(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| {
                (0..channels).map(move |channel| {
                    let code = (frame * 37 + channel * 101 + frame * channel * 13) % 991;
                    (code as f32 - 495.0) / 8192.0
                })
            })
            .collect()
    }

    #[test]
    fn ambisonics_bridge_processes_all_56_ordered_input_output_tuples() {
        const TARGETS: [(&str, usize); 8] = [
            ("5.1", 6),
            ("7.1", 8),
            ("5.1.2", 8),
            ("5.1.4", 10),
            ("7.1.2", 10),
            ("7.1.4", 12),
            ("9.1.4", 14),
            ("9.1.6", 16),
        ];
        const FRAMES: usize = 257;

        for order in 1_usize..=7 {
            let input_channels = (order + 1) * (order + 1);
            let input = aud135_ordered_input(FRAMES, input_channels);
            for (layout, output_channels) in TARGETS {
                let config = serde_json::json!({
                    "order": order,
                    "target_layout": layout,
                })
                .to_string();
                let mut plugin =
                    create_plugin("AmbisonicsDecoder", input_channels, 48_000, &config)
                        .unwrap_or_else(|error| panic!("order {order} {layout}: {error}"));
                assert_eq!(
                    plugin.input_channels(),
                    input_channels,
                    "order {order} {layout}"
                );
                assert_eq!(
                    plugin.output_channels(),
                    output_channels,
                    "order {order} {layout}"
                );

                let mut whole = vec![0.0; FRAMES * output_channels];
                assert_eq!(
                    plugin
                        .process(&input, &mut whole, &ProcessContext::new(48_000, FRAMES))
                        .unwrap(),
                    FRAMES,
                    "order {order} {layout} whole block"
                );
                assert!(whole.iter().all(|sample| sample.is_finite()));
                assert!(whole.iter().any(|sample| sample.abs() > 1e-7));

                let mut partitioned_plugin =
                    create_plugin("AmbisonicsDecoder", input_channels, 48_000, &config).unwrap();
                let mut partitioned = vec![0.0; whole.len()];
                let mut offset = 0;
                for frames in [1, 31, 64, 3, 79, 79] {
                    let input_start = offset * input_channels;
                    let input_end = (offset + frames) * input_channels;
                    let output_start = offset * output_channels;
                    let output_end = (offset + frames) * output_channels;
                    assert_eq!(
                        partitioned_plugin
                            .process(
                                &input[input_start..input_end],
                                &mut partitioned[output_start..output_end],
                                &ProcessContext::new(48_000, frames),
                            )
                            .unwrap(),
                        frames,
                        "order {order} {layout} partition at {offset}"
                    );
                    offset += frames;
                }
                assert_eq!(offset, FRAMES);
                assert_eq!(
                    partitioned, whole,
                    "order {order} {layout} partition output"
                );
            }
        }
    }

    #[test]
    fn aae_bridge_rejects_invalid_restored_state_without_panicking() {
        assert!(create_plugin("AAE", 2, 48_000, r#"{"speaker_config":"2.0"}"#).is_err());
        assert!(create_plugin("AAE", 2, 48_000, r#"{"input_diffusion":2.0}"#).is_err());
        assert!(
            create_plugin("AAE", 2, 48_000, r#"{"solo_early":true,"solo_late":true}"#,).is_err()
        );
    }

    #[test]
    fn spectrum_analyzer_aliases_initialize_process_and_publish_data() {
        for plugin_type in ["SpectrumAnalyzer", "spectrum_analyzer"] {
            let mut plugin = create_plugin(plugin_type, 2, 48_000, "{}").unwrap();
            plugin.initialize(48_000).unwrap();
            let input = vec![0.0; 4096 * 2];
            let mut output = vec![1.0; input.len()];
            let frames = plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 4096))
                .unwrap();
            assert_eq!(frames, 4096);
            assert_eq!(output, input);
            let data = plugin.get_data().expect("SpectrumData");
            assert!(data.downcast_ref::<sotf_host::SpectrumData>().is_some());
        }
        assert!(available_plugin_types().contains(&"SpectrumAnalyzer"));
    }

    #[test]
    fn test_eq_with_config() {
        let config = r#"{
            "filters": [
                {"filter_type": "peak", "freq": 1000.0, "q": 1.5, "db_gain": 3.0}
            ]
        }"#;
        let result = create_plugin("EQ", 2, 48000, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_process_silence() {
        let mut plugin = create_plugin("Gain", 2, 48000, "{}").unwrap();
        plugin.initialize(48000).unwrap();

        let input = vec![0.0f32; 256];
        let mut output = vec![0.0f32; 256];
        let ctx = ProcessContext::new(48000, 128);
        let result = plugin.process(&input, &mut output, &ctx);
        assert!(result.is_ok());
    }
}
