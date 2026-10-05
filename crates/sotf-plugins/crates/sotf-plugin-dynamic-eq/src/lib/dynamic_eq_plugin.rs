use super::dyn_eq_band::DynEqBand;
use super::dynamic_eq_data::DynamicEqData;
use super::dynamic_eq_plugin_params::DynamicEqPluginParams;
use super::misc::DB_CONVERSION_FACTOR;
use super::misc::EPSILON;
use crate::params::{
    BAND_PARAMS, DynEqPlacement, DynEqShape, MAX_BANDS, PARAMS as DQ, ROUTING_PARAMS, SHELF_PARAMS,
    default_attack_ms, default_frequency, default_gain, default_knee, default_link_channels,
    default_mix, default_num_bands, default_q, default_ratio, default_release_ms,
    default_shelf_slope, default_threshold,
};
use math_audio_dsp::fast_math::fast_log10;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::param_bridge::apply_spec_update_modes;
use sotf_host::param_specs::ParamType;
use sotf_host::param_specs::UpdateMode;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginInfo, PluginResult, ProcessContext,
};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::Smoother;
use std::any::Any;
use std::sync::Arc;

pub struct DynamicEqPlugin {
    pub(super) channels: usize,
    pub(super) sample_rate: f64,
    pub(super) num_bands: usize,

    // Global params
    pub(super) threshold_db: f32,
    pub(super) ratio: f32,
    pub(super) attack_ms: f32,
    pub(super) release_ms: f32,
    pub(super) knee_db: f32,
    pub(super) link_channels: bool,
    pub(super) mix: f32,

    // Per-band state (pre-allocated for MAX_BANDS)
    pub(super) bands: Vec<DynEqBand>,

    // Disjoint channel pairs used by Left/Right/Mid/Side bands. Two-channel
    // instances default to `[[0, 1]]`. Stereo-only instances never read this
    // on the audio path, but explicit pairs are still validated fail-fast;
    // routed bands with empty pairs bypass silently on `from_params` (README).
    pub(super) stereo_pairs: Vec<[usize; 2]>,

    // Smoothers
    pub(super) mix_smoother: Smoother,
    pub(super) threshold_smoother: Smoother,

    // Monitoring
    pub(super) monitoring_gr: Vec<f32>,
    pub(super) cache: RealTimeCache<DynamicEqData>,
    pub(super) cache_counter: usize,

    // Dry buffer for mix (pre-allocated)
    pub(super) dry_buf: Vec<f32>,

    // Parameter IDs
    pub(super) param_num_bands: ParameterId,
    pub(super) param_threshold: ParameterId,
    pub(super) param_ratio: ParameterId,
    pub(super) param_attack: ParameterId,
    pub(super) param_release: ParameterId,
    pub(super) param_knee: ParameterId,
    pub(super) param_link_channels: ParameterId,
    pub(super) param_mix: ParameterId,

    pub(super) cached_parameters: Vec<Parameter>,
}

fn is_canonical_decimal_index(index: &str) -> bool {
    !index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && (index == "0" || !index.starts_with('0'))
}

/// Default pair geometry: stereo instances use channels `[[0, 1]]`; other
/// channel counts start with no pairs until explicit geometry is provided.
fn default_stereo_pairs(channels: usize) -> Vec<[usize; 2]> {
    if channels == 2 {
        vec![[0, 1]]
    } else {
        Vec::new()
    }
}

/// Validate explicit pair geometry, mirroring the `sotf-plugin-eq` contract:
/// pairs must be in range, use distinct channels, and stay disjoint. Routed
/// bands require at least one pair; two-channel input defaults to `[[0, 1]]`.
fn validate_stereo_pairs(
    num_channels: usize,
    pairs: Option<&[[usize; 2]]>,
    required: bool,
) -> Result<Vec<[usize; 2]>, String> {
    if let Some(pairs) = pairs {
        let mut occupied = vec![false; num_channels];
        for [left, right] in pairs {
            if *left >= num_channels || *right >= num_channels {
                return Err(format!(
                    "Dynamic EQ stereo pair [{left}, {right}] exceeds {num_channels} input channels"
                ));
            }
            if left == right {
                return Err(format!(
                    "Dynamic EQ stereo pair [{left}, {right}] must use distinct channels"
                ));
            }
            if occupied[*left] || occupied[*right] {
                return Err(format!(
                    "Dynamic EQ stereo pairs must be disjoint; channel {} is repeated",
                    if occupied[*left] { left } else { right }
                ));
            }
            occupied[*left] = true;
            occupied[*right] = true;
        }
    }

    if required {
        if num_channels < 2 {
            return Err("Dynamic EQ L/R/M/S placement requires at least two input channels".into());
        }
        match pairs {
            Some(pairs) if !pairs.is_empty() => Ok(pairs.to_vec()),
            Some(_) => Err("Dynamic EQ L/R/M/S placement requires at least one stereo pair".into()),
            None if num_channels == 2 => Ok(vec![[0, 1]]),
            None => Err(format!(
                "Dynamic EQ L/R/M/S placement on {num_channels} channels requires explicit stereo_pairs"
            )),
        }
    } else if let Some(pairs) = pairs {
        Ok(pairs.to_vec())
    } else {
        Ok(default_stereo_pairs(num_channels))
    }
}

/// Best-effort pair resolution for the infallible clamping constructor:
/// structurally sound explicit geometry is kept, anything else falls back
/// to the channel-count default. Strict constructors validate instead.
fn best_effort_stereo_pairs(channels: usize, pairs: Option<&[[usize; 2]]>) -> Vec<[usize; 2]> {
    match pairs {
        Some(pairs) if validate_stereo_pairs(channels, Some(pairs), false).is_ok() => {
            pairs.to_vec()
        }
        _ => default_stereo_pairs(channels),
    }
}

#[derive(Clone, Copy)]
enum RealtimeScalar {
    Threshold,
    Ratio,
    Attack,
    Release,
    Knee,
    Mix,
    BandThreshold(usize),
    BandRatio(usize),
}

impl DynamicEqPlugin {
    fn realtime_scalar(&self, id: &str) -> Option<RealtimeScalar> {
        Some(match id {
            "threshold" => RealtimeScalar::Threshold,
            "ratio" => RealtimeScalar::Ratio,
            "attack" => RealtimeScalar::Attack,
            "release" => RealtimeScalar::Release,
            "knee" => RealtimeScalar::Knee,
            "mix" => RealtimeScalar::Mix,
            _ => {
                let rest = id.strip_prefix("band_")?;
                let (index_text, field) = rest.split_once('_')?;
                let index = index_text.parse::<usize>().ok()?;
                if index >= MAX_BANDS
                    || (index >= self.num_bands
                        && (!is_canonical_decimal_index(index_text)
                            || !matches!(field, "band_threshold" | "band_ratio")))
                {
                    return None;
                }
                match field {
                    "threshold" | "band_threshold" => RealtimeScalar::BandThreshold(index),
                    "ratio" | "band_ratio" => RealtimeScalar::BandRatio(index),
                    _ => return None,
                }
            }
        })
    }

    fn realtime_parameter(&self, scalar: RealtimeScalar) -> Option<&Parameter> {
        let global_id = match scalar {
            RealtimeScalar::Threshold => Some("threshold"),
            RealtimeScalar::Ratio => Some("ratio"),
            RealtimeScalar::Attack => Some("attack"),
            RealtimeScalar::Release => Some("release"),
            RealtimeScalar::Knee => Some("knee"),
            RealtimeScalar::Mix => Some("mix"),
            RealtimeScalar::BandThreshold(_) | RealtimeScalar::BandRatio(_) => None,
        };
        if let Some(id) = global_id {
            return self.cached_parameters.iter().find(|parameter| {
                parameter.id.as_str() == id && parameter.update_mode == UpdateMode::Realtime
            });
        }

        let (index, field) = match scalar {
            RealtimeScalar::BandThreshold(index) => (index, "band_threshold"),
            RealtimeScalar::BandRatio(index) => (index, "band_ratio"),
            _ => unreachable!("global realtime controls returned above"),
        };
        self.cached_parameters.iter().find(|parameter| {
            if parameter.update_mode != UpdateMode::Realtime {
                return false;
            }
            let Some(rest) = parameter.id.as_str().strip_prefix("band_") else {
                return false;
            };
            let Some((parameter_index, parameter_field)) = rest.split_once('_') else {
                return false;
            };
            parameter_index.parse::<usize>().ok() == Some(index) && parameter_field == field
        })
    }

    fn apply_realtime_scalar(&mut self, scalar: RealtimeScalar, value: &ParameterValue) {
        let value = value
            .as_float()
            .expect("validated Dynamic EQ realtime scalar must be a float");
        match scalar {
            RealtimeScalar::Threshold => {
                self.threshold_db = value.clamp(-60.0, 0.0);
                self.threshold_smoother.set_target(self.threshold_db);
                for band in &mut self.bands {
                    if (band.band_threshold - self.threshold_db).abs() <= 0.01 {
                        band.use_band_threshold = false;
                    }
                }
            }
            RealtimeScalar::Ratio => {
                self.ratio = value.clamp(1.0, 20.0);
                for band in &mut self.bands {
                    if (band.band_ratio - self.ratio).abs() <= 0.01 {
                        band.use_band_ratio = false;
                    }
                }
            }
            RealtimeScalar::Attack => {
                self.attack_ms = value.clamp(0.1, 100.0);
                for band in &mut self.bands {
                    for core in &mut band.cores {
                        core.set_attack_release(self.attack_ms, self.release_ms);
                    }
                }
            }
            RealtimeScalar::Release => {
                self.release_ms = value.clamp(10.0, 1000.0);
                for band in &mut self.bands {
                    for core in &mut band.cores {
                        core.set_attack_release(self.attack_ms, self.release_ms);
                    }
                }
            }
            RealtimeScalar::Knee => self.knee_db = value.clamp(0.0, 20.0),
            RealtimeScalar::Mix => {
                let leaving_settled_dry = self.mix_smoother.current().abs() < 1.0e-6
                    && self.mix_smoother.target().abs() < 1.0e-6
                    && value > 0.0;
                self.mix = value.clamp(0.0, 1.0);
                if leaving_settled_dry {
                    for band in &mut self.bands {
                        band.reset(self.sample_rate);
                    }
                }
                self.mix_smoother.set_target(self.mix);
            }
            RealtimeScalar::BandThreshold(index) => {
                let band = &mut self.bands[index];
                band.band_threshold = value.clamp(-60.0, 0.0);
                band.use_band_threshold = (band.band_threshold - self.threshold_db).abs() > 0.01;
            }
            RealtimeScalar::BandRatio(index) => {
                let band = &mut self.bands[index];
                band.band_ratio = value.clamp(1.0, 20.0);
                band.use_band_ratio = (band.band_ratio - self.ratio).abs() > 0.01;
            }
        }
    }

    fn validate_value(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        let canonical = if let Some(rest) = id.0.strip_prefix("band_") {
            let (index_text, field) = rest
                .split_once('_')
                .ok_or_else(|| format!("Invalid dynamic EQ band parameter: {id}"))?;
            let index = index_text
                .parse::<usize>()
                .map_err(|_| format!("Invalid dynamic EQ band parameter: {id}"))?;
            let stored_realtime_scalar = matches!(field, "band_threshold" | "band_ratio");
            if index >= MAX_BANDS
                || (index >= self.num_bands
                    && (!is_canonical_decimal_index(index_text) || !stored_realtime_scalar))
            {
                return Err(format!("Dynamic EQ band index {index} is not active"));
            }
            let field = match field {
                "freq" => "frequency",
                "threshold" => "band_threshold",
                "ratio" => "band_ratio",
                "frequency" | "q" | "gain" | "band_threshold" | "band_ratio" | "active"
                | "solo" | "shape" | "shelf_slope" | "placement" => field,
                _ => return Err(format!("Unknown dynamic EQ band parameter: {id}")),
            };
            ParameterId::from(format!("band_{index}_{field}"))
        } else {
            id.clone()
        };
        let parameter = self
            .cached_parameters
            .iter()
            .find(|parameter| parameter.id == canonical)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?;
        parameter.validate(value)
    }

    pub fn new(channels: usize) -> Self {
        let sr = 44_100.0;
        let num_bands = default_num_bands();
        let attack = default_attack_ms();
        let release = default_release_ms();

        let bands: Vec<DynEqBand> = (0..MAX_BANDS)
            .map(|_| {
                DynEqBand::new(
                    channels,
                    sr,
                    default_frequency(),
                    default_q(),
                    default_gain(),
                    attack,
                    release,
                )
            })
            .collect();

        let threshold = default_threshold();
        let mix = default_mix();

        let mut p = Self {
            channels,
            sample_rate: sr,
            num_bands,

            threshold_db: threshold,
            ratio: default_ratio(),
            attack_ms: attack,
            release_ms: release,
            knee_db: default_knee(),
            link_channels: default_link_channels(),
            mix,

            bands,
            stereo_pairs: default_stereo_pairs(channels),

            mix_smoother: Smoother::new(mix, 5.0, sr),
            threshold_smoother: Smoother::new(threshold, 5.0, sr),

            monitoring_gr: vec![0.0; MAX_BANDS],
            cache: RealTimeCache::new(DynamicEqData::new(MAX_BANDS)),
            cache_counter: 0,

            // Pre-allocate dry buffer for max expected frame size
            // 8192 frames * 32 channels should be more than enough
            dry_buf: vec![0.0; 8192 * channels.max(2)],

            param_num_bands: ParameterId::from("num_bands"),
            param_threshold: ParameterId::from("threshold"),
            param_ratio: ParameterId::from("ratio"),
            param_attack: ParameterId::from("attack"),
            param_release: ParameterId::from("release"),
            param_knee: ParameterId::from("knee"),
            param_link_channels: ParameterId::from("link_channels"),
            param_mix: ParameterId::from("mix"),

            cached_parameters: Vec::new(),
        };

        p.rebuild_cached_parameters();
        p
    }

    pub fn from_params(channels: usize, params: DynamicEqPluginParams) -> Self {
        let mut p = Self::new(channels);
        p.num_bands = params.num_bands.clamp(1, MAX_BANDS);
        p.threshold_db = params.threshold.clamp(-60.0, 0.0);
        p.threshold_smoother.reset(p.threshold_db);
        p.ratio = params.ratio.clamp(1.0, 20.0);
        p.attack_ms = params.attack_ms.clamp(0.1, 100.0);
        p.release_ms = params.release_ms.clamp(10.0, 1000.0);
        p.knee_db = params.knee.clamp(0.0, 20.0);
        p.link_channels = params.link_channels;
        p.mix = params.mix.clamp(0.0, 1.0);
        p.mix_smoother.reset(p.mix);
        p.stereo_pairs = best_effort_stereo_pairs(channels, params.stereo_pairs.as_deref());

        // Apply per-band params
        for (i, band_params) in params.bands.iter().enumerate().take(MAX_BANDS) {
            let band = &mut p.bands[i];
            band.frequency = band_params.frequency.clamp(20.0, 20000.0);
            band.q = band_params.q.clamp(0.1, 10.0);
            band.target_gain_db = band_params.gain.clamp(-24.0, 24.0);
            band.shape = band_params.shape;
            band.placement = band_params.placement;
            band.shelf_slope = if band_params.shelf_slope.is_finite() {
                band_params.shelf_slope.clamp(0.1, 1.0)
            } else {
                default_shelf_slope()
            };
            band.band_threshold = band_params.band_threshold.clamp(-60.0, 0.0);
            band.band_ratio = band_params.band_ratio.clamp(1.0, 20.0);
            band.active = band_params.active;
            band.solo = band_params.solo;

            // If band values differ from global defaults, mark as overrides
            band.use_band_threshold = (band_params.band_threshold - params.threshold).abs() > 0.01;
            band.use_band_ratio = (band_params.band_ratio - params.ratio).abs() > 0.01;

            band.rebuild_sidechain_filters(p.sample_rate);
            band.rebuild_eq_filters(p.sample_rate);
        }

        // Update dynamics cores
        for band in &mut p.bands {
            for core in &mut band.cores {
                core.set_attack_release(p.attack_ms, p.release_ms);
            }
        }

        p.rebuild_cached_parameters();
        p
    }

    /// Construct from serialized/factory state without silently clamping an
    /// invalid value.  The legacy `from_params` constructor remains available
    /// for callers that intentionally use its clamping behaviour; factories
    /// and state-restore paths must use this fallible entry point instead.
    pub fn try_from_params(channels: usize, params: DynamicEqPluginParams) -> PluginResult<Self> {
        Self::validate_params(channels, &params, 48_000.0)?;
        Ok(Self::from_params(channels, params))
    }

    /// Factory variant that validates frequencies against the rate at which
    /// the instance will be used.  This prevents a valid-at-48 kHz preset from
    /// constructing invalid detector/EQ coefficients at low sample rates.
    pub fn try_from_params_at_sample_rate(
        channels: usize,
        params: DynamicEqPluginParams,
        sample_rate: impl Into<f64>,
    ) -> PluginResult<Self> {
        let sample_rate = sample_rate.into();
        Self::validate_params(channels, &params, sample_rate)?;
        let mut plugin = Self::from_params(channels, params);
        plugin.initialize(sample_rate)?;
        Ok(plugin)
    }

    fn validate_params(
        channels: usize,
        params: &DynamicEqPluginParams,
        sample_rate: f64,
    ) -> PluginResult<()> {
        if channels == 0 {
            return Err("Dynamic EQ requires at least one channel".to_string());
        }
        if !sample_rate.is_finite() || sample_rate < 100.0 {
            return Err(format!("Unsupported Dynamic EQ sample rate: {sample_rate}"));
        }
        if !(1..=MAX_BANDS).contains(&params.num_bands) {
            return Err(format!(
                "Dynamic EQ num_bands must be in 1..={MAX_BANDS}, got {}",
                params.num_bands
            ));
        }
        if params.bands.len() > MAX_BANDS {
            return Err(format!(
                "Dynamic EQ has {} band entries; maximum is {MAX_BANDS}",
                params.bands.len()
            ));
        }

        fn finite_range(name: &str, value: f32, min: f32, max: f32) -> PluginResult<()> {
            if value.is_finite() && (min..=max).contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "Dynamic EQ {name} must be finite and in {min}..={max}, got {value}"
                ))
            }
        }

        finite_range("threshold", params.threshold, -60.0, 0.0)?;
        finite_range("ratio", params.ratio, 1.0, 20.0)?;
        finite_range("attack_ms", params.attack_ms, 0.1, 100.0)?;
        finite_range("release_ms", params.release_ms, 10.0, 1000.0)?;
        finite_range("knee", params.knee, 0.0, 20.0)?;
        finite_range("mix", params.mix, 0.0, 1.0)?;

        let needs_pairs = params
            .bands
            .iter()
            .any(|band| band.placement.requires_stereo_pair());
        let _validated_pairs =
            validate_stereo_pairs(channels, params.stereo_pairs.as_deref(), needs_pairs)?;

        let max_frequency = (sample_rate as f32 * 0.475).min(20_000.0);
        for (index, band) in params.bands.iter().enumerate() {
            finite_range(
                &format!("band_{index}_frequency"),
                band.frequency,
                20.0,
                max_frequency,
            )?;
            finite_range(&format!("band_{index}_q"), band.q, 0.1, 10.0)?;
            finite_range(&format!("band_{index}_gain"), band.gain, -24.0, 24.0)?;
            finite_range(
                &format!("band_{index}_shelf_slope"),
                band.shelf_slope,
                0.1,
                1.0,
            )?;
            finite_range(
                &format!("band_{index}_threshold"),
                band.band_threshold,
                -60.0,
                0.0,
            )?;
            finite_range(&format!("band_{index}_ratio"), band.band_ratio, 1.0, 20.0)?;
            if band.shape == DynEqShape::Peak {
                let (_, high_edge) = super::misc::bandpass_edges(band.frequency, band.q);
                if high_edge >= max_frequency {
                    return Err(format!(
                        "Dynamic EQ band_{index} detector upper edge {high_edge} must be below {max_frequency} Hz"
                    ));
                }
            }
            if matches!(band.shape, DynEqShape::LowShelf | DynEqShape::HighShelf)
                && super::dyn_eq_band::design_shelf_coefficients(
                    band.shape,
                    band.frequency as f64,
                    sample_rate as f64,
                    band.gain as f64,
                    band.shelf_slope as f64,
                )
                .is_none()
            {
                return Err(format!(
                    "Dynamic EQ band_{index} shelf coefficients are invalid"
                ));
            }
            if band.shape == DynEqShape::Tilt
                && super::dyn_eq_band::design_tilt_coefficients(
                    band.frequency as f64,
                    sample_rate as f64,
                    band.gain as f64,
                )
                .is_none()
            {
                return Err(format!(
                    "Dynamic EQ band_{index} tilt coefficients are invalid"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let mut params = vec![
            Parameter::new_int(
                "num_bands",
                "Num Bands",
                self.num_bands as i32,
                pk(DQ, "num_bands").min_f64() as i32,
                pk(DQ, "num_bands").max_f64() as i32,
            )
            .with_description("Number of dynamic EQ bands")
            .with_group("Setup")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "threshold",
                "Threshold",
                self.threshold_db,
                pk(DQ, "threshold").min_f64() as f32,
                pk(DQ, "threshold").max_f64() as f32,
            )
            .with_description("Global detection threshold (dB)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "ratio",
                "Ratio",
                self.ratio,
                pk(DQ, "ratio").min_f64() as f32,
                pk(DQ, "ratio").max_f64() as f32,
            )
            .with_description("Global dynamics ratio")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "attack",
                "Attack",
                self.attack_ms,
                pk(DQ, "attack").min_f64() as f32,
                pk(DQ, "attack").max_f64() as f32,
            )
            .with_description("Attack time (ms)")
            .with_group("Timing")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "release",
                "Release",
                self.release_ms,
                pk(DQ, "release").min_f64() as f32,
                pk(DQ, "release").max_f64() as f32,
            )
            .with_description("Release time (ms)")
            .with_group("Timing")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "knee",
                "Knee",
                self.knee_db,
                pk(DQ, "knee").min_f64() as f32,
                pk(DQ, "knee").max_f64() as f32,
            )
            .with_description("Soft knee width (dB)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("link_channels", "Link Channels", self.link_channels)
                .with_description("Stereo-link detection")
                .with_group("Channels")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "mix",
                "Mix",
                self.mix,
                pk(DQ, "mix").min_f64() as f32,
                pk(DQ, "mix").max_f64() as f32,
            )
            .with_description("Dry/wet mix")
            .with_group("Output")
            .with_importance(ParameterImportance::Useful),
        ];

        // Expose stored settings for inactive slots as well. `num_bands`
        // controls processing topology; it must not hide values needed for
        // state round-trips or prevent later re-expansion.
        for i in 0..MAX_BANDS {
            let band = &self.bands[i];
            for spec in BAND_PARAMS {
                let id = format!("band_{i}_{}", spec.engine_key);
                let p = match spec.param_type {
                    ParamType::Float { min, max, .. } => {
                        let value = match spec.engine_key {
                            "frequency" => band.frequency,
                            "q" => band.q,
                            "gain" => band.target_gain_db,
                            "band_threshold" => band.band_threshold,
                            "band_ratio" => band.band_ratio,
                            _ => spec.default_f64() as f32,
                        };
                        Parameter::new_float(&id, spec.name, value, min as f32, max as f32)
                    }
                    ParamType::Bool { .. } => {
                        let value = match spec.engine_key {
                            "active" => band.active,
                            "solo" => band.solo,
                            _ => spec.default_bool(),
                        };
                        Parameter::new_bool(&id, spec.name, value)
                    }
                    _ => continue,
                };
                params.push(
                    p.with_description(spec.doc)
                        .with_group(spec.group)
                        .with_update_mode(match spec.engine_key {
                            "frequency" | "q" | "gain" | "active" | "solo" => {
                                UpdateMode::Structural
                            }
                            _ => UpdateMode::Realtime,
                        })
                        .with_importance(ParameterImportance::Useful),
                );
            }
            let shape_id = format!("band_{i}_shape");
            params.push(
                Parameter::new_int(&shape_id, "Shape", self.bands[i].shape.choice_index(), 0, 3)
                    .with_description("Peak, low shelf, high shelf, or tilt filter shape")
                    .with_group("EQ")
                    .with_update_mode(UpdateMode::Structural)
                    .with_importance(ParameterImportance::Useful),
            );
            let slope_spec = SHELF_PARAMS[1];
            params.push(
                Parameter::new_float(
                    &format!("band_{i}_shelf_slope"),
                    "Shelf Slope",
                    self.bands[i].shelf_slope,
                    slope_spec.min_f64() as f32,
                    slope_spec.max_f64() as f32,
                )
                .with_description(slope_spec.doc)
                .with_group(slope_spec.group)
                .with_update_mode(UpdateMode::Structural)
                .with_importance(ParameterImportance::Useful),
            );
            let placement_spec = ROUTING_PARAMS[0];
            params.push(
                Parameter::new_int(
                    &format!("band_{i}_placement"),
                    "Placement",
                    self.bands[i].placement.choice_index(),
                    0,
                    4,
                )
                .with_description(placement_spec.doc)
                .with_group(placement_spec.group)
                .with_update_mode(UpdateMode::Structural)
                .with_importance(ParameterImportance::Useful),
            );
        }

        apply_spec_update_modes(&mut params, DQ);
        self.cached_parameters = params;
    }

    /// Dry detector input of one routed band inside one stereo pair.
    fn routed_dry_sample(
        &self,
        frame: usize,
        placement: DynEqPlacement,
        left_ch: usize,
        right_ch: usize,
    ) -> f64 {
        let nc = self.channels;
        let left = self.dry_buf[frame * nc + left_ch] as f64;
        let right = self.dry_buf[frame * nc + right_ch] as f64;
        match placement {
            DynEqPlacement::Left => left,
            DynEqPlacement::Right => right,
            DynEqPlacement::Mid => 0.5 * (left + right),
            DynEqPlacement::Side => 0.5 * (left - right),
            DynEqPlacement::Stereo => unreachable!("stereo bands use the direct path"),
        }
    }

    /// Process one frame of one routed (non-stereo) band inside its pairs.
    ///
    /// Detection reads the dry buffer and the audible EQ edits the working
    /// buffer serially, exactly like the stereo path. Bands run in ascending
    /// index order and pairs in `stereo_pairs` order, so routing is
    /// deterministic. Linked mode shares one envelope driven by the maximum
    /// routed level; unlinked mode keeps one envelope per pair. Mid and Side
    /// use the `sotf-plugin-eq` law (`M=(L+R)/2`, `S=(L-R)/2`) with the
    /// pair's left-channel filter and envelope state.
    fn process_routed_band(
        &mut self,
        frame: usize,
        band_idx: usize,
        threshold: f32,
        band_ratio: f32,
        knee: f32,
        buffer: &mut [f32],
    ) {
        let nc = self.channels;
        let placement = self.bands[band_idx].placement;
        let target_gain_db = self.bands[band_idx].target_gain_db;
        if self.stereo_pairs.is_empty() {
            self.monitoring_gr[band_idx] = 0.0;
            return;
        }

        if self.link_channels {
            let mut max_level = 0.0f32;
            for pair_index in 0..self.stereo_pairs.len() {
                let [left_ch, right_ch] = self.stereo_pairs[pair_index];
                if left_ch >= nc || right_ch >= nc {
                    continue;
                }
                let sample = self.routed_dry_sample(frame, placement, left_ch, right_ch);
                let state_ch = routed_state_channel(placement, left_ch, right_ch);
                let filtered = self.bands[band_idx].apply_sidechain_bp(state_ch, sample) as f32;
                max_level = max_level.max(filtered.abs());
            }
            let level_db = DB_CONVERSION_FACTOR * fast_log10(max_level.max(EPSILON));
            let band = &mut self.bands[band_idx];
            let gr = band.cores[0].calculate_gain_reduction(level_db, threshold, band_ratio, knee);
            let smoothed = band.cores[0].apply_envelope(0, gr);
            let proportion = DynEqBand::modulation_proportion(target_gain_db, smoothed);
            self.monitoring_gr[band_idx] = smoothed;
            for pair_index in 0..self.stereo_pairs.len() {
                let [left_ch, right_ch] = self.stereo_pairs[pair_index];
                if left_ch >= nc || right_ch >= nc {
                    continue;
                }
                let band = &mut self.bands[band_idx];
                apply_routed_eq(
                    band,
                    placement,
                    frame,
                    nc,
                    left_ch,
                    right_ch,
                    proportion,
                    &mut *buffer,
                );
            }
        } else {
            let mut first_envelope = 0.0f32;
            let mut first = true;
            for pair_index in 0..self.stereo_pairs.len() {
                let [left_ch, right_ch] = self.stereo_pairs[pair_index];
                if left_ch >= nc || right_ch >= nc {
                    continue;
                }
                let sample = self.routed_dry_sample(frame, placement, left_ch, right_ch);
                let state_ch = routed_state_channel(placement, left_ch, right_ch);
                let band = &mut self.bands[band_idx];
                let filtered = band.apply_sidechain_bp(state_ch, sample) as f32;
                let level_db = DB_CONVERSION_FACTOR * fast_log10(filtered.abs().max(EPSILON));
                let gr = band.cores[state_ch]
                    .calculate_gain_reduction(level_db, threshold, band_ratio, knee);
                let smoothed = band.cores[state_ch].apply_envelope(0, gr);
                let proportion = DynEqBand::modulation_proportion(target_gain_db, smoothed);
                if first {
                    first_envelope = band.cores[state_ch].envelope_db(0);
                    first = false;
                }
                apply_routed_eq(
                    band,
                    placement,
                    frame,
                    nc,
                    left_ch,
                    right_ch,
                    proportion,
                    &mut *buffer,
                );
            }
            self.monitoring_gr[band_idx] = first_envelope;
        }
    }
}

/// Filter and envelope state channel of one routed pair.
///
/// Mid and Side share the pair's left-channel state, mirroring the
/// `sotf-plugin-eq` ordered route; Left and Right use their own channel.
fn routed_state_channel(placement: DynEqPlacement, left_ch: usize, right_ch: usize) -> usize {
    match placement {
        DynEqPlacement::Right => right_ch,
        _ => left_ch,
    }
}

/// Apply one routed band's held EQ blend inside one stereo pair.
#[allow(clippy::too_many_arguments)]
fn apply_routed_eq(
    band: &mut DynEqBand,
    placement: DynEqPlacement,
    frame: usize,
    channels: usize,
    left_ch: usize,
    right_ch: usize,
    proportion: f32,
    buffer: &mut [f32],
) {
    let left_index = frame * channels + left_ch;
    let right_index = frame * channels + right_ch;
    match placement {
        DynEqPlacement::Left => {
            let dry = buffer[left_index];
            let eq_out = band.process_eq(left_ch, dry as f64) as f32;
            buffer[left_index] = dry + (eq_out - dry) * proportion;
        }
        DynEqPlacement::Right => {
            let dry = buffer[right_index];
            let eq_out = band.process_eq(right_ch, dry as f64) as f32;
            buffer[right_index] = dry + (eq_out - dry) * proportion;
        }
        DynEqPlacement::Mid => {
            let left = buffer[left_index] as f64;
            let right = buffer[right_index] as f64;
            let mid = 0.5 * (left + right);
            let side = 0.5 * (left - right);
            let eq_out = band.process_eq(left_ch, mid);
            let mid_out = mid + (eq_out - mid) * proportion as f64;
            buffer[left_index] = (mid_out + side) as f32;
            buffer[right_index] = (mid_out - side) as f32;
        }
        DynEqPlacement::Side => {
            let left = buffer[left_index] as f64;
            let right = buffer[right_index] as f64;
            let mid = 0.5 * (left + right);
            let side = 0.5 * (left - right);
            let eq_out = band.process_eq(left_ch, side);
            let side_out = side + (eq_out - side) * proportion as f64;
            buffer[left_index] = (mid + side_out) as f32;
            buffer[right_index] = (mid - side_out) as f32;
        }
        DynEqPlacement::Stereo => unreachable!("stereo bands use the direct path"),
    }
}

impl ParametricInPlacePlugin for DynamicEqPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("DynamicEQ", env!("CARGO_PKG_VERSION"), "SotF")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Dynamics
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(PluginCostClass::Dynamics, None, 0, self.link_channels)
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            self.param_num_bands.clone(),
            ParameterValue::Int(self.num_bands as i32),
        );
        values.insert(
            self.param_threshold.clone(),
            ParameterValue::Float(self.threshold_db),
        );
        values.insert(self.param_ratio.clone(), ParameterValue::Float(self.ratio));
        values.insert(
            self.param_attack.clone(),
            ParameterValue::Float(self.attack_ms),
        );
        values.insert(
            self.param_release.clone(),
            ParameterValue::Float(self.release_ms),
        );
        values.insert(self.param_knee.clone(), ParameterValue::Float(self.knee_db));
        values.insert(
            self.param_link_channels.clone(),
            ParameterValue::Bool(self.link_channels),
        );
        values.insert(self.param_mix.clone(), ParameterValue::Float(self.mix));
        // Keep dormant slots in saved state. `num_bands` controls which bands
        // process audio; it does not erase the stored settings for bands that
        // are temporarily hidden by a smaller layout.
        for (i, band) in self.bands.iter().enumerate() {
            values.insert(
                ParameterId::from(format!("band_{i}_frequency").as_str()),
                ParameterValue::Float(band.frequency),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_q").as_str()),
                ParameterValue::Float(band.q),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_gain").as_str()),
                ParameterValue::Float(band.target_gain_db),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_band_threshold").as_str()),
                ParameterValue::Float(band.band_threshold),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_band_ratio").as_str()),
                ParameterValue::Float(band.band_ratio),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_active").as_str()),
                ParameterValue::Bool(band.active),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_solo").as_str()),
                ParameterValue::Bool(band.solo),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_shape").as_str()),
                ParameterValue::Int(band.shape.choice_index()),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_shelf_slope").as_str()),
                ParameterValue::Float(band.shelf_slope),
            );
            values.insert(
                ParameterId::from(format!("band_{i}_placement").as_str()),
                ParameterValue::Int(band.placement.choice_index()),
            );
        }
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        // Validate the whole batch before mutating so bulk preset application is atomic.
        for (id, value) in &values {
            self.validate_value(id, value)?;
        }
        if values.keys().any(|id| {
            id == &self.param_num_bands
                || id == &self.param_link_channels
                || id.0.strip_prefix("band_").is_some_and(|rest| {
                    let Some((_, field)) = rest.split_once('_') else {
                        return false;
                    };
                    matches!(
                        field,
                        "frequency"
                            | "freq"
                            | "q"
                            | "gain"
                            | "active"
                            | "solo"
                            | "shape"
                            | "shelf_slope"
                            | "placement"
                    )
                })
        }) {
            return Err("Dynamic EQ structural parameters require rebuilding the plugin".into());
        }
        for (id, value) in values {
            if id == self.param_num_bands {
                return Err("num_bands is structural; rebuild the plugin to change it".into());
            } else if id == self.param_threshold {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "threshold").default_f64() as f32);
                if v.is_finite() {
                    self.threshold_db = v.clamp(-60.0, 0.0);
                    self.threshold_smoother.set_target(self.threshold_db);
                    for band in &mut self.bands {
                        if (band.band_threshold - self.threshold_db).abs() <= 0.01 {
                            band.use_band_threshold = false;
                        }
                    }
                }
            } else if id == self.param_ratio {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "ratio").default_f64() as f32);
                if v.is_finite() {
                    self.ratio = v.clamp(1.0, 20.0);
                    for band in &mut self.bands {
                        if (band.band_ratio - self.ratio).abs() <= 0.01 {
                            band.use_band_ratio = false;
                        }
                    }
                }
            } else if id == self.param_attack {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "attack").default_f64() as f32);
                if v.is_finite() {
                    self.attack_ms = v.clamp(0.1, 100.0);
                    for band in &mut self.bands {
                        for core in &mut band.cores {
                            core.set_attack_release(self.attack_ms, self.release_ms);
                        }
                    }
                }
            } else if id == self.param_release {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "release").default_f64() as f32);
                if v.is_finite() {
                    self.release_ms = v.clamp(10.0, 1000.0);
                    for band in &mut self.bands {
                        for core in &mut band.cores {
                            core.set_attack_release(self.attack_ms, self.release_ms);
                        }
                    }
                }
            } else if id == self.param_knee {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "knee").default_f64() as f32);
                if v.is_finite() {
                    self.knee_db = v.clamp(0.0, 20.0);
                }
            } else if id == self.param_link_channels {
                unreachable!("structural parameters were rejected before batch mutation");
            } else if id == self.param_mix {
                let v = value
                    .as_float()
                    .unwrap_or(pk(DQ, "mix").default_f64() as f32);
                if v.is_finite() {
                    let leaving_settled_dry = self.mix_smoother.current().abs() < 1.0e-6
                        && self.mix_smoother.target().abs() < 1.0e-6
                        && v > 0.0;
                    self.mix = v.clamp(0.0, 1.0);
                    if leaving_settled_dry {
                        for band in &mut self.bands {
                            band.reset(self.sample_rate);
                        }
                    }
                    self.mix_smoother.set_target(self.mix);
                }
            } else if let Some(rest) = id.0.strip_prefix("band_") {
                // Per-band parameters: band_N_field
                if let Some(sep) = rest.find('_') {
                    let b_idx = rest[..sep].parse::<usize>().unwrap_or(0);
                    let field = &rest[sep + 1..];
                    if b_idx < self.bands.len() {
                        let band = &mut self.bands[b_idx];
                        match field {
                            "frequency" | "freq" | "q" | "gain" | "active" | "solo" | "shape"
                            | "shelf_slope" | "placement" => unreachable!(
                                "structural parameters were rejected before batch mutation"
                            ),
                            "threshold" | "band_threshold" => {
                                if let Some(v) = value.as_float() {
                                    band.band_threshold = v.clamp(-60.0, 0.0);
                                    band.use_band_threshold =
                                        (band.band_threshold - self.threshold_db).abs() > 0.01;
                                }
                            }
                            "ratio" | "band_ratio" => {
                                if let Some(v) = value.as_float() {
                                    band.band_ratio = v.clamp(1.0, 20.0);
                                    band.use_band_ratio =
                                        (band.band_ratio - self.ratio).abs() > 0.01;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        let id = id.as_str();
        let global_value = match id {
            "num_bands" => Some(ParameterValue::Int(self.num_bands as i32)),
            "threshold" => Some(ParameterValue::Float(self.threshold_db)),
            "ratio" => Some(ParameterValue::Float(self.ratio)),
            "attack" => Some(ParameterValue::Float(self.attack_ms)),
            "release" => Some(ParameterValue::Float(self.release_ms)),
            "knee" => Some(ParameterValue::Float(self.knee_db)),
            "link_channels" => Some(ParameterValue::Bool(self.link_channels)),
            "mix" => Some(ParameterValue::Float(self.mix)),
            _ => None,
        };
        if global_value.is_some() {
            return global_value;
        }

        let rest = id.strip_prefix("band_")?;
        let (index, field) = rest.split_once('_')?;
        let index = index.parse::<usize>().ok()?;
        let band = self.bands.get(index)?;
        let active = index < self.num_bands;
        if !active && !is_canonical_decimal_index(rest.split_once('_')?.0) {
            return None;
        }

        match field {
            "frequency" => Some(ParameterValue::Float(band.frequency)),
            "q" => Some(ParameterValue::Float(band.q)),
            "gain" => Some(ParameterValue::Float(band.target_gain_db)),
            "threshold" if active => Some(ParameterValue::Float(band.band_threshold)),
            "band_threshold" => Some(ParameterValue::Float(band.band_threshold)),
            "ratio" if active => Some(ParameterValue::Float(band.band_ratio)),
            "band_ratio" => Some(ParameterValue::Float(band.band_ratio)),
            "active" => Some(ParameterValue::Bool(band.active)),
            "solo" => Some(ParameterValue::Bool(band.solo)),
            "shape" => Some(ParameterValue::Int(band.shape.choice_index())),
            "shelf_slope" => Some(ParameterValue::Float(band.shelf_slope)),
            "placement" => Some(ParameterValue::Int(band.placement.choice_index())),
            _ => None,
        }
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        if let Some(scalar) = self.realtime_scalar(id.as_str()) {
            let parameter = self
                .realtime_parameter(scalar)
                .ok_or_else(|| format!("Unknown realtime parameter: {id}"))?;
            parameter.validate(&value)?;
            self.apply_realtime_scalar(scalar, &value);
            return Ok(());
        }
        self.validate_value(&id, &value)?;
        let mut values = ParameterSet::new();
        values.insert(id, value);
        self.apply_values(values)
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate < 100.0 {
            return Err(format!("Unsupported Dynamic EQ sample rate: {sample_rate}"));
        }
        // Conservative policy: preflight every stored slot, including bands
        // hidden by `num_bands`, so a rate change cannot strand an invalid
        // stored band that a later layout change would re-expose.
        for (index, band) in self.bands.iter().enumerate() {
            band.preflight_reinitialize(sample_rate)
                .map_err(|error| format!("Dynamic EQ band_{index} cannot initialize: {error}"))?;
        }

        self.sample_rate = sample_rate;

        for band in &mut self.bands {
            for core in &mut band.cores {
                core.initialize(sample_rate);
                core.set_attack_release(self.attack_ms, self.release_ms);
            }
        }

        self.mix_smoother.set_time(5.0, sample_rate);
        self.threshold_smoother.set_time(5.0, sample_rate);

        // Pre-allocate dry buffer for max expected frame size (up to 1s @ 96kHz)
        let buf_size = 96000 * self.channels.max(2);
        if self.dry_buf.len() < buf_size {
            self.dry_buf.resize(buf_size, 0.0);
        }

        // Successful reinitialization starts fresh recursive detector and EQ state.
        self.reset();

        Ok(())
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.reset(self.sample_rate);
        }
        self.monitoring_gr.fill(0.0);
        self.mix_smoother.reset(self.mix);
        self.threshold_smoother.reset(self.threshold_db);
        self.cache_counter = 0;
        self.cache.update(|data| data.update(&self.monitoring_gr));
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let nf = context.num_frames;
        let nc = self.channels;
        let total = nf
            .checked_mul(nc)
            .ok_or_else(|| "Frame/channel count overflow".to_string())?;
        if buffer.len() != total {
            return Err(format!(
                "Buffer size mismatch: expected {}, got {}",
                total,
                buffer.len()
            ));
        }

        // Reject blocks larger than pre-allocated dry buffer to maintain real-time safety.
        if total > self.dry_buf.len() {
            return Err(format!(
                "dynamic-eq: block size {} samples exceeds max {} samples",
                total,
                self.dry_buf.len()
            ));
        }

        // Save dry signal
        self.dry_buf[..total].copy_from_slice(&buffer[..total]);

        // A fully dry settled state is exactly transparent. Detector and EQ
        // state are deterministically reset before a later wet ramp begins.
        if self.mix_smoother.current().abs() < 1.0e-6 && self.mix_smoother.target().abs() < 1.0e-6 {
            self.monitoring_gr[..self.num_bands].fill(0.0);
            flush_denormals_inplace(buffer);
            return Ok(nf);
        }

        let knee = self.knee_db;
        let ratio = self.ratio;

        // Check for solo
        let any_solo = self.bands[..self.num_bands].iter().any(|b| b.solo);

        for frame in 0..nf {
            let global_threshold = self.threshold_smoother.advance();
            for band_idx in 0..self.num_bands {
                let (active, solo, target_gain_db, placement) = {
                    let band = &self.bands[band_idx];
                    (band.active, band.solo, band.target_gain_db, band.placement)
                };
                if !active {
                    continue;
                }
                if any_solo && !solo {
                    continue;
                }
                if target_gain_db.abs() < 0.01 {
                    self.monitoring_gr[band_idx] = 0.0;
                    continue;
                }

                let threshold = self.bands[band_idx].get_effective_threshold(global_threshold);
                let band_ratio = self.bands[band_idx].get_effective_ratio(ratio);

                if placement != DynEqPlacement::Stereo {
                    self.process_routed_band(
                        frame,
                        band_idx,
                        threshold,
                        band_ratio,
                        knee,
                        &mut *buffer,
                    );
                    continue;
                }

                let band = &mut self.bands[band_idx];

                if self.link_channels && nc > 1 {
                    // Linked: max detection across channels.
                    // Sidechain reads dry_buf to avoid inter-band contamination.
                    let mut max_level = 0.0f32;
                    for ch in 0..nc {
                        let idx = frame * nc + ch;
                        let filtered = band.apply_sidechain_bp(ch, self.dry_buf[idx] as f64) as f32;
                        let level = filtered.abs();
                        max_level = max_level.max(level);
                    }
                    let level_db = DB_CONVERSION_FACTOR * fast_log10(max_level.max(EPSILON));
                    let gr = band.cores[0]
                        .calculate_gain_reduction(level_db, threshold, band_ratio, knee);
                    let smoothed = band.cores[0].apply_envelope(0, gr);

                    // Proportion of the full EQ band shape to apply this sample.
                    // EQ biquad is held at target_gain_db; blend avoids coefficient updates.
                    let proportion =
                        DynEqBand::modulation_proportion(band.target_gain_db, smoothed);

                    self.monitoring_gr[band_idx] = smoothed;

                    for ch in 0..nc {
                        let idx = frame * nc + ch;
                        let dry = buffer[idx];
                        let eq_out = band.process_eq(ch, dry as f64) as f32;
                        // Blend: proportion=0 → dry passthrough; proportion=1 → full EQ
                        buffer[idx] = dry + (eq_out - dry) * proportion;
                    }
                } else {
                    // Per-channel detection.
                    // Sidechain reads dry_buf to avoid inter-band contamination.
                    for ch in 0..nc {
                        let idx = frame * nc + ch;
                        let filtered = band.apply_sidechain_bp(ch, self.dry_buf[idx] as f64) as f32;
                        let level = filtered.abs();
                        let level_db = DB_CONVERSION_FACTOR * fast_log10(level.max(EPSILON));
                        let gr = band.cores[ch]
                            .calculate_gain_reduction(level_db, threshold, band_ratio, knee);
                        // Each entry in `cores` owns state for exactly one channel,
                        // so its internal channel index is always zero.
                        let smoothed = band.cores[ch].apply_envelope(0, gr);

                        let proportion =
                            DynEqBand::modulation_proportion(band.target_gain_db, smoothed);

                        let dry = buffer[idx];
                        let eq_out = band.process_eq(ch, dry as f64) as f32;
                        buffer[idx] = dry + (eq_out - dry) * proportion;
                    }

                    // Use channel 0 GR for monitoring (read-only)
                    self.monitoring_gr[band_idx] = if nc > 0 {
                        band.cores[0].envelope_db(0)
                    } else {
                        0.0
                    };
                }
            }
        }

        // Mix dry/wet
        for frame in 0..nf {
            let mix = self.mix_smoother.advance();
            let dry_mix = 1.0 - mix;
            let offset = frame * nc;
            for (sample, dry) in buffer[offset..offset + nc]
                .iter_mut()
                .zip(self.dry_buf[offset..offset + nc].iter())
            {
                *sample = *dry * dry_mix + *sample * mix;
            }
        }

        // Update diagnostic cache (throttled)
        self.cache_counter += 1;
        if self.cache_counter >= 10 {
            self.cache_counter = 0;
            self.cache.update(|d| {
                d.update(&self.monitoring_gr);
            });
        }

        flush_denormals_inplace(buffer);
        Ok(nf)
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }
}
