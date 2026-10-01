use super::cutoff_bank::CutoffBank;
use super::resampler_quality::ResamplerQuality;
use super::stream_endpoint::StreamEndpoint;
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Adjustable, Async, FixedAsync, Indexing, ResampleError, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};
use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext,
};

/// Typed refusal for realtime dynamic updates.
///
/// `Copy` error owning no heap allocation, so audio-thread automation can
/// handle refusals without allocating or freeing. The compatibility `String`
/// API keeps the same messages through [`Display`](std::fmt::Display); valid
/// calls through either API allocate nothing. Only the dynamic ratio and
/// cutoff-smoothing controls use this path; structural setup controls stay on
/// the control thread with the existing `String` API.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResamplerControlError {
    /// Stream finalized; reset before changing the named control.
    Finalized(&'static str),
    /// Dynamic ratio updates are disabled.
    DynamicDisabled,
    /// Backend is missing (defensive; construction always installs one).
    NotInitialized,
    /// Backend rejected the ratio (outside nominal/2 through nominal*2).
    Backend(ResampleError),
}

impl std::fmt::Display for ResamplerControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Finalized(control) => write!(
                formatter,
                "stream has been finalized; reset before changing {control}"
            ),
            Self::DynamicDisabled => formatter.write_str(
                "Dynamic ratio is not enabled. Set dynamic_ratio to true first.",
            ),
            Self::NotInitialized => formatter.write_str("Resampler not initialized"),
            Self::Backend(error) => write!(formatter, "Failed to set ratio: {error:?}"),
        }
    }
}

impl std::error::Error for ResamplerControlError {}

/// Resampler plugin using rubato
///
/// This plugin resamples audio from one sample rate to another using high-quality
/// sinc interpolation. It maintains the same number of channels.
///
/// Note: The output buffer size will differ from input size based on the resampling ratio.
/// For example, resampling from 44.1kHz to 48kHz will produce more output frames.
pub struct ResamplerPlugin {
    /// Number of channels
    pub(super) num_channels: usize,
    /// Input sample rate
    pub(super) input_sample_rate: u32,
    /// Output sample rate
    pub(super) output_sample_rate: u32,
    /// Rubato resampler (planar format)
    pub(super) resampler: Option<Async<f32>>,
    /// Prepared cutoff policy; all coefficient allocation happens during setup.
    pub(super) cutoffs: CutoffBank,
    /// Chunk size for processing (number of frames per chunk)
    pub(super) chunk_size: usize,
    /// Output buffer (planar: one vec per channel, pre-allocated to max output size)
    pub(super) output_buffer: Vec<Vec<f32>>,
    /// Actual output frames from last process() call
    pub(super) last_output_frames: usize,
    /// Residual input buffer for variable-length input support (planar, per-channel)
    pub(super) residual_input: Vec<Vec<f32>>,
    /// Number of residual frames buffered
    pub(super) residual_frames: usize,
    /// Quality preset
    pub(super) quality: ResamplerQuality,
    /// Whether dynamic ratio changes are enabled
    pub(super) dynamic_ratio: bool,
    /// Current effective ratio (may differ from nominal when dynamic_ratio is enabled)
    pub(super) current_ratio: f64,
    /// Parameter IDs
    pub(super) param_quality: ParameterId,
    pub(super) param_dynamic_ratio: ParameterId,
    pub(super) param_ratio: ParameterId,
    pub(super) param_cutoff_smoothing: ParameterId,
    /// Cached parameters
    pub(super) cached_parameters: Vec<Parameter>,
    /// Set after the host negotiates the configured input rate.
    pub(super) initialized: bool,
    /// Programme frames accepted since the last reset.
    pub(super) stream_input_frames: u64,
    /// Output frames actually exposed, including leading delay.
    pub(super) stream_output_frames: u64,
    /// Submitted-input origin, emitted trajectory, and explicit EOF lifecycle.
    pub(super) endpoint: StreamEndpoint,
}

impl ResamplerPlugin {
    /// Minimum queued-work horizon used by the engine scheduler.
    ///
    /// The streaming adapter accepts smaller callback partitions and buffers
    /// them, but sinc work is performed when a complete chunk is assembled.
    /// A queued engine therefore keeps at least this many input-rate frames
    /// ahead of hardware consumption instead of assuming the cost is spread
    /// uniformly over every sub-chunk callback. This is not a longer physical
    /// callback deadline for fixed-rate plugin-format hosts.
    pub fn realtime_quantum_frames(&self) -> usize {
        if self.is_unity_passthrough() {
            1
        } else {
            self.chunk_size
        }
    }

    /// Create a new resampler plugin
    ///
    /// # Arguments
    /// * `num_channels` - Number of audio channels
    /// * `input_sample_rate` - Input sample rate in Hz
    /// * `output_sample_rate` - Output sample rate in Hz
    /// * `chunk_size` - Number of input frames to process at once (default: 1024)
    pub fn new(
        num_channels: usize,
        input_sample_rate: u32,
        output_sample_rate: u32,
        chunk_size: usize,
    ) -> Result<Self, String> {
        Self::with_quality(
            num_channels,
            input_sample_rate,
            output_sample_rate,
            chunk_size,
            ResamplerQuality::Medium,
        )
    }

    /// Create a new resampler plugin with a specified quality preset
    pub fn with_quality(
        num_channels: usize,
        input_sample_rate: u32,
        output_sample_rate: u32,
        chunk_size: usize,
        quality: ResamplerQuality,
    ) -> Result<Self, String> {
        if num_channels == 0 {
            return Err("num_channels must be > 0".to_string());
        }
        if input_sample_rate == 0 || output_sample_rate == 0 {
            return Err("sample rates must be > 0".to_string());
        }
        if chunk_size == 0 {
            return Err("chunk_size must be > 0".to_string());
        }

        let nominal_ratio = output_sample_rate as f64 / input_sample_rate as f64;

        // Create resampler
        let (resampler, cutoffs) = Self::create_resampler(
            num_channels,
            input_sample_rate,
            output_sample_rate,
            chunk_size,
            quality,
        )?;

        let max_output_frames = resampler.output_frames_max();

        let mut plugin = Self {
            num_channels,
            input_sample_rate,
            output_sample_rate,
            resampler: Some(resampler),
            cutoffs,
            chunk_size,
            output_buffer: vec![vec![0.0; max_output_frames]; num_channels],
            last_output_frames: 0,
            residual_input: vec![vec![0.0; chunk_size]; num_channels],
            residual_frames: 0,
            quality,
            dynamic_ratio: false,
            current_ratio: nominal_ratio,
            param_quality: ParameterId::from("quality"),
            param_dynamic_ratio: ParameterId::from("dynamic_ratio"),
            param_ratio: ParameterId::from("ratio"),
            param_cutoff_smoothing: ParameterId::from("cutoff_smoothing"),
            cached_parameters: Vec::new(),
            initialized: false,
            stream_input_frames: 0,
            stream_output_frames: 0,
            endpoint: StreamEndpoint::default(),
        };
        plugin.rebuild_cached_parameters();
        Ok(plugin)
    }

    /// Create a new resampler with default chunk size (1024)
    pub fn new_default(
        num_channels: usize,
        input_sample_rate: u32,
        output_sample_rate: u32,
    ) -> Result<Self, String> {
        Self::new(num_channels, input_sample_rate, output_sample_rate, 1024)
    }

    /// Create the rubato resampler with quality-dependent parameters
    pub(super) fn create_resampler(
        num_channels: usize,
        input_sample_rate: u32,
        output_sample_rate: u32,
        chunk_size: usize,
        quality: ResamplerQuality,
    ) -> Result<(Async<f32>, CutoffBank), String> {
        let params = SincInterpolationParameters {
            sinc_len: quality.sinc_len(),
            f_cutoff: Some(quality.f_cutoff()),
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: quality.oversampling_factor(),
            window: WindowFunction::BlackmanHarris2,
        };

        let nominal = output_sample_rate as f64 / input_sample_rate as f64;
        let cutoffs = CutoffBank::new(nominal);
        let resampler = Async::<f32>::new_sinc_with_cutoff_bank(
            nominal,
            2.0, // Maximum relative ratio deviation
            &params,
            cutoffs.additional_ratios(),
            chunk_size,
            num_channels,
            FixedAsync::Input,
        )
        .map_err(|e| format!("Failed to create resampler: {:?}", e))?;

        Ok((resampler, cutoffs))
    }

    /// Rebuild the resampler with current quality settings.
    /// Called when quality changes.
    ///
    /// This allocates the backend and cutoff tables and belongs on a control thread.
    /// Output and residual buffers are reused because their capacities depend on
    /// chunk size and ratio bounds, not filter quality.
    pub(super) fn rebuild_resampler(&mut self) -> Result<(), String> {
        let (resampler, cutoffs) = Self::create_resampler(
            self.num_channels,
            self.input_sample_rate,
            self.output_sample_rate,
            self.chunk_size,
            self.quality,
        )?;
        // output_frames_max() depends only on chunk_size, ratio, and max_relative_ratio —
        // not on sinc_len or oversampling_factor.  The existing buffers are already sized
        // for this chunk_size/ratio pair, so we reuse them in-place.
        debug_assert_eq!(
            resampler.output_frames_max(),
            self.output_buffer
                .first()
                .map(|v| v.len())
                .unwrap_or(resampler.output_frames_max()),
            "output_frames_max changed on quality rebuild — buffer reuse assumption violated"
        );
        // Zero residual to avoid stale data from the previous resampler.
        self.residual_frames = 0;
        for ch in 0..self.num_channels {
            self.residual_input[ch].fill(0.0);
            self.output_buffer[ch].fill(0.0);
        }
        self.resampler = Some(resampler);
        let smoothing = self.cutoffs.smoothing();
        self.cutoffs = cutoffs;
        // A fresh bank starts untracked at slot zero like its fresh backend;
        // the smoothing configuration persists across the quality rebuild.
        self.cutoffs.set_smoothing(smoothing);
        self.current_ratio = self.output_sample_rate as f64 / self.input_sample_rate as f64;
        Ok(())
    }

    pub(super) fn rebuild_cached_parameters(&mut self) {
        let nominal_ratio = self.output_sample_rate as f64 / self.input_sample_rate as f64;
        self.cached_parameters = vec![
            Parameter::new_int("quality", "Quality", self.quality.index(), 0, 2)
                .with_update_mode(UpdateMode::Structural)
                .with_description(
                    "Resampling quality: fast (64-tap), medium (128-tap), high (256-tap)",
                ),
            Parameter::new_bool("dynamic_ratio", "Dynamic Ratio", self.dynamic_ratio)
                .with_update_mode(if self.input_sample_rate == self.output_sample_rate {
                    UpdateMode::Structural
                } else {
                    UpdateMode::Realtime
                })
                .with_description(
                    "Enable ratio updates; equal-rate mode changes require a fresh or reset stream",
                ),
            Parameter::new_float(
                "ratio",
                "Ratio",
                self.current_ratio as f32,
                (nominal_ratio / 2.0) as f32,
                (nominal_ratio * 2.0) as f32,
            )
            .with_description(
                "Current resampling ratio (only adjustable when dynamic_ratio is enabled)",
            ),
            Parameter::new_bool("cutoff_smoothing", "Cutoff Smoothing", self.cutoffs.smoothing())
                .with_update_mode(UpdateMode::Realtime)
                .with_description(
                    "Smooth upward cutoff widening one prepared table per chunk; downward narrowing stays immediate",
                ),
        ];
    }

    /// Convert planar output to interleaved format
    pub(super) fn planar_to_interleaved(
        planar: &[Vec<f32>],
        output: &mut [f32],
        num_frames: usize,
        num_channels: usize,
    ) {
        for frame in 0..num_frames {
            for ch in 0..num_channels {
                output[frame * num_channels + ch] = planar[ch][frame];
            }
        }
    }

    /// Get the maximum number of output frames for a given number of input frames
    ///
    /// Returns the maximum possible output frame count from rubato.
    /// This should be used for buffer allocation to ensure the buffer is always large enough.
    /// The actual output frame count may be less and is returned by process().
    pub fn output_frames_for_input(&self, input_frames: usize) -> usize {
        if self.is_unity_passthrough() {
            return input_frames;
        }
        // Use rubato's output_frames_max() for safe buffer allocation
        // The actual output varies based on resampler internal state
        if let Some(ref resampler) = self.resampler {
            let pending = self.residual_frames.saturating_add(input_frames);
            let chunks = pending / self.chunk_size;
            chunks.saturating_mul(resampler.output_frames_max())
        } else {
            // Fallback estimate if resampler not initialized
            let ratio = self.output_sample_rate as f64 / self.input_sample_rate as f64;
            (input_frames as f64 * ratio).ceil() as usize + 1
        }
    }

    /// Frames immediately available from complete buffered input chunks.
    /// This is a scheduling estimate, not a destination-capacity bound.
    pub fn available_output_frames(&self, input_frames: usize) -> usize {
        if self.is_unity_passthrough() {
            return input_frames;
        }
        let pending = self.residual_frames.saturating_add(input_frames);
        let chunks = pending / self.chunk_size;
        match (chunks, self.resampler.as_ref()) {
            (0, _) => 0,
            (1, Some(resampler)) => resampler.output_frames_next().saturating_add(4),
            (count, Some(resampler)) => count.saturating_mul(resampler.output_frames_max()),
            _ => 0,
        }
    }

    /// Maximum frames written by one complete-stream drain step.
    pub fn flush_output_frames_max(&self) -> usize {
        if self.stream_input_frames == 0 || self.is_unity_passthrough() || self.endpoint.complete()
        {
            0
        } else {
            self.resampler
                .as_ref()
                .map(Resampler::output_frames_max)
                .unwrap_or(0)
        }
    }

    fn is_unity_passthrough(&self) -> bool {
        self.input_sample_rate == self.output_sample_rate && !self.dynamic_ratio
    }

    /// Get the nominal resampling ratio (output_rate / input_rate)
    pub fn ratio(&self) -> f64 {
        self.output_sample_rate as f64 / self.input_sample_rate as f64
    }

    /// Intrinsic output-domain delay of rubato's interpolation filter.
    ///
    /// Offline callers can trim this many leading frames after feeding enough
    /// zero tail to retain the complete time-aligned signal.
    pub fn output_delay_frames(&self) -> usize {
        self.resampler
            .as_ref()
            .map(|resampler| resampler.output_delay())
            .unwrap_or(self.quality.sinc_len() / 2)
    }

    /// Get the current effective resampling ratio (may differ from nominal when dynamic_ratio is used)
    pub fn current_ratio(&self) -> f64 {
        self.current_ratio
    }

    /// Get the current quality preset
    pub fn quality(&self) -> ResamplerQuality {
        self.quality
    }

    /// Check if dynamic ratio is enabled
    pub fn is_dynamic_ratio(&self) -> bool {
        self.dynamic_ratio
    }

    /// Set ratio without allocating, for audio-thread automation.
    ///
    /// Same cutoff policy as [`Self::set_ratio`]: the allowed range is nominal
    /// / 2.0 through nominal * 2.0, `ramp` interpolates the change, and the
    /// selected cutoff never exceeds either ramp endpoint. Neither the valid
    /// path nor any refusal allocates or frees, and the active ratio is
    /// unchanged on error. The backend rejects out-of-range, non-finite, and
    /// non-positive ratios transactionally without touching its state.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sotf_plugin_resampler::ResamplerPlugin;
    /// let mut plugin = ResamplerPlugin::new(1, 48_000, 48_000, 256).unwrap();
    /// // Dynamic updates are disabled by default, so this typed refusal
    /// // needs no heap allocation.
    /// assert!(plugin.try_set_ratio(1.5, true).is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ResamplerControlError::Finalized`] after draining has begun,
    /// [`ResamplerControlError::DynamicDisabled`] when dynamic updates are off,
    /// [`ResamplerControlError::NotInitialized`] when the backend is missing,
    /// or [`ResamplerControlError::Backend`] when the backend rejects the
    /// ratio. The active ratio is unchanged on error.
    pub fn try_set_ratio(
        &mut self,
        new_ratio: f64,
        ramp: bool,
    ) -> Result<(), ResamplerControlError> {
        if self.endpoint.finalized() {
            return Err(ResamplerControlError::Finalized("ratio"));
        }
        if !self.dynamic_ratio {
            return Err(ResamplerControlError::DynamicDisabled);
        }
        let resampler = self
            .resampler
            .as_mut()
            .ok_or(ResamplerControlError::NotInitialized)?;
        resampler
            .set_resample_ratio(new_ratio, ramp)
            .map_err(ResamplerControlError::Backend)?;
        self.current_ratio = new_ratio;
        self.cutoffs.select(resampler, new_ratio);
        Ok(())
    }

    /// Set cutoff smoothing without allocating, for audio use.
    ///
    /// Enables or disables the upward slew; downward narrowing still jumps
    /// immediately in both modes. Idempotent: setting the current value
    /// succeeds even after finalization. Neither the valid path nor the
    /// finalized refusal allocates or frees.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sotf_plugin_resampler::ResamplerPlugin;
    /// let mut plugin = ResamplerPlugin::new(1, 48_000, 48_000, 256).unwrap();
    /// assert!(plugin.try_set_cutoff_smoothing(true).is_ok());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`ResamplerControlError::Finalized`] when changing the flag
    /// after draining has begun. The flag is unchanged on error.
    pub fn try_set_cutoff_smoothing(
        &mut self,
        enabled: bool,
    ) -> Result<(), ResamplerControlError> {
        if enabled == self.cutoffs.smoothing() {
            return Ok(());
        }
        if self.endpoint.finalized() {
            return Err(ResamplerControlError::Finalized("cutoff smoothing"));
        }
        self.cutoffs.set_smoothing(enabled);
        Ok(())
    }

    /// Set the resampling ratio at runtime when dynamic ratio is enabled.
    ///
    /// The allowed range is nominal / 2.0 through nominal * 2.0.
    /// When `ramp` is true, the ratio change is smoothly interpolated.
    ///
    /// Selects a prepared cutoff no higher than either endpoint of the ramp.
    /// Selection preserves filter history and does not allocate. The cutoff grid
    /// can reduce bandwidth by up to 8.3%; a separate 0.1% step covers small
    /// negative clock drift. Abrupt widening can introduce spectral transients;
    /// enabling `cutoff_smoothing` slews upward widening at most one prepared
    /// table per selection call (the control call plus once per backend chunk;
    /// ramped upward advances only per chunk) while downward narrowing still
    /// jumps immediately, so no transient alias burst is exposed. Rejection
    /// near the new Nyquist remains limited by the selected quality.
    ///
    /// Compatibility wrapper around [`Self::try_set_ratio`] with the same
    /// messages; audio-thread automation should use the typed version so
    /// refusals never allocate a `String`.
    ///
    /// # Errors
    /// Returns an error if dynamic ratio is disabled or the ratio is outside
    /// the allowed range, non-finite, or non-positive, or draining has begun.
    /// The active ratio is unchanged on error. Reset before updating a finalized stream.
    pub fn set_ratio(&mut self, new_ratio: f64, ramp: bool) -> Result<(), String> {
        self.try_set_ratio(new_ratio, ramp)
            .map_err(|error| error.to_string())
    }

    /// Finish the current programme using its emitted interpolation clock.
    ///
    /// When `process()` receives input that is not a multiple of `chunk_size`, the remaining
    /// frames are held in an internal residual buffer and will not be processed until the next
    /// `process()` call that fills it.  Call `flush()` at the end of a stream to drain those
    /// frames. The final partial chunk is submitted with rubato's `partial_len`, then zero-input
    /// chunks are pumped to the programme endpoint. A fixed emitted ratio r retains
    /// `ceil(input_frames*r) + floor(sinc_len*r/2)` frames. A variable trajectory retains
    /// the first anchor reaching `input_frames - sinc_len/2 + 1`, using exact backend
    /// positions. This is programme-extent trimming, not every finite FIR ringing sample.
    /// Zero-output steps can remain unfinished; use `Plugin::drain()` for explicit completion.
    /// The returned output is already trimmed at the selected boundary; `discard` is
    /// retained for source compatibility and is always zero.
    ///
    /// Returns the number of output frames written into `output`.
    ///
    /// `output` must be at least `flush_output_frames_max() * num_channels` samples long
    /// (i.e., large enough for one chunk's maximum output).
    pub fn flush(&mut self, output: &mut [f32]) -> Result<(usize, usize), String> {
        let result = self.drain(output, &ProcessContext::new(self.input_sample_rate, 0))?;
        Ok((result.frames, 0))
    }

    /// Scale ratio without allocating, for audio-thread automation.
    ///
    /// Multiplies the current target by `rel_ratio`, including successive
    /// changes; rubato's relative setter instead uses the original ratio.
    /// Neither the valid path nor any refusal allocates or frees.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use sotf_plugin_resampler::ResamplerPlugin;
    /// let mut plugin = ResamplerPlugin::new(1, 48_000, 48_000, 256).unwrap();
    /// assert!(plugin.try_set_ratio_relative(1.01, true).is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error under the same conditions as [`Self::try_set_ratio`],
    /// with range validation applied to the cumulative ratio. The active
    /// ratio is unchanged on error.
    pub fn try_set_ratio_relative(
        &mut self,
        rel_ratio: f64,
        ramp: bool,
    ) -> Result<(), ResamplerControlError> {
        // Rubato's relative setter uses the original ratio; this API promises
        // multiplication of the current target, including successive changes.
        self.try_set_ratio(self.current_ratio * rel_ratio, ramp)
    }

    /// Multiply the current resampling ratio by a relative factor.
    ///
    /// For example, `rel_ratio=1.01` increases the current target ratio by 1%.
    /// Repeated calls accumulate. Cutoff and ramp behavior match [`Self::set_ratio`].
    ///
    /// Compatibility wrapper around [`Self::try_set_ratio_relative`];
    /// audio-thread automation should use the typed version.
    ///
    /// # Errors
    /// Returns an error under the same conditions as [`Self::set_ratio`], with
    /// range validation applied to the cumulative ratio. The active ratio is unchanged.
    pub fn set_ratio_relative(&mut self, rel_ratio: f64, ramp: bool) -> Result<(), String> {
        self.try_set_ratio_relative(rel_ratio, ramp)
            .map_err(|error| error.to_string())
    }
}

impl Plugin for ResamplerPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Resampler", env!("CARGO_PKG_VERSION"), "SotF").with_description(format!(
            "Sample rate converter: {}Hz -> {}Hz (ratio: {:.4}, quality: {})",
            self.input_sample_rate,
            self.output_sample_rate,
            self.current_ratio,
            self.quality.as_str()
        ))
    }

    fn input_channels(&self) -> usize {
        self.num_channels
    }

    fn output_channels(&self) -> usize {
        self.num_channels
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

    fn parameters(&self) -> Vec<Parameter> {
        self.cached_parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if id == self.param_quality {
            let new_quality = match &value {
                ParameterValue::Int(index) => ResamplerQuality::from_index(*index)
                    .ok_or_else(|| format!("Invalid quality index {index}: expected 0, 1, or 2"))?,
                ParameterValue::String(label) => {
                    ResamplerQuality::from_str(label).ok_or_else(|| {
                        format!("Invalid quality '{label}': expected fast, medium, or high")
                    })?
                }
                _ => return Err("quality must be a choice index".to_string()),
            };
            if new_quality != self.quality {
                if self.endpoint.finalized() {
                    return Err(
                        "stream has been finalized; reset before changing quality".to_string()
                    );
                }
                if self.initialized || self.residual_frames != 0 {
                    return Err(
                        "quality is a structural setup parameter; rebuild the plugin to change it"
                            .to_string(),
                    );
                }
                self.quality = new_quality;
                self.rebuild_resampler()?;
            }
        } else if id == self.param_dynamic_ratio {
            let v = value
                .as_bool()
                .ok_or_else(|| "dynamic_ratio must be a bool".to_string())?;
            // Idempotent state synchronization does not change topology or
            // the frozen EOF trajectory, including after completion.
            if v == self.dynamic_ratio {
                return Ok(());
            }
            if self.endpoint.finalized() {
                return Err("stream has been finalized; reset before changing dynamic mode".into());
            }
            if self.input_sample_rate == self.output_sample_rate {
                if self.stream_input_frames != 0 {
                    return Err(
                        "equal-rate dynamic mode changes require a fresh or reset stream".into(),
                    );
                }
                // Equal-rate mode changes switch between bit-exact bypass and
                // a delayed filter. Only a fresh stream may change this path.
                // Also clear ratio/ramp state configured before any real input.
                self.reset();
            } else if !v {
                // Unequal rates always use this same prepared backend. Return
                // to nominal without discarding residual input/filter history.
                let nominal = self.ratio();
                let resampler = self.resampler.as_mut().ok_or("Resampler not initialized")?;
                resampler
                    .set_resample_ratio(nominal, true)
                    .map_err(|e| format!("Failed to reset ratio: {e:?}"))?;
                self.current_ratio = nominal;
                self.cutoffs.select(resampler, nominal);
            }
            self.dynamic_ratio = v;
        } else if id == self.param_ratio {
            let v = value
                .as_float()
                .ok_or_else(|| "ratio must be a float".to_string())?;
            if !self.dynamic_ratio {
                return Err("Cannot change ratio when dynamic_ratio is disabled".to_string());
            }
            self.set_ratio(v as f64, true)?;
            return Ok(());
        } else if id == self.param_cutoff_smoothing {
            let v = value
                .as_bool()
                .ok_or_else(|| "cutoff_smoothing must be a bool".to_string())?;
            // Control-thread wrapper around the allocation-free typed core;
            // the idempotent and finalized semantics plus messages match.
            self.try_set_cutoff_smoothing(v)
                .map_err(|error| error.to_string())?;
        } else {
            return Err(format!("Unknown parameter: {id}"));
        }
        Ok(())
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        if id == &self.param_quality {
            Some(ParameterValue::Int(self.quality.index()))
        } else if id == &self.param_dynamic_ratio {
            Some(ParameterValue::Bool(self.dynamic_ratio))
        } else if id == &self.param_ratio {
            Some(ParameterValue::Float(self.current_ratio as f32))
        } else if id == &self.param_cutoff_smoothing {
            Some(ParameterValue::Bool(self.cutoffs.smoothing()))
        } else {
            None
        }
    }

    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()> {
        // The resampler has its own fixed input/output rates. If the host's
        // processing rate differs from our input rate, log a warning since the
        // resampling ratio may not produce the expected output rate.
        if sample_rate != self.input_sample_rate && self.input_sample_rate > 0 {
            return Err(format!(
                "Host sample rate ({sample_rate} Hz) differs from configured input rate ({} Hz)",
                self.input_sample_rate
            ));
        }
        self.initialized = true;
        Ok(())
    }

    fn reset(&mut self) {
        // Reset the resampler state
        if let Some(ref mut resampler) = self.resampler {
            resampler.reset();
        }
        // The backend reset returns to slot zero; track it. The smoothing
        // configuration persists like the dynamic-ratio permission flag.
        self.cutoffs.reset();
        // Reset ratio to nominal
        self.current_ratio = self.output_sample_rate as f64 / self.input_sample_rate as f64;
        // Clear residual buffer — zero the data to prevent stale audio leaking through
        // if a future code path reads residual_input without tight bounds checking.
        self.residual_frames = 0;
        self.last_output_frames = 0;
        self.stream_input_frames = 0;
        self.stream_output_frames = 0;
        self.endpoint = StreamEndpoint::default();
        for ch in 0..self.num_channels {
            self.residual_input[ch].fill(0.0);
        }
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if context.sample_rate != self.input_sample_rate {
            return Err("Resampler process sample-rate mismatch".into());
        }
        if self.endpoint.finalized() {
            return Err("stream has been finalized; reset before processing new input".to_string());
        }
        let num_input_frames = context.num_frames;
        let expected_input_samples = num_input_frames
            .checked_mul(self.num_channels)
            .ok_or("Resampler input sample count overflow")?;

        if input.len() != expected_input_samples {
            return Err(format!(
                "Input size mismatch: expected {} samples ({} frames x {} channels), got {}",
                expected_input_samples,
                num_input_frames,
                self.num_channels,
                input.len()
            ));
        }

        let accepted_frames = self
            .stream_input_frames
            .checked_add(num_input_frames as u64)
            .ok_or("Resampler accepted frame count overflow")?;

        if self.is_unity_passthrough() {
            if output.len() < input.len() {
                return Err(format!(
                    "Output buffer too small: need {} samples, got {}",
                    input.len(),
                    output.len()
                ));
            }
            output[..input.len()].copy_from_slice(input);
            self.last_output_frames = num_input_frames;
            self.stream_input_frames = accepted_frames;
            self.stream_output_frames = accepted_frames;
            return Ok(num_input_frames);
        }

        let complete_chunks =
            self.residual_frames.saturating_add(num_input_frames) / self.chunk_size;
        let capacity_frames = complete_chunks.saturating_mul(
            self.resampler
                .as_ref()
                .ok_or("Resampler not initialized")?
                .output_frames_max(),
        );
        let required_samples = capacity_frames.saturating_mul(self.num_channels);
        if required_samples > output.len() {
            return Err(format!(
                "Output buffer too small: need {required_samples} samples, got {}",
                output.len()
            ));
        }

        self.stream_output_frames
            .checked_add(capacity_frames as u64)
            .ok_or("Resampler output frame count overflow")?;

        // Variable-length input support: buffer input frames in residual_input
        // and process full chunk_size blocks through the resampler.
        let resampler = self.resampler.as_mut().ok_or("Resampler not initialized")?;
        let max_output_frames = resampler.output_frames_max();
        let chunk_size = self.chunk_size;

        let mut total_output_frames = 0usize;
        let mut input_offset = 0usize;
        let mut remaining_frames = num_input_frames;

        // Fill residual buffer from input, process full chunks
        while remaining_frames > 0 {
            let space_in_residual = chunk_size - self.residual_frames;
            let frames_to_copy = remaining_frames.min(space_in_residual);

            // Copy interleaved input into planar residual buffer
            for ch in 0..self.num_channels {
                for frame in 0..frames_to_copy {
                    self.residual_input[ch][self.residual_frames + frame] =
                        input[(input_offset + frame) * self.num_channels + ch];
                }
            }
            self.residual_frames += frames_to_copy;
            input_offset += frames_to_copy;
            remaining_frames -= frames_to_copy;

            // When we have a full chunk, process it
            if self.residual_frames == chunk_size {
                let input_adapter =
                    SequentialSliceOfVecs::new(&self.residual_input, self.num_channels, chunk_size)
                        .map_err(|e| format!("Input adapter error: {:?}", e))?;
                let mut output_adapter = SequentialSliceOfVecs::new_mut(
                    &mut self.output_buffer,
                    self.num_channels,
                    max_output_frames,
                )
                .map_err(|e| format!("Output adapter error: {:?}", e))?;

                let candidate = self.endpoint.after_block(
                    resampler.input_frames_next(),
                    resampler.output_frames_next(),
                    resampler.resample_ratio(),
                    self.current_ratio,
                )?;
                let (consumed, output_frames) = resampler
                    .process_into_buffer(&input_adapter, &mut output_adapter, None)
                    .map_err(|e| format!("Resampling failed: {:?}", e))?;
                debug_assert_eq!(consumed, chunk_size);
                self.endpoint = candidate;
                self.cutoffs.select(resampler, self.current_ratio);
                self.residual_frames = 0;

                // Check output buffer capacity
                let out_sample_offset = total_output_frames * self.num_channels;
                let new_output_samples = output_frames * self.num_channels;
                if out_sample_offset + new_output_samples > output.len() {
                    return Err(format!(
                        "Output buffer too small: need {} samples, got {}",
                        out_sample_offset + new_output_samples,
                        output.len()
                    ));
                }

                // Convert planar to interleaved into output at current offset
                Self::planar_to_interleaved(
                    &self.output_buffer,
                    &mut output[out_sample_offset..],
                    output_frames,
                    self.num_channels,
                );

                total_output_frames += output_frames;
            }
        }

        // Store actual output frame count
        self.last_output_frames = total_output_frames;
        self.stream_input_frames = accepted_frames;
        self.stream_output_frames += total_output_frames as u64;

        Ok(total_output_frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.flush_output_frames_max()
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if self.endpoint.complete() || self.is_unity_passthrough() || self.stream_input_frames == 0
        {
            return std::num::NonZeroU64::new(1);
        }
        let backend = self.resampler.as_ref()?;
        let candidate = self
            .endpoint
            .after_block(
                backend.input_frames_next(),
                backend.output_frames_next(),
                backend.resample_ratio(),
                self.current_ratio,
            )
            .ok()?;
        self.endpoint.drain_call_bound(
            candidate,
            self.stream_input_frames,
            self.stream_output_frames,
            self.quality.sinc_len(),
            backend.input_frames_next(),
            self.current_ratio,
            backend.last_input_index(),
        )
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if context.sample_rate != self.input_sample_rate {
            return Err("Resampler drain sample-rate mismatch".into());
        }
        if self.endpoint.complete() || self.is_unity_passthrough() || self.stream_input_frames == 0
        {
            // Even an empty valid drain explicitly finalizes this stream.
            self.last_output_frames = 0;
            self.endpoint.set_draining(true);
            return Ok(PluginDrainResult::COMPLETE);
        }

        let resampler = self.resampler.as_ref().ok_or("Resampler not initialized")?;
        let max_output_frames = resampler.output_frames_max();
        let planned_frames = resampler.output_frames_next();
        let mut candidate = self.endpoint.after_block(
            resampler.input_frames_next(),
            planned_frames,
            resampler.resample_ratio(),
            self.current_ratio,
        )?;
        let (frames, complete) = self.endpoint.drain_plan(
            candidate,
            self.stream_input_frames,
            self.stream_output_frames,
            self.quality.sinc_len(),
            resampler.input_positions_next(),
        )?;
        if frames == 0 && complete {
            self.last_output_frames = 0;
            self.endpoint.set_draining(true);
            return Ok(PluginDrainResult::COMPLETE);
        }
        // An empty-output backend block can still consume padding and finish a
        // ramp. Require usable destination space but report 0/incomplete progress.
        let required_samples = frames
            .max(1)
            .checked_mul(self.num_channels)
            .ok_or("Resampler drain sample count overflow")?;
        if output.len() < required_samples {
            return Err(format!(
                "Output buffer too small for drain: need {required_samples} samples, got {}",
                output.len()
            ));
        }
        let emitted = self
            .stream_output_frames
            .checked_add(frames as u64)
            .ok_or("Resampler output frame count overflow")?;

        let partial_len = self.residual_frames;
        for ch in 0..self.num_channels {
            self.residual_input[ch][partial_len..self.chunk_size].fill(0.0);
        }

        let input_adapter =
            SequentialSliceOfVecs::new(&self.residual_input, self.num_channels, self.chunk_size)
                .map_err(|e| format!("Input adapter error: {e:?}"))?;
        let mut output_adapter = SequentialSliceOfVecs::new_mut(
            &mut self.output_buffer,
            self.num_channels,
            max_output_frames,
        )
        .map_err(|e| format!("Output adapter error: {e:?}"))?;
        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(partial_len),
            active_channels_mask: None,
        };
        let resampler = self.resampler.as_mut().ok_or("Resampler not initialized")?;
        let (consumed, produced) = resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
            .map_err(|e| format!("Resampling drain failed: {e:?}"))?;
        self.cutoffs.select(resampler, self.current_ratio);

        debug_assert_eq!(consumed, self.chunk_size);
        debug_assert_eq!(produced, planned_frames);
        candidate.set_draining(complete);
        self.endpoint = candidate;
        self.residual_frames = 0;
        Self::planar_to_interleaved(&self.output_buffer, output, frames, self.num_channels);
        self.stream_output_frames = emitted;
        self.last_output_frames = frames;
        Ok(PluginDrainResult { frames, complete })
    }

    fn latency_samples(&self) -> usize {
        if self.is_unity_passthrough() {
            return 0;
        }
        // Use rubato's exact output_delay() which accounts for the full FIR group delay,
        // ring-buffer offsets, and polyphase filter delays — not just sinc_len / 2.
        // Also add the chunking buffer latency: up to chunk_size - 1 frames can sit in
        // residual_input before producing output.
        let rubato_delay = self.output_delay_frames();
        let priming_output_frames = (((self.chunk_size - 1) as f64) * self.current_ratio).ceil();
        rubato_delay.saturating_add(priming_output_frames as usize)
    }

    fn realtime_quantum_frames(&self) -> usize {
        ResamplerPlugin::realtime_quantum_frames(self)
    }

    fn signal_delay_samples(&self) -> f64 {
        // process() returns only produced chunks. Waiting for a chunk does
        // not insert startup samples into the concatenated output stream.
        if self.is_unity_passthrough() {
            0.0
        } else {
            // Pinned rubato 5 fork: InnerSinc starts at -(N - 1), advances
            // before emitting, and make_sincs centers phase zero at
            // N/2 - 1 + 1/table_phases. Their combination gives this
            // fractional output-clock group delay. rubato's integer
            // output_delay() rounds a different N*ratio/2 convention.
            // Keep fractional phase until the offline consumer rounds once.
            ((self.quality.sinc_len() as f64 / 2.0
                - 1.0 / self.quality.oversampling_factor() as f64)
                * self.current_ratio
                - 1.0)
                .max(0.0)
        }
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        ResamplerPlugin::output_frames_for_input(self, input_frames)
    }

    fn output_sample_rate(&self, _input_rate: u32) -> u32 {
        self.output_sample_rate
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }
}

#[test]
fn test_flush_produces_trailing_output() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();
    resampler.initialize(44100).unwrap();

    // Process a partial chunk (512 frames)
    let num_frames = 512;
    let input = vec![0.5_f32; num_frames * 2];
    let max_output = resampler.output_frames_for_input(num_frames);
    let mut output = vec![0.0_f32; max_output * 2];
    let ctx = ProcessContext::new(44100, num_frames);
    let produced = resampler.process(&input, &mut output, &ctx).unwrap();
    assert_eq!(produced, 0, "Partial chunk should produce no output yet");

    // Flush returns the exact complete-stream prefix; no caller-side estimate is needed.
    let mut flush_buf = vec![0.0_f32; resampler.flush_output_frames_max() * 2];
    let (flush_output, discard) = resampler.flush(&mut flush_buf).unwrap();
    assert!(flush_output > 0, "Flush should produce trailing output");
    assert_eq!(discard, 0);
}

#[test]
fn test_rebuild_resampler_reuses_buffers() {
    let mut resampler = ResamplerPlugin::new(2, 44100, 48000, 1024).unwrap();

    let output_ptr = resampler.output_buffer[0].as_ptr();
    let residual_ptr = resampler.residual_input[0].as_ptr();

    // Switch quality via set_parameter (calls rebuild_resampler internally)
    resampler
        .set_parameter(
            ParameterId::from("quality"),
            ParameterValue::String("high".to_string()),
        )
        .unwrap();

    assert_eq!(
        resampler.output_buffer[0].as_ptr(),
        output_ptr,
        "output_buffer was reallocated"
    );
    assert_eq!(
        resampler.residual_input[0].as_ptr(),
        residual_ptr,
        "residual_input was reallocated"
    );
}
