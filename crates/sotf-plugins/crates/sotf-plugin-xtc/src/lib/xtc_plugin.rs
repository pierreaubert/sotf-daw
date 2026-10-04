// Rust guideline compliant 2026-02-21
use super::apply::apply_filter_left;
use super::apply::apply_filter_left_blended;
use super::apply::apply_filter_pair;
use super::apply::apply_filter_pair_blended;
use super::apply::apply_filter_right;
use super::apply::apply_filter_right_blended;
use super::compute::compute_room_params_hash;
use super::compute::compute_room_reflection_data;
pub use super::config::*;
use super::filters::{
    HrtfTransferFunctions, XtcFilters, compute_geometry_cache,
    compute_xtc_filters_full_with_cache_and_hrtf,
};
use super::load::load_hrtf_for_xtc;
use super::load::load_roomeq_recommended_filters;
use super::load::validate_roomeq_recommended_source;
use super::misc::MAX_PROCESS_FRAMES;
use super::reflections::{
    RoomReflectionData, build_reflection_data_image_source, build_reflection_data_ir,
};
use super::types::{FilterUpdateRequest, PendingFilterUpdate};
use super::xtc_data::XtcData;
use crate::params::PARAMS as XT;
use math_audio_dsp::stft::generate_hann_window;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex;
use sotf_host::analyzer::RealTimeCache;
use sotf_host::auto_gain::{AutoGain, AutoGainParams};
use sotf_host::param_bridge;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use sotf_host::simd::{deinterleave_stereo, flush_denormals_inplace, window_mul_simd};
use std::any::Any;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

#[inline(always)]
pub(super) fn accumulate_ifft_channel(
    accumulator: &mut [f32],
    start_frame: usize,
    output_channels: usize,
    channel: usize,
    ifft: &[f32],
    window: &[f32],
    scale: f32,
) {
    let ring_frames = accumulator.len() / output_channels;
    debug_assert_eq!(ifft.len(), window.len());
    debug_assert!(start_frame < ring_frames);
    debug_assert_eq!(accumulator.len(), ring_frames * output_channels);
    debug_assert!(channel < output_channels);

    let first_frames = ifft.len().min(ring_frames - start_frame);
    for i in 0..first_frames {
        let dst = (start_frame + i) * output_channels + channel;
        accumulator[dst] += ifft[i] * window[i] * scale;
    }
    for i in first_frames..ifft.len() {
        let dst = (i - first_frames) * output_channels + channel;
        accumulator[dst] += ifft[i] * window[i] * scale;
    }
}

#[inline(always)]
pub(super) fn drain_output_accumulator(
    accumulator: &mut [f32],
    read_position: usize,
    output_channels: usize,
    output: &mut [f32],
) -> usize {
    debug_assert!(output_channels > 0);
    debug_assert_eq!(accumulator.len() % output_channels, 0);
    debug_assert_eq!(output.len() % output_channels, 0);
    let ring_frames = accumulator.len() / output_channels;
    let frames_to_drain = output.len() / output_channels;
    debug_assert!(ring_frames.is_power_of_two());
    debug_assert!(read_position < ring_frames);
    debug_assert!(frames_to_drain <= ring_frames);

    let first_frames = frames_to_drain.min(ring_frames - read_position);
    let first_samples = first_frames * output_channels;
    let acc_start = read_position * output_channels;
    output[..first_samples].copy_from_slice(&accumulator[acc_start..acc_start + first_samples]);
    accumulator[acc_start..acc_start + first_samples].fill(0.0);

    let remaining_samples = output.len() - first_samples;
    if remaining_samples > 0 {
        output[first_samples..].copy_from_slice(&accumulator[..remaining_samples]);
        accumulator[..remaining_samples].fill(0.0);
    }

    (read_position + frames_to_drain) & (ring_frames - 1)
}

/// FFT / STFT configuration and shared resources.
pub(super) struct XtcFftConfig {
    /// FFT size (must be power of 2)
    pub(super) fft_size: usize,

    /// Hop size for overlap-add (75% overlap = fft_size / 4)
    pub(super) hop_size: usize,

    /// Sample rate
    pub(super) sample_rate: f64,

    /// Forward FFT planner
    pub(super) fft_forward: Arc<dyn RealToComplex<f32>>,

    /// Inverse FFT planner
    pub(super) fft_inverse: Arc<dyn ComplexToReal<f32>>,

    /// Analysis window (Hann)
    pub(super) analysis_window: Vec<f32>,

    /// Combined scale factor: COLA normalization / FFT size
    pub(super) output_scale: f32,
}

/// Input staging buffers.
pub(super) struct XtcInputBuffers {
    /// Input buffer: holds fft_size samples per channel
    /// Uses linear buffer with shift instead of ring buffer to avoid modulo
    pub(super) input_buffer_l: Vec<f32>,
    pub(super) input_buffer_r: Vec<f32>,

    /// Number of samples currently in input buffer (0 to fft_size)
    pub(super) input_fill: usize,

    /// Temporary buffers for block processing (avoid per-call allocation)
    pub(super) temp_input_l: Vec<f32>,
    pub(super) temp_input_r: Vec<f32>,
}

/// Working buffers used during the FFT / IFFT stages.
pub(super) struct XtcWorkBuffers {
    /// Working buffers for FFT
    pub(super) fft_buffer: Vec<f32>,
    pub(super) fft_output_l: Vec<Complex<f32>>,
    pub(super) fft_output_r: Vec<Complex<f32>>,
    pub(super) ifft_input: Vec<Complex<f32>>,
    pub(super) ifft_output: Vec<f32>,

    /// Working buffer for crossfade: holds IFFT of prev_filters result
    pub(super) prev_ifft_output: Vec<f32>,
}

/// Bounded publication and retirement storage shared with the filter worker.
#[derive(Default)]
pub(super) struct XtcFilterExchange {
    pub(super) pending: Option<Arc<PendingFilterUpdate>>,
    // Two slots cover a completed fade plus an interrupted successor. A full
    // exchange postpones adoption until the worker drains its owned garbage.
    pub(super) retired_updates: [Option<Arc<PendingFilterUpdate>>; 2],
    pub(super) retired_snapshots: [Option<Arc<XtcFilters>>; 2],
}

/// Thread-safe filter state, including the current filters, asynchronous updates,
/// crossfade smoothing, and room/HRTF data.
pub(super) struct XtcFilterState {
    /// Audio-owned current filters; only explicit adoption replaces this Arc.
    pub(super) cached_current_filters: Arc<XtcFilters>,

    /// Prepared bounded exchange. The callback only uses try_lock; the worker
    /// destroys displaced publications and retired owners outside the lock.
    pub(super) exchange: Arc<Mutex<XtcFilterExchange>>,

    /// Retains all current auxiliary data until ownership moves to the worker.
    pub(super) active_filter_update: Option<Arc<PendingFilterUpdate>>,

    /// Latest requested asynchronous filter generation; workers use it to drop stale results.
    pub(super) filter_update_generation: Arc<AtomicU64>,

    /// Latest-only control-thread request mailbox. At most one worker exists
    /// per instance; rapid automation overwrites this slot instead of spawning
    /// unbounded expensive jobs on Rayon's global pool.
    pub(super) filter_request: Arc<Mutex<Option<FilterUpdateRequest>>>,
    /// Bounded notification channel for the persistent filter worker. Request
    /// payloads stay in `filter_request`, so notifications coalesce as well.
    filter_worker_waker: Option<SyncSender<()>>,
    pub(super) filter_worker_launches: Arc<AtomicU64>,
    #[cfg(test)]
    pub(super) worker_test_barrier: Option<Arc<crate::generation_tests::WorkerBarrier>>,

    /// Previous filter snapshot for crossfading (Block mode)
    pub(super) prev_filters: Option<Arc<XtcFilters>>,

    /// Crossfade progress (0.0 = prev, 1.0 = current)
    pub(super) crossfade_progress: f32,

    /// Cached progress increment per STFT hop (recomputed in update_filters)
    pub(super) progress_per_hop: f32,

    /// Loaded HRTF transfer functions (from SOFA file)
    pub(super) hrtf_transfer_functions: Option<Arc<HrtfTransferFunctions>>,

    /// Cached room reflection data (Optimization 4)
    pub(super) room_reflection_cache: Option<Arc<RoomReflectionData>>,

    /// Hash of room-related parameters for cache invalidation (Optimization 4)
    pub(super) room_params_hash: u64,
}

/// Overlap-add output ring buffer state.
pub(super) struct XtcOutputBuffers {
    /// Output accumulator for overlap-add (flat interleaved ring buffer)
    /// Layout: [L0, R0, L1, R1, ...]
    /// Buffer size in frames is always power-of-2 (4 * fft_size) for efficient masking
    pub(super) output_accumulator: Vec<f32>,
    /// Bitmask for ring buffer frame index (buffer_frames - 1)
    pub(super) output_accumulator_mask: usize,
    /// Number of valid frames in output accumulator
    pub(super) output_accumulator_fill: usize,
    /// Next frame position to add a block (tracks overlap-add offset)
    pub(super) next_add_position: usize,
    /// Current read frame position in the output accumulator ring buffer
    pub(super) output_read_position: usize,
    /// Declared causal delay still to emit as startup silence.
    pub(super) startup_delay_remaining: usize,
    /// Negative-origin synthesis frames to discard before programme time zero.
    pub(super) synthesis_prefix_remaining: usize,
}

/// Fixed-latency dry path and sample-counted, complementary wet transition.
pub(super) struct XtcBypassState {
    pub(super) dry: Vec<f32>,
    pub(super) position: usize,
    pub(super) mix: f64,
    pub(super) step: f64,
    pub(super) remaining: usize,
    pub(super) duration: usize,
}

impl XtcBypassState {
    fn reset(&mut self, enabled: bool, rate: u32) {
        self.dry.fill(0.0);
        self.position = 0;
        self.mix = f64::from(enabled);
        self.step = 0.0;
        self.remaining = 0;
        // Integer rounding avoids a rate-dependent floating-point boundary.
        self.duration = ((u64::from(rate) + 50) / 100).max(1) as usize;
    }

    fn start(&mut self, enabled: bool) {
        self.remaining = self.duration;
        self.step = (f64::from(enabled) - self.mix) / self.duration as f64;
    }

    pub(super) fn apply(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        channels: usize,
        enabled: bool,
    ) {
        for (source, frame) in input
            .as_chunks::<2>()
            .0
            .iter()
            .zip(output.chunks_exact_mut(channels))
        {
            let dry = [self.dry[self.position], self.dry[self.position + 1]];
            self.dry[self.position..self.position + 2].copy_from_slice(source);
            self.position += 2;
            if self.position == self.dry.len() {
                self.position = 0;
            }
            if self.remaining > 0 {
                self.remaining -= 1;
                self.mix = if self.remaining == 0 {
                    f64::from(enabled)
                } else {
                    (self.mix + self.step).clamp(0.0, 1.0)
                };
            }
            if self.mix == 1.0 {
                // Preserve the original all-enabled output bit for bit.
                continue;
            }
            for (ch, wet) in frame.iter_mut().enumerate() {
                let dry = dry.get(ch).copied().unwrap_or(0.0);
                *wet = if self.mix == 0.0 {
                    dry
                } else {
                    // A convex combination in f64 cannot overflow merely
                    // because finite f32 dry/wet have opposite overrange signs.
                    ((1.0 - self.mix) * f64::from(dry) + self.mix * f64::from(*wet)) as f32
                };
            }
        }
    }
}

/// Prepared finite-stream continuation with one canonical-hop output cache.
#[derive(Default)]
pub(super) struct XtcDrainState {
    pub(super) received_input: bool,
    pub(super) input_phase: usize,
    /// Preserve the accepted EOF support declaration through refill/completion.
    pub(super) tail_bound: Option<usize>,
    /// Total frames not yet returned, including unread cache; Some freezes EOF.
    pub(super) remaining: Option<usize>,
    pub(super) zeros: Vec<f32>,
    pub(super) output: Vec<f32>,
    pub(super) cached_frames: usize,
    pub(super) cache_position: usize,
}

/// Output dynamics processing (auto-gain + peak limiter).
pub(super) struct XtcDynamics {
    /// Auto-gain compensation to match output loudness to input
    pub(super) auto_gain: Option<AutoGain>,

    /// Per-sample limiter envelope (0.0..=1.0). Smooth attack and release.
    /// Prevents output from exceeding ±0.95 after XTC filter summation + auto-gain.
    pub(super) limiter_envelope: f32,

    /// Per-sample attack coefficient for the limiter (~0.2ms time constant).
    pub(super) limiter_attack_coeff: f32,

    /// Per-sample release coefficient for the limiter (~50ms release).
    pub(super) limiter_release_coeff: f32,
}

/// Diagnostic and parameter caching state.
pub(super) struct XtcDiagnostics {
    /// Diagnostic data cache (Real-time safe)
    pub(super) cache: RealTimeCache<XtcData>,

    /// Frames since the last sample-clock AutoGain measurement refresh
    pub(super) auto_gain_frames: usize,

    pub(super) cached_parameters: Vec<Parameter>,
}

/// Crosstalk Cancellation plugin
///
/// Optimized with:
/// - Block-based I/O processing (no sample-by-sample loops)
/// - SIMD complex multiplication for frequency domain filtering
/// - Modulo-free wrapped overlap-add regions
/// - Contiguous ring drain/copy/clear operations
/// - Asynchronous filter recomputation to avoid audio glitches
pub struct XtcPlugin {
    /// Configuration parameters
    pub(super) params: XtcPluginParams,

    /// FFT / STFT configuration and shared resources
    pub(super) fft: XtcFftConfig,

    /// Input staging buffers
    pub(super) input: XtcInputBuffers,

    /// Overlap-add output ring buffer state
    pub(super) output: XtcOutputBuffers,

    /// Working buffers for FFT / IFFT
    pub(super) work: XtcWorkBuffers,

    /// Filter state, asynchronous updates, crossfade smoothing, room/HRTF data
    pub(super) filter_state: XtcFilterState,

    pub(super) bypass: XtcBypassState,

    /// Finite continuation storage and the accepted-input clock.
    pub(super) drain_state: XtcDrainState,

    /// Output dynamics (auto-gain + peak limiter)
    pub(super) dynamics: XtcDynamics,

    /// Diagnostic and parameter caching state
    pub(super) diagnostics: XtcDiagnostics,

    /// Set by `initialize()`. `process()` rejects blocks until it is set so
    /// a pre-init call fails fast instead of running on staging buffers that
    /// were never sized for the active sample rate.
    pub(super) initialized: bool,
}

/// Control-side candidate; nothing here is published before preparation succeeds.
struct PreparedInitialization {
    update: PendingFilterUpdate,
    auto_gain: Option<AutoGain>,
}

impl XtcPlugin {
    const STRUCTURAL_SOURCE_ERROR: &'static str =
        "XTC source/artifact changes are structural and require rebuilding the plugin graph";

    /// Return the causal SOFA offset represented by the active plant, in seconds.
    ///
    /// The value is absent when the active source is not an HRTF file. A
    /// returned zero means the source required no common causal rebase.
    pub fn sofa_delay_rebase_seconds(&self) -> Option<f64> {
        self.filter_state
            .hrtf_transfer_functions
            .as_ref()
            .map(|hrtf| hrtf.delay_rebase_seconds)
    }

    /// Validate a source configuration before exposing it through parameters.
    ///
    /// Source changes are structural: an asynchronous recompute may fail after
    /// a setter returns, so accepting a configuration that cannot produce its
    /// requested plant would leave the UI state ahead of the effective filters.
    /// Validate the complete candidate synchronously while the control thread is
    /// still allowed to perform file I/O.
    fn validate_source_configuration(
        params: &XtcPluginParams,
        sample_rate: f64,
        num_bins: usize,
    ) -> Result<(), String> {
        match params.source_mode.as_str() {
            "synthetic" | "roomeq_recommended" if params.hrtf_file.is_some() => Err(format!(
                "source_mode='{}' cannot be combined with hrtf_file; use source_mode='hrtf_file'",
                params.source_mode
            )),
            "hrtf_file" => {
                let hrtf_path = params
                    .hrtf_file
                    .as_deref()
                    .ok_or_else(|| "source_mode='hrtf_file' requires hrtf_file".to_string())?;
                load_hrtf_for_xtc(hrtf_path, params, sample_rate, num_bins).map(|_| ())
            }
            "roomeq_recommended" => {
                validate_roomeq_recommended_source(params, sample_rate, num_bins)
            }
            "synthetic" => Ok(()),
            other => Err(format!(
                "source_mode must be 'synthetic', 'hrtf_file', or 'roomeq_recommended', got '{}'",
                other
            )),
        }
    }

    fn validate_room_ir_configuration(
        params: &XtcPluginParams,
        sample_rate: f64,
        num_bins: usize,
        fft_forward: Arc<dyn RealToComplex<f32>>,
    ) -> Result<(), String> {
        if params.room_reflections_enabled
            && let Some(path) = params.room_ir_file.as_deref()
        {
            build_reflection_data_ir(path, sample_rate, num_bins, Some(fft_forward))?;
        }
        Ok(())
    }

    /// Create a new XTC plugin
    pub fn new(params: XtcPluginParams, sample_rate: impl Into<f64>) -> Result<Self, String> {
        let sample_rate = sample_rate.into();
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("XTC sample rate must be finite and positive".into());
        }
        match params.source_mode.as_str() {
            "synthetic" if params.hrtf_file.is_some() => {
                return Err(
                    "source_mode='synthetic' cannot be combined with hrtf_file; use source_mode='hrtf_file'"
                        .to_string(),
                );
            }
            "hrtf_file" if params.hrtf_file.is_none() => {
                return Err("source_mode='hrtf_file' requires hrtf_file".to_string());
            }
            "synthetic" | "hrtf_file" | "roomeq_recommended" => {}
            other => {
                return Err(format!(
                    "source_mode must be 'synthetic', 'hrtf_file', or 'roomeq_recommended', got '{}'",
                    other
                ));
            }
        }

        // Validate FFT size
        if !params.fft_size.is_power_of_two() {
            return Err(format!(
                "XTC FFT size must be power of 2, got {}",
                params.fft_size
            ));
        }

        if params.fft_size < 128 || params.fft_size > 16384 {
            return Err(format!(
                "XTC FFT size must be between 128 and 16384, got {}",
                params.fft_size
            ));
        }

        // Validate IR file path if provided
        if let Some(ref ir_path) = params
            .room_ir_file
            .as_ref()
            .filter(|p| !std::path::Path::new(p.as_str()).exists())
        {
            return Err(format!("Room IR file not found: {}", ir_path));
        }

        let fft_size = params.fft_size;
        let hop_size = fft_size / 4; // 75% overlap

        // Create FFT planners
        let mut planner = RealFftPlanner::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        // Periodic Hann window for STFT
        let analysis_window = generate_hann_window(fft_size);

        // Combined scale factor: COLA normalization / FFT size
        // For 75% overlap dual-windowing Hann, Sum(w^2) = 1.5.
        // scale = 1.0 / (1.5 * N).
        let output_scale = 1.0 / (fft_size as f32 * 1.5);

        // Compute frequency-domain filters
        let num_bins = fft_size / 2 + 1;
        Self::validate_room_ir_configuration(&params, sample_rate, num_bins, fft_forward.clone())?;

        // Compute initial room reflection data if enabled (Optimization 4)
        let room_params_hash = compute_room_params_hash(&params);
        let room_reflection_cache = if params.room_reflections_enabled {
            // Pass the pre-planned FFT to avoid re-creating the planner (Optimization 4)
            compute_room_reflection_data(&params, sample_rate, num_bins, Some(fft_forward.clone()))
        } else {
            None
        };

        // Load HRTF file if specified. The roomEQ recommended source bypasses
        // geometry/HRTF solving and loads its co-designed filters directly.
        let hrtf_transfer_functions = if params.source_mode == "hrtf_file" {
            let hrtf_path = params.hrtf_file.as_deref().expect("validated above");
            load_hrtf_for_xtc(hrtf_path, &params, sample_rate, num_bins)?.map(Arc::new)
        } else {
            None
        };

        let filters = if params.source_mode == "roomeq_recommended" {
            let matrix_path = params.recommended_matrix_file.as_deref().ok_or_else(|| {
                "source_mode='roomeq_recommended' requires recommended_matrix_file".to_string()
            })?;
            load_roomeq_recommended_filters(matrix_path, sample_rate, num_bins)?
        } else {
            // Compute geometry cache (Optimization 3)
            let cache = compute_geometry_cache(&params, sample_rate, num_bins);
            compute_xtc_filters_full_with_cache_and_hrtf(
                &params,
                sample_rate,
                num_bins,
                &cache,
                room_reflection_cache.clone(),
                hrtf_transfer_functions.as_deref(),
            )
        };
        let output_channels = filters.output_channels();
        let drain_samples = hop_size
            .checked_mul(output_channels)
            .ok_or_else(|| "XTC drain output dimensions overflow".to_string())?;
        let cached_current_filters = Arc::new(filters);
        let exchange = Arc::new(Mutex::new(XtcFilterExchange::default()));
        // Initialize lazy native mutex resources at their final Arc address,
        // on the construction thread, including the macOS pthread backend.
        drop(exchange.lock().expect("new filter exchange is unpoisoned"));
        let active_filter_update = Some(Arc::new(PendingFilterUpdate {
            generation: 0,
            filters: Arc::clone(&cached_current_filters),
            hrtf_transfer_functions: hrtf_transfer_functions.clone(),
            room_reflection_cache: room_reflection_cache.clone(),
            room_params_hash,
        }));

        let auto_gain = if params.auto_gain_enabled && output_channels == 2 {
            Some(
                AutoGain::new(
                    2, // stereo
                    sample_rate,
                    AutoGainParams {
                        enabled: true,
                        loudness_type: Default::default(),
                        max_gain_db: params.auto_gain_max_db,
                        smoothing_ms: params.auto_gain_smoothing_ms,
                    },
                )
                .map_err(|e| format!("AutoGain init failed: {}", e))?,
            )
        } else {
            None
        };

        let mut p = Self {
            params: params.clone(),
            fft: XtcFftConfig {
                fft_size,
                hop_size,
                sample_rate,
                fft_forward,
                fft_inverse,
                analysis_window,
                output_scale,
            },
            input: XtcInputBuffers {
                input_buffer_l: vec![0.0; fft_size],
                input_buffer_r: vec![0.0; fft_size],
                input_fill: fft_size - hop_size,
                temp_input_l: vec![0.0; MAX_PROCESS_FRAMES],
                temp_input_r: vec![0.0; MAX_PROCESS_FRAMES],
            },
            output: XtcOutputBuffers {
                output_accumulator: vec![0.0; fft_size * 4 * output_channels],
                output_accumulator_mask: (fft_size * 4) - 1,
                output_accumulator_fill: 0,
                next_add_position: 0,
                output_read_position: fft_size - hop_size,
                startup_delay_remaining: fft_size,
                synthesis_prefix_remaining: fft_size - hop_size,
            },
            work: XtcWorkBuffers {
                fft_buffer: vec![0.0; fft_size],
                fft_output_l: vec![Complex::new(0.0, 0.0); num_bins],
                fft_output_r: vec![Complex::new(0.0, 0.0); num_bins],
                ifft_input: vec![Complex::new(0.0, 0.0); num_bins],
                ifft_output: vec![0.0; fft_size],
                prev_ifft_output: vec![0.0; fft_size],
            },
            filter_state: XtcFilterState {
                cached_current_filters,
                exchange,
                active_filter_update,
                filter_update_generation: Arc::new(AtomicU64::new(0)),
                filter_request: Arc::new(Mutex::new(None)),
                filter_worker_waker: None,
                filter_worker_launches: Arc::new(AtomicU64::new(0)),
                #[cfg(test)]
                worker_test_barrier: None,
                prev_filters: None,
                crossfade_progress: 1.0, // Start fully faded to current
                progress_per_hop: 0.0,
                hrtf_transfer_functions,
                room_reflection_cache,
                room_params_hash,
            },
            bypass: XtcBypassState {
                dry: vec![0.0; fft_size * 2],
                position: 0,
                mix: f64::from(params.enabled),
                step: 0.0,
                remaining: 0,
                duration: (sample_rate / 100.0).round().max(1.0) as usize,
            },
            drain_state: XtcDrainState {
                zeros: vec![0.0; hop_size * 2],
                output: vec![0.0; drain_samples],
                ..Default::default()
            },
            dynamics: XtcDynamics {
                auto_gain,
                limiter_envelope: 1.0,
                limiter_attack_coeff: math_audio_dsp::fast_math::fast_exp(
                    -1.0 / (0.2 * 0.001 * sample_rate as f32),
                ),
                limiter_release_coeff: math_audio_dsp::fast_math::fast_exp(
                    -1.0 / (50.0 * 0.001 * sample_rate as f32),
                ),
            },
            diagnostics: XtcDiagnostics {
                cache: RealTimeCache::new(XtcData::default()),
                auto_gain_frames: 0,
                cached_parameters: Vec::new(),
            },
            initialized: false,
        };
        p.rebuild_cached_parameters();
        Ok(p)
    }

    /// Get the f64 value of parameter at PARAMS index.
    /// Order must match params::PARAMS exactly.
    pub(super) fn param_value(&self, index: usize) -> Option<f64> {
        match index {
            0 => Some(self.params.distance_m as f64),
            1 => Some(self.params.speaker_angle_deg as f64),
            2 => Some(self.params.head_radius_m as f64),
            3 => Some(self.params.head_offset_x as f64),
            4 => Some(self.params.head_offset_z as f64),
            5 => Some(self.params.head_yaw_deg as f64),
            6 => Some(self.params.head_tracking_smooth_s as f64),
            7 => Some(self.params.beta_base as f64),
            8 => Some(self.params.beta_low_freq_boost as f64),
            9 => Some(self.params.beta_high_freq_boost as f64),
            10 => Some(self.params.head_shadow_cutoff_hz as f64),
            11 => Some(self.params.head_shadow_slope_db_per_octave as f64),
            12 => Some(self.params.max_gain_db as f64),
            13 => Some(if self.params.spectral_normalization {
                1.0
            } else {
                0.0
            }),
            14 => Some(if self.params.pinna_model_enabled {
                1.0
            } else {
                0.0
            }),
            15 => Some(if self.params.room_reflections_enabled {
                1.0
            } else {
                0.0
            }),
            16 => None, // room_ir_file (FilePath) is handled as structural string state.
            17 => Some(self.params.room_width_m as f64),
            18 => Some(self.params.room_depth_m as f64),
            19 => Some(self.params.wall_absorption as f64),
            20 => Some(self.params.reflection_beta_boost as f64),
            21 => Some(if self.params.bypass_xtc_filters {
                1.0
            } else {
                0.0
            }),
            22 => Some(if self.params.bypass_spectral_normalization {
                1.0
            } else {
                0.0
            }),
            23 => Some(if self.params.bypass_neumann_refinement {
                1.0
            } else {
                0.0
            }),
            24 => Some(if self.params.auto_gain_enabled {
                1.0
            } else {
                0.0
            }),
            25 => Some(self.params.auto_gain_max_db as f64),
            26 => Some(self.params.auto_gain_smoothing_ms as f64),
            27 => Some(self.params.head_model as f64),
            _ => None,
        }
    }

    /// Set the f64 value of parameter at PARAMS index.
    /// Order must match params::PARAMS exactly.
    pub(super) fn set_param_value(&mut self, index: usize, value: f64) {
        match index {
            0 => self.params.distance_m = XT[0].clamp_f64(value) as f32,
            1 => self.params.speaker_angle_deg = XT[1].clamp_f64(value) as f32,
            2 => self.params.head_radius_m = XT[2].clamp_f64(value) as f32,
            3 => self.params.head_offset_x = XT[3].clamp_f64(value) as f32,
            4 => self.params.head_offset_z = XT[4].clamp_f64(value) as f32,
            5 => self.params.head_yaw_deg = XT[5].clamp_f64(value) as f32,
            6 => self.params.head_tracking_smooth_s = XT[6].clamp_f64(value) as f32,
            7 => self.params.beta_base = XT[7].clamp_f64(value) as f32,
            8 => self.params.beta_low_freq_boost = XT[8].clamp_f64(value) as f32,
            9 => self.params.beta_high_freq_boost = XT[9].clamp_f64(value) as f32,
            10 => self.params.head_shadow_cutoff_hz = XT[10].clamp_f64(value) as f32,
            11 => self.params.head_shadow_slope_db_per_octave = XT[11].clamp_f64(value) as f32,
            12 => self.params.max_gain_db = XT[12].clamp_f64(value) as f32,
            13 => self.params.spectral_normalization = XT[13].clamp_f64(value) > 0.5,
            14 => self.params.pinna_model_enabled = XT[14].clamp_f64(value) > 0.5,
            15 => self.params.room_reflections_enabled = XT[15].clamp_f64(value) > 0.5,
            16 => {}
            17 => self.params.room_width_m = XT[17].clamp_f64(value) as f32,
            18 => self.params.room_depth_m = XT[18].clamp_f64(value) as f32,
            19 => self.params.wall_absorption = XT[19].clamp_f64(value) as f32,
            20 => self.params.reflection_beta_boost = XT[20].clamp_f64(value) as f32,
            21 => self.params.bypass_xtc_filters = XT[21].clamp_f64(value) > 0.5,
            22 => self.params.bypass_spectral_normalization = XT[22].clamp_f64(value) > 0.5,
            23 => self.params.bypass_neumann_refinement = XT[23].clamp_f64(value) > 0.5,
            24 => self.params.auto_gain_enabled = XT[24].clamp_f64(value) > 0.5,
            25 => self.params.auto_gain_max_db = XT[25].clamp_f64(value) as f32,
            26 => self.params.auto_gain_smoothing_ms = XT[26].clamp_f64(value) as f32,
            27 => self.params.head_model = XT[27].clamp_f64(value) as usize,
            _ => {}
        }
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        self.diagnostics.cached_parameters =
            param_bridge::build_parameters(XT, |i| self.param_value(i));
        // Append parameters not in PARAMS
        self.diagnostics.cached_parameters.push(Parameter::new_bool(
            "enabled",
            "Enabled",
            self.params.enabled,
        ));
        self.diagnostics
            .cached_parameters
            .push(Parameter::new_float(
                "kappa_target",
                "Kappa Target",
                self.params.kappa_target,
                1.0,
                1000.0,
            ));
        self.diagnostics
            .cached_parameters
            .push(Parameter::new_string(
                "hrtf_file",
                "HRTF File",
                self.params.hrtf_file.clone().unwrap_or_default(),
            ));
        self.diagnostics
            .cached_parameters
            .push(Parameter::new_string(
                "source_mode",
                "Source Mode",
                self.params.source_mode.clone(),
            ));
        self.diagnostics
            .cached_parameters
            .push(Parameter::new_string(
                "recommended_matrix_file",
                "roomEQ Matrix",
                self.params
                    .recommended_matrix_file
                    .clone()
                    .unwrap_or_default(),
            ));
        self.diagnostics
            .cached_parameters
            .push(Parameter::new_string(
                "itd_modeling",
                "ITD Mode",
                self.params.itd_modeling.clone(),
            ));
    }

    /// Create from parameters helper
    pub fn from_params(
        params: XtcPluginParams,
        sample_rate: impl Into<f64>,
    ) -> Result<Self, String> {
        Self::new(params, sample_rate)
    }

    pub(super) fn set_crossfade_rate(&mut self) {
        let smooth_samples = self.params.head_tracking_smooth_s * self.fft.sample_rate as f32;
        self.filter_state.progress_per_hop = if smooth_samples > 0.0 {
            self.fft.hop_size as f32 / smooth_samples
        } else {
            1.0
        };
    }

    /// Load a complete target-rate configuration without changing the live epoch.
    fn prepare_initialization(&self, sample_rate: f64) -> PluginResult<PreparedInitialization> {
        let num_bins = self.fft.fft_size / 2 + 1;
        // Synchronous initialization reloads artifacts once and uses those exact
        // in-memory results. The parameter hash alone does not include rate.
        let room_data = if !self.params.room_reflections_enabled {
            None
        } else if let Some(path) = self.params.room_ir_file.as_deref() {
            Some(Arc::new(build_reflection_data_ir(
                path,
                sample_rate,
                num_bins,
                Some(Arc::clone(&self.fft.fft_forward)),
            )?))
        } else {
            Some(Arc::new(build_reflection_data_image_source(
                &self.params,
                sample_rate,
                num_bins,
            )))
        };
        let hrtf_data = if self.params.source_mode == "hrtf_file" {
            let path = self
                .params
                .hrtf_file
                .as_deref()
                .ok_or("source_mode='hrtf_file' requires hrtf_file")?;
            load_hrtf_for_xtc(path, &self.params, sample_rate, num_bins)?.map(Arc::new)
        } else {
            None
        };
        let filters = if self.params.source_mode == "roomeq_recommended" {
            let path = self
                .params
                .recommended_matrix_file
                .as_deref()
                .ok_or("source_mode='roomeq_recommended' requires recommended_matrix_file")?;
            load_roomeq_recommended_filters(path, sample_rate, num_bins)?
        } else {
            let geometry = compute_geometry_cache(&self.params, sample_rate, num_bins);
            compute_xtc_filters_full_with_cache_and_hrtf(
                &self.params,
                sample_rate,
                num_bins,
                &geometry,
                room_data.clone(),
                hrtf_data.as_deref(),
            )
        };
        if filters.output_channels() != self.output_channels() {
            return Err(format!(
                "XTC initialization cannot change output channels from {} to {}; rebuild the plugin graph",
                self.output_channels(),
                filters.output_channels(),
            ));
        }
        let auto_gain = if self.params.auto_gain_enabled && self.output_channels() == 2 {
            Some(AutoGain::new(
                2,
                sample_rate,
                AutoGainParams {
                    enabled: true,
                    loudness_type: Default::default(),
                    max_gain_db: self.params.auto_gain_max_db,
                    smoothing_ms: self.params.auto_gain_smoothing_ms,
                },
            )?)
        } else {
            None
        };
        Ok(PreparedInitialization {
            update: PendingFilterUpdate {
                // The commit assigns a new generation only after all loading
                // and allocation that can return an error has succeeded.
                generation: 0,
                filters: Arc::new(filters),
                hrtf_transfer_functions: hrtf_data,
                room_reflection_cache: room_data,
                room_params_hash: compute_room_params_hash(&self.params),
            },
            auto_gain,
        })
    }

    /// Request asynchronous filters using the existing latest-only worker.
    pub(super) fn update_filters(&mut self) {
        let num_bins = self.fft.fft_size / 2 + 1;
        let sample_rate = self.fft.sample_rate;
        self.set_crossfade_rate();
        // Latest-only coalescing worker. Expensive recomputation never
        // fans out onto Rayon's global pool under high-rate automation.
        let generation = self
            .filter_state
            .filter_update_generation
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        let request = FilterUpdateRequest {
            generation,
            params: self.params.clone(),
            sample_rate,
            num_bins,
            expected_output_channels: self.output_channels(),
            fft_forward: self.fft.fft_forward.clone(),
        };
        *self
            .filter_state
            .filter_request
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(request);
        if let Some(waker) = self.filter_state.filter_worker_waker.as_ref() {
            match waker.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => return,
                Err(TrySendError::Disconnected(())) => {
                    self.filter_state.filter_worker_waker = None;
                }
            }
        }

        let request_mailbox = self.filter_state.filter_request.clone();
        let worker_launches = self.filter_state.filter_worker_launches.clone();
        let exchange = Arc::clone(&self.filter_state.exchange);
        let requested_generation = self.filter_state.filter_update_generation.clone();
        let (worker_waker, worker_wakeups) = std::sync::mpsc::sync_channel(1);
        #[cfg(test)]
        let worker_test_barrier = self.filter_state.worker_test_barrier.clone();
        let spawn_result = std::thread::Builder::new()
            .name("xtc-filter-worker".to_string())
            .spawn(move || {
                while worker_wakeups.recv().is_ok() {
                    loop {
                        // A bounded wakeup only says that the latest-only
                        // mailbox may contain work. Drain stale wakeups so
                        // each iteration computes at most the newest request.
                        while worker_wakeups.try_recv().is_ok() {}

                        // Reclaim audio-thread state before producing another
                        // publication. The callback only transfers ownership
                        // into these slots; it never destroys filter objects.
                        let retired = {
                            let mut exchange =
                                exchange.lock().unwrap_or_else(|error| error.into_inner());
                            (
                                std::mem::take(&mut exchange.retired_updates),
                                std::mem::take(&mut exchange.retired_snapshots),
                            )
                        };
                        drop(retired);
                        let request = request_mailbox
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .take();
                        let Some(request) = request else {
                            break;
                        };
                        if let Some(update) = Self::compute_filter_update(&request)
                            && requested_generation.load(Ordering::Acquire) == request.generation
                        {
                            let update = Arc::new(update);
                            #[cfg(test)]
                            let test_paused = worker_test_barrier.as_ref().is_some_and(|barrier| {
                                barrier.after_check(request.sample_rate, &update)
                            });
                            let displaced = exchange
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .pending
                                .replace(update);
                            drop(displaced);
                            #[cfg(test)]
                            if test_paused {
                                worker_test_barrier.as_ref().unwrap().after_publication();
                            }
                        }
                    }
                }
            });
        if spawn_result.is_ok() {
            worker_launches.fetch_add(1, Ordering::Relaxed);
            let _ = worker_waker.try_send(());
            self.filter_state.filter_worker_waker = Some(worker_waker);
        }
    }

    fn compute_filter_update(request: &FilterUpdateRequest) -> Option<PendingFilterUpdate> {
        let params = &request.params;
        let room_params_hash = compute_room_params_hash(params);
        let room_data = compute_room_reflection_data(
            params,
            request.sample_rate,
            request.num_bins,
            Some(request.fft_forward.clone()),
        );
        let hrtf_data = if params.source_mode == "hrtf_file" {
            let hrtf_path = params.hrtf_file.as_deref()?;
            load_hrtf_for_xtc(hrtf_path, params, request.sample_rate, request.num_bins)
                .ok()?
                .map(Arc::new)
        } else {
            None
        };
        let new_filters = if params.source_mode == "roomeq_recommended" {
            let matrix_path = params.recommended_matrix_file.as_deref()?;
            load_roomeq_recommended_filters(matrix_path, request.sample_rate, request.num_bins)
                .ok()?
        } else {
            let cache = compute_geometry_cache(params, request.sample_rate, request.num_bins);
            compute_xtc_filters_full_with_cache_and_hrtf(
                params,
                request.sample_rate,
                request.num_bins,
                &cache,
                room_data.clone(),
                hrtf_data.as_deref(),
            )
        };
        if new_filters.output_channels() != request.expected_output_channels {
            return None;
        }
        Some(PendingFilterUpdate {
            generation: request.generation,
            filters: Arc::new(new_filters),
            hrtf_transfer_functions: hrtf_data,
            room_reflection_cache: room_data,
            room_params_hash,
        })
    }

    pub(super) fn adopt_pending_filters(&mut self) {
        self.retire_completed_filter_snapshot();
        let state = &mut self.filter_state;
        let Ok(mut exchange) = state.exchange.try_lock() else {
            return;
        };
        let Some(pending) = &exchange.pending else {
            return;
        };
        let stale = pending.generation != state.filter_update_generation.load(Ordering::Acquire);
        let mismatched =
            pending.filters.output_channels() != state.cached_current_filters.output_channels();
        let update_slot = exchange.retired_updates.iter().position(Option::is_none);
        if stale || mismatched {
            if let Some(slot) = update_slot {
                exchange.retired_updates[slot] = exchange.pending.take();
            }
            return;
        }
        // Reserve all required retirement capacity before taking a publication.
        // If the worker is delayed, keep the current fade and latest pending
        // update intact. No callback path overwrites a live retired owner.
        if state.active_filter_update.is_some() && update_slot.is_none() {
            return;
        }
        let snapshot_slot = exchange.retired_snapshots.iter().position(Option::is_none);
        if state.prev_filters.is_some() && snapshot_slot.is_none() {
            return;
        }
        let update = exchange
            .pending
            .take()
            .expect("pending publication checked");
        let previous = std::mem::replace(
            &mut state.cached_current_filters,
            Arc::clone(&update.filters),
        );
        // The old active bundle still owns these auxiliary resources.
        state.hrtf_transfer_functions = update.hrtf_transfer_functions.clone();
        state.room_reflection_cache = update.room_reflection_cache.clone();
        state.room_params_hash = update.room_params_hash;
        if let Some(previous_crossfade) = state.prev_filters.replace(previous) {
            exchange.retired_snapshots[snapshot_slot.expect("retirement capacity checked")] =
                Some(previous_crossfade);
        }
        state.crossfade_progress = 0.0;
        if let Some(previous_update) = state.active_filter_update.replace(update) {
            exchange.retired_updates[update_slot.expect("retirement capacity checked")] =
                Some(previous_update);
        }
    }

    /// Retain a completed fade until the worker can accept its ownership.
    fn retire_completed_filter_snapshot(&mut self) {
        let state = &mut self.filter_state;
        if state.crossfade_progress < 1.0 || state.prev_filters.is_none() {
            return;
        }
        let Ok(mut exchange) = state.exchange.try_lock() else {
            return;
        };
        if let Some(slot) = exchange.retired_snapshots.iter().position(Option::is_none) {
            exchange.retired_snapshots[slot] = state.prev_filters.take();
        }
    }

    /// Process one STFT frame using SIMD-optimized operations.
    ///
    /// During crossfade (after parameter change), blends output from old and new
    /// filters over ~100ms to avoid clicks. This costs 4 IFFTs per frame instead
    /// of the normal 2, but crossfade transitions are brief.
    #[inline(always)]
    pub(super) fn process_stft_frame(&mut self) {
        // Window and FFT left channel (SIMD optimized)
        window_mul_simd(
            &mut self.work.fft_buffer,
            &self.input.input_buffer_l,
            &self.fft.analysis_window,
        );
        self.fft
            .fft_forward
            .process(&mut self.work.fft_buffer, &mut self.work.fft_output_l)
            .expect("FFT processing failed");

        // Window and FFT right channel (SIMD optimized)
        window_mul_simd(
            &mut self.work.fft_buffer,
            &self.input.input_buffer_r,
            &self.fft.analysis_window,
        );
        self.fft
            .fft_forward
            .process(&mut self.work.fft_buffer, &mut self.work.fft_output_r)
            .expect("FFT processing failed");

        let scale = self.fft.output_scale;
        let mask = self.output.output_accumulator_mask;

        // Diagnostic bypass: skip all XTC filter math, just IFFT the windowed input.
        // This tests whether the STFT framework (windowing + OLA) itself is clean.
        if self.params.bypass_xtc_filters {
            let output_channels = self.filter_state.cached_current_filters.output_channels();

            // Left channel: IFFT the FFT output directly (identity in freq domain)
            self.work
                .ifft_input
                .copy_from_slice(&self.work.fft_output_l);
            let n = self.work.ifft_input.len();
            self.work.ifft_input[0].im = 0.0;
            self.work.ifft_input[n - 1].im = 0.0;
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            // Accumulate Left
            accumulate_ifft_channel(
                &mut self.output.output_accumulator,
                self.output.next_add_position,
                output_channels,
                0,
                &self.work.ifft_output,
                &self.fft.analysis_window,
                scale,
            );

            // Right channel
            self.work
                .ifft_input
                .copy_from_slice(&self.work.fft_output_r);
            self.work.ifft_input[0].im = 0.0;
            self.work.ifft_input[n - 1].im = 0.0;
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            // Accumulate Right
            if output_channels > 1 {
                accumulate_ifft_channel(
                    &mut self.output.output_accumulator,
                    self.output.next_add_position,
                    output_channels,
                    1,
                    &self.work.ifft_output,
                    &self.fft.analysis_window,
                    scale,
                );
            }
        } else if let Some(speaker_filters) = self
            .filter_state
            .cached_current_filters
            .speaker_filters
            .as_ref()
        {
            let current_filters = &self.filter_state.cached_current_filters;
            let output_channels = current_filters.output_channels();
            let can_crossfade = self.filter_state.crossfade_progress < 1.0
                && self
                    .filter_state
                    .prev_filters
                    .as_ref()
                    .and_then(|prev| prev.speaker_filters.as_ref())
                    .is_some_and(|prev| prev.len() == output_channels);
            let alpha = self.filter_state.crossfade_progress;

            for (speaker_idx, filters_for_speaker) in speaker_filters.iter().enumerate() {
                if can_crossfade {
                    let prev_filters = self.filter_state.prev_filters.as_ref().unwrap();
                    let prev_speaker_filters = prev_filters.speaker_filters.as_ref().unwrap();
                    // Blend prev and current filters in frequency domain → 1 IFFT instead of 2.
                    apply_filter_pair_blended(
                        &mut self.work.ifft_input,
                        &self.work.fft_output_l,
                        &self.work.fft_output_r,
                        &prev_speaker_filters[speaker_idx][0],
                        &prev_speaker_filters[speaker_idx][1],
                        &filters_for_speaker[0],
                        &filters_for_speaker[1],
                        alpha,
                    );
                    self.fft
                        .fft_inverse
                        .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                        .expect("IFFT processing failed");

                    accumulate_ifft_channel(
                        &mut self.output.output_accumulator,
                        self.output.next_add_position,
                        output_channels,
                        speaker_idx,
                        &self.work.ifft_output,
                        &self.fft.analysis_window,
                        scale,
                    );
                } else {
                    apply_filter_pair(
                        &mut self.work.ifft_input,
                        &self.work.fft_output_l,
                        &self.work.fft_output_r,
                        &filters_for_speaker[0],
                        &filters_for_speaker[1],
                    );
                    self.fft
                        .fft_inverse
                        .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                        .expect("IFFT processing failed");

                    accumulate_ifft_channel(
                        &mut self.output.output_accumulator,
                        self.output.next_add_position,
                        output_channels,
                        speaker_idx,
                        &self.work.ifft_output,
                        &self.fft.analysis_window,
                        scale,
                    );
                }
            }
        } else if self.filter_state.crossfade_progress < 1.0
            && let Some(prev_filters) = self.filter_state.prev_filters.as_ref()
        {
            let alpha = self.filter_state.crossfade_progress;
            // Use cached filter snapshot (loaded once per process() call)
            let current_filters = &self.filter_state.cached_current_filters;

            // --- Left channel with frequency-domain crossfade (1 IFFT instead of 2) ---
            // Blend prev and current filters per bin, then run a single IFFT.
            // Valid because IFFT is linear: (1-α)·IFFT(prev) + α·IFFT(curr) = IFFT(blended).
            apply_filter_left_blended(
                &mut self.work.ifft_input,
                &self.work.fft_output_l,
                &self.work.fft_output_r,
                prev_filters,
                current_filters,
                alpha,
            );
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            accumulate_ifft_channel(
                &mut self.output.output_accumulator,
                self.output.next_add_position,
                2,
                0,
                &self.work.ifft_output,
                &self.fft.analysis_window,
                scale,
            );

            // --- Right channel with frequency-domain crossfade (1 IFFT instead of 2) ---
            apply_filter_right_blended(
                &mut self.work.ifft_input,
                &self.work.fft_output_l,
                &self.work.fft_output_r,
                prev_filters,
                current_filters,
                alpha,
            );
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            accumulate_ifft_channel(
                &mut self.output.output_accumulator,
                self.output.next_add_position,
                2,
                1,
                &self.work.ifft_output,
                &self.fft.analysis_window,
                scale,
            );
        } else {
            // Normal path: no crossfade needed
            let filters = &self.filter_state.cached_current_filters;

            // Left channel
            apply_filter_left(
                &mut self.work.ifft_input,
                &self.work.fft_output_l,
                &self.work.fft_output_r,
                filters,
            );
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            accumulate_ifft_channel(
                &mut self.output.output_accumulator,
                self.output.next_add_position,
                2,
                0,
                &self.work.ifft_output,
                &self.fft.analysis_window,
                scale,
            );

            // Right channel
            apply_filter_right(
                &mut self.work.ifft_input,
                &self.work.fft_output_l,
                &self.work.fft_output_r,
                filters,
            );
            self.fft
                .fft_inverse
                .process(&mut self.work.ifft_input, &mut self.work.ifft_output)
                .expect("IFFT processing failed");

            accumulate_ifft_channel(
                &mut self.output.output_accumulator,
                self.output.next_add_position,
                2,
                1,
                &self.work.ifft_output,
                &self.fft.analysis_window,
                scale,
            );
        }

        if self.output.synthesis_prefix_remaining > 0 {
            // Each of the first three windows finalizes one negative-time hop.
            // Discard it after accumulation so no stale contribution survives a
            // ring wrap. The remaining nonnegative samples retain all overlaps.
            let channels = self.output_channels();
            let start = self.output.next_add_position * channels;
            let end = start + self.fft.hop_size * channels;
            self.output.output_accumulator[start..end].fill(0.0);
            self.output.synthesis_prefix_remaining -= self.fft.hop_size;
        } else {
            self.output.output_accumulator_fill += self.fft.hop_size;
        }
        self.output.next_add_position = (self.output.next_add_position + self.fft.hop_size) & mask;

        // Advance crossfade progress
        if self.filter_state.crossfade_progress < 1.0 {
            self.filter_state.crossfade_progress = (self.filter_state.crossfade_progress
                + self.filter_state.progress_per_hop)
                .min(1.0);
            self.retire_completed_filter_snapshot();
        }
    }

    /// Advance validated audio without adopting an asynchronous publication.
    fn process_audio(&mut self, input: &[f32], output: &mut [f32], num_frames: usize) {
        let channels = self.output_channels();
        if self.dynamics.auto_gain.is_none() {
            self.process_wet_audio(input, output, num_frames);
            self.apply_wet_limiter(output, num_frames);
            flush_denormals_inplace(output);
            self.bypass
                .apply(input, output, channels, self.params.enabled);
            return;
        }

        // AutoGain exists only for stereo output. Refresh after each completed
        // sample interval, so new measurements affect only subsequent audio.
        debug_assert_eq!(channels, 2);
        let interval = (self.fft.sample_rate / 10.0).floor().max(1.0) as usize;
        let mut offset = 0;
        while offset < num_frames {
            let dry_start = self.bypass.position;
            let frames = (num_frames - offset)
                .min(interval - self.diagnostics.auto_gain_frames)
                .min((self.bypass.dry.len() - dry_start) / 2);
            let source = &input[offset * 2..(offset + frames) * 2];
            let wet = &mut output[offset * 2..(offset + frames) * 2];
            self.process_wet_audio(source, wet, frames);

            if let Some(ag) = &mut self.dynamics.auto_gain {
                // Before this contiguous ring span is advanced, it contains
                // precisely the original input delayed by the declared N.
                // Compare that reference with uncompensated wet audio.
                let _ = ag.ingest_input(&self.bypass.dry[dry_start..dry_start + frames * 2]);
                let _ = ag.ingest_output(wet);
                for frame in wet.as_chunks_mut::<2>().0 {
                    // The helper's near-target shortcut is evaluated on a
                    // fixed one-frame clock rather than caller boundaries.
                    ag.apply_compensation(frame, 1);
                }
            }
            self.apply_wet_limiter(wet, frames);
            flush_denormals_inplace(wet);
            self.bypass
                .apply(source, wet, channels, self.params.enabled);

            offset += frames;
            self.diagnostics.auto_gain_frames += frames;
            if self.diagnostics.auto_gain_frames == interval {
                self.diagnostics.auto_gain_frames = 0;
                if let Some(ag) = &mut self.dynamics.auto_gain {
                    ag.refresh_input_measurement();
                    ag.refresh_output_measurement();
                    let data = ag.get_data();
                    let envelope = self.dynamics.limiter_envelope;
                    self.diagnostics.cache.update(|cached| {
                        cached.auto_gain = data;
                        cached.limiter_envelope = envelope;
                    });
                }
            }
        }
    }

    /// Emit the unchanged windowed filter output on the accepted-input clock.
    fn process_wet_audio(&mut self, input: &[f32], output: &mut [f32], num_frames: usize) {
        let output_channels = self.output_channels();
        output.fill(0.0);
        let mut block_start = 0;
        while block_start < num_frames {
            let block_frames = self.input.temp_input_l.len().min(num_frames - block_start);
            debug_assert!(block_frames > 0, "initialize prepares nonempty staging");
            let input_start = block_start * 2;
            let input_end = input_start + block_frames * 2;
            deinterleave_stereo(
                &input[input_start..input_end],
                &mut self.input.temp_input_l[..block_frames],
                &mut self.input.temp_input_r[..block_frames],
            );

            // The accepted input clock owns progress. Queued output can never
            // end the loop early and discard an unconsumed callback suffix.
            for frame in 0..block_frames {
                self.input.input_buffer_l[self.input.input_fill] = self.input.temp_input_l[frame];
                self.input.input_buffer_r[self.input.input_fill] = self.input.temp_input_r[frame];
                self.input.input_fill += 1;
                if self.input.input_fill == self.fft.fft_size {
                    self.process_stft_frame();
                    self.shift_input_buffer();
                }

                if self.output.startup_delay_remaining > 0 {
                    self.output.startup_delay_remaining -= 1;
                } else if self.output.output_accumulator_fill > 0 {
                    let out_start = (block_start + frame) * output_channels;
                    self.output.output_read_position = drain_output_accumulator(
                        &mut self.output.output_accumulator,
                        self.output.output_read_position,
                        output_channels,
                        &mut output[out_start..out_start + output_channels],
                    );
                    self.output.output_accumulator_fill -= 1;
                }
            }
            block_start += block_frames;
        }
    }

    fn apply_wet_limiter(&mut self, output: &mut [f32], num_frames: usize) {
        let output_channels = self.output_channels();
        let output_pos = num_frames;
        // Per-sample peak limiter: prevent clipping after XTC filter summation + AutoGain.
        // Smooth attack (~0.2ms) and release (~50ms) to avoid gain modulation artifacts.
        // Skip when filters are bypassed — no amplification occurs.
        if !self.params.bypass_xtc_filters && output_pos > 0 {
            let threshold = 0.95_f32;
            for frame in 0..output_pos {
                let base = frame * output_channels;
                let frame_slice = &output[base..base + output_channels];
                let peak = frame_slice
                    .iter()
                    .map(|sample| sample.abs())
                    .fold(0.0, f32::max);
                let target_gr = if peak > threshold {
                    threshold / peak
                } else {
                    1.0
                };
                if target_gr < self.dynamics.limiter_envelope {
                    // Smooth attack (~0.2ms) to avoid per-sample gain jumps
                    self.dynamics.limiter_envelope = target_gr
                        + self.dynamics.limiter_attack_coeff
                            * (self.dynamics.limiter_envelope - target_gr);
                } else {
                    self.dynamics.limiter_envelope = target_gr
                        + self.dynamics.limiter_release_coeff
                            * (self.dynamics.limiter_envelope - target_gr);
                }
                for ch in 0..output_channels {
                    let idx = base + ch;
                    output[idx] *= self.dynamics.limiter_envelope;
                    // Hard clamp: the one-pole envelope has finite attack time, so a
                    // few samples can overshoot during transient onset. Clamp to ±1.0
                    // as a safety ceiling — matches standard digital limiter practice.
                    output[idx] = output[idx].clamp(-1.0, 1.0);
                }
            }
        }
    }

    fn settled_dry(&self) -> bool {
        !self.params.enabled && self.bypass.mix == 0.0
    }

    fn current_tail_bound(&self) -> usize {
        if self.settled_dry() {
            self.fft.fft_size
        } else {
            self.fft.fft_size * 2 - 1
        }
    }

    fn remaining_tail_frames(&self) -> Option<usize> {
        if let Some(remaining) = self.drain_state.remaining {
            return Some(remaining);
        }
        if !self.drain_state.received_input {
            return Some(0);
        }
        if self.settled_dry() {
            return Some(self.fft.fft_size);
        }
        let hop = self.fft.hop_size;
        let padding = (hop - self.drain_state.input_phase) % hop;
        self.fft
            .fft_size
            .checked_mul(2)?
            .checked_sub(hop)?
            .checked_add(padding)
    }

    /// Compare snapshots without constructing owned strings or triggering setters.
    fn parameter_unchanged(&self, id: &ParameterId, value: &ParameterValue) -> bool {
        let borrowed = match id.as_str() {
            "hrtf_file" => Some(self.params.hrtf_file.as_deref().unwrap_or("")),
            "room_ir_file" => Some(self.params.room_ir_file.as_deref().unwrap_or("")),
            "source_mode" => Some(self.params.source_mode.as_str()),
            "recommended_matrix_file" => {
                Some(self.params.recommended_matrix_file.as_deref().unwrap_or(""))
            }
            "itd_modeling" => Some(self.params.itd_modeling.as_str()),
            _ => None,
        };
        if let Some(current) = borrowed {
            value.as_string() == Some(current)
        } else {
            self.get_parameter(id).as_ref() == Some(value)
        }
    }

    /// Shift input buffer left by hop_size and clear tail
    #[inline(always)]
    pub(super) fn shift_input_buffer(&mut self) {
        let overlap = self.fft.fft_size - self.fft.hop_size;
        self.input
            .input_buffer_l
            .copy_within(self.fft.hop_size.., 0);
        self.input
            .input_buffer_r
            .copy_within(self.fft.hop_size.., 0);
        // Clear the tail (will be filled with new samples)
        self.input.input_buffer_l[overlap..].fill(0.0);
        self.input.input_buffer_r[overlap..].fill(0.0);
        self.input.input_fill = overlap;
    }
}

impl Plugin for XtcPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Crosstalk Cancellation (XTC)", "2.0.0", "SotF").with_description(format!(
            "Crosstalk cancellation (Async) - FFT size: {}, speakers at {}° and {}m",
            self.fft.fft_size, self.params.speaker_angle_deg, self.params.distance_m
        ))
    }

    fn input_channels(&self) -> usize {
        2 // Stereo input
    }

    fn output_channels(&self) -> usize {
        self.filter_state.cached_current_filters.output_channels()
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        if self.params.auto_gain_enabled {
            return PluginCompileMetadata::boundary(PluginCostClass::Fft, self.latency_samples());
        }
        PluginCompileMetadata::linear_transform(
            PluginCostClass::Fft,
            None,
            self.latency_samples(),
            true,
            true,
            false,
        )
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.diagnostics.cached_parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if self.drain_state.remaining.is_some() {
            return if self.parameter_unchanged(&id, &value) {
                Ok(())
            } else {
                Err("XTC parameters are frozen after EOF; reset before changing them".into())
            };
        }
        // Parameters not in PARAMS — handle separately
        if id.as_str() == "enabled" {
            let enabled = value
                .as_bool()
                .ok_or_else(|| "enabled must be a boolean".to_string())?;
            if enabled != self.params.enabled {
                self.params.enabled = enabled;
                self.bypass.start(enabled);
                if let Some(parameter) = self
                    .diagnostics
                    .cached_parameters
                    .iter_mut()
                    .find(|parameter| parameter.id.as_str() == "enabled")
                {
                    parameter.default_value = ParameterValue::Bool(enabled);
                }
            }
            return Ok(());
        }
        if id.as_str() == "kappa_target" {
            let v = value
                .as_float()
                .ok_or_else(|| "kappa_target must be a float".to_string())?;
            if v.is_finite() {
                self.params.kappa_target = v.clamp(1.0, 1000.0);
                self.update_filters();
            }
            self.rebuild_cached_parameters();
            return Ok(());
        }
        if id.as_str() == "hrtf_file" {
            let v = value
                .as_string()
                .ok_or_else(|| "hrtf_file must be a string".to_string())?;
            let mut candidate = self.params.clone();
            candidate.hrtf_file = (!v.is_empty()).then(|| v.to_string());
            if candidate.hrtf_file != self.params.hrtf_file {
                return Err(Self::STRUCTURAL_SOURCE_ERROR.to_string());
            }
            Self::validate_source_configuration(
                &candidate,
                self.fft.sample_rate,
                self.fft.fft_size / 2 + 1,
            )?;
            self.params = candidate;
            self.update_filters();
            self.rebuild_cached_parameters();
            return Ok(());
        }
        if id.as_str() == "room_ir_file" {
            let v = value
                .as_string()
                .ok_or_else(|| "room_ir_file must be a string".to_string())?;
            let candidate = (!v.is_empty()).then(|| v.to_string());
            if candidate != self.params.room_ir_file {
                return Err(Self::STRUCTURAL_SOURCE_ERROR.to_string());
            }
            return Ok(());
        }
        if id.as_str() == "source_mode" {
            let v = value
                .as_string()
                .ok_or_else(|| "source_mode must be a string".to_string())?;
            let mut candidate = self.params.clone();
            candidate.source_mode = v.to_string();
            if candidate.source_mode != self.params.source_mode {
                return Err(Self::STRUCTURAL_SOURCE_ERROR.to_string());
            }
            Self::validate_source_configuration(
                &candidate,
                self.fft.sample_rate,
                self.fft.fft_size / 2 + 1,
            )?;
            self.params = candidate;
            self.update_filters();
            self.rebuild_cached_parameters();
            return Ok(());
        }
        if id.as_str() == "recommended_matrix_file" {
            let v = value
                .as_string()
                .ok_or_else(|| "recommended_matrix_file must be a string".to_string())?;
            let candidate = (!v.is_empty()).then(|| v.to_string());
            if candidate != self.params.recommended_matrix_file {
                return Err(Self::STRUCTURAL_SOURCE_ERROR.to_string());
            }
            return Ok(());
        }
        if id.as_str() == "itd_modeling" {
            let v = value
                .as_string()
                .ok_or_else(|| "itd_modeling must be a string".to_string())?;
            if v != "phase_only" && v != "explicit_delay" {
                return Err(format!(
                    "itd_modeling must be 'phase_only' or 'explicit_delay', got '{}'",
                    v
                ));
            }
            self.params.itd_modeling = v.to_string();
            self.update_filters();
            self.rebuild_cached_parameters();
            return Ok(());
        }

        if id.as_str() == "room_reflections_enabled"
            && value
                .as_bool()
                .ok_or_else(|| "room_reflections_enabled must be boolean".to_string())?
        {
            let mut candidate = self.params.clone();
            candidate.room_reflections_enabled = true;
            Self::validate_room_ir_configuration(
                &candidate,
                self.fft.sample_rate,
                self.fft.fft_size / 2 + 1,
                self.fft.fft_forward.clone(),
            )?;
        }

        let idx = param_bridge::set_parameter(XT, &id, &value, |i, v| self.set_param_value(i, v))?;

        // Side effects based on parameter index
        let needs_filter_update = match idx {
            0..=5 => true,   // geometry + head tracking
            7..=9 => true,   // beta
            10..=12 => true, // shadow + filter
            13 => true,      // spectral_normalization
            14 => true,      // pinna_model_enabled
            15..=20 => true, // room, including room_ir_file
            21 => {
                // bypass_xtc_filters
                self.dynamics.limiter_envelope = 1.0;
                false
            }
            22 | 23 => true, // bypass_spectral_normalization, bypass_neumann_refinement
            24 => {
                // auto_gain_enabled
                let had_auto_gain = self.dynamics.auto_gain.is_some();
                if self.output_channels() != 2 {
                    self.dynamics.auto_gain = None;
                } else if self.params.auto_gain_enabled && self.dynamics.auto_gain.is_none() {
                    self.dynamics.auto_gain = Some(AutoGain::new(
                        2,
                        self.fft.sample_rate,
                        AutoGainParams {
                            enabled: true,
                            loudness_type: Default::default(),
                            max_gain_db: self.params.auto_gain_max_db,
                            smoothing_ms: self.params.auto_gain_smoothing_ms,
                        },
                    )?);
                } else if !self.params.auto_gain_enabled {
                    self.dynamics.auto_gain = None;
                }
                if had_auto_gain != self.dynamics.auto_gain.is_some() {
                    self.diagnostics.auto_gain_frames = 0;
                }
                false
            }
            25 => {
                // auto_gain_max_db
                if let Some(ag) = &mut self.dynamics.auto_gain {
                    ag.set_max_gain_db(self.params.auto_gain_max_db);
                }
                false
            }
            26 => {
                // auto_gain_smoothing_ms
                if let Some(ag) = &mut self.dynamics.auto_gain {
                    ag.set_smoothing_ms(self.params.auto_gain_smoothing_ms);
                }
                false
            }
            27 => true, // head_model
            _ => false,
        };

        if needs_filter_update {
            self.update_filters();
        }
        self.rebuild_cached_parameters();

        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        if id.as_str() == "fft_size" {
            return Some(ParameterValue::Int(self.params.fft_size as i32));
        }
        // Parameters not in PARAMS — handle separately
        if id.as_str() == "enabled" {
            return Some(ParameterValue::Bool(self.params.enabled));
        }
        if id.as_str() == "kappa_target" {
            return Some(ParameterValue::Float(self.params.kappa_target));
        }
        if id.as_str() == "hrtf_file" {
            return Some(ParameterValue::String(
                self.params.hrtf_file.clone().unwrap_or_default(),
            ));
        }
        if id.as_str() == "room_ir_file" {
            return Some(ParameterValue::String(
                self.params.room_ir_file.clone().unwrap_or_default(),
            ));
        }
        if id.as_str() == "source_mode" {
            return Some(ParameterValue::String(self.params.source_mode.clone()));
        }
        if id.as_str() == "recommended_matrix_file" {
            return Some(ParameterValue::String(
                self.params
                    .recommended_matrix_file
                    .clone()
                    .unwrap_or_default(),
            ));
        }
        if id.as_str() == "itd_modeling" {
            return Some(ParameterValue::String(self.params.itd_modeling.clone()));
        }
        param_bridge::get_parameter(XT, id, |i| self.param_value(i))
    }

    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(self.diagnostics.cache.load() as Arc<dyn Any + Send + Sync>)
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("XTC sample rate must be finite and positive".into());
        }
        // AutoGain's two fixed stereo loudness monitors use this validated range.
        // Reject before changing the audio clock or invalidating a pending request.
        if self.params.auto_gain_enabled
            && self.output_channels() == 2
            && !(16.0..=2_822_400.0).contains(&sample_rate)
        {
            return Err("XTC AutoGain sample rate must be in 16..=2822400 Hz".into());
        }
        let mut prepared = self.prepare_initialization(sample_rate)?;
        // Constructor storage has the fixed FFT/layout dimensions. Complete
        // control-side preparation before advancing the publication generation.
        self.input.temp_input_l.resize(MAX_PROCESS_FRAMES, 0.0);
        self.input.temp_input_r.resize(MAX_PROCESS_FRAMES, 0.0);
        prepared.update.generation = self
            .filter_state
            .filter_update_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let update = Arc::new(prepared.update);
        self.fft.sample_rate = sample_rate;
        self.dynamics.limiter_attack_coeff =
            math_audio_dsp::fast_math::fast_exp(-1.0 / (0.2 * 0.001 * sample_rate as f32));
        self.dynamics.limiter_release_coeff =
            math_audio_dsp::fast_math::fast_exp(-1.0 / (50.0 * 0.001 * sample_rate as f32));
        self.set_crossfade_rate();
        self.filter_state.cached_current_filters = Arc::clone(&update.filters);
        self.filter_state.hrtf_transfer_functions = update.hrtf_transfer_functions.clone();
        self.filter_state.room_reflection_cache = update.room_reflection_cache.clone();
        self.filter_state.room_params_hash = update.room_params_hash;
        self.filter_state.active_filter_update = Some(update);
        self.dynamics.auto_gain = prepared.auto_gain;
        // A successful synchronous installation supersedes the ready update;
        // a worker already past its own check is rejected at later adoption.
        self.filter_state
            .exchange
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending = None;

        self.initialized = true;
        // A new initialized clock starts the same meter cadence as a fresh
        // instance. Failed preparation never reaches this epoch boundary.
        self.diagnostics.auto_gain_frames = 0;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.drain_state.received_input = false;
        self.diagnostics.auto_gain_frames = 0;
        self.bypass.reset(self.params.enabled, self.fft.sample_rate);
        self.drain_state.tail_bound = None;
        self.drain_state.input_phase = 0;
        self.drain_state.remaining = None;
        self.drain_state.cached_frames = 0;
        self.drain_state.cache_position = 0;
        self.drain_state.zeros.fill(0.0);
        self.drain_state.output.fill(0.0);
        // Clear all buffers
        self.input.input_buffer_l.fill(0.0);
        self.input.input_buffer_r.fill(0.0);
        self.output.output_accumulator.fill(0.0);
        self.output.output_accumulator_fill = 0;
        self.output.next_add_position = 0;
        let prefix = self.fft.fft_size - self.fft.hop_size;
        self.output.output_read_position = prefix;
        self.work.prev_ifft_output.fill(0.0);
        self.input.input_fill = prefix;
        self.output.startup_delay_remaining = self.fft.fft_size;
        self.output.synthesis_prefix_remaining = prefix;

        // Reset crossfade state
        self.filter_state.crossfade_progress = 1.0;
        self.retire_completed_filter_snapshot();

        if let Some(ag) = &mut self.dynamics.auto_gain {
            ag.reset();
        }
        self.dynamics.limiter_envelope = 1.0;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let num_frames = context.num_frames;
        // Fail fast on pre-init calls: with unsized staging buffers the
        // block loop below could otherwise make no progress.
        if !self.initialized {
            return Err("XTC process called before initialize".to_string());
        }
        if context.sample_rate != self.fft.sample_rate {
            return Err("XTC process requires the initialized sample rate".into());
        }
        let output_channels = self.output_channels();
        let input_samples = num_frames
            .checked_mul(2)
            .ok_or_else(|| "XTC input sample count overflow".to_string())?;
        let output_samples = num_frames
            .checked_mul(output_channels)
            .ok_or_else(|| "XTC output sample count overflow".to_string())?;
        if input.len() != input_samples {
            return Err(format!(
                "Input size mismatch: expected {input_samples}, got {}",
                input.len()
            ));
        }
        if output.len() != output_samples {
            return Err(format!(
                "Output size mismatch: expected {output_samples}, got {}",
                output.len()
            ));
        }
        if num_frames == 0 {
            return Ok(0);
        }
        if self.drain_state.remaining.is_some() {
            return Err("XTC input is frozen after EOF; reset before processing".into());
        }
        // All rejection checks precede publication adoption and audio mutation.
        // Adoption only accepts matrices with the already validated width.
        self.adopt_pending_filters();

        self.process_audio(input, output, num_frames);
        self.drain_state.received_input = true;
        self.drain_state.input_phase =
            (self.drain_state.input_phase + num_frames % self.fft.hop_size) % self.fft.hop_size;
        Ok(num_frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.fft.hop_size
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.initialized {
            return None;
        }
        let remaining = self.remaining_tail_frames()?;
        let cached = self
            .drain_state
            .cached_frames
            .checked_sub(self.drain_state.cache_position)?;
        let calls = usize::from(cached > 0)
            .checked_add(remaining.checked_sub(cached)?.div_ceil(self.fft.hop_size))?;
        std::num::NonZeroU64::new(u64::try_from(calls.max(1)).ok()?)
    }

    fn tail_length(&self) -> TailLength {
        if !self.initialized {
            TailLength::Unknown
        } else {
            TailLength::Finite(
                self.drain_state
                    .tail_bound
                    .unwrap_or_else(|| self.current_tail_bound()) as u64,
            )
        }
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.initialized {
            return Err("XTC drain called before initialize".into());
        }
        if context.sample_rate != self.fft.sample_rate {
            return Err("XTC drain requires the initialized sample rate".into());
        }
        let channels = self.output_channels();
        if !output.len().is_multiple_of(channels) {
            return Err("XTC drain output must contain complete frames".into());
        }
        let remaining = self
            .remaining_tail_frames()
            .ok_or_else(|| "XTC finite support overflow".to_string())?;
        if remaining > 0 && output.is_empty() {
            return Err("XTC drain requires positive capacity for pending audio".into());
        }
        if !self.drain_state.received_input {
            return Ok(PluginDrainResult::COMPLETE);
        }
        // Freeze the audible support before a canonical refill can finish a fade.
        // No-input and rejected calls do not begin an EOF epoch.
        let bound = self.current_tail_bound();
        self.drain_state.tail_bound.get_or_insert(bound);
        self.drain_state.remaining = Some(remaining);
        if remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if self.drain_state.cache_position == self.drain_state.cached_frames {
            let frames = remaining.min(self.fft.hop_size);
            // Temporarily move prepared vectors to permit the shared audio kernel
            // to borrow self. This infallible refill neither resizes nor allocates.
            let zeros = std::mem::take(&mut self.drain_state.zeros);
            let mut cache = std::mem::take(&mut self.drain_state.output);
            self.process_audio(
                &zeros[..frames * 2],
                &mut cache[..frames * channels],
                frames,
            );
            self.drain_state.zeros = zeros;
            self.drain_state.output = cache;
            self.drain_state.cached_frames = frames;
            self.drain_state.cache_position = 0;
        }
        let frames = (self.drain_state.cached_frames - self.drain_state.cache_position)
            .min(output.len() / channels);
        let start = self.drain_state.cache_position * channels;
        output[..frames * channels]
            .copy_from_slice(&self.drain_state.output[start..start + frames * channels]);
        self.drain_state.cache_position += frames;
        self.drain_state.remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining == frames,
        })
    }

    fn latency_samples(&self) -> usize {
        // The sample clock emits one full frame of startup silence, independent
        // of callback boundaries; negative-origin windows preserve startup gain.
        self.fft.fft_size
    }
}
