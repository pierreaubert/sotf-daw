// Rust guideline compliant 2026-02-21
pub use super::config::*;
use super::delay_line::DelayLine;
use super::factory::{build_path_from_config, build_path_from_config_with_factory};
use super::types::ABCompareData;
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_gain::{AutoGain, AutoGainLoudnessType, AutoGainParams};
use sotf_host::host::DawHost;
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{Parameter, ParameterId, ParameterImportance, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext,
};
use sotf_host::smoothing::Smoother;
use std::any::Any;
use std::sync::Arc;

pub(super) struct TransitionSmoothers {
    pub(super) mix: Smoother,
    pub(super) bypass: Smoother,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainLifecycle {
    Accepting,
    Draining,
    Complete,
    ResetRequired,
}

#[derive(Debug, Default, Clone, Copy)]
struct ChildDrainProgress {
    queued_frames: usize,
    consumed_frames: usize,
    complete: bool,
}

impl ChildDrainProgress {
    fn queued_remaining(self) -> usize {
        debug_assert!(self.consumed_frames <= self.queued_frames);
        self.queued_frames - self.consumed_frames
    }
}

/// A/B Comparison Plugin
///
/// Allows fair comparison between two audio processing chains with automatic
/// loudness matching. Each path (A or B) can be a single plugin, a rack
/// (linear chain), or a full graph.
pub struct ABComparePlugin {
    // Configuration
    pub(super) num_channels: usize,
    pub(super) sample_rate: u32,

    /// External plugin factory -- when set, supports all plugin types.
    /// Falls back to the built-in limited factory when None.
    pub(super) plugin_factory: Option<sotf_host::PluginFactoryFn>,

    // Processing paths - use DawHost for flexibility
    pub(super) host_a: DawHost,
    pub(super) host_b: DawHost,

    // Path configurations (stored for runtime changes)
    pub(super) path_a_config: PathConfig,
    pub(super) path_b_config: PathConfig,

    // Auto-gain for matching B to A's loudness
    // Uses A's output as "input reference" and B's output as "output to compensate"
    // Also provides loudness and peak data for both paths
    pub(super) auto_gain: AutoGain,

    // State
    pub(super) mix_mode: MixMode,
    pub(super) mix: f32,
    /// Crossfades path selection and latency-aligned bypass without expanding
    /// the already-large outer plugin state.
    pub(super) transition_smoothers: TransitionSmoothers,
    pub(super) selected_path: i32,
    pub(super) bypass: bool,
    pub(super) mix_transition_ms: f32,

    // Phase inversion
    pub(super) phase_invert: [bool; 2],

    // Difference mode (A - B)
    pub(super) difference_mode: bool,

    // Latency compensation delay lines
    pub(super) delay_a: DelayLine,
    pub(super) delay_b: DelayLine,
    /// Dry delay keeps bypass aligned with the latency reported to the host.
    pub(super) delay_dry: DelayLine,

    // Internal buffers
    pub(super) buffers: [Vec<f32>; 2],
    /// One bounded child-drain result per path; each buffer holds one declared
    /// `DawHost::drain_output_frames_max()` result.
    drain_buffers: [Vec<f32>; 2],
    drain_children: [ChildDrainProgress; 2],
    drain_delay_remaining: [usize; 3],
    drain_lifecycle: DrainLifecycle,
    band_mask_used_since_reset: bool,

    // Band mask (bandpass filter for isolating frequency range in comparison)
    pub(super) band_mask_low_hz: f32,
    pub(super) band_mask_high_hz: f32,
    /// Per-channel highpass filters (one per channel) for band mask low cutoff
    pub(super) band_mask_hp: Vec<Biquad>,
    /// Per-channel lowpass filters (one per channel) for band mask high cutoff
    pub(super) band_mask_lp: Vec<Biquad>,

    // Cached peak values
    pub(super) last_peaks: [f64; 2],
    /// Cached unity-preserving gain for the empty-path fast path.
    pub(super) empty_path_fast_gain: f32,

    pub(super) cache: RealTimeCache<ABCompareData>,
    /// Frames elapsed since the last loudness target and diagnostic update.
    pub(super) cache_update_counter: usize,
    pub(super) cached_parameters: Vec<Parameter>,
}

impl ABComparePlugin {
    const MAX_REALTIME_FRAMES: usize = 48_000;

    fn validate_params(num_channels: usize, params: &ABComparePluginParams) -> Result<(), String> {
        if num_channels == 0 {
            return Err("A/B Compare requires at least one channel".into());
        }
        fn finite_range(name: &str, value: f32, min: f32, max: f32) -> Result<(), String> {
            if value.is_finite() && (min..=max).contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "{name} must be finite and in {min}..={max}, got {value}"
                ))
            }
        }
        finite_range("mix", params.mix, -1.0, 1.0)?;
        if !(0..=1).contains(&params.selected_path) {
            return Err(format!(
                "selected_path must be 0 or 1, got {}",
                params.selected_path
            ));
        }
        finite_range("gain_smoothing_ms", params.gain_smoothing_ms, 1.0, 500.0)?;
        finite_range("max_auto_gain_db", params.max_auto_gain_db, 0.0, 24.0)?;
        finite_range("mix_transition_ms", params.mix_transition_ms, 1.0, 500.0)?;
        finite_range("band_mask_low_hz", params.band_mask_low_hz, 20.0, 20_000.0)?;
        finite_range(
            "band_mask_high_hz",
            params.band_mask_high_hz,
            20.0,
            20_000.0,
        )?;
        if params.band_mask_low_hz >= params.band_mask_high_hz {
            return Err("band mask low cutoff must be below high cutoff".into());
        }
        Ok(())
    }

    fn validate_for_sample_rate(
        params: &ABComparePluginParams,
        sample_rate: u32,
    ) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("A/B Compare sample rate must be greater than zero".into());
        }
        let nyquist = sample_rate as f32 * 0.5;
        if params.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            && params.band_mask_low_hz >= nyquist
        {
            return Err(format!(
                "band_mask_low_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        if params.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
            && params.band_mask_high_hz >= nyquist
        {
            return Err(format!(
                "band_mask_high_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        Ok(())
    }

    /// Create a new A/B Compare plugin with default settings
    pub fn new(num_channels: usize) -> Result<Self, String> {
        Self::from_params(num_channels, ABComparePluginParams::default())
    }

    /// Set the external plugin factory, enabling all plugin types in sub-racks.
    /// Call this after construction but before initialize() or processing.
    pub fn set_plugin_factory(&mut self, factory: sotf_host::PluginFactoryFn) {
        self.plugin_factory = Some(factory);
    }

    /// Create from parameters
    pub fn from_params(num_channels: usize, params: ABComparePluginParams) -> Result<Self, String> {
        Self::from_params_internal(num_channels, 48_000, params, None)
    }

    /// Construct initial paths with the authoritative factory already installed.
    pub fn from_params_with_factory(
        num_channels: usize,
        sample_rate: u32,
        params: ABComparePluginParams,
        factory: sotf_host::PluginFactoryFn,
    ) -> Result<Self, String> {
        Self::from_params_internal(num_channels, sample_rate, params, Some(factory))
    }

    fn from_params_internal(
        num_channels: usize,
        sample_rate: u32,
        params: ABComparePluginParams,
        factory: Option<sotf_host::PluginFactoryFn>,
    ) -> Result<Self, String> {
        Self::validate_params(num_channels, &params)?;
        Self::validate_for_sample_rate(&params, sample_rate)?;

        let host_a = if factory.is_some() {
            build_path_from_config_with_factory(&params.path_a, num_channels, sample_rate, factory)?
        } else {
            build_path_from_config(&params.path_a, num_channels, sample_rate)?
        };
        let host_b = if factory.is_some() {
            build_path_from_config_with_factory(&params.path_b, num_channels, sample_rate, factory)?
        } else {
            build_path_from_config(&params.path_b, num_channels, sample_rate)?
        };

        // Create AutoGain for matching B's loudness to A's loudness
        // A's output is the "input reference", B's output is "what to compensate"
        let auto_gain_params = AutoGainParams {
            enabled: params.auto_gain_enabled,
            loudness_type: params.loudness_type,
            max_gain_db: params.max_auto_gain_db,
            smoothing_ms: params.gain_smoothing_ms,
        };
        let auto_gain = AutoGain::new(num_channels, sample_rate, auto_gain_params)?;

        let mix_smoother = Smoother::new(params.mix, params.mix_transition_ms, sample_rate);
        let bypass_value = if params.bypass { 1.0 } else { 0.0 };
        let bypass_smoother = Smoother::new(bypass_value, params.mix_transition_ms, sample_rate);

        let band_mask_low_hz = params.band_mask_low_hz.clamp(20.0, 20000.0);
        let band_mask_high_hz = params.band_mask_high_hz.clamp(20.0, 20000.0);
        let q = 1.0 / std::f64::consts::SQRT_2;
        let band_mask_hp: Vec<Biquad> = (0..num_channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Highpass,
                    band_mask_low_hz as f64,
                    sample_rate as f64,
                    q,
                    0.0,
                )
            })
            .collect();
        let band_mask_lp: Vec<Biquad> = (0..num_channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Lowpass,
                    band_mask_high_hz as f64,
                    sample_rate as f64,
                    q,
                    0.0,
                )
            })
            .collect();

        let mut p = Self {
            num_channels,
            sample_rate,
            plugin_factory: factory,
            host_a,
            host_b,
            path_a_config: params.path_a,
            path_b_config: params.path_b,
            auto_gain,
            mix_mode: params.mix_mode,
            mix: params.mix,
            transition_smoothers: TransitionSmoothers {
                mix: mix_smoother,
                bypass: bypass_smoother,
            },
            selected_path: params.selected_path,
            bypass: params.bypass,
            mix_transition_ms: params.mix_transition_ms,
            phase_invert: [params.phase_invert_a, params.phase_invert_b],
            difference_mode: params.difference_mode,
            band_mask_low_hz,
            band_mask_high_hz,
            band_mask_hp,
            band_mask_lp,
            delay_a: DelayLine::new(),
            delay_b: DelayLine::new(),
            delay_dry: DelayLine::new(),
            buffers: [
                vec![0.0; Self::MAX_REALTIME_FRAMES * num_channels],
                vec![0.0; Self::MAX_REALTIME_FRAMES * num_channels],
            ],
            drain_buffers: [Vec::new(), Vec::new()],
            drain_children: [ChildDrainProgress::default(); 2],
            drain_delay_remaining: [0; 3],
            drain_lifecycle: DrainLifecycle::Accepting,
            band_mask_used_since_reset: false,
            last_peaks: [0.0; 2],
            empty_path_fast_gain: 0.0,
            cache: RealTimeCache::new(ABCompareData::default()),
            cache_update_counter: 0,
            cached_parameters: Vec::new(),
        };
        p.recompute_empty_path_fast_gain();
        p.rebuild_cached_parameters();
        Ok(p)
    }

    /// Identical paths must remain at unity for every crossfade position.
    pub(super) fn recompute_empty_path_fast_gain(&mut self) {
        self.empty_path_fast_gain = 1.0;
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.cached_parameters = vec![
            Parameter::new_float("mix", "A/B Mix", self.mix, -1.0, 1.0)
                .with_description("Mix between A and B: -1.0 = A, 0.0 = 50/50, +1.0 = B")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_int(
                "mix_mode",
                "Mix Mode",
                match self.mix_mode {
                    MixMode::Potentiometer => 0,
                    MixMode::Binary => 1,
                },
                0,
                1,
            )
            .with_description("0 = Potentiometer (continuous), 1 = Binary (A/B switch)")
            .with_group("Mix Control")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_int("selected_path", "Selected Path", self.selected_path, 0, 1)
                .with_description("0 = A, 1 = B (only used in binary mode)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_bool("bypass", "Bypass", self.bypass)
                .with_description("Bypass A/B processing, output original input")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Critical),
            Parameter::new_bool(
                "auto_gain_enabled",
                "Auto Gain",
                self.auto_gain.is_enabled(),
            )
            .with_description("Automatically match loudness between A and B")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_int(
                "loudness_type",
                "Loudness Type",
                match self.auto_gain.loudness_type() {
                    AutoGainLoudnessType::Momentary => 0,
                    AutoGainLoudnessType::ShortTerm => 1,
                },
                0,
                1,
            )
            .with_description("0 = Momentary (400ms), 1 = Short-term (3s)")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "max_auto_gain_db",
                "Max Auto Gain",
                self.auto_gain.max_gain_db(),
                0.0,
                24.0,
            )
            .with_description("Maximum loudness correction in dB")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_float(
                "gain_smoothing_ms",
                "Gain Smoothing",
                self.auto_gain.smoothing_ms(),
                1.0,
                500.0,
            )
            .with_description("Auto-gain smoothing time in milliseconds")
            .with_group("Loudness Matching")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_float(
                "mix_transition_ms",
                "Mix Transition",
                self.mix_transition_ms,
                1.0,
                500.0,
            )
            .with_description("A/B transition smoothing time in milliseconds")
            .with_group("Timing")
            .with_importance(ParameterImportance::FineTuning),
            Parameter::new_bool("phase_invert_a", "Phase Invert A", self.phase_invert[0])
                .with_description("Invert phase of path A output (multiply by -1.0)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("phase_invert_b", "Phase Invert B", self.phase_invert[1])
                .with_description("Invert phase of path B output (multiply by -1.0)")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_bool("difference_mode", "Difference Mode", self.difference_mode)
                .with_description("Output A - B instead of crossfade mix")
                .with_group("Mix Control")
                .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "band_mask_low_hz",
                "Band Mask Low",
                self.band_mask_low_hz,
                20.0,
                20000.0,
            )
            .with_description("Highpass cutoff for band-masking the comparison output (Hz)")
            .with_group("Band Mask")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_float(
                "band_mask_high_hz",
                "Band Mask High",
                self.band_mask_high_hz,
                20.0,
                20000.0,
            )
            .with_description("Lowpass cutoff for band-masking the comparison output (Hz)")
            .with_group("Band Mask")
            .with_importance(ParameterImportance::Useful),
            Parameter::new_string(
                "path_a_config",
                "Path A Config",
                serde_json::to_string(&self.path_a_config)
                    .unwrap_or_else(|_| r#"{"type":"None"}"#.to_string()),
            )
            .with_description("JSON configuration for path A")
            .with_group("Configuration")
            .with_importance(ParameterImportance::Critical),
            Parameter::new_string(
                "path_b_config",
                "Path B Config",
                serde_json::to_string(&self.path_b_config)
                    .unwrap_or_else(|_| r#"{"type":"None"}"#.to_string()),
            )
            .with_description("JSON configuration for path B")
            .with_group("Configuration")
            .with_importance(ParameterImportance::Critical),
        ];
        sotf_host::param_bridge::apply_spec_update_modes(
            &mut self.cached_parameters,
            crate::params::PARAMS,
        );
    }

    #[cfg(test)]
    pub(super) fn rebuild_path_a(&mut self) -> Result<(), String> {
        self.host_a = build_path_from_config_with_factory(
            &self.path_a_config,
            self.num_channels,
            self.sample_rate,
            self.plugin_factory,
        )?;
        self.update_latency_compensation()
    }

    #[cfg(test)]
    pub(super) fn rebuild_path_b(&mut self) -> Result<(), String> {
        self.host_b = build_path_from_config_with_factory(
            &self.path_b_config,
            self.num_channels,
            self.sample_rate,
            self.plugin_factory,
        )?;
        self.update_latency_compensation()
    }

    /// Minimum audible frequency (Hz). Band mask low values at or below this
    /// are treated as "no highpass filtering".
    pub(super) const BAND_MASK_MIN_HZ: f32 = 20.0;

    /// Maximum audible frequency (Hz). Band mask high values at or above this
    /// are treated as "no lowpass filtering".
    pub(super) const BAND_MASK_MAX_HZ: f32 = 20000.0;

    /// Half-step epsilon (Hz) used when comparing band mask edges to the
    /// parameter limits. A value equal to the parameter minimum/maximum
    /// means "full range" — we accept anything within 0.5 Hz of those limits
    /// so that floating-point serialise/deserialise round-trips (e.g. JSON)
    /// cannot accidentally activate the filter chain.
    pub(super) const BAND_MASK_EDGE_EPSILON: f32 = 0.5;

    /// Returns true if the band mask range is narrower than the full audible
    /// spectrum, i.e. if the biquad filter pair should be applied.
    pub(super) fn band_mask_active(&self) -> bool {
        self.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            || self.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
    }

    #[allow(dead_code)]
    pub(super) fn has_empty_paths(&self) -> bool {
        matches!(self.path_a_config, PathConfig::None)
            && matches!(self.path_b_config, PathConfig::None)
    }

    #[allow(dead_code)]
    pub(super) fn can_use_empty_path_fast_path(&self) -> bool {
        self.has_empty_paths()
            && self.mix_mode == MixMode::Potentiometer
            && !self.phase_invert[0]
            && !self.phase_invert[1]
            && !self.difference_mode
            && !self.band_mask_active()
            && self.auto_gain.is_unity_gain_stable()
            && (self.transition_smoothers.mix.current() - self.transition_smoothers.mix.target())
                .abs()
                < 1e-5
    }

    #[allow(dead_code)]
    pub(super) fn process_empty_path_fast(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        num_frames: usize,
    ) -> Result<(), String> {
        let mut frame = 0;
        while frame < num_frames {
            let count = self.frames_until_measurement().min(num_frames - frame);
            let samples = &input[frame * self.num_channels..(frame + count) * self.num_channels];
            if self.auto_gain.is_enabled() {
                self.auto_gain.ingest_input(samples)?;
                self.auto_gain.ingest_output(samples)?;
            }
            self.auto_gain.next_n(count);
            self.finish_measurement_segment(count);
            frame += count;
        }

        let gain = self.empty_path_fast_gain;

        if (gain - 1.0).abs() < 1e-6 {
            output.copy_from_slice(input);
        } else {
            for (out, &sample) in output.iter_mut().zip(input.iter()) {
                *out = sample * gain;
            }
        }

        Ok(())
    }

    fn frames_until_measurement(&self) -> usize {
        // Refresh at 20 Hz on the sample clock, independently of callback size.
        let interval = (self.sample_rate as usize / 20).max(1);
        interval - self.cache_update_counter
    }

    fn finish_measurement_segment(&mut self, frames: usize) {
        debug_assert!(frames <= self.frames_until_measurement());
        self.cache_update_counter += frames;
        if self.frames_until_measurement() != 0 {
            return;
        }
        self.cache_update_counter = 0;
        // The segment has already been rendered with its previous gain target.
        // These statistics may change only the samples following this boundary.
        if self.auto_gain.is_enabled() {
            self.auto_gain.refresh_input_measurement();
            self.auto_gain.refresh_output_measurement();
            self.last_peaks[0] = self.auto_gain.last_input_peak();
            self.last_peaks[1] = self.auto_gain.last_output_peak();
        }
        let data = ABCompareData {
            loudness_a_lufs: self.auto_gain.last_input_lufs(),
            loudness_b_lufs: self.auto_gain.last_output_lufs(),
            auto_gain_db: self.auto_gain.current_gain_db(),
            peak_a: self.last_peaks[0],
            peak_b: self.last_peaks[1],
            current_mix: self.transition_smoothers.mix.current(),
            bypass_active: self.bypass,
        };
        self.cache.update(|d| *d = data);
    }

    /// Rebuild the bandpass filter pair for the current band mask settings.
    pub(super) fn rebuild_band_mask_filters(&mut self) {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let sr = self.sample_rate as f64;
        if self.band_mask_hp.len() == self.num_channels {
            // Update coefficients in place — preserves filter delay state (click-free)
            for f in &mut self.band_mask_hp {
                f.update_params(
                    BiquadFilterType::Highpass,
                    self.band_mask_low_hz as f64,
                    sr,
                    q,
                    0.0,
                );
            }
            for f in &mut self.band_mask_lp {
                f.update_params(
                    BiquadFilterType::Lowpass,
                    self.band_mask_high_hz as f64,
                    sr,
                    q,
                    0.0,
                );
            }
        } else {
            // First time: create filters from scratch
            self.band_mask_hp = (0..self.num_channels)
                .map(|_| {
                    Biquad::new(
                        BiquadFilterType::Highpass,
                        self.band_mask_low_hz as f64,
                        sr,
                        q,
                        0.0,
                    )
                })
                .collect();
            self.band_mask_lp = (0..self.num_channels)
                .map(|_| {
                    Biquad::new(
                        BiquadFilterType::Lowpass,
                        self.band_mask_high_hz as f64,
                        sr,
                        q,
                        0.0,
                    )
                })
                .collect();
        }
    }

    fn reset_band_mask_filter_state(&mut self) {
        let q = 1.0 / std::f64::consts::SQRT_2;
        let sample_rate = self.sample_rate as f64;
        for filter in &mut self.band_mask_hp {
            *filter = Biquad::new(
                BiquadFilterType::Highpass,
                self.band_mask_low_hz as f64,
                sample_rate,
                q,
                0.0,
            );
        }
        for filter in &mut self.band_mask_lp {
            *filter = Biquad::new(
                BiquadFilterType::Lowpass,
                self.band_mask_high_hz as f64,
                sample_rate,
                q,
                0.0,
            );
        }
    }

    /// Align both paths by delaying the shorter one.
    ///
    /// Returns an error if either host fails to build (which would make latency
    /// queries unreliable and lead to silent phase misalignment). On error,
    /// both delay lines are set to zero so the plugin stays audible while
    /// latency compensation is disabled.
    pub(super) fn update_latency_compensation(&mut self) -> Result<(), String> {
        // Build both hosts so that `total_latency_samples()` reflects the
        // current graph topology. Ignore errors separately so we can report
        // both failures in one message if necessary.
        let err_a = self.host_a.build().err();
        let err_b = self.host_b.build().err();
        if err_a.is_some() || err_b.is_some() {
            // Disable compensation: set both delays to zero so the plugin
            // remains audible rather than silently misaligning the paths.
            self.delay_a.set_delay(0, self.num_channels);
            self.delay_b.set_delay(0, self.num_channels);
            self.delay_dry.set_delay(0, self.num_channels);
            let msg = match (err_a, err_b) {
                (Some(a), Some(b)) => format!(
                    "Latency compensation disabled: host_a build error: {a}; host_b build error: {b}"
                ),
                (Some(a), None) => {
                    format!("Latency compensation disabled: host_a build error: {a}")
                }
                (None, Some(b)) => {
                    format!("Latency compensation disabled: host_b build error: {b}")
                }
                (None, None) => unreachable!(),
            };
            return Err(msg);
        }
        let lat_a = self.host_a.total_latency_samples();
        let lat_b = self.host_b.total_latency_samples();
        if lat_a > lat_b {
            self.delay_a.set_delay(0, self.num_channels);
            self.delay_b.set_delay(lat_a - lat_b, self.num_channels);
        } else {
            self.delay_a.set_delay(lat_b - lat_a, self.num_channels);
            self.delay_b.set_delay(0, self.num_channels);
        }
        self.delay_dry
            .set_delay(lat_a.max(lat_b), self.num_channels);
        Ok(())
    }

    fn mix_prepared_buffers(
        &mut self,
        output_with_dry: &mut [f32],
        num_frames: usize,
    ) -> PluginResult<()> {
        let expected_samples = num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare block sample count overflow".to_string())?;
        if expected_samples > self.buffers[0].len()
            || expected_samples > self.buffers[1].len()
            || output_with_dry.len() != expected_samples
        {
            return Err("A/B Compare mixer received inconsistent frame geometry".into());
        }

        let sign_a: f32 = if self.phase_invert[0] { -1.0 } else { 1.0 };
        let sign_b: f32 = if self.phase_invert[1] { -1.0 } else { 1.0 };
        let band_mask_active = self.band_mask_active();

        let target_mix = match self.mix_mode {
            MixMode::Potentiometer => self.mix,
            MixMode::Binary => {
                if self.selected_path == 0 {
                    -1.0
                } else {
                    1.0
                }
            }
        };
        if (self.transition_smoothers.mix.target() - target_mix).abs() > f32::EPSILON {
            self.transition_smoothers.mix.set_target(target_mix);
            self.recompute_empty_path_fast_gain();
        }

        let mut segment_start = 0;
        while segment_start < num_frames {
            let count = self
                .frames_until_measurement()
                .min(num_frames - segment_start);
            let segment_end = segment_start + count;
            if self.auto_gain.is_enabled() {
                let samples = segment_start * self.num_channels..segment_end * self.num_channels;
                self.auto_gain
                    .ingest_input(&self.buffers[0][samples.clone()])?;
                self.auto_gain.ingest_output(&self.buffers[1][samples])?;
            }
            for frame in segment_start..segment_end {
                let gain_linear = self.auto_gain.next_gain_linear();
                let current_mix = self.transition_smoothers.mix.advance();
                let bypass_mix = self.transition_smoothers.bypass.advance();

                for ch in 0..self.num_channels {
                    let idx = frame * self.num_channels + ch;
                    let dry_sample = output_with_dry[idx];
                    let sample_a = self.buffers[0][idx] * sign_a;
                    let sample_b = self.buffers[1][idx] * gain_linear * sign_b;

                    let mut wet_sample = if self.difference_mode {
                        sample_a - sample_b
                    } else {
                        let mix_01 = (current_mix + 1.0) / 2.0;
                        let gain_a = 1.0 - mix_01;
                        let gain_b = mix_01;
                        sample_a * gain_a + sample_b * gain_b
                    };

                    if band_mask_active {
                        wet_sample = self.band_mask_hp[ch].process(wet_sample as f64) as f32;
                        wet_sample = self.band_mask_lp[ch].process(wet_sample as f64) as f32;
                    }
                    output_with_dry[idx] =
                        wet_sample * (1.0 - bypass_mix) + dry_sample * bypass_mix;
                }
            }
            self.finish_measurement_segment(count);
            segment_start = segment_end;
        }

        Ok(())
    }

    fn pump_child_drain(&mut self, child_index: usize) -> PluginResult<()> {
        if child_index >= self.drain_children.len() {
            return Err("A/B Compare child drain index is out of range".into());
        }
        if self.drain_children[child_index].complete
            || self.drain_children[child_index].queued_remaining() > 0
        {
            return Ok(());
        }

        self.drain_buffers[child_index].fill(0.0);
        let result = if child_index == 0 {
            self.host_a.drain(&mut self.drain_buffers[child_index])
        } else {
            self.host_b.drain(&mut self.drain_buffers[child_index])
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(format!(
                    "A/B Compare child {child_index} drain failed: {error}"
                ));
            }
        };
        let Some(sample_count) = result.frames.checked_mul(self.num_channels) else {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare child drain sample count overflow".into());
        };
        if sample_count > self.drain_buffers[child_index].len() {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err("A/B Compare child exceeded prepared drain capacity".into());
        }
        self.drain_children[child_index] = ChildDrainProgress {
            queued_frames: result.frames,
            consumed_frames: 0,
            complete: result.complete,
        };
        Ok(())
    }

    fn drain_complete(&self) -> bool {
        self.drain_children
            .iter()
            .all(|child| child.complete && child.queued_remaining() == 0)
            && self.drain_delay_remaining.iter().all(|&frames| frames == 0)
    }

    fn drain_capacity_frames(&self) -> usize {
        self.host_a
            .drain_output_frames_max()
            .max(self.host_b.drain_output_frames_max())
            .max(self.delay_a.delay_frames(self.num_channels))
            .max(self.delay_b.delay_frames(self.num_channels))
            .max(self.delay_dry.delay_frames(self.num_channels))
            .min(Self::MAX_REALTIME_FRAMES)
    }

    fn validate_drain_host(
        &self,
        host: &DawHost,
        drain_buffer: &[f32],
        path_name: &str,
    ) -> PluginResult<()> {
        if host.input_channels() != self.num_channels || host.output_channels() != self.num_channels
        {
            return Err(format!(
                "A/B Compare {path_name} drain requires {} input/output channels, got {}/{}",
                self.num_channels,
                host.input_channels(),
                host.output_channels()
            ));
        }
        if host.output_sample_rate(self.sample_rate) != self.sample_rate {
            return Err(format!(
                "A/B Compare {path_name} drain requires a same-rate child path"
            ));
        }
        if !host.has_identity_frame_geometry() {
            return Err(format!(
                "A/B Compare {path_name} drain requires every active child node to declare identity frame geometry"
            ));
        }
        let required_samples = host
            .drain_output_frames_max()
            .checked_mul(self.num_channels)
            .ok_or_else(|| format!("A/B Compare {path_name} drain capacity overflow"))?;
        if required_samples > drain_buffer.len() {
            return Err(format!(
                "A/B Compare {path_name} drain staging buffer is smaller than its prepared capacity"
            ));
        }
        Ok(())
    }

    fn validate_drain_path_topology(path_config: &PathConfig, path_name: &str) -> PluginResult<()> {
        let PathConfig::Graph { nodes, edges } = path_config else {
            return Ok(());
        };
        if nodes.is_empty() {
            return if edges.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "A/B Compare {path_name} drain graph has edges but no nodes"
                ))
            };
        }
        if edges.len() != nodes.len().saturating_sub(1) {
            return Err(format!(
                "A/B Compare {path_name} drain supports only linear Graph paths"
            ));
        }
        for edge in edges {
            if edge.channel_map.is_some()
                || edge.destination_offset != 0
                || !nodes.iter().any(|node| node.id == edge.from)
                || !nodes.iter().any(|node| node.id == edge.to)
            {
                return Err(format!(
                    "A/B Compare {path_name} drain supports only unmodified linear Graph edges"
                ));
            }
        }
        for node in nodes {
            let incoming = edges.iter().filter(|edge| edge.to == node.id).count();
            let outgoing = edges.iter().filter(|edge| edge.from == node.id).count();
            if incoming > 1 || outgoing > 1 {
                return Err(format!(
                    "A/B Compare {path_name} drain supports only linear Graph paths"
                ));
            }
        }

        let mut roots = nodes
            .iter()
            .filter(|node| !edges.iter().any(|edge| edge.to == node.id));
        let Some(root) = roots.next() else {
            return Err(format!(
                "A/B Compare {path_name} drain Graph has no linear source"
            ));
        };
        if roots.next().is_some() {
            return Err(format!(
                "A/B Compare {path_name} drain supports only linear Graph paths"
            ));
        }

        let mut current_id = root.id.as_str();
        let mut visited_nodes = 0_usize;
        loop {
            visited_nodes += 1;
            if visited_nodes > nodes.len() {
                return Err(format!(
                    "A/B Compare {path_name} drain Graph contains a cycle"
                ));
            }
            let Some(edge) = edges.iter().find(|edge| edge.from == current_id) else {
                break;
            };
            current_id = &edge.to;
        }
        if visited_nodes != nodes.len() {
            return Err(format!(
                "A/B Compare {path_name} drain supports only connected linear Graph paths"
            ));
        }
        Ok(())
    }
}

impl Plugin for ABComparePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("A/B Compare", env!("CARGO_PKG_VERSION"), "SotF")
            .with_description("A/B comparison with automatic loudness matching")
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }

    fn output_channels(&self) -> usize {
        self.num_channels
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::boundary(PluginCostClass::External, self.latency_samples())
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    fn validate_parameter(&self, id: &ParameterId, value: &ParameterValue) -> PluginResult<()> {
        if let Some(param) = self.cached_parameters.iter().find(|p| p.id == *id) {
            param.validate(value).map_err(|e| format!("{}: {}", id, e))
        } else {
            Err(format!("Unknown parameter: {}", id))
        }
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_lifecycle != DrainLifecycle::Accepting {
            return Err("A/B Compare parameters are frozen until reset after drain begins".into());
        }
        self.validate_parameter(&id, &value)?;
        if crate::params::PARAMS
            .iter()
            .find(|spec| spec.engine_key == id.as_str())
            .is_some_and(|spec| spec.update_mode == UpdateMode::Structural)
        {
            return Err(format!(
                "parameter '{}' is structural; rebuild the outer plugin graph to change it",
                id.as_str()
            ));
        }
        match id.as_str() {
            "mix" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "mix must be a float".to_string())?;
                if v.is_finite() {
                    self.mix = v.clamp(-1.0, 1.0);
                    self.transition_smoothers.mix.set_target(self.mix);
                    self.recompute_empty_path_fast_gain();
                }
            }
            "mix_mode" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "mix_mode must be an integer".to_string())?;
                self.mix_mode = if v == 0 {
                    MixMode::Potentiometer
                } else {
                    MixMode::Binary
                };
            }
            "selected_path" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "selected_path must be an integer".to_string())?;
                self.selected_path = v.clamp(0, 1);
                // Update mix target for binary mode
                if self.mix_mode == MixMode::Binary {
                    let target = if self.selected_path == 0 { -1.0 } else { 1.0 };
                    self.transition_smoothers.mix.set_target(target);
                    self.recompute_empty_path_fast_gain();
                }
            }
            "bypass" => {
                self.bypass = value
                    .as_bool()
                    .ok_or_else(|| "bypass must be a boolean".to_string())?;
                self.transition_smoothers
                    .bypass
                    .set_target(if self.bypass { 1.0 } else { 0.0 });
            }
            "auto_gain_enabled" => {
                self.auto_gain.set_enabled(
                    value
                        .as_bool()
                        .ok_or_else(|| "auto_gain_enabled must be a boolean".to_string())?,
                );
            }
            "loudness_type" => {
                let v = value
                    .as_int()
                    .ok_or_else(|| "loudness_type must be an integer".to_string())?;
                let loudness_type = if v == 0 {
                    AutoGainLoudnessType::Momentary
                } else {
                    AutoGainLoudnessType::ShortTerm
                };
                self.auto_gain.set_loudness_type(loudness_type);
            }
            "max_auto_gain_db" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "max_auto_gain_db must be a float".to_string())?;
                if v.is_finite() {
                    self.auto_gain.set_max_gain_db(v.clamp(0.0, 24.0));
                }
            }
            "gain_smoothing_ms" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "gain_smoothing_ms must be a float".to_string())?;
                if v.is_finite() {
                    self.auto_gain.set_smoothing_ms(v.clamp(1.0, 500.0));
                }
            }
            "mix_transition_ms" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "mix_transition_ms must be a float".to_string())?;
                if v.is_finite() {
                    self.mix_transition_ms = v.clamp(1.0, 500.0);
                    self.transition_smoothers
                        .mix
                        .set_time(self.mix_transition_ms, self.sample_rate);
                    self.transition_smoothers
                        .bypass
                        .set_time(self.mix_transition_ms, self.sample_rate);
                }
            }
            "phase_invert_a" => {
                self.phase_invert[0] = value
                    .as_bool()
                    .ok_or_else(|| "phase_invert_a must be a boolean".to_string())?;
            }
            "phase_invert_b" => {
                self.phase_invert[1] = value
                    .as_bool()
                    .ok_or_else(|| "phase_invert_b must be a boolean".to_string())?;
            }
            "difference_mode" => {
                self.difference_mode = value
                    .as_bool()
                    .ok_or_else(|| "difference_mode must be a boolean".to_string())?;
            }
            "band_mask_low_hz" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "band_mask_low_hz must be a float".to_string())?;
                if v.is_finite() {
                    self.band_mask_low_hz = v.clamp(20.0, 20000.0);
                    self.rebuild_band_mask_filters();
                }
            }
            "band_mask_high_hz" => {
                let v = value
                    .as_float()
                    .ok_or_else(|| "band_mask_high_hz must be a float".to_string())?;
                if v.is_finite() {
                    self.band_mask_high_hz = v.clamp(20.0, 20000.0);
                    self.rebuild_band_mask_filters();
                }
            }
            "path_a_config" => {
                if let ParameterValue::String(json) = value {
                    let config: PathConfig = serde_json::from_str(&json)
                        .map_err(|e| format!("Invalid path A config JSON: {}", e))?;
                    let mut candidate = super::factory::build_path_from_config_with_factory(
                        &config,
                        self.num_channels,
                        self.sample_rate,
                        self.plugin_factory,
                    )?;
                    candidate
                        .build()
                        .map_err(|e| format!("Invalid path A graph: {e}"))?;
                    self.host_b
                        .build()
                        .map_err(|e| format!("Existing path B graph is invalid: {e}"))?;
                    self.host_a = candidate;
                    self.path_a_config = config;
                    self.update_latency_compensation()?;
                }
            }
            "path_b_config" => {
                if let ParameterValue::String(json) = value {
                    let config: PathConfig = serde_json::from_str(&json)
                        .map_err(|e| format!("Invalid path B config JSON: {}", e))?;
                    let mut candidate = super::factory::build_path_from_config_with_factory(
                        &config,
                        self.num_channels,
                        self.sample_rate,
                        self.plugin_factory,
                    )?;
                    candidate
                        .build()
                        .map_err(|e| format!("Invalid path B graph: {e}"))?;
                    self.host_a
                        .build()
                        .map_err(|e| format!("Existing path A graph is invalid: {e}"))?;
                    self.host_b = candidate;
                    self.path_b_config = config;
                    self.update_latency_compensation()?;
                }
            }
            _ => return Err(format!("Unknown parameter: {}", id.0)),
        }
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        match id.as_str() {
            "mix" => Some(ParameterValue::Float(self.mix)),
            "mix_mode" => Some(ParameterValue::Int(match self.mix_mode {
                MixMode::Potentiometer => 0,
                MixMode::Binary => 1,
            })),
            "selected_path" => Some(ParameterValue::Int(self.selected_path)),
            "bypass" => Some(ParameterValue::Bool(self.bypass)),
            "auto_gain_enabled" => Some(ParameterValue::Bool(self.auto_gain.is_enabled())),
            "loudness_type" => Some(ParameterValue::Int(match self.auto_gain.loudness_type() {
                AutoGainLoudnessType::Momentary => 0,
                AutoGainLoudnessType::ShortTerm => 1,
            })),
            "max_auto_gain_db" => Some(ParameterValue::Float(self.auto_gain.max_gain_db())),
            "gain_smoothing_ms" => Some(ParameterValue::Float(self.auto_gain.smoothing_ms())),
            "mix_transition_ms" => Some(ParameterValue::Float(self.mix_transition_ms)),
            "phase_invert_a" => Some(ParameterValue::Bool(self.phase_invert[0])),
            "phase_invert_b" => Some(ParameterValue::Bool(self.phase_invert[1])),
            "difference_mode" => Some(ParameterValue::Bool(self.difference_mode)),
            "band_mask_low_hz" => Some(ParameterValue::Float(self.band_mask_low_hz)),
            "band_mask_high_hz" => Some(ParameterValue::Float(self.band_mask_high_hz)),
            "path_a_config" => serde_json::to_string(&self.path_a_config)
                .ok()
                .map(ParameterValue::String),
            "path_b_config" => serde_json::to_string(&self.path_b_config)
                .ok()
                .map(ParameterValue::String),
            _ => None,
        }
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        if sample_rate == 0 {
            return Err("A/B Compare sample rate must be greater than zero".into());
        }
        let nyquist = sample_rate as f32 * 0.5;
        if self.band_mask_low_hz > Self::BAND_MASK_MIN_HZ + Self::BAND_MASK_EDGE_EPSILON
            && self.band_mask_low_hz >= nyquist
        {
            return Err(format!(
                "band_mask_low_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        if self.band_mask_high_hz < Self::BAND_MASK_MAX_HZ - Self::BAND_MASK_EDGE_EPSILON
            && self.band_mask_high_hz >= nyquist
        {
            return Err(format!(
                "band_mask_high_hz must be below Nyquist ({nyquist} Hz)"
            ));
        }
        // Prepare every fallible component before committing any live state.
        let mut host_a = build_path_from_config_with_factory(
            &self.path_a_config,
            self.num_channels,
            sample_rate,
            self.plugin_factory,
        )?;
        let mut host_b = build_path_from_config_with_factory(
            &self.path_b_config,
            self.num_channels,
            sample_rate,
            self.plugin_factory,
        )?;
        host_a.build()?;
        host_b.build()?;
        let auto_gain = AutoGain::new(
            self.num_channels,
            sample_rate,
            AutoGainParams {
                enabled: self.auto_gain.is_enabled(),
                loudness_type: self.auto_gain.loudness_type(),
                max_gain_db: self.auto_gain.max_gain_db(),
                smoothing_ms: self.auto_gain.smoothing_ms(),
            },
        )?;
        let latency_a = host_a.total_latency_samples();
        let latency_b = host_b.total_latency_samples();
        let mut delay_a = DelayLine::new();
        let mut delay_b = DelayLine::new();
        let mut delay_dry = DelayLine::new();
        if latency_a > latency_b {
            delay_b.set_delay(latency_a - latency_b, self.num_channels);
        } else {
            delay_a.set_delay(latency_b - latency_a, self.num_channels);
        }
        delay_dry.set_delay(latency_a.max(latency_b), self.num_channels);

        let mut drain_buffers = [Vec::new(), Vec::new()];
        for (buffer, host) in drain_buffers.iter_mut().zip([&host_a, &host_b]) {
            let samples = host
                .drain_output_frames_max()
                .checked_mul(self.num_channels)
                .ok_or_else(|| "A/B Compare child drain buffer size overflow".to_string())?;
            buffer.try_reserve_exact(samples).map_err(|error| {
                format!("A/B Compare child drain buffer allocation failed: {error}")
            })?;
            buffer.resize(samples, 0.0);
        }

        self.sample_rate = sample_rate;
        self.host_a = host_a;
        self.host_b = host_b;
        self.auto_gain = auto_gain;
        self.delay_a = delay_a;
        self.delay_b = delay_b;
        self.delay_dry = delay_dry;
        self.drain_buffers = drain_buffers;
        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [0; 3];
        self.drain_lifecycle = DrainLifecycle::Accepting;
        self.band_mask_used_since_reset = false;

        // Reset mix smoother with new sample rate
        self.transition_smoothers.mix =
            Smoother::new(self.mix, self.mix_transition_ms, sample_rate);
        self.transition_smoothers.bypass = Smoother::new(
            if self.bypass { 1.0 } else { 0.0 },
            self.mix_transition_ms,
            sample_rate,
        );
        self.recompute_empty_path_fast_gain();

        // Rebuild band mask filters for new sample rate
        self.rebuild_band_mask_filters();

        // Pre-allocate processing buffers for max expected frame size (avoids hot-path resize)
        let max_buffer = Self::MAX_REALTIME_FRAMES * self.num_channels;
        for buffer in &mut self.buffers {
            if buffer.len() < max_buffer {
                buffer.resize(max_buffer, 0.0);
            }
        }
        self.cache_update_counter = 0;

        Ok(())
    }

    fn reset(&mut self) {
        // Reset hosts
        self.host_a.reset();
        self.host_b.reset();

        // Reset auto-gain (also resets loudness monitors)
        self.auto_gain.reset();

        // Reset delay lines
        self.delay_a.reset();
        self.delay_b.reset();

        // Reset mix smoother
        self.transition_smoothers.mix.reset(self.mix);
        self.transition_smoothers
            .bypass
            .reset(if self.bypass { 1.0 } else { 0.0 });

        // Reset peak values
        self.last_peaks = [0.0; 2];
        self.cache_update_counter = 0;

        // Reset band mask filters
        self.reset_band_mask_filter_state();

        // Clear contents without dropping pre-allocated capacity/length.
        self.buffers[0].fill(0.0);
        self.buffers[1].fill(0.0);
        self.drain_buffers[0].fill(0.0);
        self.drain_buffers[1].fill(0.0);
        self.delay_dry.reset();
        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [0; 3];
        self.drain_lifecycle = DrainLifecycle::Accepting;
        self.band_mask_used_since_reset = false;

        // Update diagnostic cache immediately with reset values
        let data = ABCompareData {
            loudness_a_lufs: f64::NEG_INFINITY,
            loudness_b_lufs: f64::NEG_INFINITY,
            auto_gain_db: 0.0,
            peak_a: 0.0,
            peak_b: 0.0,
            current_mix: self.transition_smoothers.mix.current(),
            bypass_active: self.bypass,
        };
        self.cache.update(|d| {
            *d = data;
        });
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected_samples = context
            .num_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare block sample count overflow".to_string())?;

        // Verify input/output size
        if input.len() != expected_samples {
            return Err(format!(
                "Input size mismatch: expected {}, got {}",
                expected_samples,
                input.len()
            ));
        }
        if output.len() != expected_samples {
            return Err(format!(
                "Output size mismatch: expected {}, got {}",
                expected_samples,
                output.len()
            ));
        }

        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "A/B Compare was initialized at {} Hz, got a {} Hz process context",
                self.sample_rate, context.sample_rate
            ));
        }
        if let Some(index) = input.iter().position(|sample| !sample.is_finite()) {
            return Err(format!(
                "A/B Compare input contains a non-finite sample at index {index}"
            ));
        }
        if context.num_frames == 0 {
            return Ok(0);
        }
        if self.drain_lifecycle != DrainLifecycle::Accepting {
            return Err("A/B Compare requires reset before processing after drain begins".into());
        }

        if self.can_use_empty_path_fast_path() {
            // Empty paths make wet and dry audio identical, but bypass state
            // must still advance by the exact number of rendered samples.
            // Resetting here made a transition restart on every callback and
            // therefore made its duration depend on host block partitioning.
            self.transition_smoothers.bypass.next_n(context.num_frames);
            if let Err(error) = self.process_empty_path_fast(input, output, context.num_frames) {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
            return Ok(context.num_frames);
        }

        if expected_samples > self.buffers[0].len() || expected_samples > self.buffers[1].len() {
            return Err(format!(
                "A/B Compare block exceeds prepared realtime capacity: {} frames (max {})",
                context.num_frames,
                Self::MAX_REALTIME_FRAMES
            ));
        }

        // Always advance the latency-aligned dry path and both nested paths,
        // even while bypass is fully engaged. This makes toggles continuous
        // and prevents stateful nested processors from freezing in bypass.
        output.copy_from_slice(input);
        self.delay_dry.process(output);

        // Process path A
        if let Err(error) = self
            .host_a
            .process(input, &mut self.buffers[0][..expected_samples])
        {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }

        // Process path B
        if let Err(error) = self
            .host_b
            .process(input, &mut self.buffers[1][..expected_samples])
        {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }

        // Apply latency compensation (delays the shorter path)
        self.delay_a
            .process(&mut self.buffers[0][..expected_samples]);
        self.delay_b
            .process(&mut self.buffers[1][..expected_samples]);

        if let Err(error) = self.mix_prepared_buffers(output, context.num_frames) {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }
        if self.band_mask_active() {
            self.band_mask_used_since_reset = true;
        }

        Ok(context.num_frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.drain_capacity_frames()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if context.num_frames != 0 {
            return Err("A/B Compare drain preparation requires a zero-frame context".into());
        }
        if self.sample_rate == 0 || context.sample_rate != self.sample_rate {
            return Err(format!(
                "A/B Compare drain context must use its initialized sample rate of {} Hz",
                self.sample_rate
            ));
        }

        match self.drain_lifecycle {
            DrainLifecycle::Draining | DrainLifecycle::Complete => return Ok(()),
            DrainLifecycle::ResetRequired => {
                return Err("A/B Compare requires reset after a partial processing failure".into());
            }
            DrainLifecycle::Accepting => {}
        }

        if self.band_mask_active() || self.band_mask_used_since_reset {
            return Err(
                "A/B Compare cannot drain a stream with current or prior band-mask history; reset is required"
                    .into(),
            );
        }
        Self::validate_drain_path_topology(&self.path_a_config, "path A")?;
        Self::validate_drain_path_topology(&self.path_b_config, "path B")?;
        self.validate_drain_host(&self.host_a, &self.drain_buffers[0], "path A")?;
        self.validate_drain_host(&self.host_b, &self.drain_buffers[1], "path B")?;

        self.drain_children = [ChildDrainProgress::default(); 2];
        self.drain_delay_remaining = [
            self.delay_a.delay_frames(self.num_channels),
            self.delay_b.delay_frames(self.num_channels),
            self.delay_dry.delay_frames(self.num_channels),
        ];
        self.drain_lifecycle = DrainLifecycle::Draining;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if context.num_frames != 0 || context.sample_rate != self.sample_rate {
            return Err(
                "A/B Compare drain requires a zero-frame context at its initialized rate".into(),
            );
        }
        match self.drain_lifecycle {
            DrainLifecycle::Complete => return Ok(PluginDrainResult::COMPLETE),
            DrainLifecycle::ResetRequired => {
                return Err("A/B Compare requires reset after a partial drain failure".into());
            }
            DrainLifecycle::Accepting => {
                return Err("A/B Compare begin_drain must succeed before drain".into());
            }
            DrainLifecycle::Draining => {}
        }

        if !output.len().is_multiple_of(self.num_channels) {
            return Err(format!(
                "A/B Compare drain output must contain whole {}-channel frames",
                self.num_channels
            ));
        }
        let capacity_frames = self.drain_capacity_frames();
        let required_samples = capacity_frames
            .checked_mul(self.num_channels)
            .ok_or_else(|| "A/B Compare drain output capacity overflow".to_string())?;
        if output.len() < required_samples {
            return Err(format!(
                "A/B Compare drain output too small: need {required_samples} samples, got {}",
                output.len()
            ));
        }

        for child_index in 0..2 {
            if let Err(error) = self.pump_child_drain(child_index) {
                self.drain_lifecycle = DrainLifecycle::ResetRequired;
                return Err(error);
            }
        }

        if (0..2).any(|index| {
            self.drain_children[index].queued_remaining() == 0
                && !self.drain_children[index].complete
        }) {
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        if self.drain_complete() {
            self.drain_lifecycle = DrainLifecycle::Complete;
            return Ok(PluginDrainResult::COMPLETE);
        }

        let mut frames = capacity_frames;
        for child_index in 0..2 {
            let progress = self.drain_children[child_index];
            let queued = progress.queued_remaining();
            if queued > 0 {
                frames = frames.min(queued);
            } else if progress.complete && self.drain_delay_remaining[child_index] > 0 {
                frames = frames.min(self.drain_delay_remaining[child_index]);
            }
        }
        if self.drain_delay_remaining[2] > 0 {
            frames = frames.min(self.drain_delay_remaining[2]);
        }

        if frames == 0 {
            if self.drain_complete() {
                self.drain_lifecycle = DrainLifecycle::Complete;
                return Ok(PluginDrainResult::COMPLETE);
            }
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        let samples = frames * self.num_channels;
        self.buffers[0][..samples].fill(0.0);
        self.buffers[1][..samples].fill(0.0);
        output[..samples].fill(0.0);

        for child_index in 0..2 {
            let progress = self.drain_children[child_index];
            let queued = progress.queued_remaining();
            let path_buffer = &mut self.buffers[child_index][..samples];
            if queued > 0 {
                let start = progress.consumed_frames * self.num_channels;
                path_buffer
                    .copy_from_slice(&self.drain_buffers[child_index][start..start + samples]);
                if child_index == 0 {
                    self.delay_a.process(path_buffer);
                } else {
                    self.delay_b.process(path_buffer);
                }
                self.drain_children[child_index].consumed_frames += frames;
            } else if self.drain_delay_remaining[child_index] > 0 {
                if child_index == 0 {
                    self.delay_a.process(path_buffer);
                } else {
                    self.delay_b.process(path_buffer);
                }
                self.drain_delay_remaining[child_index] -= frames;
            }
        }

        let dry_frames = frames.min(self.drain_delay_remaining[2]);
        if dry_frames > 0 {
            let dry_samples = dry_frames * self.num_channels;
            self.delay_dry.process(&mut output[..dry_samples]);
            self.drain_delay_remaining[2] -= dry_frames;
        }

        if let Err(error) = self.mix_prepared_buffers(&mut output[..samples], frames) {
            self.drain_lifecycle = DrainLifecycle::ResetRequired;
            return Err(error);
        }
        if self.drain_complete() {
            self.drain_lifecycle = DrainLifecycle::Complete;
            return Ok(PluginDrainResult {
                frames,
                complete: true,
            });
        }
        Ok(PluginDrainResult {
            frames,
            complete: false,
        })
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.cache.load() as Arc<dyn Any + Send + Sync>)
    }

    fn latency_samples(&self) -> usize {
        // Total latency is the max of both paths
        let latency_a = self.host_a.total_latency_samples();
        let latency_b = self.host_b.total_latency_samples();
        latency_a.max(latency_b)
    }
}
