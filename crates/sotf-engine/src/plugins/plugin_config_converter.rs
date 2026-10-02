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
        stereo_pairs,
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

    let global_has_placement = filters.iter().any(|filter| filter.placement.is_some());
    let channels_have_placement = channel_filters.as_ref().is_some_and(|channels| {
        channels
            .iter()
            .flatten()
            .any(|filter| filter.placement.is_some())
    });
    let channel_placement_guards: Vec<EQFilter> = channel_filters
        .as_deref()
        .unwrap_or_default()
        .iter()
        .flatten()
        .filter(|filter| filter.placement.is_some())
        .cloned()
        .collect();
    let channel_route_requested = global_has_placement || channels_have_placement;

    let convert_filters = |filters: &[EQFilter],
                           keep_explicit_route: bool,
                           preserve_disabled: bool|
     -> Vec<serde_json::Value> {
        use sotf_plugins::plugin_eq::EqFilterTopology;

        let any_soloed = filters.iter().any(|f| f.solo);
        filters
            .iter()
            .filter(|f| {
                if preserve_disabled {
                    return true;
                }
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
                if let Some(placement) = f.placement.or_else(|| {
                    keep_explicit_route.then_some(sotf_plugins::plugin_eq::EqBandPlacement::Stereo)
                }) {
                    value["placement"] = serde_json::json!(placement);
                }
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
    let with_stereo_pairs = |mut parameters: serde_json::Value| {
        if let Some(pairs) = stereo_pairs {
            parameters["stereo_pairs"] = serde_json::json!(pairs);
        }
        parameters
    };

    if *per_channel_mode {
        if let Some(ch_filters) = channel_filters {
            let channel_filter_configs: Vec<Vec<serde_json::Value>> = ch_filters
                .iter()
                .map(|f| convert_filters(f, channel_route_requested, false))
                .collect();
            let mut parameters = serde_json::json!({
                "channels": channels,
                "channel_filters": channel_filter_configs,
                "tdf2": tdf2,
                "topology": topology,
                "auto_gain": {"enabled": auto_gain_enabled},
                "oversampling": oversampling,
            });
            // The core rejects explicit placement with per-channel banks. Keep
            // placements from the dormant global bank visible to that validator
            // instead of silently dropping an incompatible stored setting.
            if global_has_placement {
                parameters["filters"] = serde_json::json!(convert_filters(filters, true, true));
            } else if channels_have_placement {
                // Use the inactive global list as a validation carrier for
                // placement keys from channel banks. The core checks those
                // keys before selecting channel_filters, including when the
                // placed source band is muted or removed by solo filtering.
                parameters["filters"] =
                    serde_json::json!(convert_filters(&channel_placement_guards, false, true));
            }
            Some(PluginConfig::new("eq", with_stereo_pairs(parameters)))
        } else {
            let filter_configs = convert_filters(filters, global_has_placement, false);
            Some(PluginConfig::new(
                "eq",
                with_stereo_pairs(serde_json::json!({
                    "channels": channels,
                    "filters": filter_configs,
                    "tdf2": tdf2,
                    "topology": topology,
                    "auto_gain": {"enabled": auto_gain_enabled},
                    "oversampling": oversampling,
                })),
            ))
        }
    } else {
        let filter_configs = convert_filters(filters, global_has_placement, false);
        Some(PluginConfig::new(
            "eq",
            with_stereo_pairs(serde_json::json!({
                "channels": channels,
                "filters": filter_configs,
                "tdf2": tdf2,
                "topology": topology,
                "auto_gain": {"enabled": auto_gain_enabled},
                "oversampling": oversampling,
            })),
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
        head_yaw_deg,
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
            "head_yaw_deg": head_yaw_deg,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use math_audio_iir_fir::BiquadFilterType;
    use sotf_plugins::plugin_eq::EqBandPlacement;

    fn global_eq_settings(filters: Vec<EQFilter>) -> PluginSettings {
        PluginSettings::EQ {
            channels: 5,
            filters,
            channel_filters: None,
            stereo_pairs: None,
            per_channel_mode: false,
            max_filters: 20,
            tdf2: false,
            topology: 0.0,
            auto_gain_enabled: false,
            oversampling: 1.0,
        }
    }

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
                    stereo_pairs: None,
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
    fn eq_legacy_conversion_omits_new_optional_route_fields() {
        let config = convert_eq(
            &global_eq_settings(vec![EQFilter::new(
                BiquadFilterType::Peak,
                1000.0,
                0.8,
                2.0,
            )]),
            48_000.0,
        )
        .unwrap();

        assert!(config.parameters.get("stereo_pairs").is_none());
        assert!(config.parameters["filters"][0].get("placement").is_none());
    }

    #[test]
    fn eq_explicit_route_keeps_order_after_solo_compaction_and_uses_stereo_sentinels() {
        let mut hidden = EQFilter::new(BiquadFilterType::Peak, 120.0, 0.8, 3.0);
        hidden.placement = Some(EqBandPlacement::Right);
        // The only explicit band is removed by solo filtering, but its
        // presence still opts the surviving filters into the ordered route.
        let mut first = EQFilter::new(BiquadFilterType::Lowpass, 500.0, 0.8, 0.0);
        first.solo = true;
        let mut middle = EQFilter::new(BiquadFilterType::Peak, 1500.0, 0.9, 4.0);
        middle.solo = true;
        let mut last = EQFilter::new(BiquadFilterType::Highpass, 4000.0, 0.8, 0.0);
        last.solo = true;

        let mut settings = global_eq_settings(vec![hidden, first, middle, last]);
        if let PluginSettings::EQ { stereo_pairs, .. } = &mut settings {
            *stereo_pairs = Some(vec![[0, 1], [3, 2]]);
        }
        let config = convert_eq(&settings, 48_000.0).unwrap();
        let filters = config.parameters["filters"].as_array().unwrap();

        assert_eq!(filters.len(), 3);
        assert_eq!(filters[0]["freq"], 500.0);
        assert_eq!(filters[1]["freq"], 1500.0);
        assert_eq!(filters[2]["freq"], 4000.0);
        assert_eq!(filters[0]["placement"], "stereo");
        assert_eq!(filters[1]["placement"], "stereo");
        assert_eq!(filters[2]["placement"], "stereo");
        assert_eq!(
            config.parameters["stereo_pairs"],
            serde_json::json!([[0, 1], [3, 2]])
        );
    }

    #[test]
    fn eq_compaction_preserves_surviving_mixed_realization_order() {
        let mut muted = EQFilter::new(BiquadFilterType::Peak, 200.0, 0.8, 2.0);
        muted.muted = true;
        let mut lowpass = EQFilter::new(BiquadFilterType::Lowpass, 500.0, 0.8, 0.0);
        lowpass.placement = Some(EqBandPlacement::Stereo);
        let mut kautz = EQFilter::new_kautz(
            1400.0,
            0.83,
            0.0,
            vec![sotf_plugins::plugin_eq::KautzSectionConfig {
                pole_freq: 1400.0,
                q: 0.83,
                gain: 0.7,
            }],
        );
        kautz.placement = Some(EqBandPlacement::Mid);
        let mut highpass = EQFilter::new(BiquadFilterType::Highpass, 4200.0, 0.8, 0.0);
        highpass.placement = Some(EqBandPlacement::Right);

        let config = convert_eq(
            &global_eq_settings(vec![muted, lowpass, kautz, highpass]),
            48_000.0,
        )
        .unwrap();
        let filters = config.parameters["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 3);
        assert_eq!(filters[0]["filter_type"], "lowpass");
        assert_eq!(filters[0]["placement"], "stereo");
        assert_eq!(filters[1]["topology"], "kautz_filter");
        assert_eq!(filters[1]["placement"], "mid");
        assert_eq!(filters[2]["filter_type"], "highpass");
        assert_eq!(filters[2]["placement"], "right");
    }

    #[test]
    fn eq_muted_explicit_band_keeps_surviving_global_filters_ordered() {
        let mut muted = EQFilter::new(BiquadFilterType::Peak, 200.0, 0.8, 2.0);
        muted.muted = true;
        muted.placement = Some(EqBandPlacement::Left);
        let active = EQFilter::new(BiquadFilterType::Peak, 1000.0, 0.8, -3.0);

        let config = convert_eq(&global_eq_settings(vec![muted, active]), 48_000.0).unwrap();
        let filters = config.parameters["filters"].as_array().unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0]["freq"], 1000.0);
        assert_eq!(filters[0]["placement"], "stereo");
    }

    #[test]
    fn eq_per_channel_conversion_retains_incompatible_dormant_global_placement() {
        let mut dormant = EQFilter::new(BiquadFilterType::Peak, 200.0, 0.8, 2.0);
        dormant.muted = true;
        dormant.placement = Some(EqBandPlacement::Left);
        let active = EQFilter::new(BiquadFilterType::Peak, 1000.0, 0.8, 0.0);
        let settings = PluginSettings::EQ {
            channels: 2,
            filters: vec![dormant],
            channel_filters: Some(vec![vec![active.clone()], vec![active]]),
            stereo_pairs: Some(vec![[0, 1]]),
            per_channel_mode: true,
            max_filters: 20,
            tdf2: false,
            topology: 0.0,
            auto_gain_enabled: false,
            oversampling: 1.0,
        };

        let config = convert_eq(&settings, 48_000.0).unwrap();
        assert_eq!(config.parameters["filters"][0]["placement"], "left");
        assert_eq!(
            config.parameters["channel_filters"][0][0]["placement"],
            "stereo"
        );
        assert_eq!(
            config.parameters["channel_filters"][1][0]["placement"],
            "stereo"
        );
    }

    #[test]
    fn eq_all_muted_per_channel_placement_remains_visible_to_validation() {
        let mut placed = EQFilter::new(BiquadFilterType::Peak, 1000.0, 0.8, 3.0);
        placed.muted = true;
        placed.placement = Some(EqBandPlacement::Left);
        let settings = PluginSettings::EQ {
            channels: 2,
            filters: Vec::new(),
            channel_filters: Some(vec![vec![placed], Vec::new()]),
            stereo_pairs: Some(vec![[0, 1]]),
            per_channel_mode: true,
            max_filters: 20,
            tdf2: false,
            topology: 0.0,
            auto_gain_enabled: false,
            oversampling: 1.0,
        };

        let config = convert_eq(&settings, 48_000.0).unwrap();
        assert_eq!(config.parameters["filters"][0]["placement"], "left");
        assert!(
            config.parameters["channel_filters"][0]
                .as_array()
                .unwrap()
                .is_empty()
        );
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
            head_yaw_deg: 45.0,
        };
        let config = PluginConfigConverterRegistry::global()
            .convert("crossfeed", &settings, 48_000.0)
            .expect("crossfeed converter registered");
        assert_eq!(config.plugin_type, "crossfeed");
        assert_eq!(config.parameters["bauer_fcut_hz"], 700.0);
        assert_eq!(config.parameters["head_yaw_deg"], 45.0);
    }

    #[test]
    fn registry_converts_eq_global() {
        let settings = PluginSettings::EQ {
            channels: 2,
            filters: vec![EQFilter::new(BiquadFilterType::Peak, 1000.0, 1.0, 2.0)],
            channel_filters: None,
            stereo_pairs: None,
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
                stereo_pairs: None,
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

    #[test]
    fn dynamic_eq_placement_and_pairs_reach_factory_and_audio() {
        use sotf_plugins::{
            DynEqBandParams, ParameterId, ParameterValue, ProcessContext, create_plugin,
            plugin_dynamic_eq::DynEqPlacement,
        };

        fn placed_band(
            shape: &str,
            frequency: f32,
            gain: f32,
            placement: DynEqPlacement,
        ) -> DynEqBandParams {
            let mut band: DynEqBandParams = serde_json::from_value(serde_json::json!({
                "shape": shape,
                "frequency": frequency,
                "q": 0.707,
                "gain": gain,
                "band_threshold": -48.0,
                "band_ratio": 4.0,
                "active": true,
                "solo": false,
            }))
            .expect("DynamicEQ band settings deserialize");
            band.placement = placement;
            band
        }

        let settings = PluginSettings::DynamicEq {
            num_bands: 2.0,
            threshold: -48.0,
            ratio: 4.0,
            attack: 5.0,
            release: 50.0,
            knee: 3.0,
            link_channels: true,
            mix: 1.0,
            bands: vec![
                placed_band("peak", 250.0, 8.0, DynEqPlacement::Left),
                placed_band("peak", 6_000.0, -7.0, DynEqPlacement::Right),
            ],
            stereo_pairs: Some(vec![[0, 1]]),
        };
        let registry = PluginConfigConverterRegistry::global();
        let config = registry
            .convert("dynamic_eq", &settings, 48_000.0)
            .expect("DynamicEQ settings converter is registered");
        assert_eq!(config.parameters["bands"][0]["placement"], "left");
        assert_eq!(config.parameters["bands"][1]["placement"], "right");
        assert_eq!(
            config.parameters["stereo_pairs"],
            serde_json::json!([[0, 1]])
        );

        let mut placed =
            create_plugin("dynamic_eq", &config.parameters, 2, 48_000).expect("factory builds");
        placed.initialize(48_000).expect("placed plugin initializes");
        assert_eq!(
            placed.get_parameter(&ParameterId::from("band_0_placement")),
            Some(ParameterValue::Int(1))
        );
        assert_eq!(
            placed.get_parameter(&ParameterId::from("band_1_placement")),
            Some(ParameterValue::Int(2))
        );

        let stereo_settings = PluginSettings::DynamicEq {
            num_bands: 2.0,
            threshold: -48.0,
            ratio: 4.0,
            attack: 5.0,
            release: 50.0,
            knee: 3.0,
            link_channels: true,
            mix: 1.0,
            bands: vec![
                placed_band("peak", 250.0, 8.0, DynEqPlacement::Stereo),
                placed_band("peak", 6_000.0, -7.0, DynEqPlacement::Stereo),
            ],
            stereo_pairs: Some(vec![[0, 1]]),
        };
        let stereo_config = registry
            .convert("dynamic_eq", &stereo_settings, 48_000.0)
            .expect("stereo control converts");
        let mut stereo = create_plugin("dynamic_eq", &stereo_config.parameters, 2, 48_000)
            .expect("stereo control builds");
        stereo.initialize(48_000).expect("stereo control initializes");

        // Default settings carry no pairs; the converter emits explicit
        // null, which the factory reads as the legacy default.
        let default_settings =
            PluginSettings::default_for(&crate::plugins::PluginType::DynamicEq).unwrap();
        let default_config = registry
            .convert("dynamic_eq", &default_settings, 48_000.0)
            .expect("default settings convert");
        assert_eq!(
            default_config.parameters["stereo_pairs"],
            serde_json::Value::Null
        );
        create_plugin("dynamic_eq", &default_config.parameters, 2, 48_000)
            .expect("null pairs build the legacy default");

        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let time = frame as f64 / 48_000.0;
                let left =
                    (0.22 * (std::f64::consts::TAU * 250.0 * time).sin()) as f32;
                let right =
                    (0.22 * (std::f64::consts::TAU * 6_000.0 * time).sin()) as f32;
                [left, right]
            })
            .collect();
        let mut placed_output = vec![f32::NAN; input.len()];
        let mut stereo_output = vec![f32::NAN; input.len()];
        for block_start in (0..frames).step_by(256) {
            let sample_start = block_start * 2;
            let sample_end = sample_start + 256 * 2;
            let context = ProcessContext::new(48_000, 256);
            assert_eq!(
                placed
                    .process(
                        &input[sample_start..sample_end],
                        &mut placed_output[sample_start..sample_end],
                        &context,
                    )
                    .expect("placed route processes block"),
                256
            );
            assert_eq!(
                stereo
                    .process(
                        &input[sample_start..sample_end],
                        &mut stereo_output[sample_start..sample_end],
                        &context,
                    )
                    .expect("stereo control processes block"),
                256
            );
        }
        assert!(placed_output.iter().all(|sample| sample.is_finite()));
        assert!(stereo_output.iter().all(|sample| sample.is_finite()));
        let difference_rms = placed_output
            .iter()
            .zip(&stereo_output)
            .map(|(placed, stereo)| f64::from(placed - stereo).powi(2))
            .sum::<f64>()
            / placed_output.len() as f64;
        let difference_rms = difference_rms.sqrt();
        assert!(
            difference_rms > 1.0e-3,
            "placed bands must steer rendered audio; RMS difference was {difference_rms}"
        );
    }

    #[test]
    fn linear_phase_eq_placement_and_pairs_reach_factory_and_audio() {
        use sotf_plugins::{ParameterId, ParameterValue, ProcessContext, create_plugin};

        fn peak_filter(
            frequency: f64,
            gain_db: f64,
            placement: Option<EqBandPlacement>,
        ) -> EQFilter {
            let mut filter = EQFilter::new(BiquadFilterType::Peak, frequency, 0.8, gain_db);
            filter.placement = placement;
            filter
        }

        fn render(
            plugin: &mut Box<dyn sotf_plugins::Plugin>,
            input: &[f32],
        ) -> Vec<f32> {
            let mut output = vec![f32::NAN; input.len()];
            let frames = input.len() / 2;
            for block_start in (0..frames).step_by(256) {
                let sample_start = block_start * 2;
                let sample_end = sample_start + 256 * 2;
                let context = ProcessContext::new(48_000, 256);
                assert_eq!(
                    plugin
                        .process(
                            &input[sample_start..sample_end],
                            &mut output[sample_start..sample_end],
                            &context,
                        )
                        .expect("linear-phase route processes block"),
                    256
                );
            }
            output
        }

        let registry = PluginConfigConverterRegistry::global();
        let mut placed = PluginSettings::default_for(&crate::plugins::PluginType::LinearPhaseEq)
            .expect("linear-phase defaults");
        if let PluginSettings::LinearPhaseEq {
            filters,
            stereo_pairs,
            ..
        } = &mut placed
        {
            *filters = vec![
                peak_filter(250.0, 6.0, Some(EqBandPlacement::Mid)),
                peak_filter(6_000.0, -6.0, Some(EqBandPlacement::Stereo)),
            ];
            *stereo_pairs = Some(vec![[0, 1]]);
        } else {
            panic!("expected LinearPhaseEq settings");
        }
        let config = registry
            .convert("linear_phase_eq", &placed, 48_000.0)
            .expect("linear-phase settings convert");
        assert_eq!(config.parameters["filters"][0]["placement"], "mid");
        assert_eq!(config.parameters["filters"][1]["placement"], "stereo");
        assert_eq!(
            config.parameters["stereo_pairs"],
            serde_json::json!([[0, 1]])
        );
        let mut placed_plugin =
            create_plugin("linear_phase_eq", &config.parameters, 2, 48_000)
                .expect("placed filters build");
        placed_plugin
            .initialize(48_000)
            .expect("placed filters initialize");
        assert_eq!(
            placed_plugin.get_parameter(&ParameterId::from("band_0_placement")),
            Some(ParameterValue::Int(4))
        );

        let mut stereo = PluginSettings::default_for(&crate::plugins::PluginType::LinearPhaseEq)
            .expect("linear-phase defaults");
        if let PluginSettings::LinearPhaseEq { filters, .. } = &mut stereo {
            *filters = vec![
                peak_filter(250.0, 6.0, Some(EqBandPlacement::Stereo)),
                peak_filter(6_000.0, -6.0, Some(EqBandPlacement::Stereo)),
            ];
        } else {
            panic!("expected LinearPhaseEq settings");
        }
        let stereo_config = registry
            .convert("linear_phase_eq", &stereo, 48_000.0)
            .expect("stereo control converts");
        let mut stereo_plugin =
            create_plugin("linear_phase_eq", &stereo_config.parameters, 2, 48_000)
                .expect("stereo control builds");
        stereo_plugin
            .initialize(48_000)
            .expect("stereo control initializes");

        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let time = frame as f64 / 48_000.0;
                let left = (0.22 * (std::f64::consts::TAU * 250.0 * time).sin()) as f32;
                let right = (0.22 * (std::f64::consts::TAU * 6_000.0 * time).sin()) as f32;
                [left, right]
            })
            .collect();
        let placed_output = render(&mut placed_plugin, &input);
        let stereo_output = render(&mut stereo_plugin, &input);
        assert!(placed_output.iter().all(|sample| sample.is_finite()));
        assert!(stereo_output.iter().all(|sample| sample.is_finite()));
        let difference_rms = placed_output
            .iter()
            .zip(&stereo_output)
            .map(|(placed, stereo)| f64::from(placed - stereo).powi(2))
            .sum::<f64>()
            / placed_output.len() as f64;
        let difference_rms = difference_rms.sqrt();
        assert!(
            difference_rms > 1.0e-3,
            "mid placement must steer rendered audio; RMS difference was {difference_rms}"
        );
    }

    #[test]
    fn linear_phase_eq_legacy_null_placement_matches_stereo_route() {
        use sotf_plugins::{ProcessContext, create_plugin};

        let registry = PluginConfigConverterRegistry::global();
        let mut legacy = PluginSettings::default_for(&crate::plugins::PluginType::LinearPhaseEq)
            .expect("linear-phase defaults");
        if let PluginSettings::LinearPhaseEq {
            filters,
            stereo_pairs,
            ..
        } = &mut legacy
        {
            *filters = vec![
                EQFilter::new(BiquadFilterType::Peak, 250.0, 0.8, 6.0),
                EQFilter::new(BiquadFilterType::Peak, 6_000.0, 0.8, -6.0),
            ];
            *stereo_pairs = None;
        } else {
            panic!("expected LinearPhaseEq settings");
        }
        let config = registry
            .convert("linear_phase_eq", &legacy, 48_000.0)
            .expect("legacy settings convert");
        assert_eq!(
            config.parameters["filters"][0]["placement"],
            serde_json::Value::Null
        );
        assert_eq!(
            config.parameters["stereo_pairs"],
            serde_json::Value::Null
        );
        // Old presets omit the new keys entirely and still load.
        let mut old_preset = serde_json::to_value(&legacy).unwrap();
        let fields = old_preset["LinearPhaseEq"].as_object_mut().unwrap();
        fields.remove("stereo_pairs");
        for filter in fields["filters"].as_array_mut().unwrap() {
            filter.as_object_mut().unwrap().remove("placement");
        }
        let reloaded: PluginSettings = serde_json::from_value(old_preset).unwrap();
        let reloaded_config = registry
            .convert("linear_phase_eq", &reloaded, 48_000.0)
            .expect("old preset converts");
        assert_eq!(reloaded_config.parameters, config.parameters);

        let mut stereo = legacy.clone();
        if let PluginSettings::LinearPhaseEq { filters, .. } = &mut stereo {
            for filter in filters {
                filter.placement = Some(EqBandPlacement::Stereo);
            }
        }
        let stereo_config = registry
            .convert("linear_phase_eq", &stereo, 48_000.0)
            .expect("stereo settings convert");
        assert_eq!(stereo_config.parameters["filters"][0]["placement"], "stereo");

        let frames = 8_192;
        let input: Vec<f32> = (0..frames)
            .flat_map(|frame| {
                let time = frame as f64 / 48_000.0;
                let left = (0.22 * (std::f64::consts::TAU * 250.0 * time).sin()) as f32;
                let right = (0.22 * (std::f64::consts::TAU * 6_000.0 * time).sin()) as f32;
                [left, right]
            })
            .collect();
        let mut legacy_plugin =
            create_plugin("linear_phase_eq", &config.parameters, 2, 48_000)
                .expect("legacy null placement builds");
        legacy_plugin
            .initialize(48_000)
            .expect("legacy plugin initializes");
        let mut stereo_plugin =
            create_plugin("linear_phase_eq", &stereo_config.parameters, 2, 48_000)
                .expect("stereo control builds");
        stereo_plugin
            .initialize(48_000)
            .expect("stereo plugin initializes");
        let mut legacy_output = vec![f32::NAN; input.len()];
        let mut stereo_output = vec![f32::NAN; input.len()];
        for block_start in (0..frames).step_by(256) {
            let sample_start = block_start * 2;
            let sample_end = sample_start + 256 * 2;
            let context = ProcessContext::new(48_000, 256);
            assert_eq!(
                legacy_plugin
                    .process(
                        &input[sample_start..sample_end],
                        &mut legacy_output[sample_start..sample_end],
                        &context,
                    )
                    .expect("legacy route processes block"),
                256
            );
            assert_eq!(
                stereo_plugin
                    .process(
                        &input[sample_start..sample_end],
                        &mut stereo_output[sample_start..sample_end],
                        &context,
                    )
                    .expect("stereo route processes block"),
                256
            );
        }
        let difference_rms = legacy_output
            .iter()
            .zip(&stereo_output)
            .map(|(legacy, stereo)| f64::from(legacy - stereo).powi(2))
            .sum::<f64>()
            / legacy_output.len() as f64;
        let difference_rms = difference_rms.sqrt();
        assert!(
            difference_rms < 1.0e-6,
            "null placement must match the stereo route; RMS difference was {difference_rms}"
        );
    }
}
