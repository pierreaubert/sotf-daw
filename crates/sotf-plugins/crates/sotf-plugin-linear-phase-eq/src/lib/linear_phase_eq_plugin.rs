use super::default::default_fir_length_index;
use super::default::default_num_filters;
use super::default::default_phase_mode_index;
use super::eq_band::EqBand;
use super::misc::MAG_RESPONSE_POINTS;
use super::misc::filter_type_to_index;
use super::misc::fir_length_from_index;
use super::misc::index_to_filter_type;
use super::misc::parse_filter_type;
use super::misc::placement_to_index;
use super::misc::resolve_placement;
use super::ordered::build_route_banks;
use super::ordered::channel_cascade_response;
use super::ordered::identity_fir as design_identity_fir;
use super::ordered::process_route_frame;
use super::ordered::stage_dtft;
use super::ordered::validate_stereo_pairs;
use super::types::BandConfig;
use super::types::BandSnapshot;
use super::types::CommitRefusal;
use super::types::LinearPhaseEqBandPlacement;
use super::types::LinearPhaseEqPluginParams;
use super::types::LiveFilterSnapshot;
use super::types::PreparedBandUpdate;
use super::types::RouteBanks;
use super::types::StageBanks;
use crate::params::{FIR_LENGTH_OPTIONS, MAX_FILTERS, PARAMS as LP_PARAMS, PHASE_MODE_OPTIONS};
use math_audio_iir_fir::{
    Biquad, BiquadFilterType, FirDesignConfig, FirPhase, WindowType, generate_fir_from_response,
};
use num_complex::Complex;
use plugins_spatial::nupc::NupcEngine;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use sotf_host::param_bridge::apply_spec_update_modes;
use sotf_host::param_specs::UpdateMode;
use sotf_host::param_specs::find_by_key as pk;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::parametric_plugin::{ParameterSchema, ParameterSet};
use sotf_host::plugin::{
    PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::simd::{enable_ftz_daz, flush_denormals_inplace};
use sotf_host::smoothing::Smoother;
use std::any::Any;
use std::sync::Arc;

const NUPC_REALTIME_QUANTUM_FRAMES: usize = 32;
const REALTIME_SCHEDULER_QUANTUM_FRAMES: usize = 128;
// Bound zero-continuation work independently of FIR length or caller capacity.
const DRAIN_FRAMES: usize = 256;
/// Fixed crossfade step count for committed dynamic band updates.
///
/// The blend weight advances `1 / XFADE_FRAMES` per frame from exactly 0 to
/// exactly 1, so the crossfade spans `XFADE_FRAMES + 1` = 513 frames. Both
/// routes process every frame during the blend (bounded 2x convolution work)
/// and the weights hit exactly 0 and 1, so neither end can step.
pub(crate) const XFADE_FRAMES: usize = 512;

#[allow(
    dead_code,
    reason = "legacy OLA buffers retained for state-format compatibility"
)]
pub struct LinearPhaseEqPlugin {
    pub(super) channels: usize,
    pub(super) sample_rate: f64,
    pub(super) num_filters: usize,
    pub(super) fir_length_index: usize,
    pub(super) phase_mode_index: usize,
    pub(super) auto_gain: bool,
    pub(super) mix_value: f32,
    has_input: bool,
    drain_remaining: Option<usize>,

    // EQ band definitions (for magnitude computation only)
    pub(super) bands: Vec<EqBand>,

    // FIR state
    pub(super) fir_coeffs: Vec<f32>,
    // Legacy OLA spectrum of `fir_coeffs`, refreshed at build/rebuild only.
    // Dynamic commits deliberately do not recompute it: the streaming path
    // convolves through NUPC engines (never this spectrum), so recomputing a
    // full FFT on the commit thread would be dead bounded-work cost. Treat a
    // post-commit spectrum as stale; chart code must use the DTFT-based
    // `channel_complex_response` API instead.
    pub(super) fir_spectrum: Vec<Complex<f32>>,
    pub(super) fir_dirty: bool,
    /// Non-uniform partitioned convolvers with a bounded 32-sample head.
    pub(super) convolvers: Vec<NupcEngine>,

    // FFT planners (Arc'd, no mutex)
    pub(super) fft_forward: Arc<dyn RealToComplex<f32>>,
    pub(super) fft_inverse: Arc<dyn ComplexToReal<f32>>,
    pub(super) fft_size: usize,

    // Pre-allocated processing buffers (per-channel is handled by reuse)
    pub(super) input_buf: Vec<f32>,
    pub(super) output_buf: Vec<f32>,
    pub(super) freq_buf: Vec<Complex<f32>>,
    pub(super) fft_scratch_fwd: Vec<Complex<f32>>,
    pub(super) fft_scratch_inv: Vec<Complex<f32>>,

    // Per-channel overlap-add tail
    pub(super) overlap: Vec<Vec<f32>>,

    // FIR design scratch, reused across parameter changes.
    pub(super) design_freqs: Vec<f64>,
    pub(super) design_magnitudes_db: Vec<f64>,

    // Dry buffer for mix
    pub(super) dry_buf: Vec<f32>,
    // Per-channel dry delay used to align the dry branch with linear-phase FIR latency.
    pub(super) dry_delay: Vec<Vec<f32>>,
    pub(super) dry_delay_pos: usize,

    // Smoothers
    pub(super) mix_smoother: Smoother,

    pub(super) cached_parameters: Vec<Parameter>,

    // Ordered per-band cascade route (R1). Empty selects the legacy
    // single-FIR path, which stays bit-identical in that case.
    pub(super) ordered_stages: Vec<StageBanks>,
    pub(super) stereo_pairs: Vec<[usize; 2]>,
    pub(super) identity_fir: Vec<f32>,
    // Dynamic band updates (R2): crossfade target, remaining blend frames,
    // and the last retired bank set awaiting off-thread reclamation.
    pub(super) xfade_target: Option<RouteBanks>,
    pub(super) xfade_remaining: usize,
    pub(super) route_retired: Option<RouteBanks>,
    // Input frames recorded in the dry ring since the last reset, saturating
    // at the ring capacity. Used to prime committed update targets.
    pub(super) history_len: usize,
    // Preallocated channel-wide scratch: priming input and ordered-route /
    // crossfade working frames. Never resized after construction.
    pub(super) prime_frame: Vec<f32>,
    pub(super) xfade_frame: Vec<f32>,
}

impl LinearPhaseEqPlugin {
    /// Minimum queued-work horizon for the engine scheduler.
    ///
    /// Smaller streaming partitions are accepted, but the non-uniform tail
    /// occasionally completes several FFT levels on one callback. A queued
    /// engine must keep at least this much input-rate audio ahead of hardware
    /// consumption. This does not extend a direct AU/NIH callback deadline;
    /// QA reports those physical-callback misses separately.
    pub fn realtime_quantum_frames(&self) -> usize {
        REALTIME_SCHEDULER_QUANTUM_FRAMES
    }

    pub fn new<S: Into<f64>>(channels: usize, sample_rate: S) -> Self {
        let sample_rate = sample_rate.into();
        let fir_length_index = default_fir_length_index();
        let fir_length = fir_length_from_index(fir_length_index);
        let num_filters = default_num_filters();

        Self::build(
            channels,
            sample_rate,
            num_filters,
            fir_length_index,
            fir_length,
            default_phase_mode_index(),
            false,
            1.0,
            Vec::new(),
            validate_stereo_pairs(channels, None, false)
                .expect("default stereo pairs cannot fail validation"),
        )
    }

    pub fn from_params<S: Into<f64>>(
        channels: usize,
        sample_rate: S,
        params: LinearPhaseEqPluginParams,
    ) -> Result<Self, String> {
        let sample_rate = sample_rate.into();
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be positive".into());
        }
        if !params.mix.is_finite() || !(0.0..=1.0).contains(&params.mix) {
            return Err(format!(
                "mix must be finite and within [0, 1], got {}",
                params.mix
            ));
        }
        let fir_length_index = params.fir_length_index.min(FIR_LENGTH_OPTIONS.len() - 1);
        let phase_mode_index = params.phase_mode_index.min(PHASE_MODE_OPTIONS.len() - 1);
        let fir_length = fir_length_from_index(fir_length_index);
        let num_filters = params.num_filters.clamp(1, MAX_FILTERS);
        let sr = sample_rate as f64;

        let needs_pairs = params
            .filters
            .iter()
            .take(num_filters)
            .any(|fc| fc.placement.is_some_and(|p| p.requires_stereo_pair()));
        let stereo_pairs =
            validate_stereo_pairs(channels, params.stereo_pairs.as_deref(), needs_pairs)?;
        if params.auto_gain && needs_pairs {
            return Err(
                "auto_gain is not supported with explicit L/R/M/S placements; use the stereo-linked route"
                    .into(),
            );
        }

        let mut bands = Vec::with_capacity(num_filters);
        for (i, fc) in params.filters.iter().enumerate() {
            if i >= num_filters {
                break;
            }
            let ft = parse_filter_type(&fc.filter_type)?;
            Self::validate_band(fc.frequency, fc.q, fc.gain_db, sr)?;
            bands.push(EqBand::new(
                ft,
                fc.frequency,
                fc.q,
                fc.gain_db,
                fc.active,
                fc.placement,
                sr,
            ));
        }
        // Fill remaining bands with defaults
        while bands.len() < num_filters {
            bands.push(EqBand::new(
                BiquadFilterType::Peak,
                1000.0,
                1.0,
                0.0,
                true,
                None,
                sr,
            ));
        }

        Ok(Self::build(
            channels,
            sample_rate,
            num_filters,
            fir_length_index,
            fir_length,
            phase_mode_index,
            params.auto_gain,
            params.mix,
            bands,
            stereo_pairs,
        ))
    }

    fn validate_band(frequency: f64, q: f64, gain_db: f64, sample_rate: f64) -> Result<(), String> {
        let max_frequency = (sample_rate * 0.5 * 0.99).min(20_000.0);
        if !frequency.is_finite() || frequency < 20.0 || frequency > max_frequency {
            return Err(format!(
                "frequency must be finite and within [20, {max_frequency}], got {frequency}"
            ));
        }
        if !q.is_finite() || !(0.1..=10.0).contains(&q) {
            return Err(format!("Q must be finite and within [0.1, 10], got {q}"));
        }
        if !gain_db.is_finite() || !(-24.0..=24.0).contains(&gain_db) {
            return Err(format!(
                "gain must be finite and within [-24, 24], got {gain_db}"
            ));
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "internal builder: each argument maps to a distinct FIR-EQ configuration field"
    )]
    pub(super) fn build(
        channels: usize,
        sample_rate: f64,
        num_filters: usize,
        fir_length_index: usize,
        fir_length: usize,
        phase_mode_index: usize,
        auto_gain: bool,
        mix: f32,
        mut bands: Vec<EqBand>,
        stereo_pairs: Vec<[usize; 2]>,
    ) -> Self {
        let sr = sample_rate as f64;
        // Fill bands to num_filters
        while bands.len() < num_filters {
            bands.push(EqBand::new(
                BiquadFilterType::Peak,
                1000.0,
                1.0,
                0.0,
                true,
                None,
                sr,
            ));
        }

        // FFT size = fir_length + max_frame_size - 1, rounded up to power of 2.
        // We use 2 * fir_length as a safe FFT size (supports frames up to fir_length+1).
        let fft_size = (fir_length * 2).next_power_of_two();
        let freq_size = fft_size / 2 + 1;

        let mut planner = RealFftPlanner::<f32>::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        let fft_scratch_fwd = vec![Complex::new(0.0, 0.0); fft_forward.get_scratch_len()];
        let fft_scratch_inv = vec![Complex::new(0.0, 0.0); fft_inverse.get_scratch_len()];

        // Max buffer size: generous allocation for typical audio frame sizes
        let max_buf = fft_size * channels;
        // Fixed-capacity dry ring: reserve the largest supported cascade
        // response support during construction so phase/FIR changes never
        // allocate in the processing callback. The ordered cascade route
        // stacks one stage per band slot, and every stage needs its full
        // `N - 1 + 32` support (FIR plus NUPC streaming delay) primed after
        // a dynamic commit, so capacity covers every slot at the longest FIR
        // (10 * (8191 + 32) = 82230 frames; ~4 MB at 12 channels). The ring
        // doubles as the input-history store for update priming; streams
        // longer than capacity prime from the most recent window only.
        let dry_delay_len = FIR_LENGTH_OPTIONS
            .iter()
            .filter_map(|length| length.parse::<usize>().ok())
            .map(|length| MAX_FILTERS * (length.saturating_sub(1) + 32))
            .max()
            .unwrap_or(1)
            .max(1);

        let mut plugin = Self {
            channels,
            sample_rate,
            num_filters,
            fir_length_index,
            phase_mode_index,
            auto_gain,
            mix_value: mix,
            has_input: false,
            drain_remaining: None,
            bands,
            fir_coeffs: vec![0.0; fir_length],
            fir_spectrum: vec![Complex::new(0.0, 0.0); freq_size],
            fir_dirty: true,
            convolvers: Vec::new(),
            fft_forward,
            fft_inverse,
            fft_size,
            input_buf: vec![0.0; fft_size],
            output_buf: vec![0.0; fft_size],
            freq_buf: vec![Complex::new(0.0, 0.0); freq_size],
            fft_scratch_fwd,
            fft_scratch_inv,
            overlap: vec![vec![0.0; fir_length.saturating_sub(1)]; channels],
            design_freqs: Vec::new(),
            design_magnitudes_db: Vec::new(),
            dry_buf: vec![0.0; max_buf],
            dry_delay: vec![vec![0.0; dry_delay_len]; channels],
            dry_delay_pos: 0,
            mix_smoother: Smoother::new(mix, 20.0, sample_rate),
            cached_parameters: Vec::new(),
            ordered_stages: Vec::new(),
            stereo_pairs,
            identity_fir: design_identity_fir(fir_length, phase_mode_index),
            xfade_target: None,
            xfade_remaining: 0,
            route_retired: None,
            history_len: 0,
            prime_frame: vec![0.0; channels],
            xfade_frame: vec![0.0; channels],
        };
        plugin.rebuild_cached_parameters();
        // Ordered route selection: any Left/Right/Mid/Side band slot (active
        // or not, so dynamic active toggles never change the route or its
        // latency). The route is fixed for the life of the plugin.
        let ordered = plugin.bands[..plugin.num_filters.min(plugin.bands.len())]
            .iter()
            .any(|band| band.placement.is_some_and(|p| p.requires_stereo_pair()));
        debug_assert!(!(ordered && auto_gain));
        if ordered {
            plugin.rebuild_ordered();
        } else {
            // Build the initial FIR
            plugin.rebuild_fir();
        }
        plugin.fir_dirty = false;
        plugin
    }

    pub(super) fn fir_length(&self) -> usize {
        fir_length_from_index(self.fir_length_index)
    }

    pub(super) fn band_contribution_db(bands: &[EqBand], freq: f64) -> f64 {
        let mut combined_db = 0.0;
        for band in bands {
            if !band.active {
                continue;
            }
            match band.filter_type {
                BiquadFilterType::Lowpass | BiquadFilterType::Highpass => {
                    combined_db += band.biquad.log_result(freq);
                }
                _ if band.gain_db.abs() > 1e-6 => {
                    combined_db += band.biquad.log_result(freq);
                }
                _ => {}
            }
        }
        combined_db
    }

    /// Design FIR coefficients from a band slice.
    ///
    /// Pure core shared by the legacy path, the ordered stages and off-thread
    /// update preparation. The operation sequence is the long-established
    /// design pipeline, so legacy output is unchanged.
    fn design_fir_coefficients(
        sample_rate: f64,
        fir_length: usize,
        phase_mode_index: usize,
        bands: &[EqBand],
        auto_gain: bool,
        design_freqs: &mut Vec<f64>,
        design_magnitudes_db: &mut Vec<f64>,
    ) -> Vec<f32> {
        let nyquist = sample_rate / 2.0;

        // Scale sampling density with FIR length so narrow peaks are captured.
        // For an N-tap FIR we need at least 2*N frequency samples; round to a
        // power-of-two for consistency and clamp to a minimum of MAG_RESPONSE_POINTS.
        let num_points = MAG_RESPONSE_POINTS.max(fir_length * 2).next_power_of_two();
        design_freqs.clear();
        design_magnitudes_db.clear();
        design_freqs.reserve(num_points);
        design_magnitudes_db.reserve(num_points);

        // Include DC (1 Hz to avoid log-space interpolation issues with 0)
        // while still using the real combined response at the low end.
        design_freqs.push(1.0);
        design_magnitudes_db.push(Self::band_contribution_db(bands, 1.0));

        let log_min = 1.0_f64.ln();
        let log_max = nyquist.ln();

        for i in 1..num_points {
            let t = i as f64 / (num_points - 1) as f64;
            let freq = (log_min + t * (log_max - log_min)).exp();
            design_freqs.push(freq);

            design_magnitudes_db.push(Self::band_contribution_db(bands, freq));
        }

        let phase = match phase_mode_index {
            1 => FirPhase::Minimum,
            _ => FirPhase::Linear,
        };
        let config = FirDesignConfig {
            n_taps: fir_length,
            sample_rate,
            phase,
            window: WindowType::Kaiser,
            ..Default::default()
        };

        let fir_f64 = generate_fir_from_response(design_freqs, design_magnitudes_db, &config);

        // Convert to f32 and store
        let mut fir = vec![0.0f32; fir_f64.len()];
        for (dst, src) in fir.iter_mut().zip(fir_f64.iter()) {
            *dst = *src as f32;
        }

        // Auto-gain normally restores unity gain at DC. DC-null filters (such
        // as a high-pass) use Nyquist when it is a meaningful passband
        // reference. If both endpoints are null, retain the designed scale
        // instead of amplifying numerical residue.
        if auto_gain {
            let dc = fir.iter().sum::<f32>();
            let nyquist_gain = fir
                .iter()
                .enumerate()
                .map(|(index, &coefficient)| {
                    if index % 2 == 0 {
                        coefficient
                    } else {
                        -coefficient
                    }
                })
                .sum::<f32>();
            let reference = if dc.is_finite() && dc.abs() > 1e-4 {
                Some(dc)
            } else if nyquist_gain.is_finite() && nyquist_gain.abs() > 1e-4 {
                Some(nyquist_gain)
            } else {
                None
            };
            if let Some(reference) = reference {
                let inv = 1.0 / reference;
                for c in &mut fir {
                    *c *= inv;
                }
            }
        }
        fir
    }

    /// Rebuild FIR coefficients from current band settings.
    pub(super) fn rebuild_fir(&mut self) {
        let sr = self.sample_rate as f64;
        let fir_length = self.fir_length();
        let fir = Self::design_fir_coefficients(
            sr,
            fir_length,
            self.phase_mode_index,
            &self.bands[..self.num_filters.min(self.bands.len())],
            self.auto_gain,
            &mut self.design_freqs,
            &mut self.design_magnitudes_db,
        );

        self.fir_coeffs.resize(fir.len(), 0.0);
        self.fir_coeffs.copy_from_slice(&fir);

        // Pre-compute FFT of the FIR
        self.compute_fir_spectrum();
        self.convolvers = (0..self.channels)
            .map(|_| NupcEngine::new(&self.fir_coeffs, NUPC_REALTIME_QUANTUM_FRAMES))
            .collect();
    }

    /// Design every ordered cascade stage from its band slot.
    ///
    /// Inactive slots use the explicit identity FIR so the stage count (and
    /// therefore latency and drain support) never depends on which bands are
    /// enabled. Auto gain is rejected with placed bands at construction, so
    /// stages are always designed unnormalized here.
    pub(super) fn rebuild_ordered(&mut self) {
        let sr = self.sample_rate as f64;
        let fir_length = self.fir_length();
        let phase_mode_index = self.phase_mode_index;
        let count = self.num_filters.min(self.bands.len());
        let mut firs = Vec::with_capacity(count);
        let mut placements = Vec::with_capacity(count);
        for band in self.bands.iter().take(count) {
            placements.push(resolve_placement(band.placement));
            if band.active {
                firs.push(Self::design_fir_coefficients(
                    sr,
                    fir_length,
                    phase_mode_index,
                    std::slice::from_ref(band),
                    false,
                    &mut self.design_freqs,
                    &mut self.design_magnitudes_db,
                ));
            } else {
                firs.push(design_identity_fir(fir_length, phase_mode_index));
            }
        }
        let route = build_route_banks(
            &firs,
            &placements,
            &self.stereo_pairs,
            self.channels,
            &self.identity_fir,
            NUPC_REALTIME_QUANTUM_FRAMES,
        );
        self.ordered_stages = route.stages;
    }

    /// Compute the frequency-domain representation of the FIR.
    pub(super) fn compute_fir_spectrum(&mut self) {
        let fir_len = self.fir_coeffs.len();

        // Zero-pad FIR into input_buf
        self.input_buf[..fir_len].copy_from_slice(&self.fir_coeffs);
        self.input_buf[fir_len..self.fft_size].fill(0.0);

        // FFT the FIR
        self.fft_forward
            .process_with_scratch(
                &mut self.input_buf,
                &mut self.fir_spectrum,
                &mut self.fft_scratch_fwd,
            )
            .expect("FIR FFT failed");
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let mut params = vec![
            Parameter::new_int(
                "num_filters",
                "Num Filters",
                self.num_filters as i32,
                pk(LP_PARAMS, "num_filters").min_f64() as i32,
                pk(LP_PARAMS, "num_filters").max_f64() as i32,
            )
            .with_description("Number of EQ bands")
            .with_group("EQ"),
            Parameter::new_int(
                "fir_length_index",
                "FIR Length",
                self.fir_length_index as i32,
                0,
                (FIR_LENGTH_OPTIONS.len() - 1) as i32,
            )
            .with_description("FIR length in taps")
            .with_group("Quality"),
            Parameter::new_int(
                "phase_mode_index",
                "Phase Mode",
                self.phase_mode_index as i32,
                0,
                (PHASE_MODE_OPTIONS.len() - 1) as i32,
            )
            .with_description("FIR phase design mode")
            .with_group("Phase"),
            Parameter::new_bool("auto_gain", "Auto Gain", self.auto_gain)
                .with_description("Compensate output level")
                .with_group("Output"),
            Parameter::new_float(
                "mix",
                "Mix",
                self.mix_value,
                pk(LP_PARAMS, "mix").min_f64() as f32,
                pk(LP_PARAMS, "mix").max_f64() as f32,
            )
            .with_description("Dry/wet mix")
            .with_group("Output"),
        ];

        // Per-band parameters
        for (i, band) in self.bands.iter().take(self.num_filters).enumerate() {
            let group = format!("Band {}", i + 1);
            params.push(
                Parameter::new_int(
                    &format!("band_{}_type", i),
                    "Type",
                    filter_type_to_index(band.filter_type) as i32,
                    0,
                    4,
                )
                .with_group(&group)
                .with_update_mode(UpdateMode::Structural),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_freq", i),
                    "Freq",
                    band.frequency as f32,
                    20.0,
                    20000.0,
                )
                .with_group(&group)
                .with_update_mode(UpdateMode::Structural),
            );
            params.push(
                Parameter::new_float(&format!("band_{}_q", i), "Q", band.q as f32, 0.1, 10.0)
                    .with_group(&group)
                    .with_update_mode(UpdateMode::Structural),
            );
            params.push(
                Parameter::new_float(
                    &format!("band_{}_gain", i),
                    "Gain",
                    band.gain_db as f32,
                    -24.0,
                    24.0,
                )
                .with_group(&group)
                .with_update_mode(UpdateMode::Structural),
            );
            params.push(
                Parameter::new_bool(&format!("band_{}_active", i), "Active", band.active)
                    .with_group(&group)
                    .with_update_mode(UpdateMode::Structural),
            );
            params.push(
                Parameter::new_int(
                    &format!("band_{}_placement", i),
                    "Placement",
                    placement_to_index(band.placement) as i32,
                    0,
                    5,
                )
                .with_description(
                    "0 inherits the legacy route; 1=Stereo, 2=Left, 3=Right, 4=Mid, 5=Side",
                )
                .with_group(&group)
                .with_update_mode(UpdateMode::Structural),
            );
        }

        apply_spec_update_modes(&mut params, LP_PARAMS);
        self.cached_parameters = params;
    }

    /// Resize FFT buffers when FIR length changes.
    #[allow(dead_code, reason = "retained for compatible prepared-state migration")]
    pub(super) fn resize_fft_buffers(&mut self) {
        let fir_length = self.fir_length();
        let fft_size = (fir_length * 2).next_power_of_two();
        let freq_size = fft_size / 2 + 1;

        if fft_size != self.fft_size {
            let mut planner = RealFftPlanner::<f32>::new();
            self.fft_forward = planner.plan_fft_forward(fft_size);
            self.fft_inverse = planner.plan_fft_inverse(fft_size);
            self.fft_size = fft_size;
            self.input_buf.resize(fft_size, 0.0);
            self.output_buf.resize(fft_size, 0.0);
            self.freq_buf.resize(freq_size, Complex::new(0.0, 0.0));
            self.fir_spectrum.resize(freq_size, Complex::new(0.0, 0.0));
            self.fft_scratch_fwd
                .resize(self.fft_forward.get_scratch_len(), Complex::new(0.0, 0.0));
            self.fft_scratch_inv
                .resize(self.fft_inverse.get_scratch_len(), Complex::new(0.0, 0.0));
        }

        let overlap_len = fir_length.saturating_sub(1);
        for ch_overlap in &mut self.overlap {
            ch_overlap.resize(overlap_len, 0.0);
            ch_overlap.fill(0.0);
        }
        // The dry ring has fixed capacity independent of FFT/FIR sizing.
    }
}

impl ParametricInPlacePlugin for LinearPhaseEqPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("FIR EQ", env!("CARGO_PKG_VERSION"), "SOTF")
            .with_description("Parametric EQ with linear or minimum-phase FIR convolution")
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::Convolution
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::linear_transform(
            PluginCostClass::Convolution,
            None,
            self.latency_samples(),
            false,
            true,
            false,
        )
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
            ParameterId::from("num_filters"),
            ParameterValue::Int(self.num_filters as i32),
        );
        values.insert(
            ParameterId::from("fir_length_index"),
            ParameterValue::Int(self.fir_length_index as i32),
        );
        values.insert(
            ParameterId::from("phase_mode_index"),
            ParameterValue::Int(self.phase_mode_index as i32),
        );
        // NOTE: no legacy-id entries here. Reads and discovery are
        // canonical-only; pre-migration ids are accepted at serde
        // boundaries (factory presets) and translated at format edges
        // (FFI/NIH), never in the realtime parameter set.
        values.insert(
            ParameterId::from("auto_gain"),
            ParameterValue::Bool(self.auto_gain),
        );
        values.insert(
            ParameterId::from("mix"),
            ParameterValue::Float(self.mix_value),
        );
        for (i, band) in self.bands.iter().take(self.num_filters).enumerate() {
            values.insert(
                ParameterId::from(format!("band_{}_type", i).as_str()),
                ParameterValue::Int(filter_type_to_index(band.filter_type) as i32),
            );
            values.insert(
                ParameterId::from(format!("band_{}_freq", i).as_str()),
                ParameterValue::Float(band.frequency as f32),
            );
            values.insert(
                ParameterId::from(format!("band_{}_q", i).as_str()),
                ParameterValue::Float(band.q as f32),
            );
            values.insert(
                ParameterId::from(format!("band_{}_gain", i).as_str()),
                ParameterValue::Float(band.gain_db as f32),
            );
            values.insert(
                ParameterId::from(format!("band_{}_active", i).as_str()),
                ParameterValue::Bool(band.active),
            );
            values.insert(
                ParameterId::from(format!("band_{}_placement", i).as_str()),
                ParameterValue::Int(placement_to_index(band.placement) as i32),
            );
        }
        values
    }

    fn apply_values(&mut self, values: ParameterSet) -> PluginResult<()> {
        if self.drain_remaining.is_some() && !values.is_empty() {
            return Err("reset FIR EQ before changing controls after drain".into());
        }
        for (id, value) in values {
            let id_str = id.as_str();

            match id_str {
                "num_filters" | "fir_length_index" | "phase_mode_index" | "auto_gain" => {
                    return Err(format!(
                        "{id_str} is structural; rebuild the plugin to change it"
                    ));
                }
                "mix" => {
                    if let ParameterValue::Float(v) = value {
                        if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                            return Err(format!("mix must be finite and within [0, 1], got {v}"));
                        }
                        self.mix_value = v;
                        self.mix_smoother.set_target(v);
                        self.rebuild_cached_parameters();
                    }
                }
                _ if id_str.starts_with("band_") => {
                    return Err(format!(
                        "{id_str} is structural; rebuild the plugin to change it"
                    ));
                }
                _ => return Err(format!("Unknown parameter: {id}")),
            }
        }
        Ok(())
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be positive".into());
        }
        let reset_stream = self.drain_remaining.is_some() || sample_rate != self.sample_rate;
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.mix_smoother = Smoother::new(self.mix_value, 20.0, sample_rate);
            // A rate change invalidates every prepared convolver, including
            // any staged dynamic update (control-thread reclamation here is
            // expected: this path already reallocates designs and banks).
            self.xfade_target = None;
            self.route_retired = None;
            self.xfade_remaining = 0;
            // Rebuild all biquads at new sample rate
            let sr = sample_rate as f64;
            for band in &mut self.bands {
                band.biquad =
                    Biquad::new(band.filter_type, band.frequency, sr, band.q, band.gain_db);
            }
            if self.is_ordered_route() {
                self.rebuild_ordered();
            } else {
                self.rebuild_fir();
            }
            self.fir_dirty = false;
        }
        if reset_stream {
            self.reset();
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.has_input = false;
        self.drain_remaining = None;
        // Clear overlap buffers
        for ch_overlap in &mut self.overlap {
            ch_overlap.fill(0.0);
        }
        for convolver in &mut self.convolvers {
            convolver.reset();
        }
        for stage in &mut self.ordered_stages {
            for engine in &mut stage.engines {
                engine.reset();
            }
        }
        if let Some(target) = self.xfade_target.as_mut() {
            for stage in &mut target.stages {
                for engine in &mut stage.engines {
                    engine.reset();
                }
            }
        }
        // A reset completes a pending dynamic update instantly: the accepted
        // new configuration becomes current with cleared state. This is a
        // no-op without an in-flight update, so steady-state reset stays
        // allocation-free.
        self.complete_xfade();
        for delay in &mut self.dry_delay {
            delay.fill(0.0);
        }
        self.dry_delay_pos = 0;
        self.history_len = 0;
        self.mix_smoother = Smoother::new(self.mix_value, 20.0, self.sample_rate);
    }

    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        enable_ftz_daz();
        let nf = context.num_frames;
        let nc = self.channels;

        if nf == 0 || nc == 0 {
            return Ok(nf);
        }

        let total_samples = nf
            .checked_mul(nc)
            .ok_or_else(|| "audio buffer size overflow".to_string())?;
        if buffer.len() < total_samples {
            return Err(format!(
                "audio buffer too short: need {total_samples}, got {}",
                buffer.len()
            ));
        }
        if context.sample_rate != self.sample_rate {
            return Err(format!(
                "sample-rate mismatch: initialized for {}, got {}",
                self.sample_rate, context.sample_rate
            ));
        }

        if self.fir_dirty {
            return Err(
                "FIR state is dirty; rebuild or initialize the plugin off the audio thread".into(),
            );
        }

        if self.drain_remaining.is_some() {
            return Err("reset FIR EQ before processing after drain".into());
        }
        self.process_stream(&mut buffer[..total_samples], nf);
        self.has_input = true;
        Ok(nf)
    }

    fn tail_length(&self) -> TailLength {
        if self.fir_dirty || self.sample_rate == 0 {
            TailLength::Unknown
        } else {
            TailLength::Finite(self.response_frames() as u64)
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        DRAIN_FRAMES
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        let remaining = if self.has_input {
            self.drain_remaining
                .unwrap_or_else(|| self.response_frames())
        } else {
            0
        };
        std::num::NonZeroU64::new(remaining.div_ceil(DRAIN_FRAMES).max(1) as u64)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.has_input || self.drain_remaining == Some(0) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if self.sample_rate == 0 || context.sample_rate != self.sample_rate || self.fir_dirty {
            return Err("FIR EQ drain requires clean state at the prepared sample rate".into());
        }
        if output.len() < self.channels || !output.len().is_multiple_of(self.channels) {
            return Err("FIR EQ drain needs nonempty whole output frames".into());
        }
        let remaining = self
            .drain_remaining
            .unwrap_or_else(|| self.response_frames());
        let frames = remaining
            .min(DRAIN_FRAMES)
            .min(output.len() / self.channels);
        let buffer = &mut output[..frames * self.channels];
        buffer.fill(0.0);
        enable_ftz_daz();
        self.process_stream(buffer, frames);
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: frames == remaining,
        })
    }

    fn latency_samples(&self) -> usize {
        // The even-tap designer centers its impulse at N/2. NUPC adds one
        // 32-sample head partition of streaming latency per convolver stage.
        // The ordered route stacks one stage per band slot, so its latency
        // scales with the configured band count; the legacy path keeps the
        // long-established single-stage value.
        let per_stage = if self.phase_mode_index == 0 {
            self.fir_length() / 2 + 32
        } else {
            32
        };
        self.ordered_stage_count() * per_stage
    }

    fn realtime_quantum_frames(&self) -> usize {
        REALTIME_SCHEDULER_QUANTUM_FRAMES
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }
}

impl LinearPhaseEqPlugin {
    /// Report whether the ordered per-band cascade route is active.
    ///
    /// The route is fixed at construction: any Left/Right/Mid/Side band slot
    /// selects it, otherwise the legacy single-FIR path runs bit-identically.
    pub fn is_ordered_route(&self) -> bool {
        !self.ordered_stages.is_empty()
    }

    fn ordered_stage_count(&self) -> usize {
        if self.is_ordered_route() {
            self.ordered_stages.len()
        } else {
            1
        }
    }

    fn response_frames(&self) -> usize {
        if self.is_ordered_route() {
            // Cascaded stages extend the support additively; every band slot
            // occupies a stage (identity when inactive) so the count is
            // stable across dynamic updates.
            let per_stage =
                NUPC_REALTIME_QUANTUM_FRAMES + self.fir_length().saturating_sub(1);
            (per_stage * self.ordered_stages.len()).max(self.latency_samples())
        } else {
            // FIR support is coefficient length minus one, plus NUPC's fixed
            // emitted startup delay. The dry branch is bounded by the same span.
            (NUPC_REALTIME_QUANTUM_FRAMES + self.fir_coeffs.len().saturating_sub(1))
                .max(self.latency_samples())
        }
    }

    fn process_stream(&mut self, buffer: &mut [f32], nf: usize) {
        if self.xfade_target.is_some() {
            self.process_stream_xfade(buffer, nf);
            return;
        }
        if self.is_ordered_route() {
            self.process_stream_ordered(buffer, nf);
            return;
        }
        let nc = self.channels;

        // Save a latency-aligned dry signal for mix. Linear-phase FIR output has
        // group delay; delaying the dry branch avoids comb filtering at partial mix.
        let dry_delay = self.latency_samples();
        let ring_len = self.dry_delay.first().map_or(1, Vec::len);
        for frame in 0..nf {
            let ring_pos = self.dry_delay_pos % ring_len;
            let mix = self.mix_smoother.next_n(1);
            let read_pos = if dry_delay == 0 {
                ring_pos
            } else {
                (ring_pos + ring_len - dry_delay % ring_len) % ring_len
            };
            for ch in 0..nc {
                let index = frame * nc + ch;
                let sample = buffer[index];
                let dry = if dry_delay == 0 {
                    sample
                } else {
                    self.dry_delay[ch][read_pos]
                };
                // Keep history in every phase mode so a later switch to
                // linear phase has a valid dry signal without allocation.
                self.dry_delay[ch][ring_pos] = sample;
                let wet = self.convolvers[ch].process_sample(sample);
                buffer[index] = dry * (1.0 - mix) + wet * mix;
            }
            self.dry_delay_pos = (self.dry_delay_pos + 1) % ring_len;
            // The dry ring doubles as the input-history store for priming
            // committed dynamic updates. A single saturating counter cannot
            // change any audio sample.
            self.history_len = (self.history_len + 1).min(ring_len);
        }

        flush_denormals_inplace(buffer);
    }

    /// Ordered-route streaming: per-frame cascade with shared dry alignment.
    ///
    /// Mirrors the legacy dry/mix/history operation order sample for sample;
    /// only the wet path differs (band-ordered cascade instead of one shared
    /// convolution).
    fn process_stream_ordered(&mut self, buffer: &mut [f32], nf: usize) {
        let nc = self.channels;
        let dry_delay = self.latency_samples();
        let ring_len = self.dry_delay.first().map_or(1, Vec::len);
        for frame in 0..nf {
            let ring_pos = self.dry_delay_pos % ring_len;
            let mix = self.mix_smoother.next_n(1);
            let read_pos = if dry_delay == 0 {
                ring_pos
            } else {
                (ring_pos + ring_len - dry_delay % ring_len) % ring_len
            };
            for ch in 0..nc {
                self.prime_frame[ch] = buffer[frame * nc + ch];
            }
            let (stages, pairs, work) = (
                &mut self.ordered_stages,
                &self.stereo_pairs,
                &mut self.prime_frame[..nc],
            );
            process_route_frame(stages, pairs, work);
            for ch in 0..nc {
                let index = frame * nc + ch;
                let sample = buffer[index];
                let dry = if dry_delay == 0 {
                    sample
                } else {
                    self.dry_delay[ch][read_pos]
                };
                self.dry_delay[ch][ring_pos] = sample;
                let wet = self.prime_frame[ch];
                buffer[index] = dry * (1.0 - mix) + wet * mix;
            }
            self.dry_delay_pos = (self.dry_delay_pos + 1) % ring_len;
            self.history_len = (self.history_len + 1).min(ring_len);
        }

        flush_denormals_inplace(buffer);
    }

    /// Dual-route crossfade streaming for in-flight dynamic updates.
    ///
    /// Both the live route and the committed target process every frame so
    /// both stay state-correct; output blends old-to-new over exactly
    /// `XFADE_FRAMES + 1` frames with weights hitting exactly 0 and 1, so no
    /// step can occur at either end. Bounded 2x convolution work per frame,
    /// no allocation, no deallocation; completes into the retired slot.
    fn process_stream_xfade(&mut self, buffer: &mut [f32], nf: usize) {
        let nc = self.channels;
        let dry_delay = self.latency_samples();
        let ring_len = self.dry_delay.first().map_or(1, Vec::len);
        for frame in 0..nf {
            let ring_pos = self.dry_delay_pos % ring_len;
            let mix = self.mix_smoother.next_n(1);
            let read_pos = if dry_delay == 0 {
                ring_pos
            } else {
                (ring_pos + ring_len - dry_delay % ring_len) % ring_len
            };
            let weight = 1.0 - self.xfade_remaining as f32 / XFADE_FRAMES as f32;
            for ch in 0..nc {
                self.prime_frame[ch] = buffer[frame * nc + ch];
            }
            self.process_live_route_frame();
            self.xfade_frame[..nc].copy_from_slice(&self.prime_frame[..nc]);
            for ch in 0..nc {
                self.prime_frame[ch] = buffer[frame * nc + ch];
            }
            self.process_target_route_frame();
            for ch in 0..nc {
                let index = frame * nc + ch;
                let sample = buffer[index];
                let dry = if dry_delay == 0 {
                    sample
                } else {
                    self.dry_delay[ch][read_pos]
                };
                self.dry_delay[ch][ring_pos] = sample;
                let wet_old = self.xfade_frame[ch];
                let wet_new = self.prime_frame[ch];
                let wet = wet_old * (1.0 - weight) + wet_new * weight;
                buffer[index] = dry * (1.0 - mix) + wet * mix;
            }
            self.dry_delay_pos = (self.dry_delay_pos + 1) % ring_len;
            self.history_len = (self.history_len + 1).min(ring_len);
            if self.xfade_remaining > 0 {
                self.xfade_remaining -= 1;
            }
        }
        if self.xfade_remaining == 0 {
            self.complete_xfade();
        }

        flush_denormals_inplace(buffer);
    }

    /// Process `prime_frame[..channels]` through the live route in place.
    fn process_live_route_frame(&mut self) {
        let nc = self.channels;
        if self.is_ordered_route() {
            let (stages, pairs, frame) = (
                &mut self.ordered_stages,
                &self.stereo_pairs,
                &mut self.prime_frame[..nc],
            );
            process_route_frame(stages, pairs, frame);
        } else {
            for ch in 0..nc {
                let sample = self.prime_frame[ch];
                self.prime_frame[ch] = self.convolvers[ch].process_sample(sample);
            }
        }
    }

    /// Process `prime_frame[..channels]` through the crossfade target.
    ///
    /// No-op without a target (only reachable transiently while completing).
    fn process_target_route_frame(&mut self) {
        let nc = self.channels;
        let Some(target) = self.xfade_target.as_mut() else {
            return;
        };
        process_route_frame(
            &mut target.stages,
            &self.stereo_pairs,
            &mut self.prime_frame[..nc],
        );
    }

    /// Swap the crossfade target into the live route and retire the old banks.
    ///
    /// Pointer moves only: no allocation and no deallocation (the retired set
    /// awaits [`Self::take_retired_route`]). No-op without a target.
    fn complete_xfade(&mut self) {
        let Some(mut target) = self.xfade_target.take() else {
            return;
        };
        // Commit refuses a new target while a retired set is unclaimed, so a
        // completion always retires into an empty slot.
        debug_assert!(self.route_retired.is_none());
        if self.is_ordered_route() {
            for (live, new) in self
                .ordered_stages
                .iter_mut()
                .zip(target.stages.iter_mut())
            {
                std::mem::swap(&mut live.engines, &mut new.engines);
            }
        } else if let Some(stage) = target.stages.first_mut() {
            std::mem::swap(&mut self.convolvers, &mut stage.engines);
        }
        self.xfade_remaining = 0;
        self.route_retired = Some(target);
    }

    /// Feed recorded input history through cold target banks.
    ///
    /// Allocation-free: the dry ring is the history store and `prime_frame`
    /// is preallocated. Output is discarded; convolver state is what matters.
    /// Partitioned block-phase rounding means primed state matches a
    /// continuously-run engine within ~1e-6, not bit-exactly.
    fn prime_target(&mut self, target: &mut RouteBanks, prime_len: usize) {
        let channels = self.channels;
        if channels == 0 || prime_len == 0 || target.stages.is_empty() {
            return;
        }
        let Some(ring_len) = self.dry_delay.first().map(Vec::len) else {
            return;
        };
        if ring_len == 0 {
            return;
        }
        let history = self.history_len.min(ring_len);
        let prime_len = prime_len.min(history);
        if prime_len == 0 {
            return;
        }
        let oldest = (self.dry_delay_pos + ring_len - history) % ring_len;
        let start = history - prime_len;
        for j in 0..prime_len {
            let index = (oldest + start + j) % ring_len;
            for (ch, slot) in self.prime_frame.iter_mut().enumerate().take(channels) {
                *slot = self.dry_delay[ch][index];
            }
            process_route_frame(
                &mut target.stages,
                &self.stereo_pairs,
                &mut self.prime_frame[..channels],
            );
        }
    }
}

impl LinearPhaseEqPlugin {
    /// Resolved disjoint stereo pairs (two-channel default when unset).
    pub fn stereo_pairs(&self) -> &[[usize; 2]] {
        &self.stereo_pairs
    }

    /// Number of cascade stages (1 on the legacy single-FIR path).
    pub fn stage_count(&self) -> usize {
        self.ordered_stage_count()
    }

    /// One stage's FIR design: stage 0 is the combined FIR on the legacy path.
    ///
    /// Returns `None` for an out-of-range stage. During an in-flight dynamic
    /// update this reports the committed target design (see
    /// [`Self::try_commit_prepared_update`] for the contract).
    pub fn stage_fir(&self, stage: usize) -> Option<&[f32]> {
        if self.is_ordered_route() {
            self.ordered_stages.get(stage).map(|s| s.fir.as_slice())
        } else if stage == 0 {
            Some(self.fir_coeffs.as_slice())
        } else {
            None
        }
    }

    /// Placement of one band slot: `None` means an invalid band index, and
    /// `Some(None)` the legacy unset routing.
    pub fn band_placement(&self, band: usize) -> Option<Option<LinearPhaseEqBandPlacement>> {
        self.bands.get(band).map(|b| b.placement)
    }

    /// Channel-aware complex response at `frequency_hz`.
    ///
    /// Models the exact processing topology plus the fixed per-stage
    /// partitioned streaming delay. The chart semantic is the diagonal
    /// transfer `Tcc`: the response measured on `channel` when only that
    /// channel is excited (a single-channel impulse DFT agrees with this
    /// API). Mid/Side stages spread excitation across their pair, so this is
    /// not the correlated-input response. Returns `None` for an invalid
    /// channel, an inactive (zero-rate) plugin or an out-of-range frequency.
    /// Control-thread only: allocates response scratch on ordered routes.
    ///
    /// During an in-flight dynamic update ([`Self::update_in_progress`]) this
    /// reports the committed target design immediately while audio still
    /// morphs old-to-new over the blend (see
    /// [`Self::try_commit_prepared_update`] for the contract).
    pub fn channel_complex_response(
        &self,
        channel: usize,
        frequency_hz: f64,
    ) -> Option<Complex<f64>> {
        if channel >= self.channels || self.sample_rate == 0 {
            return None;
        }
        if !frequency_hz.is_finite() || frequency_hz < 0.0 {
            return None;
        }
        let sr = self.sample_rate;
        if frequency_hz > sr * 0.5 {
            return None;
        }
        if self.is_ordered_route() {
            channel_cascade_response(
                &self.ordered_stages,
                &self.stereo_pairs,
                &self.identity_fir,
                channel,
                frequency_hz,
                sr,
                NUPC_REALTIME_QUANTUM_FRAMES,
            )
        } else {
            let response = stage_dtft(&self.fir_coeffs, frequency_hz, sr);
            let streaming = NUPC_REALTIME_QUANTUM_FRAMES as f64;
            let angle = -std::f64::consts::TAU * frequency_hz * streaming / sr;
            Some(response * Complex::new(angle.cos(), angle.sin()))
        }
    }

    /// Group delay in samples via central phase difference of
    /// [`Self::channel_complex_response`]. Control-thread only.
    pub fn channel_group_delay_samples(
        &self,
        channel: usize,
        frequency_hz: f64,
    ) -> Option<f64> {
        let sr = self.sample_rate;
        if sr <= 0.0 {
            return None;
        }
        let step = (frequency_hz * 1e-3).max(1.0);
        let lo_freq = (frequency_hz - step).max(0.0);
        let hi_freq = (frequency_hz + step).min(sr * 0.5);
        let span = hi_freq - lo_freq;
        if span <= 0.0 {
            return None;
        }
        let lo = self.channel_complex_response(channel, lo_freq)?;
        let hi = self.channel_complex_response(channel, hi_freq)?;
        let mut delta = hi.im.atan2(hi.re) - lo.im.atan2(lo.re);
        while delta > std::f64::consts::PI {
            delta -= std::f64::consts::TAU;
        }
        while delta < -std::f64::consts::PI {
            delta += std::f64::consts::TAU;
        }
        Some(-sr * delta / (std::f64::consts::TAU * span))
    }

    /// Capture the live filter configuration for off-thread update preparation.
    ///
    /// Allocates; call on a control thread and hand the snapshot to
    /// [`Self::prepare_band_update`].
    pub fn snapshot_config(&self) -> LiveFilterSnapshot {
        LiveFilterSnapshot {
            channels: self.channels,
            sample_rate: self.sample_rate,
            num_filters: self.num_filters,
            fir_length_index: self.fir_length_index,
            phase_mode_index: self.phase_mode_index,
            auto_gain: self.auto_gain,
            bands: self
                .bands
                .iter()
                .take(self.num_filters)
                .map(|band| BandSnapshot {
                    filter_type_index: filter_type_to_index(band.filter_type),
                    frequency: band.frequency,
                    q: band.q,
                    gain_db: band.gain_db,
                    active: band.active,
                    placement: band.placement,
                })
                .collect(),
            stereo_pairs: self.stereo_pairs.clone(),
        }
    }

    /// Exact allocation-free comparison of a prepared base against live state.
    fn snapshot_matches(&self, base: &LiveFilterSnapshot) -> bool {
        base.channels == self.channels
            && base.sample_rate == self.sample_rate
            && base.num_filters == self.num_filters
            && base.fir_length_index == self.fir_length_index
            && base.phase_mode_index == self.phase_mode_index
            && base.auto_gain == self.auto_gain
            && base.stereo_pairs == self.stereo_pairs
            && base.bands.len() == self.num_filters.min(self.bands.len())
            && self
                .bands
                .iter()
                .take(self.num_filters)
                .zip(base.bands.iter())
                .all(|(live, snapshot)| {
                    filter_type_to_index(live.filter_type) == snapshot.filter_type_index
                        && live.frequency == snapshot.frequency
                        && live.q == snapshot.q
                        && live.gain_db == snapshot.gain_db
                        && live.active == snapshot.active
                        && live.placement == snapshot.placement
                })
    }

    /// Design a dynamic single-band update without touching live state.
    ///
    /// Heavy work (FIR design, convolver planning) happens here, so call this
    /// off the audio thread, then install the result with
    /// [`Self::try_commit_prepared_update`] (audio thread) or
    /// [`Self::commit_prepared_update`] (control thread). Only the band's filter shape
    /// (`filter_type`, `frequency`, `q`, `gain_db`, `active`) may change;
    /// band index, placement, band count, pairs, FIR length, phase mode and
    /// auto gain are fingerprinted, so phase mode and latency never change
    /// across an update.
    ///
    /// # Errors
    ///
    /// Returns an error for an out-of-range band index, an unknown filter
    /// type, out-of-range or non-finite band parameters, or a placement
    /// change (structural: rebuild the plugin).
    pub fn prepare_band_update(
        base: &LiveFilterSnapshot,
        band_index: usize,
        new_band: BandConfig,
    ) -> Result<PreparedBandUpdate, String> {
        let live_bands = base.num_filters.min(base.bands.len());
        if band_index >= live_bands {
            return Err(format!(
                "band index {band_index} exceeds {live_bands} configured bands"
            ));
        }
        if base.sample_rate == 0 {
            return Err("sample rate must be positive".into());
        }
        let old = &base.bands[band_index];
        if new_band.placement != old.placement {
            return Err(
                "band placement changes are structural; rebuild the plugin to change them".into(),
            );
        }
        let filter_type = parse_filter_type(&new_band.filter_type)?;
        let sr = base.sample_rate;
        Self::validate_band(new_band.frequency, new_band.q, new_band.gain_db, sr)?;
        let fir_length = fir_length_from_index(base.fir_length_index);

        let new_snapshot = BandSnapshot {
            filter_type_index: filter_type_to_index(filter_type),
            frequency: new_band.frequency,
            q: new_band.q,
            gain_db: new_band.gain_db,
            active: new_band.active,
            placement: new_band.placement,
        };
        // Temporary design bands: snapshot state with the one change applied.
        let mut design_bands = Vec::with_capacity(base.bands.len());
        for (index, snapshot) in base.bands.iter().enumerate() {
            let (filter, frequency, q, gain_db, active) = if index == band_index {
                (
                    filter_type,
                    new_band.frequency,
                    new_band.q,
                    new_band.gain_db,
                    new_band.active,
                )
            } else {
                (
                    index_to_filter_type(snapshot.filter_type_index),
                    snapshot.frequency,
                    snapshot.q,
                    snapshot.gain_db,
                    snapshot.active,
                )
            };
            design_bands.push(EqBand::new(
                filter,
                frequency,
                q,
                gain_db,
                active,
                snapshot.placement,
                sr,
            ));
        }
        let ordered = design_bands
            .iter()
            .any(|band| band.placement.is_some_and(|p| p.requires_stereo_pair()));
        let mut scratch_freqs = Vec::new();
        let mut scratch_mags = Vec::new();
        let mut firs = Vec::with_capacity(design_bands.len().max(1));
        if ordered {
            for band in &design_bands {
                if band.active {
                    firs.push(Self::design_fir_coefficients(
                        sr,
                        fir_length,
                        base.phase_mode_index,
                        std::slice::from_ref(band),
                        false,
                        &mut scratch_freqs,
                        &mut scratch_mags,
                    ));
                } else {
                    firs.push(design_identity_fir(fir_length, base.phase_mode_index));
                }
            }
        } else {
            firs.push(Self::design_fir_coefficients(
                sr,
                fir_length,
                base.phase_mode_index,
                &design_bands,
                base.auto_gain,
                &mut scratch_freqs,
                &mut scratch_mags,
            ));
        }
        let placements: Vec<LinearPhaseEqBandPlacement> = if ordered {
            design_bands
                .iter()
                .map(|band| resolve_placement(band.placement))
                .collect()
        } else {
            vec![LinearPhaseEqBandPlacement::Stereo]
        };
        let identity = design_identity_fir(fir_length, base.phase_mode_index);
        let target = build_route_banks(
            &firs,
            &placements,
            &base.stereo_pairs,
            base.channels,
            &identity,
            NUPC_REALTIME_QUANTUM_FRAMES,
        );
        let new_biquad = Biquad::new(
            filter_type,
            new_band.frequency,
            sr,
            new_band.q,
            new_band.gain_db,
        );
        Ok(PreparedBandUpdate {
            base: base.clone(),
            band_index,
            new_band: new_snapshot,
            new_biquad,
            target,
        })
    }

    /// Allocation-free range check for a prepared band update.
    ///
    /// Mirrors [`Self::validate_band`] without allocating an error message, so
    /// the realtime commit path can refuse invalid parameters transactionally.
    fn prepared_band_params_valid(&self, new: &BandSnapshot) -> bool {
        if new.filter_type_index > 4 || self.sample_rate == 0 {
            return false;
        }
        let sample_rate = self.sample_rate;
        let max_frequency = (sample_rate * 0.5 * 0.99).min(20_000.0);
        if !new.frequency.is_finite()
            || new.frequency < 20.0
            || new.frequency > max_frequency
        {
            return false;
        }
        if !new.q.is_finite() || !(0.1..=10.0).contains(&new.q) {
            return false;
        }
        if !new.gain_db.is_finite() || !(-24.0..=24.0).contains(&new.gain_db) {
            return false;
        }
        true
    }

    /// Commit a prepared update on the audio thread.
    ///
    /// Realtime-safe entrypoint: validates and installs without allocating or
    /// freeing, and retains the caller-owned prepared update on refusal. The
    /// caller keeps `prepared` in its `Option` slot; on success the slot is
    /// taken (`None`) and the update blends, while on refusal the slot stays
    /// `Some` for correction or retry. Live band configuration, FIR data and
    /// cached parameter scalars switch at commit, so controls and the
    /// chart-facing response APIs report the target design immediately while
    /// audio morphs old-to-new over the fixed blend (`XFADE_FRAMES` steps over
    /// `XFADE_FRAMES + 1` frames). Phase mode and latency never change. The
    /// prepared base snapshot is stashed into the target banks (pointer moves
    /// only) for off-thread reclamation with the retired route.
    ///
    /// All validations run before any priming or live-state mutation, so every
    /// refusal is transactional: live configuration, populated audio history
    /// and the prepared resources are untouched. Priming covers at most the
    /// full cascade response support per channel (`stages * (N - 1 + 32)`
    /// frames, or the recorded history when shorter).
    ///
    /// # Errors
    ///
    /// Returns a [`CommitRefusal`] without allocating when no update is
    /// supplied, a blend is in flight, a retired route is unclaimed, the
    /// stream drained, the base snapshot is stale, or the prepared band,
    /// placement, topology, FIR length or target freshness does not match the
    /// live route. The `prepared` slot stays `Some` in every error case.
    pub fn try_commit_prepared_update(
        &mut self,
        prepared: &mut Option<PreparedBandUpdate>,
    ) -> Result<(), CommitRefusal> {
        // Validate via a shared borrow; every error leaves `prepared` intact
        // and performs no priming, so a retained update stays pristine.
        {
            let Some(candidate) = prepared.as_ref() else {
                return Err(CommitRefusal::NoPreparedUpdate);
            };
            if self.xfade_target.is_some() || self.xfade_remaining > 0 {
                return Err(CommitRefusal::UpdateInProgress);
            }
            if self.route_retired.is_some() {
                return Err(CommitRefusal::RetiredUnclaimed);
            }
            if self.drain_remaining.is_some() {
                return Err(CommitRefusal::Drained);
            }
            if !self.snapshot_matches(&candidate.base) {
                return Err(CommitRefusal::StaleBase);
            }
            if candidate.band_index >= self.bands.len() {
                return Err(CommitRefusal::BandIndexOutOfRange {
                    index: candidate.band_index,
                    live: self.bands.len(),
                });
            }
            let live_band = &self.bands[candidate.band_index];
            if candidate.new_band.placement != live_band.placement {
                return Err(CommitRefusal::PlacementMismatch);
            }
            if !self.prepared_band_params_valid(&candidate.new_band) {
                return Err(CommitRefusal::InvalidBand);
            }
            if candidate.target.stashed_base.is_some() {
                return Err(CommitRefusal::TargetNotFresh);
            }
            if self.is_ordered_route() {
                if candidate.target.stages.len() != self.ordered_stages.len() {
                    return Err(CommitRefusal::TopologyMismatch);
                }
                for (live, new) in self
                    .ordered_stages
                    .iter()
                    .zip(candidate.target.stages.iter())
                {
                    if live.placement != new.placement {
                        return Err(CommitRefusal::TopologyMismatch);
                    }
                    if live.fir.len() != new.fir.len() {
                        return Err(CommitRefusal::FirLengthMismatch);
                    }
                }
            } else {
                let Some(stage) = candidate.target.stages.first() else {
                    return Err(CommitRefusal::NoStage);
                };
                if candidate.target.stages.len() != 1 {
                    return Err(CommitRefusal::TopologyMismatch);
                }
                if stage.fir.len() != self.fir_coeffs.len() {
                    return Err(CommitRefusal::FirLengthMismatch);
                }
            }
        }
        // All checks passed; take ownership and install. The `None` arm is
        // unreachable after the validation above but stays refusal-typed.
        let Some(mut owned) = prepared.take() else {
            return Err(CommitRefusal::NoPreparedUpdate);
        };
        // Prime the cold target from recorded input history (bounded,
        // allocation-free: at most the full cascade response support per
        // channel, so multi-stage routes and the NUPC streaming delay are
        // covered, not just one FIR length).
        let prime_len = self.history_len.min(self.response_frames());
        self.prime_target(&mut owned.target, prime_len);
        // Install the new design data. Lengths and placements already match;
        // copies keep every allocation on the preparation side.
        if self.is_ordered_route() {
            for (live, new) in self
                .ordered_stages
                .iter_mut()
                .zip(owned.target.stages.iter())
            {
                live.fir.copy_from_slice(&new.fir);
            }
        } else {
            // Validation above guarantees exactly one stage with matching FIR.
            if let Some(stage) = owned.target.stages.first() {
                self.fir_coeffs.copy_from_slice(&stage.fir);
            }
            // The legacy OLA spectrum is intentionally not recomputed here:
            // no streaming or chart path reads it (see the field contract),
            // so a full FFT on the commit thread would be dead work.
        }
        let new = owned.new_band;
        let band_index = owned.band_index;
        let band = &mut self.bands[band_index];
        band.filter_type = index_to_filter_type(new.filter_type_index);
        band.frequency = new.frequency;
        band.q = new.q;
        band.gain_db = new.gain_db;
        band.active = new.active;
        band.biquad = owned.new_biquad;
        // Refresh the changed band's cached scalars in place (type, freq, q,
        // gain, active at 5 + band*6 + {0..4} by construction; placement never
        // changes here). No allocation: the schema shape is untouched.
        let base = 5 + band_index * 6;
        debug_assert_eq!(
            self.cached_parameters.len(),
            5 + self.num_filters.min(self.bands.len()) * 6
        );
        if let Some(slot) = self.cached_parameters.get_mut(base) {
            slot.default_value = ParameterValue::Int(new.filter_type_index as i32);
        }
        if let Some(slot) = self.cached_parameters.get_mut(base + 1) {
            slot.default_value = ParameterValue::Float(new.frequency as f32);
        }
        if let Some(slot) = self.cached_parameters.get_mut(base + 2) {
            slot.default_value = ParameterValue::Float(new.q as f32);
        }
        if let Some(slot) = self.cached_parameters.get_mut(base + 3) {
            slot.default_value = ParameterValue::Float(new.gain_db as f32);
        }
        if let Some(slot) = self.cached_parameters.get_mut(base + 4) {
            slot.default_value = ParameterValue::Bool(new.active);
        }
        // Stash the prepared base into the target banks (pointer moves only)
        // instead of dropping it here: dropping would free its two Vecs on the
        // commit thread. The snapshot rides to the retired slot at blend
        // completion and is dropped off-thread with the retired banks.
        let mut target = owned.target;
        target.stashed_base = Some(owned.base);
        if self.channels == 0 {
            // Degenerate: no audio to blend, install immediately.
            self.xfade_target = Some(target);
            self.xfade_remaining = 0;
            self.complete_xfade();
        } else {
            self.xfade_target = Some(target);
            self.xfade_remaining = XFADE_FRAMES;
        }
        Ok(())
    }

    /// Install a prepared update from a control thread.
    ///
    /// Compatibility wrapper around [`Self::try_commit_prepared_update`] that
    /// takes the prepared update by value. Control-thread only: on success it
    /// performs no allocation, but on refusal it drops the retained prepared
    /// update (freeing its heap) and allocates the `String` message.
    /// Audio-thread hosts must call `try_commit_prepared_update` directly with
    /// a caller-owned `Option` slot so refusals stay allocation-free and the
    /// prepared resources survive for correction or retry.
    ///
    /// # Errors
    ///
    /// Returns the same descriptive messages as the realtime refusal variants,
    /// as an allocated `String`. All errors are transactional for live state;
    /// only the owned `prepared` value is consumed by this wrapper.
    pub fn commit_prepared_update(
        &mut self,
        prepared: PreparedBandUpdate,
    ) -> Result<(), String> {
        let mut slot = Some(prepared);
        match self.try_commit_prepared_update(&mut slot) {
            Ok(()) => Ok(()),
            Err(refusal) => {
                drop(slot);
                Err(refusal.to_string())
            }
        }
    }

    /// Reclaim the last retired convolver bank set for off-thread drop.
    ///
    /// Call after an update completes (or after `reset`) and before
    /// committing another update. Dropping the returned banks deallocates
    /// (convolver state plus the stashed prepared base snapshot);
    /// do it off the audio thread.
    pub fn take_retired_route(&mut self) -> Option<RouteBanks> {
        self.route_retired.take()
    }

    /// Report whether a dynamic update blend is still in flight.
    ///
    /// While this returns true, controls and the chart-facing response APIs
    /// already report the committed target design; only the audio output is
    /// still morphing old-to-new (see [`Self::try_commit_prepared_update`]).
    pub fn update_in_progress(&self) -> bool {
        self.xfade_target.is_some()
    }
}
