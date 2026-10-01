use super::band_compressor_params::BandCompressorParams;
use super::multiband_compressor_data::MultibandCompressorData;
use super::types::BandCompressor;
use super::types::MultibandCompressorPluginParams;
use crate::params::{
    BAND_TEMPLATE as MCB, DETECTION_MODES, GLOBAL_PARAMS as MC, HPF_ORDERS, PARAMS as SC,
};
use math_audio_dsp::fast_math::{fast_log10, fast_pow10};
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_makeup::MeasuredMakeup;
use sotf_host::detector::{DetectionMode, LevelDetector};
use sotf_host::lookahead::LookaheadBuffer;
use sotf_host::lr4_crossover::Lr4Crossover;
use sotf_host::param_bridge;
use sotf_host::param_specs::{UpdateMode, find_by_key as pk};
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCompiledOp, PluginCostClass, PluginDrainResult, PluginInfo,
    PluginResult, ProcessContext, TailLength,
};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::{LogSmoother, Smoother};
use std::any::Any;
use std::sync::Arc;

/// Bounds work in each finite-stream drain call without additional scratch.
const MAX_DRAIN_FRAMES: usize = 256;

#[derive(Debug, Clone, Copy)]
pub(super) struct BandDynamicsSmoothers {
    threshold: Smoother,
    ratio: Smoother,
    attack_coeff: Smoother,
    release_coeff: Smoother,
    knee: Smoother,
    makeup_db: Smoother,
    applied_tilt_db: f32,
}

pub struct MultibandCompressorPlugin {
    pub(super) channels: usize,
    pub(super) sample_rate: u32,
    has_input: bool,
    drain_remaining: Option<usize>,
    pub(super) num_bands: usize,
    pub(super) _crossover_preset: i32,
    pub(super) crossover_frequencies: Vec<f32>,
    pub(super) threshold_db: f32,
    pub(super) ratio: f32,
    pub(super) attack_ms: f32,
    pub(super) release_ms: f32,
    pub(super) knee_db: f32,
    pub(super) range_db: f32,
    pub(super) hold_ms: f32,
    pub(super) link_channels: bool,
    pub(super) mix: f32,
    pub(super) per_band_lookahead_ms: f32,
    pub(super) ms_mode: bool,
    pub(super) sidechain_tilt_db: f32,
    pub(super) link_amount: f32,
    /// Sidechain high-pass frequency in Hz. Inert while disabled; 0 Hz
    /// also bypasses when enabled.
    pub(super) sidechain_hpf_hz: f32,
    /// Sidechain HPF enable. Default false preserves legacy no-HPF audio
    /// for presets without this key, at any stored frequency.
    pub(super) sidechain_hpf_enabled: bool,
    /// Sidechain HPF order as an `HPF_ORDERS` index (0 = 2nd, 1 = 4th).
    /// Stored as an index so live changes never allocate a label string.
    pub(super) sidechain_hpf_order_index: usize,
    /// Detection mode as a `DETECTION_MODES` index (0 = Peak, 1 = RMS).
    pub(super) detection_mode_index: usize,
    /// Program-dependent release (single-band compatibility, not yet applied to DSP)
    #[allow(dead_code)]
    pub(super) program_dependent_release: bool,
    /// External sidechain (single-band compatibility, not yet applied to DSP)
    #[allow(dead_code)]
    pub(super) sidechain_external: bool,
    /// Per-band, per-channel tilt biquad pair (lowshelf + highshelf) for sidechain tilt.
    /// Layout: [band][channel]. Empty when tilt_db ≈ 0.
    pub(super) sidechain_tilt_biquads: Vec<Vec<(Biquad, Biquad)>>,
    /// Per-band, per-channel sidechain HPF sections. Layout: [band][channel][section].
    /// Always two preallocated sections; the active prefix (0/1/2) derives from
    /// `sidechain_hpf_hz`/`sidechain_hpf_order` so live changes never allocate.
    pub(super) sidechain_hpf_biquads: Vec<Vec<Vec<Biquad>>>,
    /// Per-band, per-channel level detectors (Peak or 10 ms RMS).
    /// Layout: [band][channel]. RMS energy buffers stay preallocated so mode
    /// switches never allocate on the audio path.
    pub(super) level_detectors: Vec<Vec<LevelDetector>>,
    /// Per-channel detected-level scratch reused across frames (no audio meaning).
    pub(super) detect_scratch: Vec<f32>,
    pub(super) band_params: Vec<BandCompressorParams>,
    pub(super) crossover_points: Vec<Lr4Crossover<f32>>,
    pub(super) band_compressors: Vec<BandCompressor>,
    pub(super) band_buffers: Vec<f32>,
    pub(super) band_levels_db: Vec<f32>,
    pub(super) dry_buffer: Vec<f32>,
    pub(super) threshold_smoother: Smoother,
    pub(super) mix_smoother: Smoother,
    pub(super) link_smoother: Smoother,
    pub(super) tilt_smoother: Smoother,
    pub(super) band_smoothers: Vec<BandDynamicsSmoothers>,
    /// Exact per-frame global threshold/mix/link/tilt values for band-major processing.
    pub(super) automation_values: Vec<[f32; 4]>,
    pub(super) xover_smoothers: Vec<LogSmoother>,

    /// Per-band lookahead delay buffers (one per band, each with `channels` interleaved).
    pub(super) lookahead_buffers: Vec<LookaheadBuffer>,
    /// Matches the wet lookahead for phase-aligned dry/wet blending.
    pub(super) dry_lookahead_buffer: LookaheadBuffer,
    /// Per-band measured auto-makeup gain trackers.
    pub(super) measured_makeups: Vec<MeasuredMakeup>,
    /// Temporary frame buffer for lookahead processing.
    pub(super) lookahead_frame_tmp: Vec<f32>,

    // Internal flattened monitoring buffer
    pub(super) gain_reduction_flattened: Vec<f32>,
    pub(super) cache: RealTimeCache<MultibandCompressorData>,
    /// Sample-based counter for UI cache throttle (~50 ms at the current sample rate).
    pub(super) cache_update_counter: usize,
    /// Threshold: update UI cache after this many samples (~50 ms).
    pub(super) cache_update_threshold: usize,
    pub(super) cached_parameters: Vec<Parameter>,
    pub(super) initialized: bool,
}

impl MultibandCompressorPlugin {
    #[inline]
    fn envelope_coeff(time_ms: f32, sample_rate: u32) -> f32 {
        (-1.0 / (time_ms.max(0.01) * 0.001 * sample_rate.max(1) as f32)).exp()
    }

    fn make_band_smoothers(
        band: &BandCompressorParams,
        threshold_db: f32,
        ratio: f32,
        attack_ms: f32,
        release_ms: f32,
        knee_db: f32,
        sample_rate: u32,
    ) -> BandDynamicsSmoothers {
        const DYNAMICS_SMOOTH_MS: f32 = 20.0;
        BandDynamicsSmoothers {
            threshold: Smoother::new(
                band.threshold_db.unwrap_or(threshold_db),
                DYNAMICS_SMOOTH_MS,
                sample_rate,
            ),
            ratio: Smoother::new(band.ratio.unwrap_or(ratio), DYNAMICS_SMOOTH_MS, sample_rate),
            attack_coeff: Smoother::new(
                Self::envelope_coeff(band.attack_ms.unwrap_or(attack_ms), sample_rate),
                DYNAMICS_SMOOTH_MS,
                sample_rate,
            ),
            release_coeff: Smoother::new(
                Self::envelope_coeff(band.release_ms.unwrap_or(release_ms), sample_rate),
                DYNAMICS_SMOOTH_MS,
                sample_rate,
            ),
            knee: Smoother::new(
                band.knee_db.unwrap_or(knee_db),
                DYNAMICS_SMOOTH_MS,
                sample_rate,
            ),
            makeup_db: Smoother::new(band.makeup_gain_db, DYNAMICS_SMOOTH_MS, sample_rate),
            applied_tilt_db: 0.0,
        }
    }

    /// Validate a serialized configuration before allocating DSP state.
    ///
    /// `with_params` is intentionally retained as the infallible constructor used by
    /// in-process callers and tests. Factory boundaries should use this fallible
    /// constructor so malformed presets cannot reach the IIR coefficient builder.
    pub fn try_from_params(
        channels: usize,
        params: MultibandCompressorPluginParams,
        sample_rate: u32,
    ) -> Result<Self, String> {
        Self::validate_params(&params, sample_rate, channels)?;
        Ok(Self::with_params(channels, params))
    }

    fn validate_params(
        params: &MultibandCompressorPluginParams,
        sample_rate: u32,
        channels: usize,
    ) -> Result<(), String> {
        if channels == 0 {
            return Err("channels must be greater than zero".to_string());
        }
        if sample_rate == 0 {
            return Err("sample_rate must be greater than zero".to_string());
        }

        // The legacy `Compressor` bridge intentionally requests one band. The
        // multiband catalog still advertises two-to-five bands, but accepting
        // one here keeps that compatibility path validated rather than bypassed.
        let nb_min = 1usize;
        let nb_max = pk(MC, "num_bands").max_f64() as usize;
        if !(nb_min..=nb_max).contains(&params.num_bands) {
            return Err(format!(
                "num_bands must be between {nb_min} and {nb_max}, got {}",
                params.num_bands
            ));
        }

        fn check_range(name: &str, value: f32, min: f64, max: f64) -> Result<(), String> {
            if !value.is_finite() || value < min as f32 || value > max as f32 {
                return Err(format!(
                    "{name} must be finite and between {min} and {max}, got {value}"
                ));
            }
            Ok(())
        }

        let spec_range = |name: &str, value: f32, specs: &[sotf_host::param_specs::ParamSpec]| {
            let spec = pk(specs, name);
            check_range(name, value, spec.min_f64(), spec.max_f64())
        };

        spec_range("threshold", params.threshold_db, SC)?;
        spec_range("ratio", params.ratio, SC)?;
        spec_range("attack", params.attack_ms, SC)?;
        spec_range("release", params.release_ms, SC)?;
        spec_range("knee", params.knee_db, SC)?;
        spec_range("range_db", params.range_db, SC)?;
        spec_range("hold_ms", params.hold_ms, SC)?;
        spec_range("mix", params.mix, SC)?;
        spec_range("per_band_lookahead_ms", params.per_band_lookahead_ms, MC)?;
        spec_range("sidechain_tilt_db", params.sidechain_tilt_db, MC)?;
        spec_range("link_amount", params.link_amount, MC)?;

        if let Some(value) = params.lookahead_ms {
            spec_range("lookahead_ms", value, SC)?;
        }
        if let Some(value) = params.makeup_gain {
            spec_range("makeup_gain", value, SC)?;
        }
        if let Some(value) = params.sidechain_hpf_hz {
            spec_range("sidechain_hpf_hz", value, SC)?;
        }
        for (key, labels, value) in [
            (
                "sidechain_hpf_order",
                HPF_ORDERS,
                params.sidechain_hpf_order.as_deref(),
            ),
            (
                "detection_mode",
                DETECTION_MODES,
                params.detection_mode.as_deref(),
            ),
        ] {
            if let Some(value) = value
                && !labels.iter().any(|label| label.eq_ignore_ascii_case(value))
            {
                return Err(format!("{key} must be one of {labels:?}, got {value}"));
            }
        }
        // The remaining legacy single-band sidechain controls have no DSP
        // path: the struct fields survive so old presets still deserialize,
        // but any explicitly present value is rejected here — never silently
        // ignored — matching `set_parameter`, which rejects the same keys
        // with the same message. A preset claiming behavior that will not
        // happen must fail loudly instead of loading inaudible settings.
        for (key, present) in [
            (
                "program_dependent_release",
                params.program_dependent_release.is_some(),
            ),
            ("sidechain_external", params.sidechain_external.is_some()),
        ] {
            if present {
                return Err(format!(
                    "{key} is an unsupported legacy sidechain control; remove it instead of loading inaudible settings"
                ));
            }
        }

        let defaults = [200.0_f32, 2_000.0, 8_000.0, 12_000.0];
        let active_crossovers = params.num_bands - 1;
        let nyquist = sample_rate as f32 * 0.5;
        let mut previous = None;
        for (index, default) in defaults.iter().enumerate().take(active_crossovers) {
            let value = params
                .crossover_frequencies
                .get(index)
                .copied()
                .unwrap_or(*default);
            // Zero is the historical "use default" sentinel used by with_params.
            let value = if value == 0.0 { *default } else { value };
            let name = format!("crossover_freq_{}", index + 1);
            let spec = pk(MC, &name);
            check_range(&name, value, spec.min_f64(), spec.max_f64())?;
            if value >= nyquist {
                return Err(format!(
                    "{name} must be below Nyquist ({nyquist} Hz), got {value}"
                ));
            }
            if let Some(previous) = previous
                && value <= previous
            {
                return Err(format!(
                    "crossover frequencies must be strictly ascending: {name}={value} <= {previous}"
                ));
            }
            previous = Some(value);
        }

        for (band_index, band) in params.bands.iter().enumerate() {
            if let Some(value) = band.threshold_db {
                spec_range("threshold", value, MCB)?;
            }
            if let Some(value) = band.ratio {
                spec_range("ratio", value, MCB)?;
            }
            if let Some(value) = band.attack_ms {
                spec_range("attack", value, MCB)?;
            }
            if let Some(value) = band.release_ms {
                spec_range("release", value, MCB)?;
            }
            if let Some(value) = band.knee_db {
                spec_range("knee", value, MCB)?;
            }
            if let Some(value) = band.range_db {
                spec_range("range_db", value, MCB)?;
            }
            if let Some(value) = band.hold_ms {
                spec_range("hold_ms", value, MCB)?;
            }
            spec_range("makeup_gain", band.makeup_gain_db, MCB)
                .map_err(|error| format!("band {band_index} {error}"))?;
        }

        Ok(())
    }

    pub fn new(channels: usize) -> Self {
        Self::with_params(channels, Default::default())
    }
    pub fn with_params(channels: usize, params: MultibandCompressorPluginParams) -> Self {
        // One band is the explicit broadband Compressor mode. The multiband UI
        // still constrains its structural control to 2..=5.
        let nb = params
            .num_bands
            .clamp(1, pk(MC, "num_bands").max_f64() as usize);
        let sr = 44100;
        let default_xfs = [200.0f32, 2000.0, 8000.0, 12000.0];
        let mut xfs = params.crossover_frequencies.clone();
        for (i, &d) in default_xfs.iter().enumerate() {
            if xfs.get(i).is_none_or(|&v| v == 0.0) {
                if i < xfs.len() {
                    xfs[i] = d;
                } else {
                    xfs.push(d);
                }
            }
        }
        while xfs.len() < 4 {
            xfs.push(default_xfs[xfs.len()]);
        }
        let ratio = if params.ratio == 0.0 {
            4.0
        } else {
            params.ratio
        };
        let attack_ms = if params.attack_ms == 0.0 {
            5.0
        } else {
            params.attack_ms
        };
        let release_ms = if params.release_ms == 0.0 {
            50.0
        } else {
            params.release_ms
        };
        let mut bcomps = Vec::with_capacity(nb);
        for _ in 0..nb {
            bcomps.push(BandCompressor {
                envelope: vec![0.0; channels],
                hold_remaining: vec![0; channels],
                attack_coeff: 0.0,
                release_coeff: 0.0,
            });
        }

        let mut band_params = params.bands;
        while band_params.len() < nb {
            band_params.push(BandCompressorParams::default());
        }

        // Apply single-band aliases to band 0
        if let Some(mg) = params.makeup_gain
            && let Some(bp) = band_params.first_mut()
        {
            bp.makeup_gain_db = mg;
        }
        if let Some(am) = params.auto_makeup
            && let Some(bp) = band_params.first_mut()
        {
            bp.auto_makeup = am;
        }
        if let Some(mam) = params.measured_auto_makeup
            && let Some(bp) = band_params.first_mut()
        {
            bp.measured_auto_makeup = mam;
        }

        // lookahead_ms alias overrides per_band_lookahead_ms
        let la_ms_raw = params.lookahead_ms.unwrap_or(params.per_band_lookahead_ms);
        let la_ms = la_ms_raw.clamp(0.0, 20.0);
        let lookahead_buffers = (0..nb)
            .map(|_| {
                if la_ms > 0.0 {
                    LookaheadBuffer::from_ms(la_ms, sr, channels)
                } else {
                    LookaheadBuffer::new(1, channels)
                }
            })
            .collect();
        let measured_makeups = (0..nb).map(|_| MeasuredMakeup::new(1000.0, sr)).collect();
        let band_smoothers = band_params
            .iter()
            .map(|band| {
                Self::make_band_smoothers(
                    band,
                    params.threshold_db,
                    ratio,
                    attack_ms,
                    release_ms,
                    params.knee_db,
                    sr,
                )
            })
            .collect();

        let sidechain_hpf_hz = params
            .sidechain_hpf_hz
            .unwrap_or_else(|| pk(SC, "sidechain_hpf_hz").default_f64() as f32);
        let sidechain_hpf_enabled = params
            .sidechain_hpf_enabled
            .unwrap_or_else(|| pk(SC, "sidechain_hpf_enabled").default_bool());
        // Infallible constructor: unknown labels normalize to the default
        // index (the fallible constructor rejects them instead).
        let sidechain_hpf_order_index = params
            .sidechain_hpf_order
            .as_deref()
            .and_then(|label| {
                HPF_ORDERS
                    .iter()
                    .position(|known| known.eq_ignore_ascii_case(label))
            })
            .unwrap_or(0);
        let detection_mode_index = params
            .detection_mode
            .as_deref()
            .and_then(|label| {
                DETECTION_MODES
                    .iter()
                    .position(|known| known.eq_ignore_ascii_case(label))
            })
            .unwrap_or(0);
        let detector_mode = if Self::detection_is_rms_index(detection_mode_index) {
            DetectionMode::Rms {
                window_ms: Self::RMS_WINDOW_MS,
            }
        } else {
            DetectionMode::Peak
        };
        let mut p = Self {
            channels,
            sample_rate: sr,
            has_input: false,
            drain_remaining: None,
            num_bands: nb,
            _crossover_preset: params.crossover_preset,
            crossover_frequencies: xfs.clone(),
            threshold_db: params.threshold_db,
            ratio,
            attack_ms,
            release_ms,
            knee_db: params.knee_db,
            range_db: params.range_db,
            hold_ms: params.hold_ms,
            link_channels: params.link_channels,
            mix: params.mix,
            per_band_lookahead_ms: la_ms,
            ms_mode: params.ms_mode,
            sidechain_tilt_db: params.sidechain_tilt_db,
            link_amount: params.link_amount.clamp(0.0, 1.0),
            sidechain_hpf_hz,
            sidechain_hpf_enabled,
            sidechain_hpf_order_index,
            detection_mode_index,
            program_dependent_release: params
                .program_dependent_release
                .unwrap_or_else(|| pk(SC, "program_dependent_release").default_bool()),
            sidechain_external: params
                .sidechain_external
                .unwrap_or_else(|| pk(SC, "sidechain_external").default_bool()),
            sidechain_tilt_biquads: Vec::new(),
            sidechain_hpf_biquads: (0..nb)
                .map(|_| {
                    (0..channels)
                        .map(|_| Self::make_hpf_bank(sidechain_hpf_hz, sidechain_hpf_order_index, sr))
                        .collect()
                })
                .collect(),
            level_detectors: (0..nb)
                .map(|_| {
                    (0..channels)
                        .map(|_| Self::make_detector(detector_mode, sr))
                        .collect()
                })
                .collect(),
            detect_scratch: vec![0.0; channels],
            band_params,
            crossover_points: Vec::new(),
            band_compressors: bcomps,
            band_buffers: Vec::new(),
            band_levels_db: vec![-120.0; nb],
            dry_buffer: Vec::new(),
            threshold_smoother: Smoother::new(params.threshold_db, 20.0, sr),
            mix_smoother: Smoother::new(params.mix, 20.0, sr),
            link_smoother: Smoother::new(params.link_amount.clamp(0.0, 1.0), 20.0, sr),
            tilt_smoother: Smoother::new(params.sidechain_tilt_db, 20.0, sr),
            band_smoothers,
            automation_values: Vec::new(),
            xover_smoothers: xfs.iter().map(|&f| LogSmoother::new(f, 50.0, sr)).collect(),
            lookahead_buffers,
            dry_lookahead_buffer: if la_ms > 0.0 {
                LookaheadBuffer::from_ms(la_ms, sr, channels)
            } else {
                LookaheadBuffer::new(1, channels)
            },
            measured_makeups,
            lookahead_frame_tmp: vec![0.0; channels],
            gain_reduction_flattened: vec![0.0; nb * channels],
            cache: RealTimeCache::new(MultibandCompressorData::new(nb, channels)),
            cache_update_counter: 0,
            // Default ~50 ms @ 44100 Hz; updated in initialize() when sample rate is known.
            cache_update_threshold: (44100 * 50 / 1000),
            cached_parameters: Vec::new(),
            initialized: false,
        };
        p.build_crossovers();
        p.update_coefficients();
        p.rebuild_cached_parameters();
        p
    }

    /// Get the f64 value of parameter at GLOBAL_PARAMS index.
    /// Order must match params::GLOBAL_PARAMS exactly.
    pub(super) fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.num_bands as f64),                       // num_bands
            1 => Some(self._crossover_preset as f64),               // crossover_preset
            2 => Some(self.crossover_frequencies[0] as f64),        // crossover_freq_1
            3 => Some(self.crossover_frequencies[1] as f64),        // crossover_freq_2
            4 => Some(self.crossover_frequencies[2] as f64),        // crossover_freq_3
            5 => Some(self.crossover_frequencies[3] as f64),        // crossover_freq_4
            6 => Some(self.threshold_db as f64),                    // threshold
            7 => Some(self.ratio as f64),                           // ratio
            8 => Some(self.attack_ms as f64),                       // attack
            9 => Some(self.release_ms as f64),                      // release
            10 => Some(self.knee_db as f64),                        // knee
            11 => Some(self.mix as f64),                            // mix
            12 => Some(if self.link_channels { 1.0 } else { 0.0 }), // link_channels
            13 => Some(self.per_band_lookahead_ms as f64),          // per_band_lookahead_ms
            14 => Some(if self.ms_mode { 1.0 } else { 0.0 }),       // ms_mode
            15 => Some(self.sidechain_tilt_db as f64),              // sidechain_tilt_db
            16 => Some(self.link_amount as f64),                    // link_amount
            17 => Some(self.range_db as f64),
            18 => Some(self.hold_ms as f64),
            19 => Some(self.sidechain_hpf_hz as f64), // sidechain_hpf_hz
            20 => Some(self.sidechain_hpf_order_index as f64), // sidechain_hpf_order
            21 => Some(self.detection_mode_index as f64), // detection_mode
            22 => Some(if self.sidechain_hpf_enabled { 1.0 } else { 0.0 }), // sidechain_hpf_enabled
            _ => None,
        }
    }

    /// Set the f64 value of parameter at GLOBAL_PARAMS index.
    /// Order must match params::GLOBAL_PARAMS exactly.
    pub(super) fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.num_bands = MC[0].clamp_f64(value.round()) as usize, // num_bands (round, not truncate)
            1 => self._crossover_preset = MC[1].clamp_f64(value) as i32,   // crossover_preset
            2 => self.crossover_frequencies[0] = MC[2].clamp_f64(value) as f32, // crossover_freq_1
            3 => self.crossover_frequencies[1] = MC[3].clamp_f64(value) as f32, // crossover_freq_2
            4 => self.crossover_frequencies[2] = MC[4].clamp_f64(value) as f32, // crossover_freq_3
            5 => self.crossover_frequencies[3] = MC[5].clamp_f64(value) as f32, // crossover_freq_4
            6 => self.threshold_db = MC[6].clamp_f64(value) as f32,        // threshold
            7 => self.ratio = MC[7].clamp_f64(value) as f32,               // ratio
            8 => self.attack_ms = MC[8].clamp_f64(value) as f32,           // attack
            9 => self.release_ms = MC[9].clamp_f64(value) as f32,          // release
            10 => self.knee_db = MC[10].clamp_f64(value) as f32,           // knee
            11 => self.mix = MC[11].clamp_f64(value) as f32,               // mix
            12 => self.link_channels = MC[12].clamp_f64(value) > 0.5,      // link_channels
            13 => self.per_band_lookahead_ms = (value as f32).clamp(0.0, 20.0), // per_band_lookahead_ms
            14 => self.ms_mode = MC[14].clamp_f64(value) > 0.5,                 // ms_mode
            15 => self.sidechain_tilt_db = MC[15].clamp_f64(value) as f32,      // sidechain_tilt_db
            16 => self.link_amount = MC[16].clamp_f64(value) as f32,            // link_amount
            17 => self.range_db = MC[17].clamp_f64(value) as f32,
            18 => self.hold_ms = MC[18].clamp_f64(value) as f32,
            19 => self.sidechain_hpf_hz = MC[19].clamp_f64(value) as f32, // sidechain_hpf_hz
            20 => self.sidechain_hpf_order_index = MC[20].clamp_f64(value) as usize, // sidechain_hpf_order
            21 => self.detection_mode_index = MC[21].clamp_f64(value) as usize, // detection_mode
            22 => self.sidechain_hpf_enabled = MC[22].clamp_f64(value) > 0.5, // sidechain_hpf_enabled
            _ => {}
        }
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let mut params = param_bridge::build_parameters(MC, |i| self.param_value(i));

        // Single-band aliases (not in GLOBAL_PARAMS, but needed for Compressor PluginSettings)
        let bp0 = self.band_params.first();
        params.push(
            Parameter::new_float(
                "makeup_gain",
                "Makeup Gain",
                bp0.map_or(0.0, |bp| bp.makeup_gain_db),
                -24.0,
                24.0,
            )
            .with_group("Output"),
        );
        params.push(
            Parameter::new_bool(
                "auto_makeup",
                "Auto Makeup",
                bp0.is_some_and(|bp| bp.auto_makeup),
            )
            .with_group("Output"),
        );
        params.push(
            Parameter::new_bool(
                "measured_auto_makeup",
                "Measured Auto Makeup",
                bp0.is_some_and(|bp| bp.measured_auto_makeup),
            )
            .with_group("Output"),
        );
        // The remaining unsupported legacy controls (program-dependent
        // release, external sidechain) are deliberately absent from the
        // runtime schema and rejected when explicitly serialized.
        params.push(
            Parameter::new_float(
                "lookahead_ms",
                "Lookahead",
                self.per_band_lookahead_ms,
                0.0,
                20.0,
            )
            .with_group("Timing")
            .with_update_mode(UpdateMode::Structural),
        );

        // Per-band dynamics (not covered by GLOBAL_PARAMS)
        for i in 0..self.num_bands {
            let group = format!("Band {}", i + 1);
            let bp = &self.band_params[i];

            params.push(
                Parameter::new_float(
                    &format!("band_{}_threshold", i),
                    "Threshold",
                    bp.threshold_db.unwrap_or(self.threshold_db),
                    pk(MCB, "threshold").min_f64() as f32,
                    pk(MCB, "threshold").max_f64() as f32,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_ratio", i),
                    "Ratio",
                    bp.ratio.unwrap_or(self.ratio),
                    pk(MCB, "ratio").min_f64() as f32,
                    pk(MCB, "ratio").max_f64() as f32,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_attack", i),
                    "Attack",
                    bp.attack_ms.unwrap_or(self.attack_ms),
                    pk(MCB, "attack").min_f64() as f32,
                    pk(MCB, "attack").max_f64() as f32,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_release", i),
                    "Release",
                    bp.release_ms.unwrap_or(self.release_ms),
                    pk(MCB, "release").min_f64() as f32,
                    pk(MCB, "release").max_f64() as f32,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_makeup", i),
                    "Makeup (dB)",
                    bp.makeup_gain_db,
                    -24.0,
                    24.0,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_bool(
                    &format!("band_{}_auto_makeup", i),
                    "Auto Makeup",
                    bp.auto_makeup,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_bool(
                    &format!("band_{}_measured_auto_makeup", i),
                    "Measured Auto Makeup",
                    bp.measured_auto_makeup,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_knee", i),
                    "Knee (dB)",
                    bp.knee_db.unwrap_or(self.knee_db),
                    pk(MCB, "knee").min_f64() as f32,
                    pk(MCB, "knee").max_f64() as f32,
                )
                .with_group(&group),
            );
            params.push(
                Parameter::new_bool(&format!("band_{}_active", i), "Active", bp.active)
                    .with_group(&group),
            );
            params.push(
                Parameter::new_bool(&format!("band_{}_solo", i), "Solo", bp.solo)
                    .with_group(&group),
            );
            params.push(
                Parameter::new_bool(&format!("band_{}_bypass", i), "Bypass", bp.bypass)
                    .with_group(&group),
            );
            for (key, value) in [
                ("range_db", bp.range_db.unwrap_or(self.range_db)),
                ("hold_ms", bp.hold_ms.unwrap_or(self.hold_ms)),
            ] {
                let spec = pk(MCB, key);
                params.push(
                    Parameter::new_float(
                        &format!("band_{i}_{key}"),
                        spec.name,
                        value,
                        spec.min_f64() as f32,
                        spec.max_f64() as f32,
                    )
                    .with_description(spec.doc)
                    .with_group(&group),
                );
            }
        }

        if self.num_bands == 1 {
            const BROADBAND_KEYS: &[&str] = &[
                "threshold",
                "ratio",
                "attack",
                "release",
                "knee",
                "mix",
                "link_channels",
                "link_amount",
                "lookahead_ms",
                "makeup_gain",
                "auto_makeup",
                "measured_auto_makeup",
                "range_db",
                "hold_ms",
                "sidechain_hpf_hz",
                "sidechain_hpf_order",
                "sidechain_hpf_enabled",
                "detection_mode",
            ];
            params.retain(|parameter| BROADBAND_KEYS.contains(&parameter.id.as_str()));
        }
        self.cached_parameters = params;
    }

    fn refresh_schema_before_initialization(&mut self) {
        // Parameter metadata is structural. Runtime values are served by
        // get_parameter/current_values, so realtime automation must not rebuild
        // and allocate a complete schema on every write.
        if !self.initialized {
            self.rebuild_cached_parameters();
        }
    }
    pub fn from_params(channels: usize, params: MultibandCompressorPluginParams) -> Self {
        Self::with_params(channels, params)
    }

    pub(super) fn rebuild_sidechain_tilt(&mut self) {
        let tilt = self.tilt_smoother.current();
        let dimensions_match = self.sidechain_tilt_biquads.len() == self.num_bands
            && self
                .sidechain_tilt_biquads
                .iter()
                .all(|band| band.len() == self.channels);
        if !dimensions_match {
            self.sidechain_tilt_biquads = (0..self.num_bands)
                .map(|_| {
                    (0..self.channels)
                        .map(|_| Self::new_tilt_pair(tilt, self.sample_rate))
                        .collect()
                })
                .collect();
        } else {
            self.update_sidechain_tilt_coefficients(tilt);
        }
    }

    fn new_tilt_pair(tilt_db: f32, sample_rate: u32) -> (Biquad, Biquad) {
        let half_tilt = tilt_db as f64 * 0.5;
        (
            Biquad::new(
                BiquadFilterType::Lowshelf,
                1000.0,
                sample_rate.max(1) as f64,
                0.707,
                -half_tilt,
            ),
            Biquad::new(
                BiquadFilterType::Highshelf,
                1000.0,
                sample_rate.max(1) as f64,
                0.707,
                half_tilt,
            ),
        )
    }

    fn update_sidechain_tilt_coefficients(&mut self, tilt_db: f32) {
        let half_tilt = tilt_db as f64 * 0.5;
        for band in &mut self.sidechain_tilt_biquads {
            for (low, high) in band {
                low.update_params(
                    BiquadFilterType::Lowshelf,
                    1000.0,
                    self.sample_rate.max(1) as f64,
                    0.707,
                    -half_tilt,
                );
                high.update_params(
                    BiquadFilterType::Highshelf,
                    1000.0,
                    self.sample_rate.max(1) as f64,
                    0.707,
                    half_tilt,
                );
            }
        }
    }

    pub(super) fn build_crossovers(&mut self) {
        self.crossover_points.clear();
        for i in 0..(self.num_bands - 1) {
            let f = self.xover_smoothers[i].target();
            self.crossover_points.push(Lr4Crossover::new(
                f,
                self.sample_rate as f32,
                self.channels,
            ));
        }
    }

    pub(super) fn update_coefficients(&mut self) {
        for (i, b) in self.band_compressors.iter_mut().enumerate() {
            let a = self
                .band_params
                .get(i)
                .and_then(|p| p.attack_ms)
                .unwrap_or(self.attack_ms);
            let r = self
                .band_params
                .get(i)
                .and_then(|p| p.release_ms)
                .unwrap_or(self.release_ms);
            b.attack_coeff = (-1.0 / (a * 0.001 * self.sample_rate as f32)).exp();
            b.release_coeff = (-1.0 / (r * 0.001 * self.sample_rate as f32)).exp();
        }
    }

    pub(super) fn calculate_gain_reduction(idb: f32, th: f32, ratio: f32, knee: f32) -> f32 {
        let slope = 1.0 - 1.0 / ratio.max(1.0);
        if knee < 0.1 {
            if idb <= th { 0.0 } else { (idb - th) * slope }
        } else if idb < th - knee / 2.0 {
            0.0
        } else if idb > th + knee / 2.0 {
            (idb - th) * slope
        } else {
            let ov = idb - th + knee / 2.0;
            let kf = ov / knee;
            kf * kf * (knee / 2.0) * slope
        }
    }

    /// Sliding RMS window matching the Gate/Expander detector contract.
    const RMS_WINDOW_MS: f32 = 10.0;

    /// Second-order Butterworth Q for the HPF prototype sections.
    const BUTTERWORTH_Q_ORDER2: f64 = std::f64::consts::FRAC_1_SQRT_2;

    /// Fourth-order Butterworth Q pair (low-Q section first).
    const BUTTERWORTH_Q_ORDER4: [f64; 2] = [0.541196100146197, 1.3065629648763766];

    /// Maximum preallocated HPF sections per band and channel (4th order).
    const MAX_HPF_SECTIONS: usize = 2;

    fn detection_is_rms_index(index: usize) -> bool {
        DETECTION_MODES
            .get(index)
            .is_some_and(|label| label.eq_ignore_ascii_case("rms"))
    }

    fn hpf_active_sections_index(enabled: bool, hz: f32, order_index: usize) -> usize {
        if !enabled || hz <= 0.0 {
            0
        } else if HPF_ORDERS
            .get(order_index)
            .is_some_and(|label| label.eq_ignore_ascii_case("4th"))
        {
            2
        } else {
            1
        }
    }

    fn hpf_section_q(section: usize, order_is_4th: bool) -> f64 {
        if order_is_4th {
            Self::BUTTERWORTH_Q_ORDER4[section.min(1)]
        } else {
            Self::BUTTERWORTH_Q_ORDER2
        }
    }

    fn make_detector(mode: DetectionMode, sample_rate: u32) -> LevelDetector {
        // Always size the RMS energy buffer first so a later Peak->RMS switch
        // only reuses storage instead of allocating on the audio path.
        let mut detector = LevelDetector::new(
            DetectionMode::Rms {
                window_ms: Self::RMS_WINDOW_MS,
            },
            sample_rate.max(1),
        );
        detector.set_mode(mode);
        detector
    }

    fn make_hpf_bank(hz: f32, order_index: usize, sample_rate: u32) -> Vec<Biquad> {
        let order_is_4th = HPF_ORDERS
            .get(order_index)
            .is_some_and(|label| label.eq_ignore_ascii_case("4th"));
        // Coefficients must stay valid while disabled; sections are skipped
        // when the active prefix is empty, so clamp instead of building at 0 Hz.
        let fc = hz.max(1.0).min(sample_rate.max(1) as f32 * 0.45).max(1.0) as f64;
        (0..Self::MAX_HPF_SECTIONS)
            .map(|section| {
                Biquad::new(
                    BiquadFilterType::Highpass,
                    fc,
                    sample_rate.max(1) as f64,
                    Self::hpf_section_q(section, order_is_4th),
                    0.0,
                )
            })
            .collect()
    }

    /// Refresh HPF coefficients in place; reallocates only when the
    /// band/channel topology changed (never on the initialized audio path,
    /// since band count is structural after initialization).
    pub(super) fn rebuild_sidechain_hpf(&mut self) {
        let dimensions_match = self.sidechain_hpf_biquads.len() == self.num_bands
            && self.sidechain_hpf_biquads.iter().all(|band| {
                band.len() == self.channels
                    && band.iter().all(|sections| sections.len() == Self::MAX_HPF_SECTIONS)
            });
        if !dimensions_match {
            let hz = self.sidechain_hpf_hz;
            let order_index = self.sidechain_hpf_order_index;
            let sample_rate = self.sample_rate;
            let bands = self.num_bands;
            let channels = self.channels;
            self.sidechain_hpf_biquads = (0..bands)
                .map(|_| {
                    (0..channels)
                        .map(|_| Self::make_hpf_bank(hz, order_index, sample_rate))
                        .collect()
                })
                .collect();
        } else {
            self.update_sidechain_hpf_coefficients();
        }
    }

    fn update_sidechain_hpf_coefficients(&mut self) {
        let order_is_4th = HPF_ORDERS
            .get(self.sidechain_hpf_order_index)
            .is_some_and(|label| label.eq_ignore_ascii_case("4th"));
        let fc = self.sidechain_hpf_hz.max(1.0).min(self.sample_rate.max(1) as f32 * 0.45).max(1.0) as f64;
        for band in &mut self.sidechain_hpf_biquads {
            for sections in band {
                for (index, section) in sections.iter_mut().enumerate() {
                    section.update_params(
                        BiquadFilterType::Highpass,
                        fc,
                        self.sample_rate.max(1) as f64,
                        Self::hpf_section_q(index, order_is_4th),
                        0.0,
                    );
                }
            }
        }
    }

    fn reset_sidechain_hpf_state(&mut self) {
        for band in &mut self.sidechain_hpf_biquads {
            for sections in band {
                for section in sections {
                    section.reset();
                }
            }
        }
    }

    /// Ensure detector topology matches bands/channels, then apply the current
    /// mode in place. Reallocates only on topology change (pre-init band edits).
    pub(super) fn sync_detector_topology(&mut self) {
        let mode = if Self::detection_is_rms_index(self.detection_mode_index) {
            DetectionMode::Rms {
                window_ms: Self::RMS_WINDOW_MS,
            }
        } else {
            DetectionMode::Peak
        };
        let dimensions_match = self.level_detectors.len() == self.num_bands
            && self
                .level_detectors
                .iter()
                .all(|band| band.len() == self.channels);
        if !dimensions_match {
            let sample_rate = self.sample_rate;
            let bands = self.num_bands;
            let channels = self.channels;
            self.level_detectors = (0..bands)
                .map(|_| {
                    (0..channels)
                        .map(|_| Self::make_detector(mode, sample_rate))
                        .collect()
                })
                .collect();
        } else {
            for band in &mut self.level_detectors {
                for detector in band {
                    detector.set_mode(mode);
                }
            }
        }
    }
}

impl MultibandCompressorPlugin {
    /// Backward-compatible parameter list accessor.
    pub fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    pub fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return if self.get_parameter(&id).as_ref() == Some(&value) {
                Ok(())
            } else {
                Err("reset the multiband compressor before changing controls after drain".into())
            };
        }
        let name = id.as_str();
        if matches!(name, "range_db" | "hold_ms") {
            let v = value
                .as_float()
                .ok_or_else(|| format!("{name} must be a float"))?;
            let spec = pk(MC, name);
            if !v.is_finite() || v < spec.min_f64() as f32 || v > spec.max_f64() as f32 {
                return Err(format!("{name} is outside its finite parameter range"));
            }
        }
        // Band count determines the size and topology of all per-band state. Reject live
        // changes before the bridge invokes the mutating setter so failure is transactional.
        if self.initialized
            && matches!(
                id.as_str(),
                "num_bands" | "per_band_lookahead_ms" | "lookahead_ms"
            )
        {
            return Err(format!(
                "{} is structural; rebuild the plugin so host latency metadata stays valid",
                id.as_str()
            ));
        }

        // Try global params first via param_bridge
        let is_runtime_alias = matches!(
            name,
            "makeup_gain" | "auto_makeup" | "measured_auto_makeup" | "lookahead_ms"
        );
        if !name.starts_with("band_")
            && !is_runtime_alias
            && let Ok(idx) =
                param_bridge::set_parameter(MC, &id, &value, |i, v| self.set_param_value(i, v))
        {
            // Side effects for specific global params
            match idx {
                0 => {
                    // num_bands changed
                    let nb = self.num_bands;
                    self.build_crossovers();
                    while self.band_params.len() < nb {
                        self.band_params.push(BandCompressorParams::default());
                    }
                    while self.band_compressors.len() < nb {
                        self.band_compressors.push(BandCompressor {
                            envelope: vec![0.0; self.channels],
                            hold_remaining: vec![0; self.channels],
                            attack_coeff: 0.0,
                            release_coeff: 0.0,
                        });
                    }
                    while self.band_smoothers.len() < nb {
                        let band = &self.band_params[self.band_smoothers.len()];
                        self.band_smoothers.push(Self::make_band_smoothers(
                            band,
                            self.threshold_db,
                            self.ratio,
                            self.attack_ms,
                            self.release_ms,
                            self.knee_db,
                            self.sample_rate,
                        ));
                    }
                    while self.lookahead_buffers.len() < nb {
                        self.lookahead_buffers
                            .push(if self.per_band_lookahead_ms > 0.0 {
                                LookaheadBuffer::from_ms(
                                    self.per_band_lookahead_ms,
                                    self.sample_rate,
                                    self.channels,
                                )
                            } else {
                                LookaheadBuffer::new(1, self.channels)
                            });
                    }
                    while self.measured_makeups.len() < nb {
                        self.measured_makeups
                            .push(MeasuredMakeup::new(1000.0, self.sample_rate));
                    }
                    self.band_levels_db.resize(nb, -120.0);
                    self.gain_reduction_flattened
                        .resize(nb * self.channels, 0.0);
                    self.update_coefficients();
                    self.rebuild_sidechain_tilt();
                    self.rebuild_sidechain_hpf();
                    self.sync_detector_topology();
                }
                2..=5 => {
                    // crossover_freq_1..4 changed
                    let xover_idx = idx - 2;
                    if xover_idx < self.xover_smoothers.len() {
                        self.xover_smoothers[xover_idx]
                            .set_target(self.crossover_frequencies[xover_idx]);
                    }
                }
                6 => {
                    // threshold changed
                    if self.cache_update_counter == 0
                        && self.band_levels_db.iter().all(|level| *level <= -119.0)
                    {
                        self.threshold_smoother.reset(self.threshold_db);
                    } else {
                        self.threshold_smoother.set_target(self.threshold_db);
                    }
                    for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
                        if band.threshold_db.is_none() {
                            smoother.threshold.set_target(self.threshold_db);
                        }
                    }
                }
                7 => {
                    for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
                        if band.ratio.is_none() {
                            smoother.ratio.set_target(self.ratio);
                        }
                    }
                }
                8 => {
                    let target = Self::envelope_coeff(self.attack_ms, self.sample_rate);
                    for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
                        if band.attack_ms.is_none() {
                            smoother.attack_coeff.set_target(target);
                        }
                    }
                }
                9 => {
                    let target = Self::envelope_coeff(self.release_ms, self.sample_rate);
                    for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
                        if band.release_ms.is_none() {
                            smoother.release_coeff.set_target(target);
                        }
                    }
                }
                10 => {
                    for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
                        if band.knee_db.is_none() {
                            smoother.knee.set_target(self.knee_db);
                        }
                    }
                }
                11 => {
                    // mix changed
                    self.mix_smoother.set_target(self.mix);
                }
                13 => {
                    // per_band_lookahead_ms changed
                    for buf in &mut self.lookahead_buffers {
                        if self.per_band_lookahead_ms > 0.0 {
                            buf.set_delay_ms(self.per_band_lookahead_ms, self.sample_rate);
                        }
                    }
                    if self.per_band_lookahead_ms > 0.0 {
                        self.dry_lookahead_buffer
                            .set_delay_ms(self.per_band_lookahead_ms, self.sample_rate);
                    }
                }
                15 => {
                    // sidechain_tilt_db changed
                    self.tilt_smoother.set_target(self.sidechain_tilt_db);
                }
                12 => {
                    // Legacy boolean migration into the canonical continuous control.
                    self.link_amount = if self.link_channels { 1.0 } else { 0.0 };
                    self.link_smoother.set_target(self.link_amount);
                }
                16 => {
                    self.link_smoother.set_target(self.link_amount);
                }
                19 | 20 | 22 => {
                    // Sidechain HPF frequency/order/enable: refresh
                    // coefficients in place and reset filter state.
                    // Structural-contract controls; the change may click but
                    // never allocates.
                    self.update_sidechain_hpf_coefficients();
                    self.reset_sidechain_hpf_state();
                }
                21 => {
                    // Detection mode: Peak uses abs() directly, RMS uses the
                    // preallocated sliding window. Buffers stay allocated so
                    // the switch never allocates on the audio path.
                    let mode = if Self::detection_is_rms_index(self.detection_mode_index) {
                        DetectionMode::Rms {
                            window_ms: Self::RMS_WINDOW_MS,
                        }
                    } else {
                        DetectionMode::Peak
                    };
                    for band in &mut self.level_detectors {
                        for detector in band {
                            detector.set_mode(mode);
                        }
                    }
                }
                _ => {}
            }
            self.refresh_schema_before_initialization();
            return Ok(());
        }

        // Single-band aliases: map unprefixed names to band_params[0] or stub fields
        match name {
            "makeup_gain" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "makeup_gain must be a float".to_string())?;
                if let Some(bp) = self.band_params.first_mut() {
                    bp.makeup_gain_db = v;
                }
                if let Some(smoother) = self.band_smoothers.first_mut() {
                    smoother.makeup_db.set_target(v);
                }
                self.refresh_schema_before_initialization();
                return Ok(());
            }
            "auto_makeup" => {
                let v = value
                    .as_bool()
                    .ok_or_else(|| "auto_makeup must be a boolean".to_string())?;
                if let Some(bp) = self.band_params.first_mut() {
                    bp.auto_makeup = v;
                }
                self.refresh_schema_before_initialization();
                return Ok(());
            }
            "measured_auto_makeup" => {
                let v = value
                    .as_bool()
                    .ok_or_else(|| "measured_auto_makeup must be a boolean".to_string())?;
                if let Some(bp) = self.band_params.first_mut() {
                    bp.measured_auto_makeup = v;
                }
                self.refresh_schema_before_initialization();
                return Ok(());
            }
            // The remaining legacy single-band sidechain controls have no DSP
            // implementation in the multiband engine and are absent from
            // parameters(). Reject explicitly (never silently ignore) so old
            // presets fail loudly instead of loading inaudible settings.
            // Mirrors the `validate_params` rejection at construction.
            "program_dependent_release" | "sidechain_external" => {
                return Err(format!(
                    "{name} is an unsupported legacy sidechain control; remove it instead of loading inaudible settings"
                ));
            }
            "lookahead_ms" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "lookahead_ms must be a float".to_string())?;
                self.per_band_lookahead_ms = v.clamp(0.0, 20.0);
                for buf in &mut self.lookahead_buffers {
                    if self.per_band_lookahead_ms > 0.0 {
                        buf.set_delay_ms(self.per_band_lookahead_ms, self.sample_rate);
                    }
                }
                if self.per_band_lookahead_ms > 0.0 {
                    self.dry_lookahead_buffer
                        .set_delay_ms(self.per_band_lookahead_ms, self.sample_rate);
                }
                self.refresh_schema_before_initialization();
                return Ok(());
            }
            _ => {}
        }

        // Fall through to band-level param handling
        if let Some(rest) = name.strip_prefix("band_") {
            if let Some((index, field)) = rest.split_once('_') {
                let b_idx = index
                    .parse::<usize>()
                    .map_err(|e| format!("Invalid band index: {}", e))?;
                if b_idx < self.num_bands {
                    let bp = &mut self.band_params[b_idx];
                    let smoothers = &mut self.band_smoothers[b_idx];
                    match field {
                        "range_db" | "hold_ms" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{name} must be a float"))?;
                            let spec = pk(MCB, field);
                            if !v.is_finite()
                                || v < spec.min_f64() as f32
                                || v > spec.max_f64() as f32
                            {
                                return Err(format!(
                                    "{name} is outside its finite parameter range"
                                ));
                            }
                            if field == "range_db" {
                                bp.range_db = Some(v);
                            } else {
                                bp.hold_ms = Some(v);
                            }
                        }
                        "threshold" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.threshold_db = Some(v);
                                smoothers.threshold.set_target(v);
                            }
                        }
                        "ratio" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.ratio = Some(v);
                                smoothers.ratio.set_target(v);
                            }
                        }
                        "attack" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.attack_ms = Some(v);
                                smoothers
                                    .attack_coeff
                                    .set_target(Self::envelope_coeff(v, self.sample_rate));
                            }
                        }
                        "release" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.release_ms = Some(v);
                                smoothers
                                    .release_coeff
                                    .set_target(Self::envelope_coeff(v, self.sample_rate));
                            }
                        }
                        "makeup" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.makeup_gain_db = v;
                                smoothers.makeup_db.set_target(v);
                            }
                        }
                        "auto_makeup" => {
                            bp.auto_makeup = value
                                .as_bool()
                                .ok_or_else(|| format!("{} must be a boolean", name))?
                        }
                        "measured_auto_makeup" => {
                            bp.measured_auto_makeup = value
                                .as_bool()
                                .ok_or_else(|| format!("{} must be a boolean", name))?
                        }
                        "active" => {
                            bp.active = value
                                .as_bool()
                                .ok_or_else(|| format!("{} must be a boolean", name))?
                        }
                        "solo" => {
                            bp.solo = value
                                .as_bool()
                                .ok_or_else(|| format!("{} must be a boolean", name))?
                        }
                        "bypass" => {
                            bp.bypass = value
                                .as_bool()
                                .ok_or_else(|| format!("{} must be a boolean", name))?
                        }
                        "knee" => {
                            let v = value
                                .as_float()
                                .ok_or_else(|| format!("{} must be a float", name))?;
                            if v.is_finite() {
                                bp.knee_db = Some(v);
                                smoothers.knee.set_target(v);
                            }
                        }
                        _ => return Err(format!("Unknown band field: {}", field)),
                    }
                } else {
                    return Err(format!("Band index {} out of range", b_idx));
                }
            }
        } else {
            return Err(format!("Unknown parameter: {}", id));
        }
        self.refresh_schema_before_initialization();
        Ok(())
    }

    pub fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        // Try global params first
        if let Some(v) = param_bridge::get_parameter(MC, id, |i| self.param_value(i)) {
            return Some(v);
        }
        // Single-band aliases: map unprefixed names to band_params[0] or stub fields
        let name = id.as_str();
        match name {
            "makeup_gain" => {
                return Some(ParameterValue::Float(
                    self.band_params.first().map_or(0.0, |bp| bp.makeup_gain_db),
                ));
            }
            "auto_makeup" => {
                return Some(ParameterValue::Bool(
                    self.band_params.first().is_some_and(|bp| bp.auto_makeup),
                ));
            }
            "measured_auto_makeup" => {
                return Some(ParameterValue::Bool(
                    self.band_params
                        .first()
                        .is_some_and(|bp| bp.measured_auto_makeup),
                ));
            }
            // The remaining legacy single-band sidechain controls
            // (program_dependent_release, sidechain_external) have no DSP
            // implementation and are absent from parameters(). Like any other
            // unknown ID, get_parameter returns None for them; set_parameter
            // rejects them with a message.
            "lookahead_ms" => {
                return Some(ParameterValue::Float(self.per_band_lookahead_ms));
            }
            _ => {}
        }
        // Fall through to band-level params
        if let Some(rest) = name.strip_prefix("band_") {
            if let Some((index, field)) = rest.split_once('_') {
                let b_idx = index.parse::<usize>().unwrap_or(0);
                if b_idx < self.num_bands {
                    let bp = &self.band_params[b_idx];
                    match field {
                        "range_db" => {
                            Some(ParameterValue::Float(bp.range_db.unwrap_or(self.range_db)))
                        }
                        "hold_ms" => {
                            Some(ParameterValue::Float(bp.hold_ms.unwrap_or(self.hold_ms)))
                        }
                        "threshold" => Some(ParameterValue::Float(
                            bp.threshold_db.unwrap_or(self.threshold_db),
                        )),
                        "ratio" => Some(ParameterValue::Float(bp.ratio.unwrap_or(self.ratio))),
                        "attack" => Some(ParameterValue::Float(
                            bp.attack_ms.unwrap_or(self.attack_ms),
                        )),
                        "release" => Some(ParameterValue::Float(
                            bp.release_ms.unwrap_or(self.release_ms),
                        )),
                        "makeup" => Some(ParameterValue::Float(bp.makeup_gain_db)),
                        "auto_makeup" => Some(ParameterValue::Bool(bp.auto_makeup)),
                        "measured_auto_makeup" => {
                            Some(ParameterValue::Bool(bp.measured_auto_makeup))
                        }
                        "active" => Some(ParameterValue::Bool(bp.active)),
                        "solo" => Some(ParameterValue::Bool(bp.solo)),
                        "bypass" => Some(ParameterValue::Bool(bp.bypass)),
                        "knee" => Some(ParameterValue::Float(bp.knee_db.unwrap_or(self.knee_db))),
                        _ => None,
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}

impl MultibandCompressorPlugin {
    /// Exact audio support when no recursive wet path can contribute.
    fn finite_response_frames(&self) -> Option<usize> {
        if !self.initialized {
            return None;
        }
        let settled_dry = self.mix_smoother.current() == 0.0 && self.mix_smoother.target() == 0.0;
        let broadband = self.num_bands == 1;
        (settled_dry || broadband).then(|| self.latency_samples())
    }

    fn process_stream(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let nf = context.num_frames;
        if nf == 0 {
            return Ok(0);
        }
        let expected_len = nf
            .checked_mul(self.channels)
            .ok_or_else(|| "Multiband compressor buffer length overflow".to_string())?;
        if buffer.len() != expected_len {
            return Err(format!(
                "Multiband compressor expected {expected_len} samples, got {}",
                buffer.len()
            ));
        }
        if nf > 4096 {
            let mut processed = 0;
            while processed < nf {
                let chunk_frames = (nf - processed).min(4096);
                let sample_start = processed * self.channels;
                let sample_end = sample_start + chunk_frames * self.channels;
                let mut chunk_context = *context;
                chunk_context.num_frames = chunk_frames;
                self.process_stream(&mut buffer[sample_start..sample_end], &chunk_context)?;
                processed += chunk_frames;
            }
            return Ok(nf);
        }
        let stride = nf * self.channels;
        debug_assert!(
            self.dry_buffer.len() >= buffer.len(),
            "dry_buffer undersized: {} < {} (call initialize() before processing)",
            self.dry_buffer.len(),
            buffer.len()
        );
        debug_assert!(
            self.band_buffers.len() >= self.num_bands * stride,
            "band_buffers undersized: {} < {} (call initialize() before processing)",
            self.band_buffers.len(),
            self.num_bands * stride
        );

        self.dry_buffer[..buffer.len()].copy_from_slice(buffer);
        if self.per_band_lookahead_ms > 0.0 {
            for frame in 0..nf {
                let offset = frame * self.channels;
                self.lookahead_frame_tmp
                    .copy_from_slice(&self.dry_buffer[offset..offset + self.channels]);
                self.dry_lookahead_buffer.process_frame(
                    &self.lookahead_frame_tmp,
                    &mut self.dry_buffer[offset..offset + self.channels],
                );
            }
        }

        // M/S encode: convert L/R to Mid/Side before band splitting
        let use_ms = self.ms_mode && self.channels == 2;
        if use_ms {
            for frame in 0..nf {
                let idx = frame * 2;
                let l = buffer[idx];
                let r = buffer[idx + 1];
                buffer[idx] = (l + r) * 0.5; // Mid
                buffer[idx + 1] = (l - r) * 0.5; // Side
            }
        }

        for frame in 0..nf {
            // Advance crossover automation sample-by-sample so the output is
            // independent of host block partitioning.
            for i in 0..self.num_bands.saturating_sub(1) {
                let freq = self.xover_smoothers[i].advance();
                self.crossover_points[i].set_frequency(freq);
            }
            for ch in 0..self.channels {
                let idx = frame * self.channels + ch;
                let mut rem = buffer[idx];
                for xidx in 0..(self.num_bands - 1) {
                    let (low, high) = self.crossover_points[xidx].process(rem, ch);
                    self.band_buffers[xidx * stride + idx] = low;
                    rem = high;
                }
                self.band_buffers[(self.num_bands - 1) * stride + idx] = rem;
            }
        }

        for values in &mut self.automation_values[..nf] {
            values[0] = self.threshold_smoother.advance();
            values[1] = self.mix_smoother.advance();
            values[2] = self.link_smoother.advance();
            values[3] = self.tilt_smoother.advance();
        }

        let mut any_solo = false;
        for b in 0..self.num_bands {
            if let Some(p) = self.band_params.get(b)
                && p.solo
            {
                any_solo = true;
                break;
            }
        }

        for b in 0..self.num_bands {
            let bp = self.band_params.get(b);
            let is_bypassed = bp.map(|p| p.bypass).unwrap_or(false);
            let is_passive = !bp.map(|p| p.active).unwrap_or(true);
            let is_muted = any_solo && !bp.map(|p| p.solo).unwrap_or(false);

            if is_muted {
                let off = b * stride;
                self.band_buffers[off..off + stride].fill(0.0);
                self.band_levels_db[b] = -120.0;
                continue;
            }

            if is_bypassed || is_passive {
                let off = b * stride;
                if self.per_band_lookahead_ms > 0.0 {
                    for frame in 0..nf {
                        let f_off = off + frame * self.channels;
                        self.lookahead_frame_tmp
                            .copy_from_slice(&self.band_buffers[f_off..f_off + self.channels]);
                        self.lookahead_buffers[b].process_frame(
                            &self.lookahead_frame_tmp,
                            &mut self.band_buffers[f_off..f_off + self.channels],
                        );
                    }
                }
                let mut max_abs = 0.0f32;
                for i in 0..stride {
                    max_abs = max_abs.max(self.band_buffers[off + i].abs());
                }
                self.band_levels_db[b] = 20.0 * fast_log10(max_abs.max(1e-10));
                continue;
            }

            let use_measured_makeup = bp.map(|p| p.measured_auto_makeup).unwrap_or(false);
            let use_auto_makeup = bp.map(|p| p.auto_makeup).unwrap_or(false);
            let range_db = bp.and_then(|p| p.range_db).unwrap_or(self.range_db);
            // The top setting is an explicit unlimited sentinel, retaining the
            // original response even for floating-point inputs above full scale.
            let range_limit = if range_db >= 120.0 {
                f32::INFINITY
            } else {
                range_db
            };
            // fast_pow10 is approximate. Bound its result with an exact floor
            // so the physical applied reduction cannot exceed the Range knob.
            let range_gain_floor = if range_db >= 120.0 {
                0.0
            } else {
                10.0_f32.powf(-range_db / 20.0)
            };
            let hold_ms = bp.and_then(|p| p.hold_ms).unwrap_or(self.hold_ms);
            let hold_samples = (hold_ms as f64 * self.sample_rate as f64 / 1000.0).round() as usize;

            let use_lookahead = self.per_band_lookahead_ms > 0.0;
            // New sidechain stages are global; hoist them out of the frame loop.
            // Peak + inactive HPF (disabled, or 0 Hz) keeps the exact legacy
            // detection path below.
            let use_rms = Self::detection_is_rms_index(self.detection_mode_index);
            let hpf_sections = Self::hpf_active_sections_index(
                self.sidechain_hpf_enabled,
                self.sidechain_hpf_hz,
                self.sidechain_hpf_order_index,
            );
            let use_legacy_detection = !use_rms && hpf_sections == 0;
            let bcomp = &mut self.band_compressors[b];
            let smoothers = &mut self.band_smoothers[b];
            let tilt_filters = &mut self.sidechain_tilt_biquads[b];
            let off = b * stride;
            let mut band_max_abs = 0.0f32;

            for frame in 0..nf {
                let th = smoothers.threshold.advance();
                let rat = smoothers.ratio.advance();
                let kn = smoothers.knee.advance();
                let attack_coeff = smoothers.attack_coeff.advance();
                let release_coeff = smoothers.release_coeff.advance();
                let makeup_db = smoothers.makeup_db.advance();
                let link = self.automation_values[frame][2];
                let tilt = self.automation_values[frame][3];
                if (tilt - smoothers.applied_tilt_db).abs() > 1.0e-5 {
                    let half_tilt = tilt as f64 * 0.5;
                    for (low, high) in tilt_filters.iter_mut() {
                        low.update_params(
                            BiquadFilterType::Lowshelf,
                            1000.0,
                            self.sample_rate.max(1) as f64,
                            0.707,
                            -half_tilt,
                        );
                        high.update_params(
                            BiquadFilterType::Highshelf,
                            1000.0,
                            self.sample_rate.max(1) as f64,
                            0.707,
                            half_tilt,
                        );
                    }
                    smoothers.applied_tilt_db = tilt;
                }
                bcomp.attack_coeff = attack_coeff;
                bcomp.release_coeff = release_coeff;
                // Detect max-of-channels level for linked detection
                // Apply per-band sidechain tilt filter if configured
                let mut max_det = 0.0f32;
                if use_legacy_detection {
                    if use_ms {
                        let raw = self.band_buffers[off + frame * self.channels];
                        let (low, high) = &mut tilt_filters[0];
                        let filtered = high.process(low.process(raw as f64)) as f32;
                        max_det = filtered.abs();
                    } else {
                        for (ch, (low, high)) in tilt_filters.iter_mut().enumerate() {
                            let raw = self.band_buffers[off + frame * self.channels + ch];
                            let filtered = high.process(low.process(raw as f64)) as f32;
                            max_det = max_det.max(filtered.abs());
                        }
                    }
                }
                let max_idb_legacy = 20.0 * fast_log10(max_det.max(1e-10));

                // Apply lookahead delay: push current frame, get delayed frame
                if use_lookahead {
                    let f_off = off + frame * self.channels;
                    let input = &self.band_buffers[f_off..f_off + self.channels];
                    // Copy input into temp since process_frame borrows both
                    self.lookahead_frame_tmp.copy_from_slice(input);
                    self.lookahead_buffers[b].process_frame(
                        &self.lookahead_frame_tmp,
                        &mut self.band_buffers[f_off..f_off + self.channels],
                    );
                }

                // Full sidechain chain (HPF -> tilt -> Peak/RMS) for the new
                // detector stages. Each undelayed detection sample is filtered
                // exactly once; per-channel levels are reused below so filter
                // and detector state advance consistently for linked and
                // unlinked blends. In M/S mode the linked level follows Mid.
                let mut max_idb = max_idb_legacy;
                if !use_legacy_detection {
                    max_det = 0.0;
                    let hpf_bank = &mut self.sidechain_hpf_biquads[b];
                    let detectors = &mut self.level_detectors[b];
                    let ms_channels = if use_ms { 1 } else { self.channels };
                    for ch in 0..self.channels {
                        let idx = off + frame * self.channels + ch;
                        let input = if use_lookahead {
                            self.lookahead_frame_tmp[ch]
                        } else {
                            self.band_buffers[idx]
                        };
                        let mut x = input as f64;
                        for section in hpf_bank[ch].iter_mut().take(hpf_sections) {
                            x = section.process(x);
                        }
                        let (low, high) = &mut tilt_filters[ch];
                        let tilted = high.process(low.process(x)) as f32;
                        let level = if use_rms {
                            detectors[ch].process_linear(tilted)
                        } else {
                            tilted.abs()
                        };
                        self.detect_scratch[ch] = level;
                        if ch < ms_channels {
                            max_det = max_det.max(level);
                        }
                    }
                    max_idb = 20.0 * fast_log10(max_det.max(1e-10));
                }

                for ch in 0..self.channels {
                    let idx = off + frame * self.channels + ch;
                    let detect_raw = if use_lookahead {
                        self.lookahead_frame_tmp[ch]
                    } else {
                        self.band_buffers[idx]
                    };
                    // Legacy Peak path keeps the historical per-channel abs()
                    // (tilt applies to the linked level only); the new chain
                    // reuses the filtered level computed above.
                    let detect_abs = if use_legacy_detection {
                        detect_raw.abs()
                    } else {
                        self.detect_scratch[ch]
                    };
                    band_max_abs = band_max_abs.max(self.band_buffers[idx].abs());

                    // Blend per-channel and linked detection using link_amount
                    let per_ch_idb = 20.0 * fast_log10(detect_abs.max(1e-10));
                    let idb = if link >= 1.0 {
                        max_idb
                    } else if link <= 0.0 {
                        per_ch_idb
                    } else {
                        per_ch_idb * (1.0 - link) + max_idb * link
                    };
                    let tgr = Self::calculate_gain_reduction(idb, th, rat, kn).min(range_limit);
                    // Clamp existing state too: lowering Range limits already
                    // active compression on the very next sample.
                    bcomp.envelope[ch] = bcomp.envelope[ch].min(range_limit);
                    bcomp.hold_remaining[ch] = bcomp.hold_remaining[ch].min(hold_samples);
                    if tgr > bcomp.envelope[ch] {
                        bcomp.envelope[ch] = tgr + attack_coeff * (bcomp.envelope[ch] - tgr);
                        bcomp.hold_remaining[ch] = hold_samples;
                    } else if tgr == bcomp.envelope[ch] {
                        bcomp.hold_remaining[ch] = hold_samples;
                    } else if bcomp.hold_remaining[ch] > 0 {
                        bcomp.hold_remaining[ch] -= 1;
                    } else {
                        bcomp.envelope[ch] = tgr + release_coeff * (bcomp.envelope[ch] - tgr);
                    }

                    let gain_linear = fast_pow10(-bcomp.envelope[ch] / 20.0).max(range_gain_floor);
                    self.band_buffers[idx] *= gain_linear;
                }
                // Update measured makeup once per frame using the max envelope across channels.
                // Calling update() once per channel would halve the effective time constant on stereo.
                if use_measured_makeup {
                    let max_env = bcomp.envelope.iter().cloned().fold(0.0f32, f32::max);
                    self.measured_makeups[b].update(max_env);
                    // Hoist makeup_linear() out of the ch loop — it's constant per frame.
                    let makeup = self.measured_makeups[b].makeup_linear();
                    for ch in 0..self.channels {
                        let idx = off + frame * self.channels + ch;
                        self.band_buffers[idx] *= makeup;
                    }
                } else {
                    let makeup = if use_auto_makeup {
                        let slope = 1.0 - 1.0 / rat.max(1.0);
                        let overshoot = (-th).max(0.0) * 0.5;
                        fast_pow10((overshoot * slope).min(range_limit) / 20.0)
                    } else {
                        fast_pow10(makeup_db / 20.0)
                    };
                    for ch in 0..self.channels {
                        let idx = off + frame * self.channels + ch;
                        self.band_buffers[idx] *= makeup;
                    }
                }
            }
            self.band_levels_db[b] = 20.0 * fast_log10(band_max_abs.max(1e-10));
        }

        for frame in 0..nf {
            let g_mix = self.automation_values[frame][1];
            if use_ms {
                let idx = frame * 2;
                let mut mid = 0.0;
                let mut side = 0.0;
                for b in 0..self.num_bands {
                    mid += self.band_buffers[b * stride + idx];
                    side += self.band_buffers[b * stride + idx + 1];
                }
                let wet_l = mid + side;
                let wet_r = mid - side;
                buffer[idx] = self.dry_buffer[idx] * (1.0 - g_mix) + wet_l * g_mix;
                buffer[idx + 1] = self.dry_buffer[idx + 1] * (1.0 - g_mix) + wet_r * g_mix;
            } else {
                for ch in 0..self.channels {
                    let idx = frame * self.channels + ch;
                    let mut s = 0.0f32;
                    for b in 0..self.num_bands {
                        s += self.band_buffers[b * stride + idx];
                    }
                    buffer[idx] = self.dry_buffer[idx] * (1.0 - g_mix) + s * g_mix;
                }
            }
        }

        // Update diagnostic cache (throttled to ~50 ms, sample-based so block size independent)
        self.cache_update_counter += nf;
        if self.cache_update_counter >= self.cache_update_threshold {
            self.cache_update_counter = 0;
            for b in 0..self.num_bands {
                for ch in 0..self.channels {
                    self.gain_reduction_flattened[b * self.channels + ch] =
                        self.band_compressors[b].envelope[ch];
                }
            }
            let levels = &self.band_levels_db;
            let xovers = &self.crossover_frequencies;
            self.cache.update(|d| {
                d.update(&self.gain_reduction_flattened, levels, xovers);
            });
        }

        flush_denormals_inplace(buffer);
        Ok(nf)
    }
}

impl ParametricInPlacePlugin for MultibandCompressorPlugin {
    fn info(&self) -> PluginInfo {
        if self.num_bands == 1 {
            PluginInfo::new("Compressor", env!("CARGO_PKG_VERSION"), "Sotf")
                .with_description("Broadband dynamics processor")
        } else {
            PluginInfo::new("Multiband Compressor", env!("CARGO_PKG_VERSION"), "Sotf")
                .with_description("Cascaded LR4 multiband dynamics processor")
        }
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Dynamics
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::nonlinear(
            PluginCostClass::Dynamics,
            (self.per_band_lookahead_ms <= 0.0).then_some(PluginCompiledOp::MultibandCompressor),
            self.latency_samples(),
            false,
        )
    }

    fn channels(&self) -> usize {
        self.channels
    }
    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }
    fn parametric_validate_parameter(
        &self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> PluginResult<()> {
        let parameter = self
            .cached_parameters
            .iter()
            .find(|parameter| &parameter.id == id)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?;
        parameter
            .validate(value)
            .map_err(|error| format!("{id}: {error}"))
    }
    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        if matches!(id.as_str(), "range_db" | "hold_ms")
            || (id.as_str().starts_with("band_")
                && (id.as_str().ends_with("_range_db") || id.as_str().ends_with("_hold_ms")))
        {
            return self.set_parameter(id, value);
        }
        self.parametric_validate_parameter(&id, &value)?;
        self.set_parameter(id, value)
    }
    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return if values
                .iter()
                .all(|(id, value)| self.get_parameter(id).as_ref() == Some(value))
            {
                Ok(())
            } else {
                Err("reset the multiband compressor before changing controls after drain".into())
            };
        }
        for (id, value) in values {
            self.set_parameter(id, value)?;
        }
        Ok(())
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        // The full snapshot contains only declared IDs. Preserve that boundary
        // before the inherent getter parses dynamic band fields and aliases.
        self.cached_parameters
            .iter()
            .find(|parameter| &parameter.id == id)?;
        self.get_parameter(id)
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        for param in &self.cached_parameters {
            if let Some(value) = self.get_parameter(&param.id) {
                values.insert(param.id.clone(), value);
            }
        }
        values
    }

    fn initialize(&mut self, sr: u32) -> PluginResult<()> {
        if sr == 0 {
            return Err("multiband compressor requires a positive sample rate".into());
        }
        self.sample_rate = sr;
        // Update cache throttle threshold: fire every ~50 ms worth of samples.
        self.cache_update_threshold = (sr as usize * 50 / 1000).max(1);
        self.build_crossovers();
        self.update_coefficients();
        self.rebuild_sidechain_tilt();
        self.rebuild_sidechain_hpf();
        // Detectors store their sample rate; rebuild them with preallocated
        // RMS buffers (initialization is not on the audio path).
        let detector_mode = if Self::detection_is_rms_index(self.detection_mode_index) {
            DetectionMode::Rms {
                window_ms: Self::RMS_WINDOW_MS,
            }
        } else {
            DetectionMode::Peak
        };
        let bands = self.num_bands;
        let channels = self.channels;
        self.level_detectors = (0..bands)
            .map(|_| {
                (0..channels)
                    .map(|_| Self::make_detector(detector_mode, sr))
                    .collect()
            })
            .collect();
        self.detect_scratch.resize(channels, 0.0);
        for smoother in &mut self.band_smoothers {
            smoother.applied_tilt_db = self.tilt_smoother.current();
        }
        self.threshold_smoother.set_time(20.0, sr);
        self.mix_smoother.set_time(20.0, sr);
        self.link_smoother.set_time(20.0, sr);
        self.tilt_smoother.set_time(20.0, sr);
        for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
            smoother.threshold.set_time(20.0, sr);
            smoother.ratio.set_time(20.0, sr);
            smoother.knee.set_time(20.0, sr);
            smoother.makeup_db.set_time(20.0, sr);
            smoother.attack_coeff.set_time(20.0, sr);
            smoother.release_coeff.set_time(20.0, sr);
            smoother.attack_coeff.reset(Self::envelope_coeff(
                band.attack_ms.unwrap_or(self.attack_ms),
                sr,
            ));
            smoother.release_coeff.reset(Self::envelope_coeff(
                band.release_ms.unwrap_or(self.release_ms),
                sr,
            ));
        }
        for s in &mut self.xover_smoothers {
            *s = LogSmoother::new(s.target(), 50.0, sr);
        }

        // Reinitialize lookahead buffers for new sample rate
        let la_ms = self.per_band_lookahead_ms;
        for buf in &mut self.lookahead_buffers {
            if la_ms > 0.0 {
                let max_samples = (20.0 * 0.001 * sr as f32).round() as usize;
                buf.resize(max_samples, self.channels);
                buf.set_delay_ms(la_ms, sr);
            }
        }
        let max_samples = (20.0 * 0.001 * sr as f32).round() as usize;
        self.dry_lookahead_buffer.resize(max_samples, self.channels);
        if la_ms > 0.0 {
            self.dry_lookahead_buffer.set_delay_ms(la_ms, sr);
        }
        // Reinitialize measured makeup smoothing for new sample rate
        for mm in &mut self.measured_makeups {
            mm.set_smoothing(1000.0, sr);
        }

        // Pre-allocate buffers for real-time safety
        let max_frames = 4096;
        let stride = max_frames * self.channels;
        self.band_buffers.resize(self.num_bands * stride, 0.0);
        self.dry_buffer.resize(max_frames * self.channels, 0.0);
        self.automation_values.resize(max_frames, [0.0; 4]);
        self.lookahead_frame_tmp.resize(self.channels, 0.0);
        self.initialized = true;
        self.reset();

        Ok(())
    }
    fn reset(&mut self) {
        self.has_input = false;
        self.drain_remaining = None;
        for x in &mut self.crossover_points {
            x.reset();
        }
        for b in &mut self.band_compressors {
            b.envelope.fill(0.0);
            b.hold_remaining.fill(0);
        }
        for buf in &mut self.lookahead_buffers {
            buf.reset();
        }
        self.dry_lookahead_buffer.reset();
        for mm in &mut self.measured_makeups {
            mm.reset();
        }
        self.threshold_smoother.reset(self.threshold_db);
        self.mix_smoother.reset(self.mix);
        self.link_smoother.reset(self.link_amount);
        self.tilt_smoother.reset(self.sidechain_tilt_db);
        for (band, smoother) in self.band_params.iter().zip(&mut self.band_smoothers) {
            smoother
                .threshold
                .reset(band.threshold_db.unwrap_or(self.threshold_db));
            smoother.ratio.reset(band.ratio.unwrap_or(self.ratio));
            smoother.knee.reset(band.knee_db.unwrap_or(self.knee_db));
            smoother.makeup_db.reset(band.makeup_gain_db);
            smoother.applied_tilt_db = self.sidechain_tilt_db;
            smoother.attack_coeff.reset(Self::envelope_coeff(
                band.attack_ms.unwrap_or(self.attack_ms),
                self.sample_rate,
            ));
            smoother.release_coeff.reset(Self::envelope_coeff(
                band.release_ms.unwrap_or(self.release_ms),
                self.sample_rate,
            ));
        }
        self.band_buffers.fill(0.0);
        self.dry_buffer.fill(0.0);
        self.update_sidechain_tilt_coefficients(self.sidechain_tilt_db);
        for band in &mut self.sidechain_tilt_biquads {
            for (low, high) in band {
                low.reset();
                high.reset();
            }
        }
        self.update_sidechain_hpf_coefficients();
        self.reset_sidechain_hpf_state();
        for band in &mut self.level_detectors {
            for detector in band {
                detector.reset();
            }
        }
        self.detect_scratch.fill(0.0);
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("reset the multiband compressor before processing after drain".into());
        }
        let frames = self.process_stream(buffer, context)?;
        self.has_input |= frames > 0;
        Ok(frames)
    }

    fn process_compiled_f32(
        &mut self,
        op: PluginCompiledOp,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Option<Result<usize, String>> {
        if op != PluginCompiledOp::MultibandCompressor || self.per_band_lookahead_ms > 0.0 {
            return None;
        }
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Some(Err(
                "reset the multiband compressor before processing after drain".into(),
            ));
        }
        let sample_len = context.num_frames.checked_mul(self.channels)?;
        if input.len() < sample_len || output.len() < sample_len {
            return Some(Err(format!(
                "multiband compressor compiled buffer too small: need {sample_len} samples, input={}, output={}",
                input.len(),
                output.len()
            )));
        }
        output[..sample_len].copy_from_slice(&input[..sample_len]);
        Some(self.process_in_place(&mut output[..sample_len], context))
    }

    fn tail_length(&self) -> TailLength {
        if let Some(frames) = self.finite_response_frames() {
            TailLength::Finite(frames as u64)
        } else if !self.initialized {
            TailLength::Unknown
        } else {
            TailLength::Infinite
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        self.finite_response_frames()
            .map_or(0, |frames| frames.min(MAX_DRAIN_FRAMES))
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.initialized {
            return None;
        }
        let remaining = if self.has_input {
            self.finite_response_frames()
                .map_or(0, |support| self.drain_remaining.unwrap_or(support))
        } else {
            0
        };
        // Eligible finite paths publish up to 256 frames per full-capacity
        // call with no separate output cache. Unsupported wet paths retain
        // their existing immediate COMPLETE policy, which costs one call.
        std::num::NonZeroU64::new(remaining.div_ceil(MAX_DRAIN_FRAMES).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("multiband compressor drain requires the initialized sample rate".into());
        }
        if self.channels == 0 || !output.len().is_multiple_of(self.channels) {
            return Err("multiband compressor drain requires whole output frames".into());
        }
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let Some(bound) = self.finite_response_frames() else {
            // Preserve legacy EOS behavior for recursive wet paths. A finite
            // rendering policy remains unresolved; metadata never claims zero.
            return Ok(PluginDrainResult::COMPLETE);
        };
        let remaining = self.drain_remaining.unwrap_or(bound);
        if remaining == 0 {
            self.drain_remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }
        let frames = remaining
            .min(MAX_DRAIN_FRAMES)
            .min(output.len() / self.channels);
        if frames == 0 {
            return Err("multiband compressor drain needs output capacity".into());
        }
        let samples = frames * self.channels;
        output[..samples].fill(0.0);
        let mut drain_context = *context;
        drain_context.num_frames = frames;
        self.process_stream(&mut output[..samples], &drain_context)?;
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }

    fn latency_samples(&self) -> usize {
        if self.per_band_lookahead_ms <= 0.0 {
            0
        } else {
            // An active ring clamps positive sub-sample delays to one frame.
            self.dry_lookahead_buffer.delay()
        }
    }
}
