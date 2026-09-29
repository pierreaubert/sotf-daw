use super::misc::MAX_OS_CHANNELS;
use super::misc::OS_CHUNK_SIZE;
use super::misc::interleaved_to_planar;
use super::misc::planar_to_interleaved;
use crate::plugin::PluginDrainResult;
use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler, WindowFunction};

#[derive(Clone, Copy, PartialEq, Eq)]
enum DrainStage {
    Idle,
    Input,
    UpTail,
    InnerTail,
    DownPartial,
    DownTail,
    Complete,
    Failed,
}

/// Oversampling processor that handles up/downsampling with residual buffering.
///
/// Usage:
/// 1. Create with `Oversampler::new(factor, channels)`
/// 2. Call `process()` with interleaved audio + a callback that processes at
///    the oversampled rate
/// 3. Query `latency_samples()` for PDC
pub struct Oversampler {
    /// 1x -> Nx resampler (upsample)
    pub(super) resampler_up: Fft<f32>,
    /// Nx -> 1x resampler (downsample)
    pub(super) resampler_down: Fft<f32>,
    /// Planar input buffer for up-resampler (one Vec per channel, length = OS_CHUNK_SIZE)
    pub(super) up_in: Vec<Vec<f32>>,
    /// Planar output buffer for up-resampler (one Vec per channel, length = OS_CHUNK_SIZE * factor)
    pub(super) up_out: Vec<Vec<f32>>,
    /// Planar input buffer for down-resampler (one Vec per channel, length = OS_CHUNK_SIZE * factor)
    pub(super) down_in: Vec<Vec<f32>>,
    /// Planar output buffer for down-resampler (one Vec per channel, length = OS_CHUNK_SIZE)
    pub(super) down_out: Vec<Vec<f32>>,
    /// Residual input frames (interleaved) waiting to fill a full OS_CHUNK_SIZE chunk
    pub(super) residual_in: Vec<f32>,
    /// Read cursor into `residual_in`
    pub(super) residual_in_read: usize,
    /// Number of frames currently in `residual_in`
    pub(super) residual_frames: usize,
    /// Residual output frames (interleaved) waiting to be consumed by the caller
    pub(super) residual_out: Vec<f32>,
    /// Number of frames currently ready in `residual_out`
    pub(super) residual_out_frames: usize,
    /// Read cursor into `residual_out`
    pub(super) residual_out_read: usize,
    /// Reusable interleaved chunk buffer for full OS_CHUNK_SIZE input blocks.
    pub(super) chunk_buffer: Vec<f32>,
    /// Oversampling factor (2 or 4)
    pub(super) factor: u32,
    /// Number of audio channels
    pub(super) channels: usize,
    /// Total latency in samples (at 1x rate) from the resampler pair
    pub(super) latency: usize,
    received_input: bool,
    drain_stage: DrainStage,
    inner_tail_frames: usize,
    inner_tail_read: usize,
    inner_tail_complete: bool,
    down_pending_frames: usize,
}

impl Oversampler {
    /// Create a new oversampler. `factor` must be 2 or 4. `channels` >= 1.
    pub fn new(factor: u32, channels: usize) -> Result<Self, String> {
        if factor != 2 && factor != 4 {
            return Err(format!(
                "Invalid oversampling factor {}: must be 2 or 4",
                factor
            ));
        }
        if channels == 0 {
            return Err("channels must be >= 1".to_string());
        }
        if channels > MAX_OS_CHANNELS {
            return Err(format!(
                "Oversampler supports at most {} channels, got {}",
                MAX_OS_CHANNELS, channels
            ));
        }

        let f = factor as usize;

        // new_custom preserves the historical single-sub-chunk geometry and
        // BlackmanHarris2 window; Fft::new would auto-select sub-chunks and
        // change delay and block sizes for the downsampling stage.
        // Up-resampler: input sample_rate 1, output sample_rate factor
        // chunk_size = OS_CHUNK_SIZE (fixed input)
        let resampler_up = Fft::<f32>::new_custom(
            1,
            f,
            OS_CHUNK_SIZE,
            1,
            channels,
            WindowFunction::BlackmanHarris2,
            FixedSync::Input,
        )
        .map_err(|e| format!("Failed to create up-resampler: {:?}", e))?;

        // Down-resampler: input sample_rate factor, output sample_rate 1
        // chunk_size = OS_CHUNK_SIZE * factor (fixed input, produces OS_CHUNK_SIZE output)
        let resampler_down = Fft::<f32>::new_custom(
            f,
            1,
            OS_CHUNK_SIZE * f,
            1,
            channels,
            WindowFunction::BlackmanHarris2,
            FixedSync::Input,
        )
        .map_err(|e| format!("Failed to create down-resampler: {:?}", e))?;

        let up_out_frames = resampler_up.output_frames_max();
        let down_out_frames = resampler_down.output_frames_max();

        // Latency: up-resampler delay (in output frames at Nx rate) converted to 1x frames,
        // plus down-resampler delay (already in 1x output frames).
        // Both delays are reported as output frames. We add them in 1x units.
        let up_delay_1x = resampler_up.output_delay() / f; // Nx -> 1x
        let down_delay_1x = resampler_down.output_delay();
        // Add one chunk of input buffering latency
        let latency = up_delay_1x + down_delay_1x + OS_CHUNK_SIZE;

        Ok(Self {
            resampler_up,
            resampler_down,
            up_in: vec![vec![0.0f32; OS_CHUNK_SIZE]; channels],
            up_out: vec![vec![0.0f32; up_out_frames]; channels],
            down_in: vec![vec![0.0f32; OS_CHUNK_SIZE * f]; channels],
            down_out: vec![vec![0.0f32; down_out_frames]; channels],
            // Residual I/O buffers pre-allocated for max expected frame size (4096)
            // to avoid hot-path resize. The resize guards remain as safety nets.
            residual_in: vec![0.0f32; (4096 + OS_CHUNK_SIZE) * channels],
            residual_in_read: 0,
            residual_frames: 0,
            residual_out: vec![0.0f32; (OS_CHUNK_SIZE + latency) * channels * 4],
            // A fixed chunk of silence makes the buffering delay independent
            // of callback partitioning and matches the reported PDC latency.
            residual_out_frames: OS_CHUNK_SIZE,
            residual_out_read: 0,
            chunk_buffer: vec![0.0f32; OS_CHUNK_SIZE * channels],
            factor,
            channels,
            latency,
            received_input: false,
            drain_stage: DrainStage::Idle,
            inner_tail_frames: 0,
            inner_tail_read: 0,
            inner_tail_complete: false,
            down_pending_frames: 0,
        })
    }

    /// Prepare residual queues for a maximum callback size on the control thread.
    ///
    /// Call before audio processing. This method can allocate; subsequent calls
    /// to [`Self::process`] with at most `max_frames` do not grow the residual
    /// queues. It preserves existing samples and does not impose a runtime cap.
    /// The supplied inner processing closure must uphold its own realtime contract.
    ///
    /// # Errors
    /// Returns an error if the requested capacity is not addressable.
    pub fn reserve_for_max_frames(&mut self, max_frames: usize) -> Result<(), String> {
        let samples = max_frames
            .checked_add(OS_CHUNK_SIZE)
            .and_then(|frames| frames.checked_add(self.latency))
            .and_then(|frames| frames.checked_mul(self.channels))
            .filter(|samples| *samples <= isize::MAX as usize / std::mem::size_of::<f32>())
            .ok_or_else(|| "Oversampling residual capacity overflow".to_string())?;
        if self.residual_in.len() < samples {
            self.residual_in.resize(samples, 0.0);
        }
        if self.residual_out.len() < samples {
            self.residual_out.resize(samples, 0.0);
        }
        Ok(())
    }

    /// Prepare storage for the inner plugin's maximum drain block on the control thread.
    pub(super) fn reserve_for_drain_frames(&mut self, frames: usize) -> Result<(), String> {
        if frames > isize::MAX as usize / std::mem::size_of::<f32>() {
            return Err("Oversampling drain capacity overflow".to_string());
        }
        for channel in &mut self.up_out {
            if channel.len() < frames {
                channel.resize(frames, 0.0);
            }
        }
        Ok(())
    }

    /// Reset all internal state (resamplers, residual buffers).
    pub fn reset(&mut self) {
        self.resampler_up.reset();
        self.resampler_down.reset();
        self.residual_in_read = 0;
        self.residual_frames = 0;
        self.residual_out_frames = OS_CHUNK_SIZE;
        self.residual_out_read = 0;
        self.received_input = false;
        self.drain_stage = DrainStage::Idle;
        self.inner_tail_frames = 0;
        self.inner_tail_read = 0;
        self.inner_tail_complete = false;
        self.down_pending_frames = 0;
        self.residual_out[..OS_CHUNK_SIZE * self.channels].fill(0.0);
        for ch_buf in &mut self.up_in {
            ch_buf.fill(0.0);
        }
        for ch_buf in &mut self.up_out {
            ch_buf.fill(0.0);
        }
        for ch_buf in &mut self.down_in {
            ch_buf.fill(0.0);
        }
        for ch_buf in &mut self.down_out {
            ch_buf.fill(0.0);
        }
    }

    /// Total latency in samples (at the original sample rate).
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// Bound zero-input continuation for a memoryless inner processing operation.
    ///
    /// Frames are at the original sample rate. The bound includes the fixed
    /// startup queue, residual chunk phase, and both finite FFT overlaps. It
    /// applies only when the inner operation cannot retain or generate audio
    /// after its input becomes zero. It is conservative, not a minimal endpoint.
    pub fn passthrough_tail_frames(&self) -> usize {
        4 * OS_CHUNK_SIZE
    }

    pub(super) fn tail_length(
        &self,
        inner: crate::plugin::TailLength,
    ) -> crate::plugin::TailLength {
        use crate::plugin::TailLength;
        match inner {
            TailLength::Finite(frames) => {
                // Rubato 5 fixed FFT stages each retain one chunk overlap.
                // Also cover residual input phase, chunk completion and the
                // fixed startup queue. Group delay alone is not FIR support.
                let chunk = super::misc::OS_CHUNK_SIZE as u64;
                let base_frames = frames.div_ceil(u64::from(self.factor));
                base_frames
                    .div_ceil(chunk)
                    .checked_mul(chunk)
                    .and_then(|frames| frames.checked_add(self.passthrough_tail_frames() as u64))
                    .map_or(TailLength::Unknown, TailLength::Finite)
            }
            other => other,
        }
    }

    /// Oversampling factor (2 or 4).
    pub fn factor(&self) -> u32 {
        self.factor
    }

    /// Process interleaved audio through the oversampling pipeline.
    ///
    /// `buffer` contains interleaved audio `[ch0_f0, ch1_f0, ch0_f1, ch1_f1, ...]`.
    /// `num_frames` is the number of frames in the buffer.
    /// `process_fn` is called with `(planar_buffers, oversampled_frames)` to process
    /// the audio at the oversampled rate. The callback processes in-place on planar
    /// buffers.
    ///
    /// Returns the number of output frames written to `buffer`.
    pub fn process<F>(
        &mut self,
        buffer: &mut [f32],
        num_frames: usize,
        mut process_fn: F,
    ) -> Result<usize, String>
    where
        F: FnMut(&mut [Vec<f32>], usize),
    {
        if self.drain_stage != DrainStage::Idle {
            return Err("Oversampler must be reset before processing after drain".to_string());
        }
        let nc = self.channels;
        let total_in_samples = num_frames * nc;
        self.received_input |= num_frames != 0;

        // 1. Append incoming frames to residual_in. The read cursor allows full
        // chunks to be consumed without shifting residual data every iteration.
        self.ensure_residual_in_capacity(num_frames);
        let write_start = (self.residual_in_read + self.residual_frames) * nc;
        self.residual_in[write_start..write_start + total_in_samples]
            .copy_from_slice(&buffer[..total_in_samples]);
        self.residual_frames += num_frames;

        if self.chunk_buffer.len() < OS_CHUNK_SIZE * nc {
            self.chunk_buffer.resize(OS_CHUNK_SIZE * nc, 0.0);
        }

        // 2. Process all full chunks from the residual input
        while self.residual_frames >= OS_CHUNK_SIZE {
            let chunk_len = OS_CHUNK_SIZE * nc;
            let chunk_start = self.residual_in_read * nc;
            self.chunk_buffer[..chunk_len]
                .copy_from_slice(&self.residual_in[chunk_start..chunk_start + chunk_len]);
            self.residual_in_read += OS_CHUNK_SIZE;
            self.residual_frames -= OS_CHUNK_SIZE;
            if self.residual_frames == 0 {
                self.residual_in_read = 0;
            }

            self.process_chunk(&mut |planar, frames| {
                process_fn(planar, frames);
                Ok(())
            })?;
        }

        // 3. Drain residual_out into buffer
        let mut frames_written = 0usize;
        while frames_written < num_frames {
            let frames_ready = self.residual_out_frames;
            let frames_needed = num_frames - frames_written;

            if frames_ready == 0 {
                // Not enough output ready (latency fill with zeros)
                let fill_start = frames_written * nc;
                buffer[fill_start..fill_start + frames_needed * nc].fill(0.0);
                break;
            }

            let frames_to_copy = frames_ready.min(frames_needed);
            let src_start = self.residual_out_read * nc;
            let dst_start = frames_written * nc;
            buffer[dst_start..dst_start + frames_to_copy * nc]
                .copy_from_slice(&self.residual_out[src_start..src_start + frames_to_copy * nc]);

            self.residual_out_read += frames_to_copy;
            self.residual_out_frames -= frames_to_copy;
            if self.residual_out_frames == 0 {
                self.residual_out_read = 0;
            }
            frames_written += frames_to_copy;
        }

        Ok(frames_written)
    }

    /// Flush the two FFT overlaps and the inner plugin's explicit finite tail.
    /// `Some(frames)` requests ordinary inner processing; `None` requests drain.
    /// All output is retained through the final padded downsampling chunk.
    /// Each call performs at most one resampling chunk or one inner drain call.
    pub(super) fn drain_with<F>(
        &mut self,
        output: &mut [f32],
        mut process_fn: F,
    ) -> Result<PluginDrainResult, String>
    where
        F: FnMut(&mut [Vec<f32>], Option<usize>) -> Result<PluginDrainResult, String>,
    {
        if self.drain_stage == DrainStage::Failed {
            return Err("Oversampler must be reset after a failed drain".to_string());
        }
        if !self.received_input
            || self.drain_stage == DrainStage::Complete && self.residual_out_frames == 0
        {
            return Ok(PluginDrainResult::COMPLETE);
        }
        // Reject capacity errors before changing queues, stage, or inner DSP.
        if output.len() < self.channels || !output.len().is_multiple_of(self.channels) {
            return Err(
                "Oversampling drain needs a nonempty frame-aligned output buffer".to_string(),
            );
        }
        if self.drain_stage == DrainStage::Idle {
            self.drain_stage = if self.residual_frames == 0 {
                DrainStage::UpTail
            } else {
                DrainStage::Input
            };
        }
        if self.residual_out_frames == 0 {
            match self.drain_stage {
                DrainStage::Input | DrainStage::UpTail => {
                    self.chunk_buffer.fill(0.0);
                    if self.drain_stage == DrainStage::Input {
                        let start = self.residual_in_read * self.channels;
                        let samples = self.residual_frames * self.channels;
                        self.chunk_buffer[..samples]
                            .copy_from_slice(&self.residual_in[start..start + samples]);
                    }
                    self.process_chunk(&mut |planar, frames| {
                        process_fn(planar, Some(frames)).map(|_| ())
                    })
                    .map_err(|error| self.fail_drain(error))?;
                    self.residual_frames = 0;
                    self.residual_in_read = 0;
                    self.drain_stage = if self.drain_stage == DrainStage::Input {
                        DrainStage::UpTail
                    } else {
                        DrainStage::InnerTail
                    };
                }
                DrainStage::InnerTail => {
                    if self.inner_tail_frames == 0 && !self.inner_tail_complete {
                        let result = process_fn(&mut self.up_out, None)
                            .map_err(|error| self.fail_drain(error))?;
                        if result.frames > self.up_out[0].len() {
                            self.drain_stage = DrainStage::Failed;
                            return Err(
                                "Oversampled inner drain exceeded prepared capacity".to_string()
                            );
                        }
                        self.inner_tail_frames = result.frames;
                        self.inner_tail_read = 0;
                        self.inner_tail_complete = result.complete;
                        // The inner step itself is the bounded unit of work for this call.
                        return Ok(PluginDrainResult {
                            frames: 0,
                            complete: false,
                        });
                    }
                    let chunk_frames = OS_CHUNK_SIZE * self.factor as usize;
                    let frames = self
                        .inner_tail_frames
                        .min(chunk_frames - self.down_pending_frames);
                    for (src, dst) in self.up_out.iter().zip(&mut self.down_in) {
                        dst[self.down_pending_frames..self.down_pending_frames + frames]
                            .copy_from_slice(
                                &src[self.inner_tail_read..self.inner_tail_read + frames],
                            );
                    }
                    self.inner_tail_frames -= frames;
                    self.inner_tail_read += frames;
                    self.down_pending_frames += frames;
                    if self.down_pending_frames == chunk_frames {
                        self.process_down_chunk()
                            .map_err(|error| self.fail_drain(error))?;
                        self.down_pending_frames = 0;
                    }
                    if self.inner_tail_frames == 0 && self.inner_tail_complete {
                        self.drain_stage = if self.down_pending_frames == 0 {
                            DrainStage::DownTail
                        } else {
                            DrainStage::DownPartial
                        };
                    }
                }
                DrainStage::DownPartial | DrainStage::DownTail => {
                    for channel in &mut self.down_in {
                        channel[self.down_pending_frames..].fill(0.0);
                    }
                    self.process_down_chunk()
                        .map_err(|error| self.fail_drain(error))?;
                    self.down_pending_frames = 0;
                    self.drain_stage = if self.drain_stage == DrainStage::DownPartial {
                        DrainStage::DownTail
                    } else {
                        DrainStage::Complete
                    };
                }
                DrainStage::Complete => {}
                DrainStage::Idle => unreachable!("drain stage initialized above"),
                DrainStage::Failed => unreachable!("failed drains rejected above"),
            }
        }
        let frames = self
            .residual_out_frames
            .min(output.len() / self.channels)
            .min(OS_CHUNK_SIZE);
        let start = self.residual_out_read * self.channels;
        output[..frames * self.channels]
            .copy_from_slice(&self.residual_out[start..start + frames * self.channels]);
        self.residual_out_read += frames;
        self.residual_out_frames -= frames;
        if self.residual_out_frames == 0 {
            self.residual_out_read = 0;
        }
        Ok(PluginDrainResult {
            frames,
            complete: self.drain_stage == DrainStage::Complete && self.residual_out_frames == 0,
        })
    }

    pub(super) fn fail_drain(&mut self, error: String) -> String {
        self.drain_stage = DrainStage::Failed;
        error
    }

    /// Validate caller storage before any wrapper EOS preparation.
    pub(super) fn validate_drain_output(&self, output: &[f32]) -> Result<(), String> {
        if self.drain_stage == DrainStage::Failed {
            return Err("Oversampler must be reset after a failed drain".into());
        }
        if self.received_input
            && !(self.drain_stage == DrainStage::Complete && self.residual_out_frames == 0)
            && (output.len() < self.channels || !output.len().is_multiple_of(self.channels))
        {
            return Err("Oversampling drain needs a nonempty frame-aligned output buffer".into());
        }
        Ok(())
    }

    /// Finish at most two prepared input chunks before querying the child bound.
    pub(super) fn begin_drain_with<F>(&mut self, mut process_fn: F) -> Result<(), String>
    where
        F: FnMut(&mut [Vec<f32>], usize) -> Result<(), String>,
    {
        if self.drain_stage == DrainStage::Failed {
            return Err("Oversampler must be reset after a failed drain".into());
        }
        if !self.received_input
            || !matches!(
                self.drain_stage,
                DrainStage::Idle | DrainStage::Input | DrainStage::UpTail
            )
        {
            return Ok(());
        }
        // A normal boundary retains at most C ready frames. Both setup chunks
        // together add at most 2C; setup allocation already reserves >=8C.
        let needed = self
            .residual_out_frames
            .checked_add(2 * OS_CHUNK_SIZE)
            .and_then(|frames| frames.checked_mul(self.channels))
            .ok_or("Oversampling EOS queue capacity overflow")?;
        if self.residual_frames >= OS_CHUNK_SIZE || needed > self.residual_out.len() {
            return Err("Oversampling EOS setup exceeds prepared capacity".into());
        }
        self.compact_residual_out();
        if self.drain_stage == DrainStage::Idle {
            self.drain_stage = if self.residual_frames == 0 {
                DrainStage::UpTail
            } else {
                DrainStage::Input
            };
        }
        for _ in 0..2 {
            if !matches!(self.drain_stage, DrainStage::Input | DrainStage::UpTail) {
                break;
            }
            self.chunk_buffer.fill(0.0);
            if self.drain_stage == DrainStage::Input {
                let start = self.residual_in_read * self.channels;
                let samples = self.residual_frames * self.channels;
                self.chunk_buffer[..samples]
                    .copy_from_slice(&self.residual_in[start..start + samples]);
            }
            self.process_chunk(&mut process_fn)
                .map_err(|error| self.fail_drain(error))?;
            self.residual_frames = 0;
            self.residual_in_read = 0;
            self.drain_stage = if self.drain_stage == DrainStage::Input {
                DrainStage::UpTail
            } else {
                DrainStage::InnerTail
            };
        }
        Ok(())
    }

    pub(super) fn received_input(&self) -> bool {
        self.received_input
    }

    pub(super) fn drain_failed(&self) -> bool {
        self.drain_stage == DrainStage::Failed
    }

    /// Compose native calls, cached transfer steps and final FFT overlaps.
    pub(super) fn drain_call_bound(
        &self,
        child_bound: Option<std::num::NonZeroU64>,
        child_capacity: usize,
    ) -> Option<std::num::NonZeroU64> {
        if !self.received_input {
            return std::num::NonZeroU64::new(1);
        }
        let queued = u64::try_from(self.residual_out_frames)
            .ok()?
            .div_ceil(OS_CHUNK_SIZE as u64);
        let calls = match self.drain_stage {
            DrainStage::Idle | DrainStage::Input | DrainStage::UpTail | DrainStage::Failed => {
                return None;
            }
            DrainStage::Complete => queued,
            DrainStage::DownTail => queued.checked_add(1)?,
            DrainStage::DownPartial => queued.checked_add(2)?,
            DrainStage::InnerTail => {
                let chunk = OS_CHUNK_SIZE as u64 * u64::from(self.factor);
                let child_calls = if self.inner_tail_complete {
                    0
                } else {
                    child_bound?.get()
                };
                // A result needs one native call and at most ceil(K/U)+1
                // transfer steps, including an empty final-result transition.
                let per_child = u64::try_from(child_capacity)
                    .ok()?
                    .div_ceil(chunk)
                    .checked_add(2)?;
                let cached = if self.inner_tail_frames > 0 || self.inner_tail_complete {
                    u64::try_from(self.inner_tail_frames)
                        .ok()?
                        .div_ceil(chunk)
                        .checked_add(1)?
                } else {
                    0
                };
                queued
                    .checked_add(child_calls.checked_mul(per_child)?)?
                    .checked_add(cached)?
                    .checked_add(2)?
            }
        };
        std::num::NonZeroU64::new(calls.max(1))
    }

    pub(super) fn ensure_residual_in_capacity(&mut self, additional_frames: usize) {
        let nc = self.channels;
        let needed_end = (self.residual_in_read + self.residual_frames + additional_frames) * nc;
        if needed_end <= self.residual_in.len() {
            return;
        }

        self.compact_residual_in();
        let needed = (self.residual_frames + additional_frames) * nc;
        if needed > self.residual_in.len() {
            self.residual_in.resize(needed + OS_CHUNK_SIZE * nc, 0.0);
        }
    }

    pub(super) fn compact_residual_in(&mut self) {
        if self.residual_in_read == 0 {
            return;
        }
        let nc = self.channels;
        let remaining = self.residual_frames * nc;
        if remaining > 0 {
            let src_start = self.residual_in_read * nc;
            self.residual_in
                .copy_within(src_start..src_start + remaining, 0);
        }
        self.residual_in_read = 0;
    }

    pub(super) fn ensure_residual_out_capacity(&mut self, additional_frames: usize) -> usize {
        let nc = self.channels;
        let mut write_frame = self.residual_out_read + self.residual_out_frames;
        let needed_end = (write_frame + additional_frames) * nc;
        if needed_end <= self.residual_out.len() {
            return write_frame;
        }

        self.compact_residual_out();
        write_frame = self.residual_out_frames;
        let needed = (write_frame + additional_frames) * nc;
        if needed > self.residual_out.len() {
            self.residual_out.resize(needed + OS_CHUNK_SIZE * nc, 0.0);
        }
        write_frame
    }

    pub(super) fn compact_residual_out(&mut self) {
        if self.residual_out_read == 0 {
            return;
        }
        let nc = self.channels;
        let remaining = self.residual_out_frames * nc;
        if remaining > 0 {
            let src_start = self.residual_out_read * nc;
            self.residual_out
                .copy_within(src_start..src_start + remaining, 0);
        }
        self.residual_out_read = 0;
    }

    /// Process one OS_CHUNK_SIZE chunk of interleaved input through
    /// upsample -> callback -> downsample.
    pub(super) fn process_chunk<F>(&mut self, process_fn: &mut F) -> Result<(), String>
    where
        F: FnMut(&mut [Vec<f32>], usize) -> Result<(), String>,
    {
        let nc = self.channels;
        let factor = self.factor as usize;

        // Step 1: interleaved -> planar into up_in
        interleaved_to_planar(
            &self.chunk_buffer[..OS_CHUNK_SIZE * nc],
            &mut self.up_in,
            OS_CHUNK_SIZE,
            nc,
        );

        // Step 2: upsample
        let up_out_max = self.resampler_up.output_frames_max();
        {
            let in_adapter =
                SequentialSliceOfVecs::new(&self.up_in, nc, OS_CHUNK_SIZE).map_err(|e| {
                    crate::rate_limited_log!(error, 5, "oversampling up_in adapter: {e:?}");
                    format!("up in adapter: {:?}", e)
                })?;
            let mut out_adapter = SequentialSliceOfVecs::new_mut(&mut self.up_out, nc, up_out_max)
                .map_err(|e| {
                    crate::rate_limited_log!(error, 5, "oversampling up_out adapter: {e:?}");
                    format!("up out adapter: {:?}", e)
                })?;
            self.resampler_up
                .process_into_buffer(&in_adapter, &mut out_adapter, None)
                .map_err(|e| {
                    crate::rate_limited_log!(error, 5, "oversampling upsample failed: {e:?}");
                    format!("upsample: {:?}", e)
                })?;
        }

        // The upsampled frame count is OS_CHUNK_SIZE * factor
        let up_frames = OS_CHUNK_SIZE * factor;

        // Step 3: call the process callback on upsampled data
        process_fn(&mut self.up_out, up_frames)?;

        // Step 4: copy upsampled data to down_in (they are different buffers)
        for ch in 0..nc {
            self.down_in[ch][..up_frames].copy_from_slice(&self.up_out[ch][..up_frames]);
        }

        self.process_down_chunk()
    }

    fn process_down_chunk(&mut self) -> Result<(), String> {
        let nc = self.channels;
        let factor = self.factor as usize;

        // Step 5: downsample
        let down_out_max = self.resampler_down.output_frames_max();
        let down_frames = {
            let in_adapter = SequentialSliceOfVecs::new(&self.down_in, nc, OS_CHUNK_SIZE * factor)
                .map_err(|e| {
                    crate::rate_limited_log!(error, 5, "oversampling down_in adapter: {e:?}");
                    format!("down in adapter: {:?}", e)
                })?;
            let mut out_adapter =
                SequentialSliceOfVecs::new_mut(&mut self.down_out, nc, down_out_max).map_err(
                    |e| {
                        crate::rate_limited_log!(error, 5, "oversampling down_out adapter: {e:?}");
                        format!("down out adapter: {:?}", e)
                    },
                )?;
            let (_, out_frames) = self
                .resampler_down
                .process_into_buffer(&in_adapter, &mut out_adapter, None)
                .map_err(|e| {
                    crate::rate_limited_log!(error, 5, "oversampling downsample failed: {e:?}");
                    format!("downsample: {:?}", e)
                })?;
            out_frames
        };

        // Step 6: planar -> interleaved into residual_out
        let write_frame = self.ensure_residual_out_capacity(down_frames);
        let write_offset = write_frame * nc;
        planar_to_interleaved(
            &self.down_out,
            &mut self.residual_out[write_offset..],
            down_frames,
            nc,
        );
        self.residual_out_frames += down_frames;

        Ok(())
    }
}
