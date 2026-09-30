//! Converter registry: turn a typed [`PluginSettings`] variant into a wire [`PluginConfig`].
//!
//! The end goal is to retire the giant `match` in [`PluginSettings::to_plugin_config`].
//! Each plugin type registers a small converter function here; `to_plugin_config` looks
//! up the converter by the plugin's wire name and delegates to it.
//!
//! Unmigrated variants still fall back to the inline match in `plugin_settings.rs`.

use crate::PluginConfig;
use crate::plugins::{EQFilter, PluginSettings};
use sotf_plugins::ExternalPluginSandboxMode;
use std::collections::HashMap;
use std::sync::OnceLock;

mod dynamics;
mod effects;
mod eq;
mod spatial;

/// Signature for a function that converts one [`PluginSettings`] variant into a [`PluginConfig`].
///
/// Implementations should pattern-match on their specific variant and return `None` for any
/// other variant (this is only a safety net; the registry routes by wire name).
pub type PluginConfigConverter = fn(&PluginSettings, f64) -> Option<PluginConfig>;

/// Registry of converters keyed by the plugin's wire type string.
#[derive(Default)]
pub struct PluginConfigConverterRegistry {
    converters: HashMap<&'static str, PluginConfigConverter>,
}

impl PluginConfigConverterRegistry {
    /// Returns the global, lazily-initialized registry.
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<PluginConfigConverterRegistry> = OnceLock::new();
        GLOBAL.get_or_init(Self::build)
    }

    /// Convert a [`PluginSettings`] value if a converter is registered for its wire type.
    pub fn convert(
        &self,
        plugin_type: &str,
        settings: &PluginSettings,
        sample_rate: f64,
    ) -> Option<PluginConfig> {
        self.converters
            .get(plugin_type)
            .and_then(|c| c(settings, sample_rate))
    }

    fn build() -> Self {
        let mut registry = Self::default();
        registry.register("gain", convert_gain);
        registry.register("dither", effects::convert_dither);
        registry.register("eq", convert_eq);
        registry.register("delay", convert_delay);
        registry.register("crossfeed", convert_crossfeed);
        registry.register("aec", effects::convert_aec);
        registry.register("beamformer", spatial::convert_beamformer);
        registry.register("ambisonics_decoder", spatial::convert_ambisonics_decoder);
        registry.register("stereo_imager", effects::convert_stereo_imager);
        registry.register("de_esser", dynamics::convert_de_esser);
        registry.register("transient_shaper", dynamics::convert_transient_shaper);
        registry.register("saturation", effects::convert_saturation);
        registry.register("analog_eq", effects::convert_analog_eq);
        registry.register("analog_limiter", effects::convert_analog_limiter);
        registry.register("analog_compressor", effects::convert_analog_compressor);
        registry.register("dynamic_eq", dynamics::convert_dynamic_eq);
        registry.register("linear_phase_eq", eq::convert_linear_phase_eq);
        registry.register("fir_designer", eq::convert_linear_phase_eq);
        registry.register("spectral_compressor", dynamics::convert_spectral_compressor);
        registry.register("upmixer", spatial::convert_upmixer);
        registry.register("compressor", dynamics::convert_compressor);
        registry.register("limiter", dynamics::convert_limiter);
        registry.register("gate", dynamics::convert_gate);
        registry.register("expander", dynamics::convert_expander);
        registry.register(
            "multiband_compressor",
            dynamics::convert_multiband_compressor,
        );
        registry.register("multiband_expander", dynamics::convert_multiband_expander);
        registry.register(
            "loudness_compensation",
            effects::convert_loudness_compensation,
        );
        registry.register("fletcher_munson", effects::convert_fletcher_munson);
        registry.register("binaural_decoder", spatial::convert_binaural_decoder);
        registry.register("convolution", effects::convert_convolution);
        registry.register("loudness_monitor", effects::convert_loudness_monitor);
        registry.register("spectrum_analyzer", effects::convert_spectrum_analyzer);
        registry.register("channel_mute_solo", effects::convert_channel_mute_solo);
        registry.register("matrix", effects::convert_matrix);
        registry.register("xtc", spatial::convert_xtc);
        registry.register("denoiser", effects::convert_denoiser);
        registry.register("declick", effects::convert_declick);
        registry.register("hiss_reducer", effects::convert_hiss_reducer);
        registry.register("speech_denoiser", effects::convert_speech_denoiser);
        registry.register("pnd", effects::convert_pnd);
        registry.register("ab_compare", effects::convert_ab_compare);
        registry.register("crossover", spatial::convert_crossover);
        registry.register("band_split", spatial::convert_band_split);
        registry.register("band_merge", spatial::convert_band_merge);
        registry.register("downmix", spatial::convert_downmix);
        registry.register("mono_to_stereo", spatial::convert_mono_to_stereo);
        registry.register("aae", spatial::convert_aae);
        registry.register("external", convert_external);
        registry
    }

    fn register(&mut self, plugin_type: &'static str, converter: PluginConfigConverter) {
        self.converters.insert(plugin_type, converter);
    }
}

fn convert_external(settings: &PluginSettings, _sample_rate: f64) -> Option<PluginConfig> {
    let PluginSettings::External { state } = settings else {
        return None;
    };
    Some(PluginConfig::new(
        "external",
        serde_json::json!({
            "descriptor": state.descriptor,
            "plugin_trust": "unknown",
            "isolated": matches!(state.sandbox_mode, ExternalPluginSandboxMode::Isolated),
            "external_state": state,
        }),
    ))
}

fn convert_gain(settings: &PluginSettings, _sample_rate: f64) -> Option<PluginConfig> {
    let PluginSettings::Gain {
        channels,
        gain_db,
        smoothing_ms,
    } = settings
    else {
        return None;
    };
    Some(PluginConfig::new(
        "gain",
        serde_json::json!({
            "channels": channels,
            "gain_db": gain_db,
            "smoothing_ms": smoothing_ms,
        }),
    ))
}

fn convert_eq(settings: &PluginSettings, sample_rate: f64) -> Option<PluginConfig> {
    let PluginSettings::EQ {
        channels,
        filters,
        channel_filters,
        per_channel_mode,
        max_filters: _,
        tdf2,
        topology,
        auto_gain_enabled,
        oversampling,
    } = settings
    else {
        return None;
    };

    let convert_filters = |filters: &[EQFilter]| -> Vec<serde_json::Value> {
        use sotf_plugins::plugin_eq::EqFilterTopology;

        let any_soloed = filters.iter().any(|f| f.solo);
        filters
            .iter()
            .filter(|f| {
                if f.muted {
                    return false;
                }
                if any_soloed && !f.solo {
                    return false;
                }
                true
            })
            .map(|f| {
                let bq = f.to_biquad(sample_rate);
                let mut value = serde_json::json!({
                    "filter_type": bq.filter_type.long_name().to_lowercase(),
                    "freq": bq.freq,
                    "q": bq.q,
                    "db_gain": bq.db_gain,
                    "order": f.order,
                });
                if !matches!(f.topology, EqFilterTopology::Biquad) {
                    let obj = value.as_object_mut().expect("json! object");
                    match f.topology {
                        EqFilterTopology::Biquad => unreachable!(),
                        EqFilterTopology::WarpedBiquad => {
                            obj.insert("topology".into(), serde_json::json!("warped_biquad"));
                            if let Some(lambda) = f.lambda {
                                obj.insert("lambda".into(), serde_json::json!(lambda));
                            }
                        }
                        EqFilterTopology::KautzFilter => {
                            obj.insert("topology".into(), serde_json::json!("kautz_filter"));
                            if !f.kautz_sections.is_empty() {
                                obj.insert(
                                    "kautz_sections".into(),
                                    serde_json::to_value(&f.kautz_sections)
                                        .unwrap_or(serde_json::Value::Null),
                                );
                            }
                        }
                    }
                }
                value
            })
            .collect()
    };

    if *per_channel_mode {
        if let Some(ch_filters) = channel_filters {
            let channel_filter_configs: Vec<Vec<serde_json::Value>> =
                ch_filters.iter().map(|f| convert_filters(f)).collect();
            Some(PluginConfig::new(
                "eq",
                serde_json::json!({
                    "channels": channels,
                    "channel_filters": channel_filter_configs,
                    "tdf2": tdf2,
                    "topology": topology,
                    "auto_gain": {"enabled": auto_gain_enabled},
                    "oversampling": oversampling,
                }),
            ))
        } else {
            let filter_configs = convert_filters(filters);
            Some(PluginConfig::new(
                "eq",
                serde_json::json!({
                    "channels": channels,
                    "filters": filter_configs,
                    "tdf2": tdf2,
                    "topology": topology,
                    "auto_gain": {"enabled": auto_gain_enabled},
                    "oversampling": oversampling,
                }),
            ))
        }
    } else {
        let filter_configs = convert_filters(filters);
        Some(PluginConfig::new(
            "eq",
            serde_json::json!({
                "channels": channels,
                "filters": filter_configs,
                "tdf2": tdf2,
                "topology": topology,
                "auto_gain": {"enabled": auto_gain_enabled},
                "oversampling": oversampling,
            }),
        ))
    }
}

fn convert_delay(settings: &PluginSettings, _sample_rate: f64) -> Option<PluginConfig> {
    let PluginSettings::Delay {
        delay_ms,
        feedback,
        mix,
        lfo_rate_hz,
        lfo_depth_ms,
        pitch_preserving,
        allpass_feedback,
        allpass_coeff,
    } = settings
    else {
        return None;
    };
    Some(PluginConfig::new(
        "delay",
        serde_json::json!({
            "delay_ms": delay_ms,
            "feedback": feedback,
            "mix": mix,
            "lfo_rate_hz": lfo_rate_hz,
            "lfo_depth_ms": lfo_depth_ms,
            "pitch_preserving": pitch_preserving,
            "allpass_feedback": allpass_feedback,
            "allpass_coeff": allpass_coeff,
        }),
    ))
}

fn convert_crossfeed(settings: &PluginSettings, _sample_rate: f64) -> Option<PluginConfig> {
    let PluginSettings::Crossfeed {
        mode,
        preset,
        enabled,
        mix,
        bauer_fcut_hz,
        bauer_feed_db,
        meier_level,
        mb_low_freq_hz,
        mb_mid_high_freq_hz,
        mb_low_feed_db,
        mb_mid_feed_db,
        mb_high_feed_db,
        itd_delay_ms,
        autogain_enabled,
        autogain_target_lufs,
        autogain_max_gain_db,
        autogain_smoothing_ms,
    } = settings
    else {
        return None;
    };
    Some(PluginConfig::new(
        "crossfeed",
        serde_json::json!({
            "mode": mode,
            "preset": preset,
            "enabled": enabled,
            "mix": mix,
            "bauer_fcut_hz": bauer_fcut_hz,
            "bauer_feed_db": bauer_feed_db,
            "meier_level": meier_level,
            "mb_low_freq_hz": mb_low_freq_hz,
            "mb_mid_high_freq_hz": mb_mid_high_freq_hz,
            "mb_low_feed_db": mb_low_feed_db,
            "mb_mid_feed_db": mb_mid_feed_db,
            "mb_high_feed_db": mb_high_feed_db,
            "itd_delay_ms": itd_delay_ms,
            "autogain_enabled": autogain_enabled,
            "autogain_target_lufs": autogain_target_lufs,
            "autogain_max_gain_db": autogain_max_gain_db,
            "autogain_smoothing_ms": autogain_smoothing_ms,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use math_audio_iir_fir::BiquadFilterType;

    #[test]
    fn eq_conversion_preserves_global_and_channel_filter_orders() {
        let mut filter = EQFilter::new(BiquadFilterType::Lowpass, 1000.0, 0.707, 0.0);
        for order in [2, 4, 6, 8] {
            filter.order = order;
            for per_channel_mode in [false, true] {
                let settings = PluginSettings::EQ {
                    channels: 2,
                    filters: vec![filter.clone()],
                    channel_filters: Some(vec![vec![filter.clone()], vec![filter.clone()]]),
                    per_channel_mode,
                    max_filters: 1,
                    tdf2: false,
                    topology: 0.0,
                    auto_gain_enabled: false,
                    oversampling: 1.0,
                };
                let config = convert_eq(&settings, 48_000.0).unwrap();
                if per_channel_mode {
                    for channel in 0..2 {
                        assert_eq!(
                            config.parameters["channel_filters"][channel][0]["order"],
                            order
                        );
                    }
                } else {
                    assert_eq!(config.parameters["filters"][0]["order"], order);
                }
            }
        }
    }

    #[test]
    fn registry_converts_gain() {
        let settings = PluginSettings::Gain {
            channels: 2,
            gain_db: -3.0,
            smoothing_ms: 10.0,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("gain", &settings, 48_000.0)
            .expect("gain converter registered");
        assert_eq!(config.plugin_type, "gain");
        let params = config.parameters;
        assert_eq!(params["channels"], 2);
        assert_eq!(params["gain_db"], -3.0);
        assert_eq!(params["smoothing_ms"], 10.0);
    }

    #[test]
    fn registry_converts_delay() {
        let settings = PluginSettings::Delay {
            delay_ms: 100.0,
            feedback: 0.3,
            mix: 0.5,
            lfo_rate_hz: 0.0,
            lfo_depth_ms: 0.0,
            pitch_preserving: true,
            allpass_feedback: false,
            allpass_coeff: 0.0,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("delay", &settings, 48_000.0)
            .expect("delay converter registered");
        assert_eq!(config.plugin_type, "delay");
        assert_eq!(config.parameters["delay_ms"], 100.0);
        assert_eq!(config.parameters["pitch_preserving"], true);
    }

    #[test]
    fn registry_converts_crossfeed() {
        let settings = PluginSettings::Crossfeed {
            mode: sotf_plugins::CrossfeedMode::Bauer,
            preset: sotf_plugins::CrossfeedPreset::Default,
            enabled: true,
            mix: 0.5,
            bauer_fcut_hz: 700.0,
            bauer_feed_db: 2.0,
            meier_level: 0.5,
            mb_low_freq_hz: 200.0,
            mb_mid_high_freq_hz: 2000.0,
            mb_low_feed_db: 1.0,
            mb_mid_feed_db: 2.0,
            mb_high_feed_db: 3.0,
            itd_delay_ms: 0.0,
            autogain_enabled: false,
            autogain_target_lufs: -14.0,
            autogain_max_gain_db: 6.0,
            autogain_smoothing_ms: 100.0,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("crossfeed", &settings, 48_000.0)
            .expect("crossfeed converter registered");
        assert_eq!(config.plugin_type, "crossfeed");
        assert_eq!(config.parameters["bauer_fcut_hz"], 700.0);
    }

    #[test]
    fn registry_converts_eq_global() {
        let settings = PluginSettings::EQ {
            channels: 2,
            filters: vec![EQFilter::new(BiquadFilterType::Peak, 1000.0, 1.0, 2.0)],
            channel_filters: None,
            per_channel_mode: false,
            max_filters: 5,
            tdf2: false,
            topology: 0.0,
            auto_gain_enabled: false,
            oversampling: 1.0,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("eq", &settings, 48_000.0)
            .expect("eq converter registered");
        assert_eq!(config.plugin_type, "eq");
        assert!(config.parameters["filters"].is_array());
        assert_eq!(config.parameters["auto_gain"]["enabled"], false);
        assert_eq!(config.parameters["oversampling"], 1.0);
        assert_eq!(config.parameters["topology"], 0.0);
    }

    #[test]
    fn registry_returns_none_for_unregistered_type() {
        let settings = PluginSettings::Gain {
            channels: 2,
            gain_db: 0.0,
            smoothing_ms: 0.0,
        };
        assert!(
            PluginConfigConverterRegistry::global()
                .convert("not_a_plugin", &settings, 48_000.0)
                .is_none()
        );
    }

    #[test]
    fn external_settings_convert_descriptor_and_isolation_losslessly() {
        use sotf_plugins::{
            ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, PluginFormat,
            PluginScanStatus,
        };

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.clap");
        std::fs::write(&path, b"fixture").unwrap();
        let descriptor = PluginDescriptor {
            id: "clap.test".into(),
            name: "Test Plug-in".into(),
            vendor: "SOTF".into(),
            version: "1.0".into(),
            format: PluginFormat::Clap,
            path,
            audio_inputs: 2,
            audio_outputs: 4,
            is_instrument: false,
            categories: vec!["Effect".into()],
            scan_status: PluginScanStatus::Loadable,
        };
        let state = ExternalPluginState::new(
            descriptor.clone(),
            ExternalPluginSandboxMode::Isolated,
            vec![1, 2, 3],
        );
        let settings = PluginSettings::External {
            state: state.clone(),
        };

        let config = settings.to_plugin_config(48_000.0);

        assert_eq!(config.plugin_type, "external");
        assert_eq!(
            config.parameters["descriptor"],
            serde_json::json!(descriptor)
        );
        assert_eq!(
            config.parameters["external_state"],
            serde_json::json!(state)
        );
        assert_eq!(config.parameters["isolated"], true);
        assert_eq!(config.parameters["plugin_trust"], "unknown");
    }

    #[test]
    fn registry_converts_all_plugin_types() {
        use crate::plugins::PluginType;
        for plugin_type in PluginType::all() {
            let settings = PluginSettings::default_for(&plugin_type).unwrap();
            let wire_type = settings.plugin_type().wire_name();
            let config = PluginConfigConverterRegistry::global()
                .convert(wire_type, &settings, 48_000.0)
                .unwrap_or_else(|| panic!("converter not registered for {}", wire_type));
            assert_eq!(config.plugin_type, wire_type);
        }
    }

    #[test]
    fn registry_converts_legacy_fletcher_munson() {
        let settings = PluginSettings::FletcherMunson {
            playback_volume_db: -10.0,
            reference_level_db: 0.0,
            enabled: true,
            band1_freq: 60.0,
            band1_q: 0.7,
            band1_max_gain: 10.0,
            band1_slope: 1.0,
            band2_freq: 200.0,
            band2_q: 0.7,
            band2_max_gain: 8.0,
            band2_slope: 1.0,
            band3_freq: 4000.0,
            band3_q: 0.7,
            band3_max_gain: 6.0,
            band3_slope: 1.0,
            band4_freq: 12000.0,
            band4_q: 0.7,
            band4_max_gain: 4.0,
            band4_slope: 1.0,
            smoothing_ms: 50.0,
            auto_gain_enabled: false,
            auto_gain_max_db: 6.0,
            auto_gain_smoothing_ms: 100.0,
            auto_gain_loudness_type: 0,
            iso_226: false,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("fletcher_munson", &settings, 48_000.0)
            .expect("fletcher_munson converter registered");
        assert_eq!(config.plugin_type, "loudness_compensation");
        assert_eq!(config.parameters["mode"], 2);
    }

    #[test]
    fn dynamic_eq_shelf_settings_reach_factory_and_audio_processing() {
        use sotf_plugins::{DynEqBandParams, ProcessContext, create_plugin};

        fn make_band(shape: &str, frequency: f32, gain: f32, shelf_slope: f32) -> DynEqBandParams {
            serde_json::from_value(serde_json::json!({
                "shape": shape,
                "shelf_slope": shelf_slope,
                "frequency": frequency,
                "q": 0.707,
                "gain": gain,
                "band_threshold": -48.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false,
            }))
            .expect("DynamicEQ band settings deserialize")
        }

        fn settings(use_shelves: bool) -> PluginSettings {
            let (low_shape, high_shape) = if use_shelves {
                ("low_shelf", "high_shelf")
            } else {
                ("peak", "peak")
            };
            PluginSettings::DynamicEq {
                num_bands: 2.0,
                threshold: -48.0,
                ratio: 4.0,
                attack: 5.0,
                release: 50.0,
                knee: 3.0,
                link_channels: true,
                mix: 1.0,
                bands: vec![
                    make_band(low_shape, 250.0, 8.0, 0.7),
                    make_band(high_shape, 6_000.0, -7.0, 0.8),
                ],
            }
        }

        let registry = PluginConfigConverterRegistry::global();
        let shelf_config = registry
            .convert("dynamic_eq", &settings(true), 48_000.0)
            .expect("DynamicEQ settings converter is registered");
        assert_eq!(shelf_config.plugin_type, "dynamic_eq");
        assert_eq!(shelf_config.parameters["bands"][0]["shape"], "low_shelf");
        assert_eq!(shelf_config.parameters["bands"][1]["shape"], "high_shelf");
        assert_eq!(
            shelf_config.parameters["bands"][0]["shelf_slope"]
                .as_f64()
                .unwrap() as f32,
            0.7_f32
        );
        assert_eq!(
            shelf_config.parameters["bands"][1]["shelf_slope"]
                .as_f64()
                .unwrap() as f32,
            0.8_f32
        );

        let peak_config = registry
            .convert("dynamic_eq", &settings(false), 48_000.0)
            .expect("Peak control uses the same settings converter");
        let mut shelf = create_plugin("dynamic_eq", &shelf_config.parameters, 2, 48_000)
            .expect("factory constructs configured shelf bands");
        let mut peak = create_plugin("dynamic_eq", &peak_config.parameters, 2, 48_000)
            .expect("factory constructs the Peak control");
        shelf.initialize(48_000).expect("shelf plugin initializes");
        peak.initialize(48_000).expect("Peak plugin initializes");

        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let time = frame as f64 / 48_000.0;
                let tone = |frequency: f64, phase: f64| {
                    (0.22 * (std::f64::consts::TAU * frequency * time + phase).sin()) as f32
                };
                [
                    tone(125.0, 0.0) + tone(1_100.0, 0.3) + tone(7_500.0, -0.2),
                    tone(180.0, 0.2) + tone(1_700.0, -0.4) + tone(8_200.0, 0.5),
                ]
            })
            .collect();
        let mut shelf_output = vec![f32::NAN; input.len()];
        let mut peak_output = vec![f32::NAN; input.len()];

        for block_start in (0..frames).step_by(256) {
            let sample_start = block_start * 2;
            let sample_end = sample_start + 256 * 2;
            let context = ProcessContext::new(48_000, 256);
            assert_eq!(
                shelf
                    .process(
                        &input[sample_start..sample_end],
                        &mut shelf_output[sample_start..sample_end],
                        &context,
                    )
                    .expect("shelf route processes block"),
                256
            );
            assert_eq!(
                peak.process(
                    &input[sample_start..sample_end],
                    &mut peak_output[sample_start..sample_end],
                    &context,
                )
                .expect("Peak control processes block"),
                256
            );
        }

        assert!(shelf_output.iter().all(|sample| sample.is_finite()));
        assert!(peak_output.iter().all(|sample| sample.is_finite()));
        let difference_rms = shelf_output
            .iter()
            .zip(&peak_output)
            .map(|(shelf, peak)| f64::from(shelf - peak).powi(2))
            .sum::<f64>()
            / shelf_output.len() as f64;
        let difference_rms = difference_rms.sqrt();
        assert!(
            difference_rms > 1.0e-3,
            "shelf configuration must affect rendered audio; RMS difference was {difference_rms}"
        );
    }
}
