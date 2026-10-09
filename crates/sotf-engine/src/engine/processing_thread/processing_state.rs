use super::super::{
    DecoderMessage, GcItem, PreparedHostUpdate, PreparedTransitionDelay, ProcessingCommand,
    ProcessingMessage, ProcessingResponse, ThreadEvent,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::isolated::isolated_external_plugin_status;
use super::misc::send_or_interrupt;
use super::{ProcessingReply, ProcessingRequest};
use sotf_plugins::{Host, PluginHost};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender};

/// Short wait used while audio is active and the decoder queue briefly runs dry.
const ACTIVE_EMPTY_SLEEP_PROCESSING_US: u64 = 100;
/// Coarser wait once playback has explicitly stopped or reached end of stream.
const IDLE_EMPTY_SLEEP_PROCESSING_MS: u64 = 1;

/// Maximum engine block size (frames) used to pre-size processing scratch buffers.
const MAX_ENGINE_BLOCK_FRAMES: usize = 8192;
/// Maximum input width prepared for processing, crossfade and recycle-fallback
/// buffers. This also covers a newly installed wide-input host before any
/// callback has a chance to grow its scratch storage.
const MAX_ENGINE_CHANNELS: usize = crate::EngineConfig::MAX_INPUT_CHANNELS;
/// Worst-case interleaved sample count for one engine block.
const MAX_ENGINE_SAMPLE_CAPACITY: usize = MAX_ENGINE_BLOCK_FRAMES * MAX_ENGINE_CHANNELS;
/// Headroom for plugins that expand channels or sample rate (e.g. resamplers).
const MAX_PROCESSING_SAMPLE_CAPACITY: usize = MAX_ENGINE_SAMPLE_CAPACITY * 2;
/// Number of spare buffers kept on the processing thread for recycle-queue misses.
const RECYCLE_FALLBACK_POOL_SIZE: usize = 4;
/// The manager serializes host swaps, so reaching this limit means the GC is
/// persistently unhealthy rather than merely one item behind.
const MAX_PENDING_RETIREMENTS: usize = 64;
/// Host replacements fade for 50 ms on the emitted audio timeline.
const CROSSFADE_DURATION_MS: u64 = 50;

/// Processing state
pub(super) struct ProcessingState {
    /// Current plugin host
    pub(super) host: Box<PluginHost>,
    /// Previous host for crossfading
    pub(super) prev_host: Option<Box<PluginHost>>,
    /// Crossfade progress (0.0 to 1.0, 1.0 = current host only)
    pub(super) crossfade_progress: f32,
    /// Output-rate frames emitted since this transition began.
    crossfade_frames: usize,
    /// When host timing differs, transition old→silence→new instead of
    /// blending time-misaligned samples.
    pub(super) crossfade_through_silence: bool,
    /// Preallocated latency alignment for the old and new paths.
    pub(super) old_path_delay: PreparedTransitionDelay,
    pub(super) new_path_delay: PreparedTransitionDelay,
    /// Background state reclaimer. Production always installs this before processing.
    pub(super) gc_tx: Option<super::super::GcSender>,
    /// Retained only if the bounded GC queue is momentarily full.
    pub(super) pending_retirements: Vec<GcItem>,
    retirement_overflowed: bool,
    pub(super) host_generation: Arc<AtomicU64>,
    /// Local identity of the committed host, independent of request generations.
    committed_host_epoch: u64,
    /// Number of channels
    pub(super) channels: usize,
    pub(super) bypassed: bool,
    pub(super) process_buffer: Vec<f32>,
    /// Buffer for previous host during crossfade
    pub(super) prev_process_buffer: Vec<f32>,
    /// Frame counter for diagnostic logging
    pub(super) frame_count: u64,
    /// Total output samples produced (for effective rate measurement)
    pub(super) total_output_samples: u64,
    /// Timestamp of first frame processed
    pub(super) first_frame_time: Option<std::time::Instant>,
    /// Sample rate (for effective rate calculation)
    pub(super) sample_rate: u32,
    /// Spare Arc from previous plugin_data_cache swap, reused via Arc::get_mut
    /// to avoid per-frame Vec allocation when no UI reader holds a reference.
    pub(super) spare_cache_arc: Option<std::sync::Arc<super::super::PluginDataVec>>,
    /// RT diagnostics: how many frames hit the cache fallback (allocation) path
    pub(super) cache_fallback_count: u64,
    /// RT diagnostics: how many frames reused the spare Arc (zero-alloc fast path)
    pub(super) cache_reuse_count: u64,
    /// RT diagnostics: max process_frame duration in the current reporting window
    pub(super) max_frame_duration: std::time::Duration,
    /// RT diagnostics: how many frames took longer than the frame period
    pub(super) frames_over_budget: u64,
    /// RT diagnostics: how many recycle misses (fallback Vec allocation)
    pub(super) recycle_miss_count: u64,
    /// Spare buffers used when the playback→processing recycle queue is
    /// temporarily empty. Pre-sized so the steady-state hot path does not allocate.
    pub(super) recycle_fallback_pool: Vec<Vec<f32>>,
    /// Optional nonblocking tap for live network PCM streaming.
    #[cfg(feature = "streaming")]
    pub(super) network_stream_tap: Option<sotf_streaming::PcmStreamHandle>,
}

impl ProcessingState {
    pub(super) fn new(
        channels: usize,
        sample_rate: u32,
        #[cfg(feature = "streaming")] network_stream_tap: Option<sotf_streaming::PcmStreamHandle>,
    ) -> Self {
        let mut recycle_fallback_pool = Vec::with_capacity(RECYCLE_FALLBACK_POOL_SIZE);
        for _ in 0..RECYCLE_FALLBACK_POOL_SIZE {
            recycle_fallback_pool.push(Vec::with_capacity(MAX_PROCESSING_SAMPLE_CAPACITY));
        }

        Self {
            host: Box::new(PluginHost::new(channels, sample_rate)),
            prev_host: None,
            crossfade_progress: 1.0,
            crossfade_frames: 0,
            crossfade_through_silence: false,
            old_path_delay: PreparedTransitionDelay::default(),
            new_path_delay: PreparedTransitionDelay::default(),
            gc_tx: None,
            pending_retirements: Vec::with_capacity(MAX_PENDING_RETIREMENTS + 1),
            retirement_overflowed: false,
            host_generation: Arc::new(AtomicU64::new(0)),
            committed_host_epoch: 0,
            channels,
            bypassed: false,
            // Pre-sized for the worst-case processing block so the hot path only reuses memory.
            process_buffer: Vec::with_capacity(MAX_PROCESSING_SAMPLE_CAPACITY),
            prev_process_buffer: Vec::with_capacity(MAX_PROCESSING_SAMPLE_CAPACITY),
            frame_count: 0,
            total_output_samples: 0,
            first_frame_time: None,
            sample_rate,
            spare_cache_arc: Some(Arc::new(Vec::new())),
            cache_fallback_count: 0,
            cache_reuse_count: 0,
            max_frame_duration: std::time::Duration::ZERO,
            frames_over_budget: 0,
            recycle_miss_count: 0,
            recycle_fallback_pool,
            #[cfg(feature = "streaming")]
            network_stream_tap,
        }
    }

    /// Get the actual output channel count
    pub(super) fn output_channels(&self) -> usize {
        self.host.output_channels()
    }

    /// Get the output frame count for a given input frame count.
    /// Accounts for plugins that change frame count (like resamplers).
    pub(super) fn output_frames_for_input(&self, input_frames: usize) -> usize {
        if self.bypassed || self.host.plugin_count() == 0 {
            input_frames
        } else {
            self.host.output_frames_for_input(input_frames)
        }
    }

    /// Get the output sample rate for a given input rate.
    /// Accounts for plugins that change sample rate (like resamplers).
    pub(super) fn output_sample_rate(&self, input_rate: u32) -> Result<u32, String> {
        if self.bypassed || self.host.plugin_count() == 0 {
            if input_rate == 0 {
                Err("Processing input sample rate must be positive".to_string())
            } else {
                Ok(input_rate)
            }
        } else {
            self.host.output_sample_rate_native(input_rate)
        }
    }

    /// Bounded hot-reload crossfade gains: convex (equal-gain) mixing.
    ///
    /// Invariant: out = (1-a).old + a.new with a in [0,1], so
    /// |out| <= (1-a).|old| + a.|new| <= max(|old|, |new|) for every frame.
    /// When both chains are ceiling-bounded, every transition sample
    /// respects the end-to-end ceiling — including correlated program,
    /// where equal-power mixing would reach sqrt(2) times the ceiling.
    /// The fade clock, duration, delay compensation, and retirement logic
    /// are unchanged; only the gain law is bounded. No output clamp is
    /// applied: non-mastering audio keeps its transient shape, mixed.
    #[inline]
    pub(super) fn convex_crossfade_gains(alpha: f32) -> (f32, f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        (1.0 - alpha, alpha)
    }

    fn reset(&mut self) {
        self.host.reset();
        if let Some(previous) = self.prev_host.as_mut() {
            previous.reset();
            self.crossfade_progress = 0.0;
        }
        self.crossfade_frames = 0;
        self.old_path_delay.reset();
        self.new_path_delay.reset();
    }

    pub(super) fn prepare_scratch_buffer(buffer: &mut Vec<f32>, len: usize) {
        // Grow capacity only when necessary; never shrink, so the same backing
        // allocation can be reused across frames of different sizes.
        if buffer.capacity() < len {
            buffer.reserve(len - buffer.len());
        }
        if buffer.len() < len {
            buffer.resize(len, 0.0);
        }
    }

    #[cfg(test)]
    pub(super) fn transition_delay_samples(&self) -> usize {
        self.old_path_delay.len().max(self.new_path_delay.len())
    }

    fn flush_pending_retirement(&mut self) {
        let Some(gc_tx) = self.gc_tx.as_ref() else {
            return;
        };
        while let Some(item) = self.pending_retirements.pop() {
            if let Err(error) = gc_tx.try_send(item) {
                self.pending_retirements.push(error.into_inner());
                break;
            }
        }
    }

    fn retire(&mut self, item: GcItem) {
        self.flush_pending_retirement();
        let Some(gc_tx) = self.gc_tx.as_ref() else {
            if self.pending_retirements.len() < MAX_PENDING_RETIREMENTS {
                self.pending_retirements.push(item);
            } else {
                self.retirement_overflowed = true;
                self.pending_retirements.push(item);
            }
            return;
        };
        if let Err(error) = gc_tx.try_send(item) {
            let item = error.into_inner();
            if self.pending_retirements.len() < MAX_PENDING_RETIREMENTS {
                self.pending_retirements.push(item);
            } else {
                self.retirement_overflowed = true;
                // Capacity includes one emergency slot. Preserve ownership;
                // the loop fails at its next boundary and transfers all items
                // to GC after it has left active realtime processing.
                self.pending_retirements.push(item);
            }
        }
    }

    pub(super) fn recycle_output_buffer_locally(&mut self, mut data: Vec<f32>) {
        data.clear();
        if self.recycle_fallback_pool.len() < RECYCLE_FALLBACK_POOL_SIZE {
            self.recycle_fallback_pool.push(data);
        } else {
            self.retire(GcItem::Buffer(data));
        }
    }

    fn retire_owned_state(self) {
        let ProcessingState {
            host,
            prev_host,
            old_path_delay,
            new_path_delay,
            gc_tx,
            pending_retirements,
            ..
        } = self;
        let mut owned = pending_retirements;
        if let Some(previous) = prev_host {
            owned.push(GcItem::HostTransition {
                host: previous,
                old_path_delay,
                new_path_delay,
            });
        }
        owned.push(GcItem::PluginHost(host));

        let Some(gc_tx) = gc_tx else {
            Self::emergency_retire(owned);
            return;
        };
        let mut remaining = owned.into_iter();
        while let Some(item) = remaining.next() {
            if let Err(error) = gc_tx.send(item) {
                let mut fallback = Vec::with_capacity(remaining.len() + 1);
                fallback.push(error.0);
                fallback.extend(remaining);
                Self::emergency_retire(fallback);
                return;
            }
        }
    }

    fn emergency_retire(items: Vec<GcItem>) {
        let shared = std::sync::Arc::new(std::sync::Mutex::new(Some(items)));
        let worker_items = std::sync::Arc::clone(&shared);
        if std::thread::Builder::new()
            .name("emergency-audio-gc".to_string())
            .spawn(move || {
                let items = worker_items
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .unwrap_or_default();
                for item in items {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(item)));
                }
            })
            .is_err()
        {
            // Thread creation failure is terminal resource exhaustion. Avoid
            // running third-party destructors on the processing thread.
            std::mem::forget(shared);
        }
    }

    fn commit_host_update(
        &mut self,
        update: PreparedHostUpdate,
    ) -> Result<(u64, usize, u32, usize, usize), String> {
        let current_channels = self.host.output_channels();
        let current_latency = self.host.total_latency_samples();
        if (update.generation != 0
            && update.generation != self.host_generation.load(Ordering::Acquire))
            || current_channels != update.expected_output_channels
            || current_latency != update.expected_latency_samples
        {
            self.retire(GcItem::HostTransition {
                host: update.host,
                old_path_delay: update.old_path_delay,
                new_path_delay: update.new_path_delay,
            });
            return Err("stale prepared host update: active host changed before commit".into());
        }

        let PreparedHostUpdate {
            generation,
            host: new_host,
            output_channels,
            output_sample_rate,
            latency_samples,
            analyzer_cache,
            old_path_delay,
            new_path_delay,
            ..
        } = update;
        let negotiated_rate = match new_host.output_sample_rate_native(self.sample_rate) {
            Ok(rate) => rate,
            Err(error) => {
                self.retire(GcItem::HostTransition {
                    host: new_host,
                    old_path_delay,
                    new_path_delay,
                });
                return Err(format!("prepared host output rate became invalid: {error}"));
            }
        };
        if output_channels != new_host.output_channels()
            || output_sample_rate != negotiated_rate
            || latency_samples != new_host.total_latency_samples()
        {
            self.retire(GcItem::HostTransition {
                host: new_host,
                old_path_delay,
                new_path_delay,
            });
            return Err("prepared host metadata changed before commit".into());
        }

        let old_output_rate = match self.host.output_sample_rate_native(self.sample_rate) {
            Ok(rate) => rate,
            Err(error) => {
                self.retire(GcItem::HostTransition {
                    host: new_host,
                    old_path_delay,
                    new_path_delay,
                });
                return Err(format!("active host output rate became invalid: {error}"));
            }
        };

        if !update.ticket.try_commit() {
            self.retire(GcItem::HostTransition {
                host: new_host,
                old_path_delay,
                new_path_delay,
            });
            return Err("prepared host update was cancelled before commit".into());
        }

        let can_transition = current_channels == output_channels && self.host.plugin_count() > 0;
        if let Some(previous) = self.prev_host.take() {
            let old_path_delay = std::mem::take(&mut self.old_path_delay);
            let new_path_delay = std::mem::take(&mut self.new_path_delay);
            self.retire(GcItem::HostTransition {
                host: previous,
                old_path_delay,
                new_path_delay,
            });
        }

        if can_transition {
            self.prev_host = Some(std::mem::replace(&mut self.host, new_host));
            self.crossfade_progress = 0.0;
            self.crossfade_frames = 0;
            self.crossfade_through_silence = old_output_rate != output_sample_rate;
            self.old_path_delay = old_path_delay;
            self.new_path_delay = new_path_delay;
        } else {
            let previous = std::mem::replace(&mut self.host, new_host);
            self.retire(GcItem::PluginHost(previous));
            self.crossfade_progress = 1.0;
            self.crossfade_frames = 0;
            self.crossfade_through_silence = false;
            self.old_path_delay = PreparedTransitionDelay::default();
            self.new_path_delay = PreparedTransitionDelay::default();
        }
        self.channels = output_channels;
        self.spare_cache_arc = Some(analyzer_cache);
        self.committed_host_epoch = self.committed_host_epoch.wrapping_add(1);
        Ok((
            generation,
            output_channels,
            output_sample_rate,
            current_latency,
            latency_samples,
        ))
    }

    /// Process a frame
    /// Returns the actual number of output frames written
    pub(super) fn process_frame(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        input_frames: usize,
    ) -> Result<usize, String> {
        self.flush_pending_retirement();
        if self.retirement_overflowed {
            return Err(format!(
                "plugin retirement backlog exceeded {MAX_PENDING_RETIREMENTS} items"
            ));
        }
        if self.bypassed {
            // Preserve frame boundaries when bypassing a topology-changing
            // chain. Shared channels pass through and added outputs are silent.
            let input_channels = self.host.input_channels();
            let output_channels = self.host.output_channels();
            let required = input_frames.saturating_mul(output_channels);
            if input_channels == 0
                || input.len() < input_frames.saturating_mul(input_channels)
                || output.len() < required
            {
                return Err("bypass buffer dimensions do not match host topology".to_string());
            }
            output[..required].fill(0.0);
            let shared_channels = input_channels.min(output_channels);
            for frame in 0..input_frames {
                let input_start = frame * input_channels;
                let output_start = frame * output_channels;
                output[output_start..output_start + shared_channels]
                    .copy_from_slice(&input[input_start..input_start + shared_channels]);
            }
            return Ok(input_frames);
        }

        // Handle crossfade if in progress
        if let Some(ref mut prev_host) = self.prev_host {
            let actual_frames = self.host.process(input, output)?;
            if !self.crossfade_through_silence {
                self.new_path_delay
                    .process_in_place(&mut output[..actual_frames * self.channels]);
            }

            // Size the previous-host destination from its own output rate.
            // During a down-rate transition the old host can require more
            // frames than the new host's caller-provided output buffer.
            let prev_output_frames = prev_host.output_frames_for_input(input_frames);
            let prev_buf_len = prev_output_frames.saturating_mul(prev_host.output_channels());
            Self::prepare_scratch_buffer(&mut self.prev_process_buffer, prev_buf_len);

            let prev_actual =
                prev_host.process(input, &mut self.prev_process_buffer[..prev_buf_len])?;
            if !self.crossfade_through_silence {
                self.old_path_delay
                    .process_in_place(&mut self.prev_process_buffer[..prev_actual * self.channels]);
            }

            // Count emitted frames rather than callbacks or accepted input.
            // Buffered/rate-changing chains can emit zero or multiple chunks;
            // only output samples advance the audible transition timeline.
            let output_rate = self.host.output_sample_rate_native(self.sample_rate)?;
            let fade_frames = (u64::from(output_rate) * CROSSFADE_DURATION_MS)
                .div_ceil(1_000)
                .max(1) as usize;
            for frame in 0..actual_frames {
                let alpha = (self.crossfade_frames.saturating_add(frame) as f64
                    / fade_frames as f64)
                    .min(1.0) as f32;
                for channel in 0..self.channels {
                    let index = frame * self.channels + channel;
                    let previous = if prev_actual == 0 {
                        0.0
                    } else if prev_actual == actual_frames {
                        self.prev_process_buffer[index]
                    } else {
                        // Hosts with different output rates produce different
                        // frame counts for the same input duration. During a
                        // fade-through-silence transition, map the old block
                        // across the new block's duration so it does not end
                        // abruptly halfway through the callback.
                        let old_position = if actual_frames > 1 {
                            frame as f32 * (prev_actual - 1) as f32 / (actual_frames - 1) as f32
                        } else {
                            0.0
                        };
                        let old_frame = old_position.floor() as usize;
                        let next_old_frame = (old_frame + 1).min(prev_actual - 1);
                        let fraction = old_position - old_frame as f32;
                        let old = self.prev_process_buffer[old_frame * self.channels + channel];
                        let next =
                            self.prev_process_buffer[next_old_frame * self.channels + channel];
                        old + (next - old) * fraction
                    };
                    // Fade through silence: each half scales a single source by a
                    // gain in [0,1], so |out| <= max(|previous|, |output|).
                    if self.crossfade_through_silence {
                        output[index] = if alpha < 0.5 {
                            previous * (1.0 - 2.0 * alpha)
                        } else {
                            output[index] * (2.0 * alpha - 1.0)
                        };
                    } else {
                        let (old_gain, new_gain) = Self::convex_crossfade_gains(alpha);
                        output[index] = previous * old_gain + output[index] * new_gain;
                    }
                }
            }

            self.crossfade_frames = self.crossfade_frames.saturating_add(actual_frames);
            self.crossfade_progress =
                (self.crossfade_frames as f64 / fade_frames as f64).min(1.0) as f32;
            if self.crossfade_frames >= fade_frames {
                let previous = self.prev_host.take().expect("crossfade host exists");
                let old_path_delay = std::mem::take(&mut self.old_path_delay);
                let new_path_delay = std::mem::take(&mut self.new_path_delay);
                self.retire(GcItem::HostTransition {
                    host: previous,
                    old_path_delay,
                    new_path_delay,
                });
                self.crossfade_through_silence = false;
            }

            Ok(actual_frames)
        } else {
            // Normal processing - returns actual output frames
            self.host.process(input, output)
        }
    }
}

/// Result of an executed command; cancelled requests always continue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommandOutcome {
    Continue,
    Stopped,
    Shutdown,
}

impl CommandOutcome {
    #[cfg(test)]
    pub(super) fn is_shutdown(self) -> bool {
        self == Self::Shutdown
    }
}

/// Handles a command after atomically claiming its request ticket.
pub(super) fn handle_processing_command(
    request: ProcessingRequest,
    state: &mut ProcessingState,
    response_tx: &Sender<ProcessingReply>,
    event_tx: &crossbeam::channel::Sender<ThreadEvent>,
) -> CommandOutcome {
    let _ = event_tx;
    let ProcessingRequest {
        id: request_id,
        command,
        ticket,
    } = request;
    let response_tx = CorrelatedResponseSender {
        request_id,
        inner: response_tx,
    };
    if !ticket.try_claim() {
        response_tx
            .send(ProcessingResponse::Error(format!(
                "processing request {request_id} was cancelled before execution"
            )))
            .ok();
        return CommandOutcome::Continue;
    }
    match command {
        ProcessingCommand::CommitHostUpdate(update) => {
            let output_channels = update.output_channels;
            log::trace!(
                "[Processing Thread] CommitHostUpdate: committing prepared host, output_channels={}",
                output_channels
            );

            let (
                generation,
                output_channels,
                output_sample_rate,
                previous_latency_samples,
                latency_samples,
            ) = match state.commit_host_update(update) {
                Ok(notification) => notification,
                Err(reason) => {
                    response_tx
                        .send(ProcessingResponse::Error(reason.to_string()))
                        .ok();
                    return CommandOutcome::Continue;
                }
            };
            response_tx
                .send(ProcessingResponse::PluginChainUpdated {
                    generation,
                    output_channels,
                    output_sample_rate,
                    previous_latency_samples,
                    latency_samples,
                    latency_changed: previous_latency_samples != latency_samples,
                })
                .ok();
        }
        ProcessingCommand::SetParameter {
            plugin_index,
            param_id,
            value,
        } => {
            log::trace!(
                "[Processing Thread] Set parameter: plugin {} param {} = {}",
                plugin_index,
                param_id,
                value
            );

            // Parse string value to ParameterValue
            let param_value = sotf_plugins::ParameterValue::parse(&value);

            if let Err(e) =
                state
                    .host
                    .validate_plugin_parameter(plugin_index, &param_id, &param_value)
            {
                response_tx
                    .send(ProcessingResponse::Error(format!(
                        "Failed to set parameter: {e}"
                    )))
                    .ok();
                return CommandOutcome::Continue;
            }

            // This command already runs on the processing thread between
            // blocks. Apply it now so the acknowledgement reflects validation
            // and the actual host state, rather than merely queue admission.
            match state
                .host
                .set_plugin_parameter_immediate(plugin_index, &param_id, param_value)
            {
                Ok(_) => {
                    log::debug!(
                        "[Processing Thread] Parameter set successfully on plugin {}",
                        plugin_index
                    );
                    let output_channels = state.host.output_channels();
                    state.channels = output_channels;
                    let output_sample_rate =
                        match state.host.output_sample_rate_native(state.sample_rate) {
                            Ok(rate) => rate,
                            Err(error) => {
                                response_tx
                                    .send(ProcessingResponse::Error(format!(
                                        "Plugin parameter produced an invalid output clock: {error}"
                                    )))
                                    .ok();
                                return CommandOutcome::Continue;
                            }
                        };
                    response_tx
                        .send(ProcessingResponse::ParameterUpdated {
                            output_channels,
                            output_sample_rate,
                            latency_samples: state.host.total_latency_samples(),
                        })
                        .ok();
                }
                Err(e) => {
                    log::warn!(
                        "[Processing Thread] Failed to set parameter on plugin {}: {}",
                        plugin_index,
                        e
                    );
                    response_tx
                        .send(ProcessingResponse::Error(format!(
                            "Failed to set parameter: {}",
                            e
                        )))
                        .ok();
                }
            }
        }
        ProcessingCommand::Bypass(bypass) => {
            if bypass && let Some(previous) = state.prev_host.take() {
                let old_path_delay = std::mem::take(&mut state.old_path_delay);
                let new_path_delay = std::mem::take(&mut state.new_path_delay);
                state.retire(GcItem::HostTransition {
                    host: previous,
                    old_path_delay,
                    new_path_delay,
                });
                state.crossfade_progress = 1.0;
                state.crossfade_through_silence = false;
            }
            state.bypassed = bypass;
            log::debug!("[Processing Thread] Bypass: {}", bypass);
            response_tx.send(ProcessingResponse::Ok).ok();
        }
        ProcessingCommand::GetPluginData(index) => {
            // Commands run between blocks on the serialized control route.
            // Refresh dirty native state after reset/automation before replying;
            // the allocation-free bulk audio cache only clones existing data.
            let data = state
                .host
                .refresh_plugin_control_thread_metadata(index)
                .and_then(|()| {
                    state
                        .host
                        .get_plugin_data(index)
                        .ok_or_else(|| format!("Plugin {index} data not available"))
                });
            let response = match data {
                Ok(data) => ProcessingResponse::PluginData(data),
                Err(error) => ProcessingResponse::Error(error),
            };
            response_tx.send(response).ok();
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        ProcessingCommand::PollIsolatedExternalPluginWorkers => {
            let reports = state.host.poll_isolated_external_plugin_workers();
            let statuses = reports
                .into_iter()
                .map(isolated_external_plugin_status)
                .collect::<Vec<_>>();
            event_tx
                .try_send(ThreadEvent::IsolatedExternalPluginWorkerStatuses(statuses))
                .ok();
        }
        ProcessingCommand::Stop => {
            // Stop discards the current stream. Reset all buffered plugin
            // state so a later start cannot emit stale residual/tail audio.
            state.reset();
            log::debug!("[Processing Thread] Stopped");
            return CommandOutcome::Stopped;
        }
        ProcessingCommand::Shutdown => {
            log::debug!("[Processing Thread] Shutting down");
            return CommandOutcome::Shutdown;
        }
    }
    CommandOutcome::Continue
}

struct CorrelatedResponseSender<'a> {
    request_id: u64,
    inner: &'a Sender<ProcessingReply>,
}

impl CorrelatedResponseSender<'_> {
    fn send(
        &self,
        response: ProcessingResponse,
    ) -> Result<(), std::sync::mpsc::SendError<ProcessingReply>> {
        self.inner.send(ProcessingReply {
            id: self.request_id,
            response,
        })
    }
}

/// Update the shared plugin-data cache with the latest analyzer results.
///
/// Uses a spare `Arc` so that, when no UI reader holds the old cache, the
/// update is a zero-allocation in-place mutation.  When the UI is currently
/// reading the spare Arc, the update is skipped rather than allocating a new
/// buffer.
///
/// Returns `true` if the cache was updated this frame.
pub(super) fn update_plugin_data_cache(
    state: &mut ProcessingState,
    plugin_data_cache: &super::super::PluginDataCache,
) -> bool {
    let analyzer_indices = state.host.analyzer_indices();
    if analyzer_indices.is_empty() {
        return false;
    }

    let plugin_count = state.host.plugin_count();

    if let Some(mut spare) = state.spare_cache_arc.take() {
        if let Some(vec) = Arc::get_mut(&mut spare) {
            // Sole owner — mutate in place, zero allocations.
            state.cache_reuse_count += 1;
            if vec.len() != plugin_count {
                vec.resize(plugin_count, None);
            }
            for &i in analyzer_indices {
                vec[i] = state.host.get_plugin_data(i);
            }
            let old = plugin_data_cache.swap(spare);
            state.spare_cache_arc = Some(old);
            true
        } else {
            // Contention: UI thread still holds this Arc. Keep the spare for
            // the next attempt and skip this update — no allocation.
            state.spare_cache_arc = Some(spare);
            false
        }
    } else {
        // Defensive miss (should not happen after init because the spare is
        // pre-sized at build time). Count and skip rather than allocating on
        // the processing hot path.
        state.cache_fallback_count += 1;
        false
    }
}

/// Main processing thread function
#[allow(clippy::too_many_arguments)] // thread entrypoint receives all channel endpoints from constructor
pub(super) fn run_processing_thread(
    decoder_rx: Receiver<DecoderMessage>,
    message_tx: SyncSender<ProcessingMessage>,
    command_rx: Receiver<ProcessingRequest>,
    response_tx: Sender<ProcessingReply>,
    event_tx: crossbeam::channel::Sender<ThreadEvent>,
    sample_rate: u32,
    channels: usize,
    plugin_data_cache: super::super::PluginDataCache,
    gc_tx: super::super::GcSender,
    recycle_rx: Receiver<Vec<f32>>,
    decoder_recycle_tx: SyncSender<Vec<f32>>,
    host_generation: Arc<AtomicU64>,
    #[cfg(feature = "streaming")] network_stream_tap: Option<sotf_streaming::PcmStreamHandle>,
) -> Result<(), String> {
    // Enable FTZ/DAZ CPU flags to prevent denormal numbers from causing
    // performance issues in IIR filters and other DSP code
    sotf_plugins::enable_ftz_daz();

    // Elevate thread priority for lower latency
    match super::super::rt_priority::set_realtime_priority(
        super::super::rt_priority::RtPriority::Processing,
        None,
    ) {
        Ok(true) => log::info!("[Processing Thread] RT priority set successfully"),
        Ok(false) => log::debug!("[Processing Thread] RT priority not available on this platform"),
        Err(e) => log::warn!("[Processing Thread] Failed to set RT priority: {e}"),
    }

    let mut state = ProcessingState::new(
        channels,
        sample_rate,
        #[cfg(feature = "streaming")]
        network_stream_tap,
    );
    state.host_generation = host_generation;
    state.gc_tx = Some(gc_tx);

    log::info!(
        "[Processing Thread] Started - {}Hz, {} channels",
        sample_rate,
        channels
    );

    let mut decoder_stream_active = true;

    let mut terminal_rate_error = None;
    macro_rules! checked_processing_rate {
        ($label:lifetime, $result:expr) => {
            match $result {
                Ok(rate) => rate,
                Err(error) => {
                    terminal_rate_error = Some(error);
                    break $label;
                }
            }
        };
    }

    'processing: loop {
        // Check for commands (non-blocking)
        if let Ok(request) = command_rx.try_recv() {
            match handle_processing_command(request, &mut state, &response_tx, &event_tx) {
                CommandOutcome::Shutdown => break,
                CommandOutcome::Stopped => decoder_stream_active = false,
                CommandOutcome::Continue => {}
            }
        }

        // Process audio from decoder. Use a timeout receive instead of
        // try_recv + sleep so arriving frames wake the processing thread
        // immediately while idle engines still back off.
        let message = if decoder_stream_active {
            decoder_rx.recv_timeout(std::time::Duration::from_micros(
                ACTIVE_EMPTY_SLEEP_PROCESSING_US,
            ))
        } else {
            decoder_rx.recv_timeout(std::time::Duration::from_millis(
                IDLE_EMPTY_SLEEP_PROCESSING_MS,
            ))
        };

        match message {
            Ok(DecoderMessage::Frame(frame)) => {
                decoder_stream_active = true;
                let output_channels = state.output_channels();

                // Query plugin chain for actual output size (accounts for resampler)
                let output_frames = state.output_frames_for_input(frame.num_frames);
                let output_samples = output_frames * output_channels;

                // Query plugin chain for actual output sample rate
                let output_sample_rate = checked_processing_rate!(
                    'processing,
                    state.output_sample_rate(frame.sample_rate)
                );

                let mut process_buffer = std::mem::take(&mut state.process_buffer);
                if process_buffer.len() != output_samples {
                    ProcessingState::prepare_scratch_buffer(&mut process_buffer, output_samples);
                }

                let frame_start = std::time::Instant::now();
                // Pass only the expected output slice so the host sees exactly the
                // buffer size it advertised via `output_frames_for_input`.
                match state.process_frame(
                    &frame.data,
                    &mut process_buffer[..output_samples],
                    frame.num_frames,
                ) {
                    Ok(actual_output_frames) => {
                        let frame_elapsed = frame_start.elapsed();
                        if frame_elapsed > state.max_frame_duration {
                            state.max_frame_duration = frame_elapsed;
                        }
                        // Budget = frame_period. If processing exceeds it, the pipeline falls behind.
                        let frame_budget = std::time::Duration::from_secs_f64(
                            frame.num_frames as f64 / state.sample_rate as f64,
                        );
                        if frame_elapsed > frame_budget {
                            state.frames_over_budget += 1;
                        }

                        // Recycle the decoder frame's buffer back for reuse
                        decoder_recycle_tx.try_send(frame.data).ok();

                        // Use actual output frame count from processing (not max)
                        let actual_output_samples = actual_output_frames * output_channels;

                        state.frame_count += 1;

                        // Update shared plugin data cache so the UI can read
                        // analyzer results without blocking the audio pipeline.
                        let _ = update_plugin_data_cache(&mut state, &plugin_data_cache);

                        // Track timing for effective rate measurement
                        if state.first_frame_time.is_none() {
                            state.first_frame_time = Some(std::time::Instant::now());
                        }
                        state.total_output_samples += actual_output_samples as u64;

                        // Reuse a recycled Vec from the playback thread if available.
                        // Steady state should never allocate thanks to pre-filled recycle queues
                        // and the local fallback pool.
                        let frame_data = {
                            let mut buf = match recycle_rx.try_recv() {
                                Ok(mut v) => {
                                    v.clear();
                                    // Ensure capacity without re-allocating if possible.
                                    // reserve() is a no-op if capacity is already sufficient.
                                    if v.capacity() < actual_output_samples {
                                        v.reserve(actual_output_samples);
                                    }
                                    v
                                }
                                Err(_) => {
                                    // Fallback if recycle queue is empty (ramp-up or stall).
                                    // Pop a pre-sized spare first; only allocate as a last resort.
                                    state.recycle_fallback_pool.pop().unwrap_or_else(|| {
                                        state.recycle_miss_count += 1;
                                        Vec::with_capacity(actual_output_samples)
                                    })
                                }
                            };
                            buf.extend_from_slice(&process_buffer[..actual_output_samples]);
                            buf
                        };

                        // Use actual output frame count and sample rate (accounts for resampler)
                        let processed_frame = super::super::AudioFrame::try_new(
                            frame_data,
                            actual_output_frames,
                            output_channels,
                            output_sample_rate,
                        )
                        .map_err(|error| {
                            format!("processing produced an invalid output frame: {error}")
                        })?;

                        #[cfg(feature = "streaming")]
                        if let Some(tap) = &state.network_stream_tap {
                            tap.publish(
                                &processed_frame.data,
                                processed_frame.num_frames,
                                processed_frame.num_channels,
                                processed_frame.sample_rate,
                            );
                        }

                        let mut pending_msg = Some(ProcessingMessage::Frame(processed_frame));
                        // Retry sending until the message is delivered or we shut down
                        while let Some(msg) = pending_msg.take() {
                            match send_or_interrupt(&message_tx, &command_rx, msg) {
                                Ok(Some((cmd, unsent))) => {
                                    pending_msg = unsent;
                                    let outcome = handle_processing_command(
                                        cmd,
                                        &mut state,
                                        &response_tx,
                                        &event_tx,
                                    );
                                    if outcome != CommandOutcome::Continue {
                                        decoder_stream_active = false;
                                        if let Some(ProcessingMessage::Frame(frame)) =
                                            pending_msg.take()
                                        {
                                            state.recycle_output_buffer_locally(frame.data);
                                        }
                                    }
                                    if outcome == CommandOutcome::Shutdown {
                                        break 'processing;
                                    }
                                    // A successful update may change only the clock.
                                    // Compare the rendered frame's complete format.
                                    if state.output_channels() != output_channels
                                        || checked_processing_rate!(
                                            'processing,
                                            state.output_sample_rate(state.sample_rate)
                                        ) != output_sample_rate
                                    {
                                        if let Some(ProcessingMessage::Frame(frame)) =
                                            pending_msg.take()
                                        {
                                            state.recycle_output_buffer_locally(frame.data);
                                        }
                                        pending_msg = None;
                                    }
                                }
                                Ok(None) => {
                                    // Sent successfully
                                }
                                Err(e) => {
                                    log::debug!("[Processing Thread] Send error: {}", e);
                                    if let Some(ProcessingMessage::Frame(frame)) =
                                        pending_msg.take()
                                    {
                                        state.recycle_output_buffer_locally(frame.data);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        event_tx.try_send(ThreadEvent::ProcessingError(e)).ok();
                    }
                }
                state.process_buffer = process_buffer;

                // Diagnostics remain counter-only on the processing thread.
                // Formatting, logging, and analyzer-stat Vec construction are
                // deliberately excluded from this realtime loop.
            }
            Ok(DecoderMessage::EndOfStream) => 'eos: loop {
                decoder_stream_active = false;
                // Finalize every stateful plugin before publishing EOS. Drain
                // output is already propagated through downstream plugins by
                // DawHost, so a resampler followed by another buffered plugin
                // retains both tails in causal order.
                'drain: while !state.bypassed {
                    // A drain step can make progress without emitting frames,
                    // so it must also observe commands between plugin calls.
                    if let Ok(command) = command_rx.try_recv() {
                        match handle_processing_command(
                            command,
                            &mut state,
                            &response_tx,
                            &event_tx,
                        ) {
                            CommandOutcome::Shutdown => break 'processing,
                            CommandOutcome::Stopped => {
                                decoder_stream_active = false;
                                continue 'processing;
                            }
                            CommandOutcome::Continue => {}
                        }
                        if state.bypassed {
                            break 'drain;
                        }
                    }
                    // DawHost enforces each active plugin's prepared work bound.
                    // A global call cap would truncate legitimate long finite tails.
                    let draining_host_epoch = state.committed_host_epoch;
                    let output_channels = state.output_channels();
                    let output_sample_rate = checked_processing_rate!(
                        'processing,
                        state.output_sample_rate(state.sample_rate)
                    );
                    let capacity = state.host.drain_output_frames_max();
                    let samples = capacity.saturating_mul(output_channels);
                    ProcessingState::prepare_scratch_buffer(&mut state.process_buffer, samples);
                    let drain = match state.host.drain(&mut state.process_buffer[..samples]) {
                        Ok(result) => result,
                        Err(error) => {
                            event_tx.try_send(ThreadEvent::ProcessingError(error)).ok();
                            break 'processing;
                        }
                    };
                    // Analyzer-only finalization can change measurements while
                    // emitting zero frames. Publish those results before EOS.
                    let _ = update_plugin_data_cache(&mut state, &plugin_data_cache);
                    if drain.frames > 0 {
                        let actual_samples = drain.frames.saturating_mul(output_channels);
                        let mut data = recycle_rx.try_recv().unwrap_or_else(|_| {
                            state.recycle_fallback_pool.pop().unwrap_or_else(|| {
                                state.recycle_miss_count += 1;
                                Vec::with_capacity(actual_samples)
                            })
                        });
                        data.clear();
                        if data.capacity() < actual_samples {
                            data.reserve(actual_samples - data.capacity());
                        }
                        data.extend_from_slice(&state.process_buffer[..actual_samples]);
                        let frame = match super::super::AudioFrame::try_new(
                            data,
                            drain.frames,
                            output_channels,
                            output_sample_rate,
                        ) {
                            Ok(frame) => frame,
                            Err(error) => {
                                event_tx
                                    .try_send(ThreadEvent::ProcessingError(format!(
                                        "plugin drain produced an invalid output frame: {error}"
                                    )))
                                    .ok();
                                break 'processing;
                            }
                        };
                        let mut pending = Some(ProcessingMessage::Frame(frame));
                        while let Some(message) = pending.take() {
                            match send_or_interrupt(&message_tx, &command_rx, message) {
                                Ok(Some((command, unsent))) => {
                                    pending = unsent;
                                    let outcome = handle_processing_command(
                                        command,
                                        &mut state,
                                        &response_tx,
                                        &event_tx,
                                    );
                                    let format_changed = state.output_channels() != output_channels
                                        || checked_processing_rate!(
                                            'processing,
                                            state.output_sample_rate(state.sample_rate)
                                        ) != output_sample_rate;
                                    if (outcome != CommandOutcome::Continue
                                        || state.bypassed
                                        || format_changed)
                                        && let Some(ProcessingMessage::Frame(frame)) =
                                            pending.take()
                                    {
                                        state.recycle_output_buffer_locally(frame.data);
                                    }
                                    match outcome {
                                        CommandOutcome::Shutdown => break 'processing,
                                        CommandOutcome::Stopped => {
                                            decoder_stream_active = false;
                                            continue 'processing;
                                        }
                                        CommandOutcome::Continue => {}
                                    }
                                    if state.bypassed || format_changed {
                                        break 'drain;
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    if let Some(ProcessingMessage::Frame(frame)) = pending.take() {
                                        state.recycle_output_buffer_locally(frame.data);
                                    }
                                    event_tx.try_send(ThreadEvent::ProcessingError(error)).ok();
                                    break 'processing;
                                }
                            }
                        }
                    }
                    if drain.complete && state.committed_host_epoch == draining_host_epoch {
                        break;
                    }
                }
                if state.bypassed {
                    // The bypassed stream has ended. Discard frozen DSP state
                    // so disabling bypass cannot release an earlier stream's
                    // tail on a later start or end-of-stream notification.
                    state.reset();
                }
                let mut pending_msg = Some(ProcessingMessage::EndOfStream);
                while let Some(msg) = pending_msg.take() {
                    match send_or_interrupt(&message_tx, &command_rx, msg) {
                        Ok(Some((cmd, unsent))) => {
                            let old_host_epoch = state.committed_host_epoch;
                            let old_format = (
                                state.output_channels(),
                                checked_processing_rate!(
                                    'processing,
                                    state.output_sample_rate(state.sample_rate)
                                ),
                            );
                            pending_msg = unsent;
                            match handle_processing_command(
                                cmd,
                                &mut state,
                                &response_tx,
                                &event_tx,
                            ) {
                                CommandOutcome::Shutdown => break 'processing,
                                CommandOutcome::Stopped => {
                                    decoder_stream_active = false;
                                    pending_msg = None;
                                }
                                CommandOutcome::Continue => {}
                            }
                            if old_format
                                != (
                                    state.output_channels(),
                                    checked_processing_rate!(
                                        'processing,
                                        state.output_sample_rate(state.sample_rate)
                                    ),
                                )
                            {
                                pending_msg = None;
                            } else if pending_msg.is_some()
                                && state.committed_host_epoch != old_host_epoch
                            {
                                // The unsent EOS belongs to the replaced host.
                                continue 'eos;
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            event_tx.try_send(ThreadEvent::ProcessingError(error)).ok();
                            break 'processing;
                        }
                    }
                }
                break 'eos;
            },
            Ok(DecoderMessage::Flush) => {
                decoder_stream_active = true;
                // Reset plugin state (IIR filter history, compressor envelopes,
                // limiter lookahead, upmixer FFT buffers) so that stale pre-seek
                // audio doesn't cause transient artifacts in post-seek output.
                state.reset();
                let mut pending_msg = Some(ProcessingMessage::Flush);
                while let Some(msg) = pending_msg.take() {
                    match send_or_interrupt(&message_tx, &command_rx, msg) {
                        Ok(Some((cmd, unsent))) => {
                            pending_msg = unsent;
                            match handle_processing_command(
                                cmd,
                                &mut state,
                                &response_tx,
                                &event_tx,
                            ) {
                                CommandOutcome::Shutdown => break 'processing,
                                // The barrier survives Stop: the abandoned
                                // stream's DSP state is already reset, but
                                // downstream must still observe the discard
                                // boundary. Unlike EOS (a stream marker),
                                // Flush carries no stream content, so it is
                                // never stale — not even across channel
                                // changes — and the pending send retries.
                                CommandOutcome::Stopped => {
                                    decoder_stream_active = false;
                                }
                                CommandOutcome::Continue => {}
                            }
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                log::debug!("[Processing Thread] Decoder queue disconnected");
                break;
            }
        }
    }

    // Log effective playback rate measurement
    if let Some(start_time) = state.first_frame_time {
        let elapsed = start_time.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        let output_channels = state.output_channels();
        let total_frames = if output_channels > 0 {
            state.total_output_samples / output_channels as u64
        } else {
            0
        };
        let audio_duration_secs = total_frames as f64 / state.sample_rate as f64;
        let effective_rate = if elapsed_secs > 0.0 {
            (total_frames as f64 / elapsed_secs) as u64
        } else {
            0
        };
        let speed_ratio = if elapsed_secs > 0.0 {
            audio_duration_secs / elapsed_secs
        } else {
            0.0
        };
        log::info!(
            "[Processing Thread] PLAYBACK RATE: {} frames in {:.3}s = {} effective Hz (expected {}Hz), audio_duration={:.3}s, speed_ratio={:.4}x, plugins={}",
            total_frames,
            elapsed_secs,
            effective_rate,
            state.sample_rate,
            audio_duration_secs,
            speed_ratio,
            state.host.plugin_count(),
        );
    }

    log::debug!("[Processing Thread] Stopped");
    state.retire_owned_state();
    match terminal_rate_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
