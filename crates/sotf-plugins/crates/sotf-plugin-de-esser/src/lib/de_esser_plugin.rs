use super::consts::DB_CONVERSION_FACTOR;
use super::consts::EPSILON;
use super::consts::FIR_TAPS;
use super::consts::FIXED_KNEE_DB;
use super::consts::MAX_DRAIN_FRAMES;
use super::consts::MAX_LOOKAHEAD_MS;
use super::de_esser_data::DeEsserData;
use super::types::DeEsserPluginParams;
use crate::params::{
    PARAMS as DE, default_attack_ms, default_frequency, default_lookahead_ms, default_mix,
    default_ms_mode, default_q, default_range_db, default_ratio, default_release_ms,
    default_sidechain_external, default_stereo_link, default_threshold,
};
use math_audio_dsp::fast_math::fast_log10;
use math_audio_iir_fir::{Biquad, BiquadBank, BiquadFilterType};
use sotf_host::FirCrossover;
use sotf_host::LookaheadBuffer;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::dynamics_core::DynamicsCore;
use sotf_host::dynamics_core::DynamicsMode;
use sotf_host::lr4_crossover::Lr4Crossover;
use sotf_host::param_specs::{UpdateMode, find_by_key as pk};
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::simd::{apply_per_channel_gain_simd, enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::Smoother;
use std::any::Any;
use std::sync::Arc;

const DETECTOR_POLE_Q: f32 = std::f32::consts::FRAC_1_SQRT_2;

pub struct DeEsserPlugin {
    pub(super) channels: usize,
    pub(super) sample_rate: u32,

    // Detection
    pub(super) param_frequency: ParameterId,
    pub(super) frequency: f32,
    pub(super) param_q: ParameterId,
    pub(super) q: f32,
    /// Highpass filter bank per channel (lower bound of sidechain BPF)
    pub(super) hp_filters: BiquadBank<f32>,
    /// Lowpass filter bank per channel (upper bound of sidechain BPF)
    pub(super) lp_filters: BiquadBank<f32>,

    /// Reusable per-frame sidechain scratch buffer.
    pub(super) sidechain_frame: Vec<f32>,

    /// Reusable per-frame program scratch: M/S encode, lookahead delay,
    /// gain application and M/S decode happen here before write-back.
    pub(super) work_frame: Vec<f32>,

    /// Reusable per-frame gain multipliers for SIMD path.
    pub(super) frame_gains: Vec<f32>,

    /// Program delay so gain reduction anticipates sibilance (R1).
    pub(super) param_lookahead_ms: ParameterId,
    pub(super) lookahead_ms: f32,
    pub(super) lookahead_buffers: Vec<LookaheadBuffer>,

    /// Split-band crossover bank selection (R2).
    pub(super) param_split_topology: ParameterId,
    /// 0=minimum-phase LR4, 1=linear-phase FIR
    pub(super) split_topology_index: usize,
    pub(super) fir_split: Option<FirCrossover<f32>>,

    /// Mid/Side processing and external key input (R3).
    pub(super) param_ms_mode: ParameterId,
    pub(super) ms_mode: bool,
    pub(super) param_sidechain_external: ParameterId,
    pub(super) sidechain_external: bool,

    // Dynamics (one DynamicsCore per channel)
    pub(super) cores: Vec<DynamicsCore>,
    pub(super) param_threshold: ParameterId,
    pub(super) threshold: f32,
    pub(super) param_ratio: ParameterId,
    pub(super) ratio: f32,
    pub(super) param_range_db: ParameterId,
    pub(super) range_db: f32,
    pub(super) range_smoother: Smoother,
    pub(super) param_stereo_link: ParameterId,
    pub(super) stereo_link: f32,
    pub(super) link_smoother: Smoother,

    // Split-band mode
    pub(super) param_mode: ParameterId,
    /// 0=wideband, 1=split-band
    pub(super) mode_index: usize,
    pub(super) crossovers: Vec<Lr4Crossover<f32>>,

    // Mix
    pub(super) param_mix: ParameterId,
    pub(super) mix: f32,
    pub(super) mix_smoother: Smoother,

    // Attack/Release params (tracked for parameter get/set)
    pub(super) param_attack: ParameterId,
    pub(super) attack_ms: f32,
    pub(super) param_release: ParameterId,
    pub(super) release_ms: f32,

    // Lifecycle / end-of-stream drain
    pub(super) initialized: bool,
    pub(super) has_input: bool,
    /// Any `Some` value requires reset before accepting more input.
    pub(super) drain_remaining: Option<usize>,
    /// Zero continuation uses input stride; public drain returns program channels only.
    pub(super) drain_scratch: Vec<f32>,

    // Monitoring
    /// Per-channel gain reduction in dB for monitoring
    pub(super) monitoring_gr: Vec<f32>,
    pub(super) cache: RealTimeCache<DeEsserData>,
    pub(super) cache_counter: usize,

    // Parameters
    pub(super) cached_parameters: Vec<Parameter>,
}

impl DeEsserPlugin {
    pub fn new(channels: usize) -> Self {
        let sr = 44100u32;
        let freq = default_frequency();
        let q = default_q();

        let mut p = Self {
            channels,
            sample_rate: sr,

            param_frequency: ParameterId::from("frequency"),
            frequency: freq,
            param_q: ParameterId::from("q"),
            q,
            hp_filters: Self::make_hp_filters(channels, freq, q, sr),
            lp_filters: Self::make_lp_filters(channels, freq, q, sr),
            sidechain_frame: vec![0.0; channels],
            work_frame: vec![0.0; channels],
            frame_gains: vec![0.0; channels],

            param_lookahead_ms: ParameterId::from("lookahead_ms"),
            lookahead_ms: default_lookahead_ms(),
            lookahead_buffers: (0..channels)
                .map(|_| LookaheadBuffer::from_ms(MAX_LOOKAHEAD_MS, sr, 1))
                .collect(),

            param_split_topology: ParameterId::from("split_topology"),
            split_topology_index: 0, // default: minimum-phase LR4
            fir_split: None,

            param_ms_mode: ParameterId::from("ms_mode"),
            ms_mode: default_ms_mode(),
            param_sidechain_external: ParameterId::from("sidechain_external"),
            sidechain_external: default_sidechain_external(),

            cores: (0..channels)
                .map(|_| DynamicsCore::new(DynamicsMode::Compress, 1, sr))
                .collect(),
            param_threshold: ParameterId::from("threshold"),
            threshold: default_threshold(),
            param_ratio: ParameterId::from("ratio"),
            ratio: default_ratio(),
            param_range_db: ParameterId::from("range_db"),
            range_db: default_range_db(),
            range_smoother: Smoother::new(default_range_db(), 5.0, sr),
            param_stereo_link: ParameterId::from("stereo_link"),
            stereo_link: default_stereo_link(),
            link_smoother: Smoother::new(default_stereo_link(), 5.0, sr),

            param_mode: ParameterId::from("mode"),
            mode_index: 1, // default: split-band
            crossovers: (0..channels)
                .map(|_| Lr4Crossover::new(freq, sr as f32, 1))
                .collect(),

            param_mix: ParameterId::from("mix"),
            mix: default_mix(),
            mix_smoother: Smoother::new(1.0, 5.0, sr),

            param_attack: ParameterId::from("attack"),
            attack_ms: default_attack_ms(),
            param_release: ParameterId::from("release"),
            release_ms: default_release_ms(),

            initialized: false,
            has_input: false,
            drain_remaining: None,
            drain_scratch: Vec::new(),

            monitoring_gr: vec![0.0; channels],
            cache: RealTimeCache::new(DeEsserData::new(channels)),
            cache_counter: 0,

            cached_parameters: Vec::new(),
        };

        // Set attack/release on dynamics cores
        for core in &mut p.cores {
            core.set_attack_release(p.attack_ms, p.release_ms);
        }

        p.rebuild_cached_parameters();
        p
    }

    /// Construct from serialized state using the canonical 48 kHz validation
    /// contract. Invalid state is rejected rather than silently clamped.
    pub fn from_params(channels: usize, params: DeEsserPluginParams) -> PluginResult<Self> {
        Self::try_from_params(channels, params)
    }

    fn from_validated_params(channels: usize, params: DeEsserPluginParams) -> Self {
        let mut p = Self::new(channels);
        p.frequency = params.frequency;
        p.q = params.q;
        p.threshold = params.threshold;
        p.ratio = params.ratio;
        p.range_db = params.range_db;
        p.range_smoother.reset(p.range_db);
        p.stereo_link = params.stereo_link;
        p.link_smoother.reset(p.stereo_link);
        p.attack_ms = params.attack_ms;
        p.release_ms = params.release_ms;
        p.mix = params.mix;
        p.mix_smoother.set_target(p.mix);

        // Mode
        p.mode_index = match params.mode.as_str() {
            "Wideband" | "wideband" => 0,
            _ => 1, // validated Split-Band spelling
        };
        p.lookahead_ms = params.lookahead_ms;
        p.split_topology_index = match params.split_topology.as_str() {
            "Linear-Phase" | "linear-phase" => 1,
            _ => 0, // validated Minimum-Phase spelling
        };
        p.ms_mode = params.ms_mode;
        p.sidechain_external = params.sidechain_external;

        // Update dynamics cores
        for core in &mut p.cores {
            core.set_attack_release(p.attack_ms, p.release_ms);
        }

        // Rebuild filters
        p.rebuild_detection_filters();
        p.rebuild_crossovers();
        p.update_lookahead_delay();
        p.rebuild_fir_split();
        p.rebuild_cached_parameters();
        p
    }

    pub fn try_from_params(channels: usize, params: DeEsserPluginParams) -> PluginResult<Self> {
        Self::try_from_params_at_sample_rate(channels, params, 48_000)
    }

    /// Factory variant that also validates the detector band for its runtime
    /// sample rate. This keeps low-rate hosts from accepting a preset that
    /// only has valid filter edges at the constructor's default rate.
    pub fn try_from_params_at_sample_rate(
        channels: usize,
        params: DeEsserPluginParams,
        sample_rate: u32,
    ) -> PluginResult<Self> {
        if channels == 0 {
            return Err("De-Esser requires at least one channel".to_string());
        }
        if sample_rate == 0 {
            return Err("De-Esser sample rate must be greater than zero".to_string());
        }
        if params.sidechain_external && channels.checked_mul(2).is_none() {
            return Err("De-Esser external-sidechain channel count overflows usize".into());
        }
        if !matches!(
            params.mode.as_str(),
            "Wideband" | "wideband" | "Split-Band" | "split-band"
        ) {
            return Err(format!("Unknown De-Esser mode: {}", params.mode));
        }
        if !matches!(
            params.split_topology.as_str(),
            "Minimum-Phase" | "minimum-phase" | "Linear-Phase" | "linear-phase"
        ) {
            return Err(format!(
                "Unknown De-Esser split topology: {}",
                params.split_topology
            ));
        }
        let ranges = [
            ("frequency", params.frequency, 2000.0, 16000.0),
            ("q", params.q, 0.5, 5.0),
            ("threshold", params.threshold, -60.0, 0.0),
            ("ratio", params.ratio, 1.0, 20.0),
            ("attack_ms", params.attack_ms, 0.1, 10.0),
            ("release_ms", params.release_ms, 5.0, 200.0),
            ("mix", params.mix, 0.0, 1.0),
            ("range_db", params.range_db, 0.0, 60.0),
            ("stereo_link", params.stereo_link, 0.0, 1.0),
            ("lookahead_ms", params.lookahead_ms, 0.0, 20.0),
        ];
        for (name, value, min, max) in ranges {
            if !value.is_finite() || !(min..=max).contains(&value) {
                return Err(format!(
                    "Invalid De-Esser {name}: expected finite value in {min}..={max}, got {value}"
                ));
            }
        }
        Self::validate_detection_band(params.frequency, params.q, sample_rate)?;
        Ok(Self::from_validated_params(channels, params))
    }

    fn validate_detection_band(frequency: f32, q: f32, sample_rate: u32) -> PluginResult<()> {
        let (_, high_edge) = Self::bandpass_edges(frequency, q);
        let max_frequency = sample_rate as f32 * 0.475;
        if frequency >= max_frequency || high_edge >= max_frequency {
            return Err(format!(
                "De-Esser detection band must remain below Nyquist: center={}, upper={}, max={max_frequency}",
                frequency, high_edge
            ));
        }
        Ok(())
    }

    pub(super) fn mode_string(&self) -> String {
        match self.mode_index {
            0 => "Wideband".to_string(),
            _ => "Split-Band".to_string(),
        }
    }

    /// Compute highpass frequency from center and Q.
    /// f_hp = freq / sqrt(1 + 1/(4*Q^2)) ... simplified: freq / (2^(1/(2Q)))
    /// Simpler approach: f_low = freq / sqrt(bandwidth_ratio), f_high = freq * sqrt(bandwidth_ratio)
    /// where bandwidth_ratio = 10^(3/(20*Q)) (approx 3dB bandwidth)
    /// Even simpler: just use freq / ratio and freq * ratio where ratio = 2^(1/(2Q))
    pub(super) fn bandpass_edges(freq: f32, q: f32) -> (f32, f32) {
        // Bandwidth in octaves ~= 1/Q for a standard bandpass
        // f_low = freq / 2^(1/(2Q)), f_high = freq * 2^(1/(2Q))
        let half_bw = (1.0 / (2.0 * q.max(0.5))).exp2();
        let f_low = (freq / half_bw).max(20.0);
        let f_high = (freq * half_bw).min(20000.0);
        (f_low, f_high)
    }

    pub(super) fn make_hp_filters(channels: usize, freq: f32, q: f32, sr: u32) -> BiquadBank<f32> {
        let (f_low, _) = Self::bandpass_edges(freq, q);
        let template = Biquad::new(
            BiquadFilterType::Highpass,
            f_low,
            sr as f32,
            DETECTOR_POLE_Q,
            0.0,
        );
        BiquadBank::new(&template, channels)
    }

    pub(super) fn make_lp_filters(channels: usize, freq: f32, q: f32, sr: u32) -> BiquadBank<f32> {
        let (_, f_high) = Self::bandpass_edges(freq, q);
        let template = Biquad::new(
            BiquadFilterType::Lowpass,
            f_high,
            sr as f32,
            DETECTOR_POLE_Q,
            0.0,
        );
        BiquadBank::new(&template, channels)
    }

    pub(super) fn rebuild_detection_filters(&mut self) {
        let (f_low, f_high) = Self::bandpass_edges(self.frequency, self.q);
        self.hp_filters
            .update_params(f_low, self.sample_rate as f32, DETECTOR_POLE_Q, 0.0);
        self.lp_filters
            .update_params(f_high, self.sample_rate as f32, DETECTOR_POLE_Q, 0.0);
    }

    pub(super) fn rebuild_crossovers(&mut self) {
        for xo in &mut self.crossovers {
            xo.set_frequency(self.frequency);
        }
    }

    pub(super) fn split_topology_string(&self) -> String {
        match self.split_topology_index {
            1 => "Linear-Phase".to_string(),
            _ => "Minimum-Phase".to_string(),
        }
    }

    /// True when the linear-phase FIR bank carries the split path. The
    /// topology control is inert in wideband mode.
    pub(super) fn use_fir_split(&self) -> bool {
        self.mode_index == 1 && self.split_topology_index == 1
    }

    /// True when M/S encode/decode wraps the processing path. Stereo
    /// instances only; other channel counts process discrete channels even
    /// when M/S mode is enabled.
    pub(super) fn use_ms(&self) -> bool {
        self.ms_mode && self.channels == 2
    }

    pub(super) fn update_lookahead_delay(&mut self) {
        for buf in &mut self.lookahead_buffers {
            buf.set_delay_ms(self.lookahead_ms, self.sample_rate);
        }
    }

    /// Program delay in samples. A zero lookahead bypasses the delay lines,
    /// so report zero rather than the ring's minimum one-sample delay.
    pub(super) fn lookahead_delay_samples(&self) -> usize {
        if self.lookahead_ms > 0.0 {
            self.lookahead_buffers
                .first()
                .map_or(0, LookaheadBuffer::delay)
        } else {
            0
        }
    }

    /// Linear-phase split group delay `(taps - 1) / 2`, else zero.
    pub(super) fn fir_group_delay_samples(&self) -> usize {
        if self.use_fir_split() {
            self.fir_split
                .as_ref()
                .map_or(0, FirCrossover::latency_samples)
        } else {
            0
        }
    }

    /// Program frames retained after input stops: the lookahead delay plus
    /// the full FIR support when the linear-phase bank is active. Recursive
    /// LR4/detector/envelope tails are cut at drain completion (crossover-LR
    /// precedent); they scale silence and cannot produce audio by themselves.
    pub(super) fn retained_frames(&self) -> usize {
        let fir_support = if self.use_fir_split() {
            FIR_TAPS - 1
        } else {
            0
        };
        self.lookahead_delay_samples() + fir_support
    }

    /// Build or drop the FIR split bank for the current mode, topology,
    /// frequency, rate and channel count. Structural control only; never
    /// called from the realtime path.
    pub(super) fn rebuild_fir_split(&mut self) {
        if self.use_fir_split() {
            self.fir_split = Some(FirCrossover::new(
                self.frequency,
                self.sample_rate as f32,
                self.channels,
                FIR_TAPS,
            ));
        } else {
            self.fir_split = None;
        }
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![
            Parameter::new_float(
                "frequency",
                "Frequency",
                self.frequency,
                pk(DE, "frequency").min_f64() as f32,
                pk(DE, "frequency").max_f64() as f32,
            )
            .with_update_mode(UpdateMode::Structural)
            .with_description("Center frequency for sibilance detection (Hz)")
            .with_group("Detection")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "q",
                "Q",
                self.q,
                pk(DE, "q").min_f64() as f32,
                pk(DE, "q").max_f64() as f32,
            )
            .with_update_mode(UpdateMode::Structural)
            .with_description("Bandwidth of detection filter")
            .with_group("Detection")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "threshold",
                "Threshold",
                self.threshold,
                pk(DE, "threshold").min_f64() as f32,
                pk(DE, "threshold").max_f64() as f32,
            )
            .with_description("Sibilance detection threshold (dB)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "ratio",
                "Ratio",
                self.ratio,
                pk(DE, "ratio").min_f64() as f32,
                pk(DE, "ratio").max_f64() as f32,
            )
            .with_description("Compression ratio for sibilance")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "attack",
                "Attack",
                self.attack_ms,
                pk(DE, "attack").min_f64() as f32,
                pk(DE, "attack").max_f64() as f32,
            )
            .with_description("Attack time (ms)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "release",
                "Release",
                self.release_ms,
                pk(DE, "release").min_f64() as f32,
                pk(DE, "release").max_f64() as f32,
            )
            .with_description("Release time (ms)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_string("mode", "Mode", self.mode_string())
                .with_update_mode(UpdateMode::Structural)
                .with_description("Wideband reduces full signal; Split-band only reduces HF")
                .with_group("Mode")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_float(
                "mix",
                "Mix",
                self.mix,
                pk(DE, "mix").min_f64() as f32,
                pk(DE, "mix").max_f64() as f32,
            )
            .with_description("Dry/wet mix (0 = dry, 1 = processed)")
            .with_group("Output")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "range_db",
                "Range",
                self.range_db,
                pk(DE, "range_db").min_f64() as f32,
                pk(DE, "range_db").max_f64() as f32,
            )
            .with_description("Maximum gain reduction (dB)")
            .with_group("Dynamics")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "stereo_link",
                "Stereo Link",
                self.stereo_link,
                pk(DE, "stereo_link").min_f64() as f32,
                pk(DE, "stereo_link").max_f64() as f32,
            )
            .with_description("Link channel gains to the strongest reduction (0 to 1)")
            .with_group("Detection")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "lookahead_ms",
                "Lookahead",
                self.lookahead_ms,
                pk(DE, "lookahead_ms").min_f64() as f32,
                pk(DE, "lookahead_ms").max_f64() as f32,
            )
            .with_update_mode(UpdateMode::Structural)
            .with_description(
                "Program delay in ms so reduction anticipates sibilance (adds latency)",
            )
            .with_group("Timing")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_string(
                "split_topology",
                "Split Topology",
                self.split_topology_string(),
            )
            .with_update_mode(UpdateMode::Structural)
            .with_description("Split-band crossover: minimum-phase LR4 or linear-phase FIR")
            .with_group("Mode")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("ms_mode", "M/S Mode", self.ms_mode)
                .with_description("Process Mid/Side instead of Left/Right on stereo instances")
                .with_group("Mode")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool(
                "sidechain_external",
                "Ext Sidechain",
                self.sidechain_external,
            )
            .with_update_mode(UpdateMode::Structural)
            .with_description("Detect from the external key bus; input width doubles")
            .with_group("Detection")
            .with_importance(ParameterImportance::Useful),
        ];
    }

    fn apply_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if id == self.param_frequency {
            let frequency = value
                .as_float()
                .ok_or_else(|| "frequency must be a float".to_string())?;
            Self::validate_detection_band(frequency, self.q, self.sample_rate)?;
            if frequency != self.frequency {
                return Err("frequency is structural and requires a host rebuild".into());
            }
        } else if id == self.param_q {
            let q = value
                .as_float()
                .ok_or_else(|| "q must be a float".to_string())?;
            Self::validate_detection_band(self.frequency, q, self.sample_rate)?;
            if q != self.q {
                return Err("q is structural and requires a host rebuild".into());
            }
        } else if id == self.param_threshold {
            self.threshold = value
                .as_float()
                .ok_or_else(|| "threshold must be a float".to_string())?;
        } else if id == self.param_ratio {
            self.ratio = value
                .as_float()
                .ok_or_else(|| "ratio must be a float".to_string())?;
        } else if id == self.param_attack {
            self.attack_ms = value
                .as_float()
                .ok_or_else(|| "attack must be a float".to_string())?;
            for core in &mut self.cores {
                core.set_attack_release(self.attack_ms, self.release_ms);
            }
        } else if id == self.param_release {
            self.release_ms = value
                .as_float()
                .ok_or_else(|| "release must be a float".to_string())?;
            for core in &mut self.cores {
                core.set_attack_release(self.attack_ms, self.release_ms);
            }
        } else if id == self.param_mode {
            let new_index = match value
                .as_string()
                .ok_or_else(|| "mode must be a string".to_string())?
            {
                "Wideband" | "wideband" => 0,
                "Split-Band" | "split-band" => 1,
                other => return Err(format!("Unknown De-Esser mode: {other}")),
            };
            if new_index != self.mode_index {
                return Err("mode is structural and requires a host rebuild".into());
            }
        } else if id == self.param_mix {
            self.mix = value
                .as_float()
                .ok_or_else(|| "mix must be a float".to_string())?;
            self.mix_smoother.set_target(self.mix);
        } else if id == self.param_range_db {
            self.range_db = value
                .as_float()
                .ok_or_else(|| "range_db must be a float".to_string())?;
            self.range_smoother.set_target(self.range_db);
        } else if id == self.param_stereo_link {
            self.stereo_link = value
                .as_float()
                .ok_or_else(|| "stereo_link must be a float".to_string())?;
            self.link_smoother.set_target(self.stereo_link);
        } else if id == self.param_lookahead_ms {
            let lookahead_ms = value
                .as_float()
                .ok_or_else(|| "lookahead_ms must be a float".to_string())?;
            if lookahead_ms != self.lookahead_ms {
                return Err("lookahead_ms is structural and requires a host rebuild".into());
            }
        } else if id == self.param_split_topology {
            let new_index = match value
                .as_string()
                .ok_or_else(|| "split_topology must be a string".to_string())?
            {
                "Minimum-Phase" | "minimum-phase" => 0,
                "Linear-Phase" | "linear-phase" => 1,
                other => {
                    return Err(format!("Unknown De-Esser split topology: {other}"));
                }
            };
            if new_index != self.split_topology_index {
                return Err("split_topology is structural and requires a host rebuild".into());
            }
        } else if id == self.param_ms_mode {
            self.ms_mode = value
                .as_bool()
                .ok_or_else(|| "ms_mode must be a bool".to_string())?;
        } else if id == self.param_sidechain_external {
            let sidechain_external = value
                .as_bool()
                .ok_or_else(|| "sidechain_external must be a bool".to_string())?;
            if sidechain_external != self.sidechain_external {
                return Err("sidechain_external is structural and requires a host rebuild".into());
            }
        } else {
            return Err(format!("Unknown parameter: {id}"));
        }
        Ok(())
    }

    /// Calculates bounded, linked channel gains from the current detector frame.
    fn update_frame_gains(&mut self) {
        let range = self.range_smoother.advance();
        let link = self.link_smoother.advance();
        let mut maximum_reduction = 0.0_f32;
        for ch in 0..self.channels {
            let level = self.cores[ch].detect_level(0, self.sidechain_frame[ch]);
            let level_db = DB_CONVERSION_FACTOR * fast_log10(level.max(EPSILON));
            let reduction = self.cores[ch].calculate_gain_reduction(
                level_db,
                self.threshold,
                self.ratio,
                FIXED_KNEE_DB,
            );
            // Bound the envelope input and output: lowering Range must also
            // bound an envelope that is still releasing from a larger value.
            let reduction = self.cores[ch]
                .apply_envelope(0, reduction.min(range))
                .min(range);
            self.monitoring_gr[ch] = reduction;
            maximum_reduction = maximum_reduction.max(reduction);
        }
        for ch in 0..self.channels {
            // Link the smoothed reductions in dB. Applying the link after the
            // envelopes makes 100% linking identical across channels even if
            // their independent detector histories differ. All channels in a
            // multichannel instance share the strongest reduction.
            let independent = self.monitoring_gr[ch];
            let reduction = independent + link * (maximum_reduction - independent);
            self.monitoring_gr[ch] = reduction;
            // Use the precise exponential so the range cap and dB interpolation
            // are not biased by the fast approximation's gain error.
            self.frame_gains[ch] =
                (-reduction * std::f32::consts::LOG2_10 / DB_CONVERSION_FACTOR).exp2();
        }
    }

    /// Fetch one detector frame: the external key bus when enabled, else the
    /// program frame. The key region follows the program channels in every
    /// frame; program writes never touch it, but non-finite/denormal
    /// sanitization may normalize key samples (gate: "never writes the
    /// sidechain samples").
    fn fetch_detector_frame(&mut self, buffer: &[f32], frame_offset: usize) {
        if self.sidechain_external {
            let key_offset = frame_offset + self.channels;
            self.sidechain_frame[..self.channels]
                .copy_from_slice(&buffer[key_offset..key_offset + self.channels]);
        } else {
            self.sidechain_frame[..self.channels]
                .copy_from_slice(&buffer[frame_offset..frame_offset + self.channels]);
        }
    }

    /// Run the bandpass detector on the fetched frame and refresh gains.
    fn detect_frame_gains(&mut self) {
        self.hp_filters
            .process_interleaved_frame(&mut self.sidechain_frame[..self.channels]);
        self.lp_filters
            .process_interleaved_frame(&mut self.sidechain_frame[..self.channels]);
        self.update_frame_gains();
    }

    /// Delay the program working frame through the lookahead lines. Only
    /// called when the lookahead setting is positive; a zero lookahead
    /// bypasses the rings so the path stays bit-identical to legacy output.
    fn delay_work_frame(&mut self) {
        for ch in 0..self.channels {
            let input = self.work_frame[ch];
            self.work_frame[ch] = self.lookahead_buffers[ch].push(input);
        }
    }

    /// Shared stream processor for live input and zero-continuation drain.
    /// Keeps program width on output; program writes never touch the key
    /// region (non-finite sanitization below may still normalize it).
    fn process_stream(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        // Lifecycle guards first (gate precedent): the detector filters, FIR
        // bank and lookahead delay are only valid after `initialize` at the
        // matching rate. All guards precede any buffer mutation so rejected
        // host calls leave the buffer and DSP state untouched.
        if !self.initialized {
            return Err("De-Esser must be initialized before processing".into());
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "De-Esser process sample rate {} does not match initialized sample rate {}",
                context.sample_rate, self.sample_rate
            ));
        }
        let num_frames = context.num_frames;
        // With an external key bus each frame carries program channels
        // followed by key channels. A short buffer means the key is missing:
        // reject before any DSP state advances (no silent internal fallback).
        let stride = self.input_channels();
        let sample_len = num_frames
            .checked_mul(stride)
            .ok_or_else(|| "De-Esser block sample count overflow".to_string())?;
        if buffer.len() < sample_len {
            return Err(format!(
                "De-Esser buffer too small: need {sample_len} samples, got {}",
                buffer.len()
            ));
        }

        let use_lookahead = self.lookahead_ms > 0.0;
        let use_ms = self.use_ms();
        let use_fir = self.use_fir_split();
        if use_fir && self.fir_split.is_none() {
            return Err(
                "De-Esser linear-phase split bank is missing; rebuild the graph".to_string(),
            );
        }

        for sample in &mut buffer[..sample_len] {
            if !sample.is_finite() {
                *sample = 0.0;
            }
        }

        // Complete all frame/channel arithmetic and buffer validation before
        // touching DSP state. This keeps rejected host calls transactional.
        enable_ftz_daz();

        if self.mode_index == 0 {
            // ============================================================
            // Wideband mode
            // ============================================================
            for frame in 0..num_frames {
                let frame_offset = frame * stride;
                self.work_frame[..self.channels]
                    .copy_from_slice(&buffer[frame_offset..frame_offset + self.channels]);
                self.fetch_detector_frame(buffer, frame_offset);
                if use_ms {
                    Self::encode_ms(&mut self.work_frame);
                    Self::encode_ms(&mut self.sidechain_frame);
                }
                self.detect_frame_gains();
                if use_lookahead {
                    self.delay_work_frame();
                }

                // Advance mix smoother once per frame (not per channel) to avoid
                // block-constant mix that would cause zipper noise during automation.
                let mix = self.mix_smoother.advance();
                let dry_mix = 1.0 - mix;
                for gain in &mut self.frame_gains {
                    *gain = dry_mix + mix * *gain;
                }
                apply_per_channel_gain_simd(
                    &mut self.work_frame[..self.channels],
                    self.channels,
                    &self.frame_gains,
                );
                if use_ms {
                    Self::decode_ms(&mut self.work_frame);
                }
                buffer[frame_offset..frame_offset + self.channels]
                    .copy_from_slice(&self.work_frame[..self.channels]);
            }
        } else if use_fir {
            // ============================================================
            // Split-band mode, linear-phase FIR bank
            // ============================================================
            for frame in 0..num_frames {
                let frame_offset = frame * stride;
                self.work_frame[..self.channels]
                    .copy_from_slice(&buffer[frame_offset..frame_offset + self.channels]);
                self.fetch_detector_frame(buffer, frame_offset);
                if use_ms {
                    Self::encode_ms(&mut self.work_frame);
                    Self::encode_ms(&mut self.sidechain_frame);
                }
                self.detect_frame_gains();
                if use_lookahead {
                    self.delay_work_frame();
                }
                // Advance mix smoother once per frame (not per channel) to avoid
                // block-constant mix that would cause zipper noise during automation.
                let mix = self.mix_smoother.advance();
                let fir = self
                    .fir_split
                    .as_mut()
                    .expect("checked linear-phase split bank");
                for ch in 0..self.channels {
                    let (low, high) = fir.process_sample(self.work_frame[ch], ch);
                    let gain = self.frame_gains[ch];
                    // Low+high is the delayed dry reference. Mix controls only
                    // the reduction depth, so gain=1 yields the same delayed
                    // response for every Mix value.
                    self.work_frame[ch] = low + high * (1.0 + mix * (gain - 1.0));
                }
                if use_ms {
                    Self::decode_ms(&mut self.work_frame);
                }
                buffer[frame_offset..frame_offset + self.channels]
                    .copy_from_slice(&self.work_frame[..self.channels]);
            }
        } else {
            // ============================================================
            // Split-band mode, minimum-phase LR4 bank
            // ============================================================
            for frame in 0..num_frames {
                let frame_offset = frame * stride;
                self.work_frame[..self.channels]
                    .copy_from_slice(&buffer[frame_offset..frame_offset + self.channels]);
                self.fetch_detector_frame(buffer, frame_offset);
                if use_ms {
                    Self::encode_ms(&mut self.work_frame);
                    Self::encode_ms(&mut self.sidechain_frame);
                }
                self.detect_frame_gains();
                if use_lookahead {
                    self.delay_work_frame();
                }
                // Advance mix smoother once per frame (not per channel) to avoid
                // block-constant mix that would cause zipper noise during automation.
                let mix = self.mix_smoother.advance();
                for ch in 0..self.channels {
                    let input = self.work_frame[ch];

                    // Split into low and high bands
                    let (low, high) = self.crossovers[ch].process(input, 0);

                    let gain = self.frame_gains[ch];

                    // The LR4 low+high sum is the phase-matched dry reference.
                    // Mix controls only the reduction depth, so gain=1 yields
                    // the same all-pass response for every Mix value and cannot
                    // comb-filter a phase-rotated wet path against raw input.
                    self.work_frame[ch] = low + high * (1.0 + mix * (gain - 1.0));
                }
                if use_ms {
                    Self::decode_ms(&mut self.work_frame);
                }
                buffer[frame_offset..frame_offset + self.channels]
                    .copy_from_slice(&self.work_frame[..self.channels]);
            }
        }

        // Update diagnostic cache (throttled)
        self.cache_counter = self.cache_counter.saturating_add(num_frames);
        let cache_interval = (self.sample_rate as usize / 30).max(1);
        if self.cache_counter >= cache_interval {
            self.cache_counter %= cache_interval;
            self.cache.update(|d| {
                d.update(&self.monitoring_gr);
            });
        }

        flush_denormals_inplace(&mut buffer[..sample_len]);
        Ok(num_frames)
    }

    /// Encode a stereo working frame from Left/Right to Mid/Side.
    fn encode_ms(frame: &mut [f32]) {
        let l = frame[0];
        let r = frame[1];
        frame[0] = (l + r) * 0.5;
        frame[1] = (l - r) * 0.5;
    }

    /// Decode a stereo working frame from Mid/Side to Left/Right.
    fn decode_ms(frame: &mut [f32]) {
        let m = frame[0];
        let s = frame[1];
        frame[0] = m + s;
        frame[1] = m - s;
    }
}

impl ParametricInPlacePlugin for DeEsserPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("DeEsser", env!("CARGO_PKG_VERSION"), "SotF")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Dynamics
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        // Report coupling only for effective modes: link needs at least two
        // channels to mix, M/S needs a stereo instance, while an external key
        // bus always couples routing (doubled input width).
        let coupled = (self.stereo_link > 0.0 && self.channels > 1)
            || self.use_ms()
            || self.sidechain_external;
        PluginCompileMetadata::nonlinear(
            PluginCostClass::Dynamics,
            None,
            self.latency_samples(),
            coupled,
        )
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn input_channels(&self) -> usize {
        if self.sidechain_external {
            self.channels
                .checked_mul(2)
                .expect("validated external-sidechain channel count")
        } else {
            self.channels
        }
    }

    fn supports_bounded_subdivision(&self) -> bool {
        // All DSP state advances per sample with no block-size dependence;
        // only the throttled meter publication may follow subcalls.
        true
    }

    fn parameter_schema(&self) -> ParameterSchema {
        self.cached_parameters.clone()
    }

    fn parametric_get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "frequency" => Some(ParameterValue::Float(self.frequency)),
            "q" => Some(ParameterValue::Float(self.q)),
            "threshold" => Some(ParameterValue::Float(self.threshold)),
            "ratio" => Some(ParameterValue::Float(self.ratio)),
            "attack" => Some(ParameterValue::Float(self.attack_ms)),
            "release" => Some(ParameterValue::Float(self.release_ms)),
            "mode" => Some(ParameterValue::String(self.mode_string())),
            "mix" => Some(ParameterValue::Float(self.mix)),
            "range_db" => Some(ParameterValue::Float(self.range_db)),
            "stereo_link" => Some(ParameterValue::Float(self.stereo_link)),
            "lookahead_ms" => Some(ParameterValue::Float(self.lookahead_ms)),
            "split_topology" => Some(ParameterValue::String(self.split_topology_string())),
            "ms_mode" => Some(ParameterValue::Bool(self.ms_mode)),
            "sidechain_external" => Some(ParameterValue::Bool(self.sidechain_external)),
            _ => None,
        }
    }

    fn current_values(&self) -> ParameterSet {
        let mut values = ParameterSet::new();
        values.insert(
            self.param_frequency.clone(),
            ParameterValue::Float(self.frequency),
        );
        values.insert(self.param_q.clone(), ParameterValue::Float(self.q));
        values.insert(
            self.param_threshold.clone(),
            ParameterValue::Float(self.threshold),
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
        values.insert(
            self.param_mode.clone(),
            ParameterValue::String(self.mode_string()),
        );
        values.insert(self.param_mix.clone(), ParameterValue::Float(self.mix));
        values.insert(
            self.param_range_db.clone(),
            ParameterValue::Float(self.range_db),
        );
        values.insert(
            self.param_stereo_link.clone(),
            ParameterValue::Float(self.stereo_link),
        );
        values.insert(
            self.param_lookahead_ms.clone(),
            ParameterValue::Float(self.lookahead_ms),
        );
        values.insert(
            self.param_split_topology.clone(),
            ParameterValue::String(self.split_topology_string()),
        );
        values.insert(
            self.param_ms_mode.clone(),
            ParameterValue::Bool(self.ms_mode),
        );
        values.insert(
            self.param_sidechain_external.clone(),
            ParameterValue::Bool(self.sidechain_external),
        );
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() {
            return if values
                .iter()
                .all(|(id, value)| self.parametric_get_parameter(id).as_ref() == Some(value))
            {
                Ok(())
            } else {
                Err("De-Esser requires reset before changing parameters after drain".into())
            };
        }
        for (id, value) in values {
            self.apply_parameter(id, value)?;
        }
        Ok(())
    }

    fn parametric_validate_parameter(
        &self,
        id: &ParameterId,
        value: &ParameterValue,
    ) -> PluginResult<()> {
        self.cached_parameters
            .iter()
            .find(|parameter| &parameter.id == id)
            .ok_or_else(|| format!("Unknown parameter: {id}"))?
            .validate(value)
            .map_err(|error| format!("{id}: {error}"))
    }

    fn parametric_set_parameter(
        &mut self,
        id: ParameterId,
        value: ParameterValue,
    ) -> PluginResult<()> {
        self.parametric_validate_parameter(&id, &value)?;
        self.apply_parameter(id, value)
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("De-Esser sample rate must be greater than zero".to_string());
        }
        Self::validate_detection_band(self.frequency, self.q, sample_rate)?;
        // Size the drain scratch before mutating DSP state so a capacity
        // failure leaves the previous configuration untouched.
        let drain_samples = MAX_DRAIN_FRAMES
            .checked_mul(self.input_channels())
            .filter(|samples| *samples <= isize::MAX as usize / std::mem::size_of::<f32>())
            .ok_or_else(|| "De-Esser drain scratch capacity overflow".to_string())?;
        self.sample_rate = sample_rate;

        // Rebuild detection filters for new sample rate
        self.rebuild_detection_filters();

        // Reinit crossovers
        for xo in &mut self.crossovers {
            xo.reinit(self.frequency, sample_rate as f32, 1);
        }
        self.rebuild_fir_split();

        // Reinit dynamics cores
        for core in &mut self.cores {
            core.initialize(sample_rate);
            core.set_attack_release(self.attack_ms, self.release_ms);
        }

        // Resize lookahead delay lines for the new rate, then apply delay.
        let max_samples = (MAX_LOOKAHEAD_MS * 0.001 * sample_rate as f32).round() as usize;
        for buf in &mut self.lookahead_buffers {
            buf.resize(max_samples, 1);
        }
        self.update_lookahead_delay();

        // Reset smoother
        self.mix_smoother.set_time(5.0, sample_rate);
        self.range_smoother.set_time(5.0, sample_rate);
        self.link_smoother.set_time(5.0, sample_rate);

        self.drain_scratch.resize(drain_samples, 0.0);
        self.has_input = false;
        self.drain_remaining = None;
        self.initialized = true;

        Ok(())
    }

    fn reset(&mut self) {
        self.hp_filters.reset();
        self.lp_filters.reset();

        // Reset crossovers
        for xo in &mut self.crossovers {
            xo.reset();
        }
        if let Some(fir) = self.fir_split.as_mut() {
            fir.reset();
        }

        // Reset dynamics cores
        for core in &mut self.cores {
            core.reset();
        }

        // Reset lookahead delay lines
        for buf in &mut self.lookahead_buffers {
            buf.reset();
        }

        self.has_input = false;
        self.drain_remaining = None;
        self.drain_scratch.fill(0.0);
        self.monitoring_gr.fill(0.0);
        self.cache_counter = 0;
        self.mix_smoother.reset(self.mix);
        self.range_smoother.reset(self.range_db);
        self.link_smoother.reset(self.stereo_link);
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        if context.num_frames > 0 && self.drain_remaining.is_some() {
            return Err("De-Esser requires reset before processing input after drain".into());
        }
        let frames = self.process_stream(buffer, context)?;
        self.has_input |= frames > 0;
        Ok(frames)
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }

    fn latency_samples(&self) -> usize {
        self.lookahead_delay_samples() + self.fir_group_delay_samples()
    }

    fn tail_length(&self) -> TailLength {
        if !self.initialized {
            return TailLength::Unknown;
        }
        if self.mode_index == 1 && self.split_topology_index == 0 {
            // LR4 recurrence needs a separate truncation/settled-state policy
            // (crossover-LR precedent); delay lines still drain exactly.
            return TailLength::Unknown;
        }
        TailLength::Finite(self.retained_frames() as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.retained_frames().min(MAX_DRAIN_FRAMES)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.initialized {
            return None;
        }
        let remaining = if self.has_input {
            self.drain_remaining
                .unwrap_or_else(|| self.retained_frames())
        } else {
            0
        };
        // Every successful full-capacity call consumes up to 256 retained
        // frames, with completion on the final output call. Empty state still
        // needs one successful terminal call.
        std::num::NonZeroU64::new(remaining.div_ceil(MAX_DRAIN_FRAMES).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.initialized || context.sample_rate != self.sample_rate {
            return Err("De-Esser requires initialization at the drain sample rate".into());
        }
        if self.channels == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !output.len().is_multiple_of(self.channels) {
            return Err("De-Esser drain output must contain whole program-channel frames".into());
        }
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let remaining = self
            .drain_remaining
            .unwrap_or_else(|| self.retained_frames());
        if remaining == 0 {
            self.drain_remaining = Some(0);
            return Ok(PluginDrainResult::COMPLETE);
        }
        let frames = (output.len() / self.channels)
            .min(remaining)
            .min(MAX_DRAIN_FRAMES);
        if frames == 0 {
            return Err("De-Esser drain needs at least one output frame".into());
        }
        // Capacity/rate checks precede EOS and output mutation. Taking the
        // prepared scratch temporarily avoids aliasing it with the DSP state.
        let stride = self.input_channels();
        let mut scratch = std::mem::take(&mut self.drain_scratch);
        let samples = frames
            .checked_mul(stride)
            .ok_or_else(|| "De-Esser drain sample count overflow".to_string())?;
        if scratch.len() < samples {
            self.drain_scratch = scratch;
            return Err("De-Esser drain scratch too small; reinitialize the plugin".to_string());
        }
        scratch[..samples].fill(0.0);
        let mut drain_context = *context;
        drain_context.num_frames = frames;
        let processed = self.process_stream(&mut scratch[..samples], &drain_context);
        if processed.is_ok() {
            for frame in 0..frames {
                let source = frame * stride;
                let destination = frame * self.channels;
                output[destination..destination + self.channels]
                    .copy_from_slice(&scratch[source..source + self.channels]);
            }
        }
        self.drain_scratch = scratch;
        processed?;
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }
}
