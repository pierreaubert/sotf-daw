use super::catalog::catalog_entry;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::external::external_plugin_isolation_requested;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::external::external_plugin_trust;
use super::is::is_external_plugin_type;
use super::misc::resize_matrix;
use super::parse::parse_external_plugin_descriptor;
use super::parse::parse_external_plugin_state;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::parse::parse_isolated_external_plugin_config;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::sandboxed_plugin_creation_options::SandboxedPluginCreationOptions;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::sandboxed_plugin_creation_options::default_sandboxed_plugin_creation_options;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::sandboxed_plugin_creation_options::sandbox_policy_for_creation_options;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::validate::validate_external_plugin_security_config;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
use super::validate::validate_external_plugin_security_config;
use crate::ExternalPlugin;
use crate::{
    ABComparePlugin, ABComparePluginParams, AaePlugin, AaePluginParams, AecPlugin, AecPluginParams,
    AnalogCompressorPlugin, AnalogCompressorPluginParams, AnalogEqPlugin, AnalogEqPluginParams,
    AnalogLimiterPlugin, AnalogLimiterPluginParams, BandMergePlugin, BandMergePluginParams,
    BandSplitPlugin, BandSplitPluginParams, BeamformerPlugin, BeamformerPluginParams,
    BinauralDecoderParams, BinauralDecoderPlugin, ChannelMuteSoloParams, ChannelMuteSoloPlugin,
    CompressorPlugin, CompressorPluginParams, ConvolutionPlugin, ConvolutionPluginParams,
    CrossfeedPlugin, CrossfeedPluginParams, CrossoverPlugin, CrossoverPluginParams, DeEsserPlugin,
    DeEsserPluginParams, DeclickPlugin, DeclickPluginParams, DelayPlugin, DelayPluginParams,
    DenoiserPlugin, DenoiserPluginParams, DitherPlugin, DitherPluginParams, DownmixPlugin,
    DownmixPluginParams, DynamicEqPlugin, DynamicEqPluginParams, EqPlugin, EqPluginParams,
    ExpanderPlugin, ExpanderPluginParams, GainPlugin, GainPluginParams, GatePlugin,
    GatePluginParams, HissReducerPlugin, HissReducerPluginParams, LimiterPlugin,
    LimiterPluginParams, LinearPhaseEqPluginParams,
    LoudnessCompensationPlugin, LoudnessCompensationPluginParams, LoudnessMonitorPlugin,
    MatrixPlugin, MonoToStereoPlugin, MonoToStereoPluginParams, MultibandCompressorPlugin,
    MultibandCompressorPluginParams, MultibandExpanderPlugin, MultibandExpanderPluginParams,
    ParametricInPlacePluginAdapter, ParametricPluginAdapter, Plugin, PndPlugin, PndPluginParams,
    ResamplerPlugin, SaturationPlugin, SaturationPluginParams, SpectralCompressorPlugin,
    SpectralCompressorPluginParams, SpectrumAnalyzerPlugin, SpectrumConfig, SpeechDenoiserPlugin,
    SpeechDenoiserPluginParams, StereoImagerPlugin, StereoImagerPluginParams,
    TransientShaperPlugin, TransientShaperPluginParams, UpmixerPlugin, UpmixerPluginParams,
    XtcPlugin, XtcPluginParams,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use crate::{
    ExternalPluginWorkerCommand, IsolatedExternalPlugin, PluginSandboxGrantStore,
    PluginSandboxLaunchBackend,
};
use sotf_host::{
    ChannelLayout, IntegratedLoudnessMode, ParameterId, ParameterValue, ParametricInPlacePlugin,
    ParametricPlugin,
};
use std::path::PathBuf;

/// Create a plugin instance from its type string and JSON parameters.
///
/// Supports all plugin types in the SOTF ecosystem. This is the single
/// authoritative factory -- both the audio engine and the A/B Compare
/// plugin's sub-rack builder delegate to this function.
///
/// This constructs the plugin; it does not guarantee initialization. Direct
/// callers must call [`Plugin::initialize`] at the processing sample rate before
/// processing audio. Inserting the plugin into a host performs that step at the
/// node's input sample rate. The factory's `sample_rate` argument supplies any
/// rate-dependent constructor configuration.
///
/// External-key construction is refused here: single-width callers cannot
/// route the 2N key bus. Graph contexts that route key buses through
/// sidechain edges use [`create_plugin_with_external_key`] instead.
pub fn create_plugin<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_inner(plugin_type, parameters, channels, sample_rate.into(), false)
}

/// Create a plugin instance, permitting external-key construction.
///
/// Identical to [`create_plugin`] except that keyed plugins requesting an
/// external key bus construct instead of refusing. Only callers that route
/// the 2N input bus may use this entry: engine graph builds (which map
/// sidechain edges onto the key bus) and the FFI two-bus path. Linear
/// single-width chains must keep using [`create_plugin`] so external-key
/// requests keep failing loudly there.
pub fn create_plugin_with_external_key<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_inner(plugin_type, parameters, channels, sample_rate.into(), true)
}

fn create_plugin_inner(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: f64,
    allow_external_key_bus: bool,
) -> Result<Box<dyn Plugin>, String> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(format!("Plugin sample rate must be finite and positive, got {sample_rate}"));
    }
    let plugin_type = catalog_entry(plugin_type)
        .map(|entry| entry.canonical_type)
        .ok_or_else(|| format!("Unknown plugin type: {plugin_type}"))?;

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    if is_external_plugin_type(plugin_type)
        && let Some(options) = default_sandboxed_plugin_creation_options()
    {
        return create_plugin_with_sandbox_options(
            plugin_type,
            parameters,
            channels,
            sample_rate,
            &options,
        );
    }

    match plugin_type {
        "gain" => {
            let params: GainPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse gain params: {e}"))?;
            let plugin = GainPlugin::from_params(channels, params)?;
            Ok(Box::new(ParametricPluginAdapter::new(plugin)))
        }

        "eq" => {
            let params: EqPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse EQ params: {e}"))?;
            let mut plugin = EqPlugin::from_params(channels, sample_rate, params)?;
            if let Some(tdf2) = parameters.get("tdf2").and_then(serde_json::Value::as_bool) {
                plugin.parametric_set_parameter(
                    ParameterId::from("tdf2"),
                    ParameterValue::Bool(tdf2),
                )?;
            }
            if let Some(topology) = parameters
                .get("topology")
                .and_then(serde_json::Value::as_f64)
            {
                plugin.parametric_set_parameter(
                    ParameterId::from("topology"),
                    ParameterValue::Int(topology as i32),
                )?;
            }
            // The toolbar sends the spec labels ("Off"/"2x"/"4x"); hand-written
            // configs use factor values (1/2/4). Both are accepted; anything
            // else is a loud error, never a silent remap.
            if let Some(oversampling) = parameters.get("oversampling") {
                let factor = match oversampling {
                    // The daemon wire format carries integral floats (`1.0`);
                    // accept them exactly, reject anything fractional. Index 0
                    // (the spec's "Off" position) is also accepted since 0 is
                    // never a valid factor; 1/2/4 keep their hand-written
                    // config meaning as factors.
                    serde_json::Value::Number(number) => match number.as_f64() {
                        Some(0.0) => 1,
                        Some(1.0) => 1,
                        Some(2.0) => 2,
                        Some(4.0) => 4,
                        _ => {
                            return Err(format!(
                                "Invalid oversampling factor {oversampling}: must be 1, 2, or 4"
                            ));
                        }
                    },
                    serde_json::Value::String(label)
                        if label.eq_ignore_ascii_case("off") || label == "1" =>
                    {
                        1
                    }
                    serde_json::Value::String(label)
                        if label.eq_ignore_ascii_case("2x") || label == "2" =>
                    {
                        2
                    }
                    serde_json::Value::String(label)
                        if label.eq_ignore_ascii_case("4x") || label == "4" =>
                    {
                        4
                    }
                    other => {
                        return Err(format!(
                            "Invalid oversampling {other}: expected \"Off\"/\"2x\"/\"4x\" or factor 1/2/4"
                        ));
                    }
                };
                plugin.parametric_set_parameter(
                    ParameterId::from("oversampling"),
                    ParameterValue::Int(factor),
                )?;
            }
            Ok(plugin.into_boxed_plugin())
        }

        "compressor" => {
            let mut params: CompressorPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse compressor params: {e}"))?;
            params.num_bands = 1;
            let plugin = CompressorPlugin::try_from_params(channels, params, sample_rate)
                .map_err(|e| format!("Invalid compressor params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "expander" => {
            let mut params: ExpanderPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse expander params: {e}"))?;
            params.num_bands = 1;
            params.processing_mode = "time_domain".into();
            let plugin = ExpanderPlugin::try_from_params(channels, params, sample_rate)
                .map_err(|e| format!("Invalid expander params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "limiter" => {
            let params: LimiterPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse limiter params: {e}"))?;
            let plugin = LimiterPlugin::from_params(channels, params);
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "gate" => {
            let params: GatePluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse gate params: {e}"))?;
            let plugin = GatePlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "dither" => {
            if channels == 0 {
                return Err("Dither requires at least one channel".into());
            }
            let params: DitherPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse dither params: {e}"))?;
            let mut plugin = DitherPlugin::from_params(channels, params);
            plugin.initialize(sample_rate)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "delay" => {
            let params: DelayPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse delay params: {e}"))?;
            let plugin = DelayPlugin::from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "convolution" => {
            let true_stereo = match parameters.get("true_stereo") {
                Some(value) => value.as_bool().ok_or_else(|| {
                    "Failed to parse convolution params: true_stereo must be a boolean".to_string()
                })?,
                None => false,
            };
            let params: ConvolutionPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse convolution params: {e}"))?;
            let plugin = ConvolutionPlugin::from_params_with_routing(
                channels,
                sample_rate,
                params,
                true_stereo,
            )
            .map_err(|e| format!("Failed to create convolution plugin: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "upmixer" => {
            if channels != 2 {
                return Err(format!("Upmixer requires 2 input channels, got {channels}"));
            }
            let params: UpmixerPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse upmixer params: {e}"))?;
            let plugin = UpmixerPlugin::from_params(params);
            Ok(Box::new(plugin))
        }

        "aae" => {
            if channels != 2 {
                return Err(format!("AAE requires 2 input channels, got {channels}"));
            }
            let params: AaePluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse AAE params: {e}"))?;
            let mut plugin = AaePlugin::try_from_params(params)?;
            plugin.initialize(sample_rate)?;
            Ok(Box::new(plugin))
        }

        "downmix" => {
            let mut params: DownmixPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse downmix params: {e}"))?;
            if params.input_channels != channels {
                log::info!(
                    "[factory:downmix] Adapting input_channels from {} to current chain width {}",
                    params.input_channels,
                    channels
                );
                params.input_channels = channels;
                params.input_layout = match channels {
                    6 => Some("5.1".to_string()),
                    8 => Some("7.1".to_string()),
                    10 => Some("5.1.4".to_string()),
                    12 => Some("7.1.4".to_string()),
                    _ => None,
                };
            }
            let plugin = DownmixPlugin::try_from_params(params)?;
            Ok(Box::new(plugin))
        }

        "mono_to_stereo" => {
            if channels != 1 {
                return Err(format!(
                    "Mono-to-stereo requires 1 input channel, got {channels}"
                ));
            }
            let params: MonoToStereoPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse mono_to_stereo params: {e}"))?;
            let plugin =
                MonoToStereoPlugin::try_from_params_at_sample_rate(channels, params, sample_rate)
                    .map_err(|e| format!("Invalid mono_to_stereo params: {e}"))?;
            Ok(Box::new(plugin))
        }

        "multiband_compressor" => {
            let params: MultibandCompressorPluginParams =
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Failed to parse multiband compressor params: {e}"))?;
            if params.num_bands < 2 {
                return Err("Multiband compressor requires at least 2 bands".into());
            }
            let plugin = MultibandCompressorPlugin::try_from_params(channels, params, sample_rate)
                .map_err(|e| format!("Invalid multiband compressor params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "multiband_expander" => {
            let params: MultibandExpanderPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse multiband expander params: {e}"))?;
            if params.num_bands < 2 {
                return Err("Multiband expander requires at least 2 bands".into());
            }
            let plugin = MultibandExpanderPlugin::try_from_params(channels, params, sample_rate)
                .map_err(|e| format!("Invalid multiband expander params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "de_esser" => {
            let params: DeEsserPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse de-esser params: {e}"))?;
            // External-key de-essers need a 2N input bus (program + key) for
            // N program channels. Single-width callers cannot route that
            // bus, so external construction fails loudly here instead of
            // returning a flag without an audio path. Callers that route
            // the key bus (engine graphs via sidechain edges, or the FFI
            // two-bus path with input 2N and output N) construct through
            // `create_plugin_with_external_key`, which sets
            // `allow_external_key_bus`.
            if params.sidechain_external && !allow_external_key_bus {
                return Err(format!(
                    "De-esser external sidechain requires a 2N input bus for {channels} program channels (got single-width {channels}); engine single-width chains cannot route the key bus — use the FFI input/output-channel path"
                ));
            }
            let plugin =
                DeEsserPlugin::try_from_params_at_sample_rate(channels, params, sample_rate)
                    .map_err(|e| format!("Invalid de-esser params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "dynamic_eq" => {
            let params: DynamicEqPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse dynamic EQ params: {e}"))?;
            let plugin =
                DynamicEqPlugin::try_from_params_at_sample_rate(channels, params, sample_rate)
                    .map_err(|e| format!("Invalid dynamic EQ params: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "linear_phase_eq" => {
            let params: LinearPhaseEqPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse linear-phase EQ params: {e}"))?;
            let plugin =
                crate::plugin_linear_phase_eq::dynamic_host::LinearPhaseEqDynamicPlugin::from_params(
                    channels,
                    sample_rate,
                    params,
                )
                .map_err(|e| format!("Failed to create linear-phase EQ plugin: {e}"))?;
            Ok(Box::new(plugin))
        }

        "spectral_compressor" => {
            let params: SpectralCompressorPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse spectral compressor params: {e}"))?;
            let plugin = SpectralCompressorPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "stereo_imager" => {
            let params: StereoImagerPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse stereo imager params: {e}"))?;
            let plugin = StereoImagerPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "transient_shaper" => {
            let params: TransientShaperPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse transient shaper params: {e}"))?;
            let plugin = TransientShaperPlugin::try_from_params(channels, params)
                .map_err(|e| format!("Failed to create transient shaper: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "saturation" => {
            let params: SaturationPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse saturation params: {e}"))?;
            let plugin = SaturationPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "analog_compressor" => {
            let params: AnalogCompressorPluginParams =
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Failed to parse analog compressor params: {e}"))?;
            let plugin = AnalogCompressorPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "analog_eq" => {
            let params: AnalogEqPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse analog EQ params: {e}"))?;
            let plugin = AnalogEqPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "analog_limiter" => {
            let params: AnalogLimiterPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse analog limiter params: {e}"))?;
            let plugin = AnalogLimiterPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "loudness_compensation" => {
            let params: LoudnessCompensationPluginParams =
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Failed to parse loudness compensation params: {e}"))?;
            let plugin = LoudnessCompensationPlugin::from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "fletcher_munson" => {
            use crate::plugin_loudness_compensation::FletcherMunsonCompat;
            let fm: FletcherMunsonCompat = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse Fletcher-Munson params: {e}"))?;
            let plugin = LoudnessCompensationPlugin::from_params(
                channels,
                fm.into_loudness_compensation_params(),
            )?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "crossfeed" => {
            if channels != 2 {
                return Err(format!(
                    "Crossfeed requires 2 input channels (stereo), got {channels}"
                ));
            }
            let params: CrossfeedPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse crossfeed params: {e}"))?;
            let plugin = CrossfeedPlugin::new(params)
                .map_err(|e| format!("Failed to create crossfeed plugin: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "xtc" => {
            if channels != 2 {
                return Err(format!(
                    "XTC requires 2 input channels (stereo), got {channels}"
                ));
            }
            let params: XtcPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse XTC params: {e}"))?;
            let plugin = XtcPlugin::from_params(params, sample_rate)?;
            Ok(Box::new(plugin))
        }

        "denoiser" => {
            let params: DenoiserPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse denoiser params: {e}"))?;
            let plugin = DenoiserPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "speech_denoiser" => {
            let params: SpeechDenoiserPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse speech denoiser params: {e}"))?;
            let plugin = SpeechDenoiserPlugin::try_from_params(channels, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "hiss_reducer" => {
            let params: HissReducerPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse hiss reducer params: {e}"))?;
            let plugin =
                HissReducerPlugin::try_from_params_at_sample_rate(channels, sample_rate, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "declick" => {
            let params: DeclickPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse declick params: {e}"))?;
            let plugin = DeclickPlugin::from_params(channels, sample_rate, params)?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "pnd" => {
            let params: PndPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse PND params: {e}"))?;
            let plugin = PndPlugin::try_from_params(channels, params)?;
            Ok(Box::new(plugin))
        }

        "binaural_decoder" => {
            let params: BinauralDecoderParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse binaural decoder params: {e}"))?;
            if params.input_channels != channels {
                return Err(format!(
                    "Binaural decoder is configured for {} input channels, got {channels}",
                    params.input_channels
                ));
            }
            let plugin = BinauralDecoderPlugin::try_from_params(params)?;
            Ok(Box::new(plugin))
        }

        "crossover" => {
            let normalized = normalize_crossover_toolbar_choice_forms(parameters);
            let params: CrossoverPluginParams = serde_json::from_value(normalized)
                .map_err(|e| format!("Failed to parse crossover params: {e}"))?;
            let plugin = CrossoverPlugin::from_params(channels, &params)?;
            Ok(Box::new(plugin))
        }

        "matrix" => create_matrix_plugin(parameters, channels)
            .and_then(|plugin| require_graph_input_channels("Matrix", channels, plugin)),

        "channel_mute_solo" => {
            let params: ChannelMuteSoloParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse channel_mute_solo params: {e}"))?;
            let plugin = ChannelMuteSoloPlugin::try_from_params(channels, params)
                .map_err(|e| format!("Failed to create channel mute/solo plugin: {e}"))?;
            Ok(Box::new(ParametricInPlacePluginAdapter::new(plugin)))
        }

        "loudness_monitor" => {
            let integrated_mode = parameters
                .get("integrated_mode")
                .map(|value| {
                    serde_json::from_value::<IntegratedLoudnessMode>(value.clone()).map_err(|e| {
                        format!(
                            "Invalid loudness integrated_mode (expected rolling or whole_program): {e}"
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let explicit_layout = match (
                parameters.get("channel_layout"),
                parameters.get("speaker_config"),
            ) {
                (Some(layout), None) => Some(
                    serde_json::from_value::<ChannelLayout>(layout.clone())
                        .map_err(|e| format!("Invalid loudness channel_layout: {e}"))?,
                ),
                (None, Some(config_id)) => {
                    let config_id = config_id.as_str().ok_or_else(|| {
                        "loudness speaker_config must be a configuration ID string".to_string()
                    })?;
                    let config = sotf_host::speaker_config::get_speaker_config(config_id)
                        .ok_or_else(|| format!("Unknown loudness speaker_config: {config_id}"))?;
                    Some(ChannelLayout::from_speaker_config(config)?)
                }
                (Some(layout), Some(config_id)) => {
                    let layout = serde_json::from_value::<ChannelLayout>(layout.clone())
                        .map_err(|e| format!("Invalid loudness channel_layout: {e}"))?;
                    let config_id = config_id.as_str().ok_or_else(|| {
                        "loudness speaker_config must be a configuration ID string".to_string()
                    })?;
                    let config = sotf_host::speaker_config::get_speaker_config(config_id)
                        .ok_or_else(|| format!("Unknown loudness speaker_config: {config_id}"))?;
                    let config_layout = ChannelLayout::from_speaker_config(config)?;
                    if layout != config_layout {
                        return Err(
                            "loudness channel_layout conflicts with speaker_config".to_string()
                        );
                    }
                    Some(layout)
                }
                (None, None) => None,
            };
            let plugin = if let Some(layout) = explicit_layout {
                LoudnessMonitorPlugin::new_with_layout(channels, layout)
            } else {
                LoudnessMonitorPlugin::new(channels)
            }
            .map_err(|e| format!("Failed to create loudness monitor: {e}"))?
            .with_integrated_mode(integrated_mode)
            .map_err(|e| format!("Failed to configure loudness monitor: {e}"))?;
            Ok(Box::new(plugin))
        }

        "spectrum_analyzer" => {
            let config: SpectrumConfig = if parameters.is_null() {
                SpectrumConfig::default()
            } else {
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Failed to parse spectrum analyzer params: {e}"))?
            };
            let plugin =
                SpectrumAnalyzerPlugin::with_config_at_sample_rate(channels, sample_rate, config)
                    .map_err(|e| format!("Failed to create spectrum analyzer: {e}"))?;
            Ok(Box::new(plugin))
        }

        "resampler" => {
            #[derive(serde::Deserialize)]
            struct ResamplerParams {
                input_sample_rate: u32,
                output_sample_rate: u32,
                #[serde(default = "default_chunk_size")]
                chunk_size: usize,
                #[serde(default)]
                cutoff_smoothing: bool,
            }
            fn default_chunk_size() -> usize {
                1024
            }

            let params: ResamplerParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse resampler params: {e}"))?;
            let mut plugin = ResamplerPlugin::new(
                channels,
                params.input_sample_rate,
                params.output_sample_rate,
                params.chunk_size,
            )
            .map_err(|e| format!("Failed to create resampler: {e}"))?;
            if params.cutoff_smoothing {
                plugin
                    .set_parameter(
                        ParameterId::from("cutoff_smoothing"),
                        ParameterValue::Bool(true),
                    )
                    .map_err(|e| format!("Failed to apply resampler cutoff_smoothing: {e}"))?;
            }
            Ok(Box::new(plugin))
        }

        "band_split" => {
            let normalized = normalize_band_split_toolbar_choice_forms(parameters);
            let params: BandSplitPluginParams = serde_json::from_value(normalized)
                .map_err(|e| format!("Failed to parse band_split params: {e}"))?;
            let plugin = BandSplitPlugin::from_params(channels, &params)?;
            Ok(Box::new(plugin))
        }

        "band_merge" => {
            let params: BandMergePluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse band_merge params: {e}"))?;
            if params.bands == 0 {
                return Err("BandMerge requires at least 1 band".to_string());
            }
            if !channels.is_multiple_of(params.bands) {
                return Err(format!(
                    "BandMerge input width {channels} is not divisible by {} bands",
                    params.bands
                ));
            }
            let output_channels = channels / params.bands;
            let plugin = BandMergePlugin::from_params(output_channels, &params)?;
            Ok(Box::new(plugin))
        }

        "ab_compare" => {
            let params: ABComparePluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse A/B compare params: {e}"))?;
            let mut plugin = ABComparePlugin::from_params_with_factory(
                channels,
                sample_rate,
                params,
                create_plugin,
            )?;
            plugin.initialize(sample_rate)?;
            Ok(Box::new(plugin))
        }

        "aec" => {
            if channels != 2 {
                return Err(format!(
                    "AEC requires 2 input channels (microphone + reference), got {channels}"
                ));
            }
            let params: AecPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse AEC params: {e}"))?;
            let plugin = AecPlugin::from_params(sample_rate, params)?;
            Ok(Box::new(plugin))
        }

        "beamformer" => {
            let params: BeamformerPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse beamformer params: {e}"))?;
            if !(2..=crate::plugin_beamformer::mvdr::MAX_MICS).contains(&params.num_mics) {
                return Err(format!(
                    "Beamformer requires 2..={} microphones, got {}",
                    crate::plugin_beamformer::mvdr::MAX_MICS,
                    params.num_mics
                ));
            }
            if params.num_mics != channels {
                return Err(format!(
                    "Beamformer is configured for {} microphones, got {channels} input channels",
                    params.num_mics
                ));
            }
            let plugin = BeamformerPlugin::from_params(sample_rate, params)?;
            Ok(Box::new(plugin))
        }

        "ambisonics_decoder" => {
            let routing: sotf_plugin_ambisonics::AmbisonicsDecoderConfig =
                serde_json::from_value(parameters.clone())
                    .map_err(|e| format!("Failed to parse ambisonics decoder params: {e}"))?;
            // A custom target selects the user-geometry constructor; every
            // other value keeps the historical named-layout path unchanged.
            let mut plugin = if routing.target_layout
                == sotf_plugin_ambisonics::custom_layout::CUSTOM_LAYOUT_KEY
            {
                let custom: sotf_plugin_ambisonics::CustomDecoderConfig =
                    serde_json::from_value(parameters.clone()).map_err(|e| {
                        format!("Failed to parse custom ambisonics layout: {e}")
                    })?;
                sotf_plugin_ambisonics::AmbisonicsDecoderPlugin::new_custom(&custom)?
            } else {
                sotf_plugin_ambisonics::AmbisonicsDecoderPlugin::new(&routing)?
            };
            if plugin.input_channels() != channels {
                return Err(format!(
                    "Order-{} ambisonics requires {} input channels, got {channels}",
                    routing.order,
                    plugin.input_channels()
                ));
            }
            plugin.initialize(sample_rate)?;
            Ok(Box::new(plugin))
        }

        "external" => {
            validate_external_plugin_security_config(parameters)?;
            let descriptor = parse_external_plugin_descriptor(parameters)
                .map_err(|e| format!("Failed to parse external plugin descriptor: {e}"))?;
            let external_state = parse_external_plugin_state(parameters, &descriptor)?;
            let (required_input_channels, setup_is_explicit) = match external_state
                .as_ref()
                .and_then(|state| state.audio_setup.as_ref())
            {
                Some(setup) => (setup.channel_counts()?.0, true),
                None => (descriptor.audio_inputs, false),
            };
            if required_input_channels != 0 && required_input_channels != channels {
                let requirement = if setup_is_explicit {
                    "setup requires"
                } else {
                    "requires"
                };
                return Err(format!(
                    "External plugin '{}' {requirement} {required_input_channels} input channels, got {channels}",
                    descriptor.name
                ));
            }

            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                let plugin_trust = external_plugin_trust(parameters)?;
                if external_plugin_isolation_requested(parameters, plugin_trust)? {
                    let mut config =
                        parse_isolated_external_plugin_config(parameters, plugin_trust)?;
                    config.initial_state = external_state.clone();
                    let plugin = IsolatedExternalPlugin::new(descriptor, sample_rate, config)
                        .map_err(|e| format!("Failed to create isolated external plugin: {e}"))?;
                    return Ok(Box::new(plugin));
                }
            }

            let plugin = match external_state {
                Some(state) => ExternalPlugin::from_placeholder_state(&state, sample_rate),
                None => ExternalPlugin::new(&descriptor, sample_rate),
            }
            .map_err(|e| format!("Failed to load external plugin: {e}"))?;
            Ok(Box::new(plugin))
        }

        #[cfg(all(target_os = "macos", feature = "hal"))]
        "hal_input" => {
            use crate::{HalInputPlugin, HalInputPluginParams};
            let params: HalInputPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse HAL input params: {e}"))?;
            let plugin = HalInputPlugin::from_params(params)?;
            Ok(Box::new(plugin))
        }

        #[cfg(all(target_os = "macos", feature = "hal"))]
        "hal_output" => {
            use crate::{HalOutputPlugin, HalOutputPluginParams};
            let params: HalOutputPluginParams = serde_json::from_value(parameters.clone())
                .map_err(|e| format!("Failed to parse HAL output params: {e}"))?;
            let plugin = HalOutputPlugin::from_params(params)?;
            Ok(Box::new(plugin))
        }

        other => Err(format!("Unknown plugin type: {other}")),
    }
}

/// The Studio toolbar emits choice parameters as their visible labels or as
/// numeric indices. BandSplit's serialized constructor state uses enum strings
/// and an actual band count, so translate only those unambiguous toolbar forms
/// before deserializing. Numeric `num_bands` values retain their constructor
/// meaning (2, 3, or 4 bands).
fn normalize_band_split_toolbar_choice_forms(parameters: &serde_json::Value) -> serde_json::Value {
    let mut normalized = parameters.clone();
    let Some(fields) = normalized.as_object_mut() else {
        return normalized;
    };

    if let Some(mode) = fields.get("recombination_mode") {
        let canonical_mode = match mode {
            serde_json::Value::Number(index) => match index.as_i64() {
                Some(0) => Some("legacy_cascade"),
                Some(1) => Some("phase_compensated"),
                _ => None,
            },
            serde_json::Value::String(label) => match label.as_str() {
                "Legacy Cascade" => Some("legacy_cascade"),
                "Phase Compensated" => Some("phase_compensated"),
                _ => None,
            },
            _ => None,
        };
        if let Some(mode) = canonical_mode {
            fields.insert("recombination_mode".to_string(), serde_json::json!(mode));
        }
    }

    if let Some(bands) = fields.get("num_bands") {
        let band_count = match bands.as_str() {
            Some("2 Bands") => Some(2),
            Some("3 Bands") => Some(3),
            Some("4 Bands") => Some(4),
            _ => None,
        };
        if let Some(band_count) = band_count {
            fields.insert("num_bands".to_string(), serde_json::json!(band_count));
        }
    }

    normalized
}

/// Canonicalizes visible Crossover choice forms without changing stored counts.
///
/// Topology indices and labels are unambiguous. Band-count labels become actual
/// counts, while numeric values remain constructor counts rather than choice
/// indices; `PluginSettings::set_param_value` performs the index conversion at
/// the toolbar boundary.
fn normalize_crossover_toolbar_choice_forms(parameters: &serde_json::Value) -> serde_json::Value {
    let mut normalized = parameters.clone();
    let Some(fields) = normalized.as_object_mut() else {
        return normalized;
    };

    if let Some(topology) = fields.get("topology") {
        let canonical_topology = match topology {
            serde_json::Value::Number(index) => match index.as_u64() {
                Some(0) => Some("bands"),
                Some(1) => Some("per_channel"),
                _ => None,
            },
            serde_json::Value::String(label) => match label.as_str() {
                "Bands" => Some("bands"),
                "Per Channel" => Some("per_channel"),
                _ => None,
            },
            _ => None,
        };
        if let Some(topology) = canonical_topology {
            fields.insert("topology".to_string(), serde_json::json!(topology));
        }
    }

    if let Some(count) = fields.get("band_count") {
        let canonical_count = match count.as_str() {
            Some("2") => Some(2),
            Some("3") => Some(3),
            Some("4") => Some(4),
            _ => None,
        };
        if let Some(count) = canonical_count {
            fields.insert("band_count".to_string(), serde_json::json!(count));
        }
    }

    normalized
}

fn require_graph_input_channels(
    plugin_name: &str,
    graph_channels: usize,
    plugin: Box<dyn Plugin>,
) -> Result<Box<dyn Plugin>, String> {
    let plugin_channels = plugin.input_channels();
    if plugin_channels != graph_channels {
        return Err(format!(
            "{plugin_name} is configured for {plugin_channels} input channels, got {graph_channels}"
        ));
    }
    Ok(plugin)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn create_plugin_with_sandbox_grants<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
    sandbox_grants: &PluginSandboxGrantStore,
    preset_root: impl Into<PathBuf>,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_with_sandbox_options(
        plugin_type,
        parameters,
        channels,
        sample_rate.into(),
        &SandboxedPluginCreationOptions::authorized_runtime(
            sandbox_grants.clone(),
            preset_root,
            Vec::new(),
        ),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn create_plugin_with_sandbox_grants_for_backend<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
    sandbox_grants: &PluginSandboxGrantStore,
    preset_root: impl Into<PathBuf>,
    sandbox_launch_backend: PluginSandboxLaunchBackend,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_with_sandbox_options(
        plugin_type,
        parameters,
        channels,
        sample_rate.into(),
        &SandboxedPluginCreationOptions::authorized_runtime(
            sandbox_grants.clone(),
            preset_root,
            Vec::new(),
        )
        .with_backend(sandbox_launch_backend)
        .with_launcher(None),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
pub fn create_plugin_with_sandbox_grants_for_backend_and_launcher<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
    sandbox_grants: &PluginSandboxGrantStore,
    preset_root: impl Into<PathBuf>,
    sandbox_launch_backend: PluginSandboxLaunchBackend,
    sandbox_launcher_command: Option<ExternalPluginWorkerCommand>,
) -> Result<Box<dyn Plugin>, String> {
    create_plugin_with_sandbox_options(
        plugin_type,
        parameters,
        channels,
        sample_rate.into(),
        &SandboxedPluginCreationOptions::authorized_runtime(
            sandbox_grants.clone(),
            preset_root,
            Vec::new(),
        )
        .with_backend(sandbox_launch_backend)
        .with_launcher(sandbox_launcher_command),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn create_plugin_with_sandbox_options<S: Into<f64>>(
    plugin_type: &str,
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: S,
    options: &SandboxedPluginCreationOptions,
) -> Result<Box<dyn Plugin>, String> {
    let sample_rate = sample_rate.into();
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(format!("Plugin sample rate must be finite and positive, got {sample_rate}"));
    }
    if catalog_entry(plugin_type).is_some_and(|entry| entry.canonical_type == "external") {
        create_external_plugin_with_sandbox_options(parameters, channels, sample_rate, options)
    } else {
        create_plugin(plugin_type, parameters, channels, sample_rate)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn create_external_plugin_with_sandbox_options(
    parameters: &serde_json::Value,
    channels: usize,
    sample_rate: f64,
    options: &SandboxedPluginCreationOptions,
) -> Result<Box<dyn Plugin>, String> {
    validate_external_plugin_security_config(parameters)?;
    let descriptor = parse_external_plugin_descriptor(parameters)
        .map_err(|e| format!("Failed to parse external plugin descriptor: {e}"))?;
    let external_state = parse_external_plugin_state(parameters, &descriptor)?;
    if descriptor.audio_inputs != 0 && descriptor.audio_inputs != channels {
        return Err(format!(
            "External plugin '{}' requires {} input channels, got {channels}",
            descriptor.name, descriptor.audio_inputs
        ));
    }

    let plugin_trust = external_plugin_trust(parameters)?;
    if external_plugin_isolation_requested(parameters, plugin_trust)? {
        let mut config = parse_isolated_external_plugin_config(parameters, plugin_trust)?;
        config.sandbox_launch_backend = options.sandbox_launch_backend;
        config.sandbox_launcher_command = options.sandbox_launcher_command.clone();
        config.capability_sandbox_policy =
            Some(sandbox_policy_for_creation_options(options, &descriptor)?);
        config.initial_state = external_state;
        let plugin = IsolatedExternalPlugin::new(descriptor, sample_rate, config)
            .map_err(|e| format!("Failed to create isolated external plugin: {e}"))?;
        return Ok(Box::new(plugin));
    }

    let plugin = match external_state {
        Some(state) => ExternalPlugin::from_placeholder_state(&state, sample_rate),
        None => ExternalPlugin::new(&descriptor, sample_rate),
    }
    .map_err(|e| format!("Failed to load external plugin: {e}"))?;
    Ok(Box::new(plugin))
}

/// Create a matrix plugin (complex logic with auto-resize).
fn create_matrix_plugin(
    parameters: &serde_json::Value,
    channels: usize,
) -> Result<Box<dyn Plugin>, String> {
    #[derive(Debug, Clone, serde::Deserialize)]
    struct MatrixPluginParams {
        #[serde(default)]
        input_channels: Option<usize>,
        #[serde(default)]
        output_channels: Option<usize>,
        #[serde(default)]
        input_channel_map: Option<Vec<usize>>,
        #[serde(default)]
        output_channel_map: Option<Vec<usize>>,
        matrix: Vec<f32>,
        #[serde(default)]
        channel_states: Option<Vec<crate::ChannelState>>,
    }

    let params: MatrixPluginParams = serde_json::from_value(parameters.clone())
        .map_err(|e| format!("Failed to parse matrix params: {e}"))?;

    let mut plugin = if let (Some(in_map), Some(out_map)) =
        (params.input_channel_map, params.output_channel_map)
    {
        MatrixPlugin::with_sparse_mapping(in_map, out_map, params.matrix)
            .map_err(|e| format!("Failed to create sparse matrix plugin: {e}"))?
    } else if let (Some(in_ch), Some(out_ch)) = (params.input_channels, params.output_channels) {
        if in_ch == out_ch && in_ch != channels {
            log::info!(
                "[factory:matrix] Resizing square matrix from {in_ch}x{out_ch} to {channels}x{channels}"
            );
            let mut matrix = params.matrix;
            resize_matrix(&mut matrix, in_ch, out_ch, channels, channels);
            MatrixPlugin::with_matrix(channels, channels, matrix)
                .map_err(|e| format!("Failed to create resized matrix plugin: {e}"))?
        } else {
            MatrixPlugin::with_matrix(in_ch, out_ch, params.matrix)
                .map_err(|e| format!("Failed to create matrix plugin: {e}"))?
        }
    } else {
        return Err(
            "Matrix plugin requires either (input_channels, output_channels) \
             or (input_channel_map, output_channel_map)"
                .to_string(),
        );
    };

    if let Some(mut states) = params.channel_states {
        let needed = plugin.output_channels();
        if states.len() != needed {
            log::info!(
                "[factory] Resizing channel_states from {} to {needed}",
                states.len()
            );
            states.resize(needed, crate::ChannelState::default());
        }
        plugin = plugin.with_channel_states(states)?;
    }

    Ok(Box::new(plugin))
}
