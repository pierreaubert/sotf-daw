//! Focused R8 tests: graph drain sizing under stream-state-dependent declarations.
//!
//! A fresh resampler reports `drain_output_frames_max() == 0` and grows to its
//! block maximum mid-stream, while `output_frames_for_input` is chunk-quantized
//! around a residual carry. The build-time drain plan sized every holdover,
//! edge queue, and scratch buffer from the fresh zeros, so the first drain of
//! the AB087 diamond failed with `graph drain holdover for node 1 is smaller
//! than its declared drain capacity` (R7). The host now refreshes the plan from
//! live declarations at drain entry (grow-only merge) and answers the bound
//! query from live declarations, both gated by an allocation-free change check.
//!
//! [`ChunkedRateFixture`] reproduces those declaration dynamics exactly over
//! dyadic-exact DSP, inside a twin-converter diamond whose first converter
//! sits at node id 1 like the R7 failure. Both branches share one chunk grid,
//! so the join carries zero skew and the oracle models the host scheduler
//! bit-exactly. The precision legs pin lossless whole-stream content against
//! an independent raw-fixture oracle for the f32 and f64 process paths, plus
//! bound growth (fresh < post-process), mid-drain bound constancy
//! (once-sizing safety for this geometry), and allocation-free steady-state
//! process. Drain itself is not under the allocation counter: declaration
//! growth reserves on the first changed drain call by design, matching the
//! chain path's per-call scratch growth from live declarations.
//!
//! R9 additions. The f64 leg exposed the real host precision contract: drain
//! consume legitimately calls f32 `process` after an f64 stream (drain is
//! f32-only by design), so the fixture converts its older carry across the
//! documented bridge on a precision switch instead of refusing it. And live
//! declarations can legitimately rise mid-drain (chunk straddle as residuals
//! walk), so the host freezes the enforced per-call output bound for the
//! drain session while internal reservations keep tracking drift; the third
//! leg pins that freeze on a wide-block geometry whose live derivation
//! provably exceeds the frozen bound mid-drain, with drift refreshes
//! observed, emission paced to the frozen bound, and content still lossless.
//!
//! R11 additions. The fixtures model updated plugins via
//! `with_envelopes()` on identical DSP: honest stream-independent
//! envelopes that engage the prepared path (bound cached at rebuild, no
//! mid-drain growth possible, refresh path quiet). The fourth leg pins
//! that path on the narrow twin: bound still from build to drain end, no
//! refresh at entry or mid-drain, whole cold drain allocation-free, and
//! content bitwise against the same oracle.
//!
//! R12 additions. Build-time MAX_BLOCK preparation closes the cold
//! first-touch gap the fifth leg used to exempt: the strengthened
//! process leg now starts its allocation counter before the very first
//! process call (cold), walks every residual phase over eight 7813-frame
//! blocks, re-proves the first post-reset blocks (warm, capacities
//! survive reset), and pins capacity queries allocation-free too.

use super::super::{daw_host::DawHost, graph_edge::GraphEdge};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use crate::test_utils::assert_no_allocs_or_deallocs;
use std::collections::VecDeque;

/// Input frames per backend block, mirroring the resampler `chunk_size`.
const CHUNK_FRAMES: usize = 8;
/// Down 48→24 output frames per block (ratio 1/2).
const DOWN_OUT_PER_BLOCK: usize = 4;
/// Up 24→48 output frames per block (ratio 2).
const UP_OUT_PER_BLOCK: usize = 16;
/// Wide up block bound for the session-freeze geometry: 20 is 4 mod the
/// 8-frame chunk, so the live derivation straddles a chunk multiple as the
/// residual walks past 4 mid-drain.
const WIDE_UP_OUT_PER_BLOCK: usize = 20;

/// Chunk-blocked 2:1 rate converter with resampler-identical declarations.
///
/// Processing completes whole [`CHUNK_FRAMES`] input blocks and carries the
/// leftover across blocks; each block emits [`DOWN_OUT_PER_BLOCK`] pair
/// averages (`(a + b) * 0.5`) or `out_per_block` nearest-neighbor upsampled
/// frames (output `i` copies input `(i * CHUNK) / OUT`), all dyadic-exact.
/// The narrow bound ([`UP_OUT_PER_BLOCK`]) reproduces plain duplication
/// exactly; the wide bound ([`WIDE_UP_OUT_PER_BLOCK`]) resamples 8 frames
/// onto 20, which duplication cannot fill. `drain_output_frames_max` is 0
/// until the first accepted block and the block maximum after, exactly like
/// the resampler flush bound; the drain emits the residual-derived tail
/// (leftover pairs averaged, one unpaired down frame unchanged; the up tail
/// resampled to `ceil(res * OUT / CHUNK)` covering every leftover frame) in
/// a single call. Initialization and every
/// realtime entry point reject any rate but the configured input rate, so
/// mid-graph placement genuinely requires explicit-rate insertion. Latency
/// reports 0 like the strict converter fixture in
/// [`explicit_rate_node_append`](super::explicit_rate_node_append): the real
/// chunking delay is symmetric across both test branches, so relative join
/// skew is zero and absolute timing is the oracle's job.
struct ChunkedRateFixture {
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    down: bool,
    out_per_block: usize,
    residual: Vec<f32>,
    residual_f64: Vec<f64>,
    stream_input_frames: u64,
    last_output_frames: usize,
    publish_envelopes: bool,
}

impl ChunkedRateFixture {
    fn down_48_to_24(channels: usize) -> Self {
        Self::new(channels, 48_000, 24_000, DOWN_OUT_PER_BLOCK)
    }

    fn up_24_to_48(channels: usize) -> Self {
        Self::new(channels, 24_000, 48_000, UP_OUT_PER_BLOCK)
    }

    /// Up converter with a non-multiple-of-chunk block bound (20): the live
    /// quantum/emission derivation straddles a chunk multiple as the residual
    /// walks mid-drain, which is exactly the mid-drain bound-growth geometry
    /// the session-freeze test pins down.
    fn up_24_to_48_wide_block(channels: usize) -> Self {
        Self::new(channels, 24_000, 48_000, WIDE_UP_OUT_PER_BLOCK)
    }

    fn new(channels: usize, input_rate: u32, output_rate: u32, out_per_block: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        assert!(
            input_rate == 2 * output_rate || output_rate == 2 * input_rate,
            "fixture only models exact 2:1 rate steps"
        );
        let down = input_rate > output_rate;
        // Drain honesty bounds: the block bound doubles as the mid-stream
        // drain bound, so it must cover the largest single-block tail.
        // Down covers leftover pairs plus one unpaired frame; up covers
        // every chunk frame through the nearest-neighbor map, which is
        // surjective exactly when the bound reaches the chunk, and the
        // ceil tail then always fits the bound.
        if down {
            assert!(
                out_per_block * 2 >= CHUNK_FRAMES,
                "down block bound must cover one unpaired tail frame"
            );
        } else {
            assert!(
                out_per_block >= CHUNK_FRAMES,
                "up block bound must cover the chunk it resamples"
            );
        }
        Self {
            channels,
            input_rate,
            output_rate,
            down,
            out_per_block,
            residual: Vec::with_capacity(CHUNK_FRAMES * channels),
            residual_f64: Vec::with_capacity(CHUNK_FRAMES * channels),
            stream_input_frames: 0,
            last_output_frames: 0,
            publish_envelopes: false,
        }
    }

    /// Twin with honest stream-independent envelopes published, modeling
    /// an updated plugin on identical DSP. Legacy twins (flag off) keep
    /// unknown envelopes and the freeze-plus-refresh path.
    fn with_envelopes(mut self) -> Self {
        self.publish_envelopes = true;
        self
    }

    fn out_per_block(&self) -> usize {
        self.out_per_block
    }

    fn check_rate(&self, rate: u32, entry: &str) -> Result<(), String> {
        if rate == self.input_rate {
            Ok(())
        } else {
            Err(format!(
                "chunked rate fixture {entry} requires init at {} Hz, got {rate} Hz",
                self.input_rate
            ))
        }
    }

    fn residual_frames(&self) -> usize {
        (self.residual.len() / self.channels).max(self.residual_f64.len() / self.channels)
    }

    /// Up tail length for a residual of `residual_frames` frames.
    ///
    /// The drain resamples the leftover to the block grid with the same
    /// nearest-neighbor pattern as process, so the tail covers every
    /// residual frame in `ceil(residual * OUT / CHUNK)` frames. The count
    /// never exceeds the block bound (`residual < CHUNK` implies the
    /// ceiling is at most `OUT`), and the narrow bound reproduces the
    /// historical doubled tail exactly. Single formula shared by the
    /// drain emission and the tail declaration, so the two agree by
    /// construction.
    fn up_tail_frames(&self, residual_frames: usize) -> usize {
        residual_frames
            .saturating_mul(self.out_per_block)
            .saturating_add(CHUNK_FRAMES - 1)
            / CHUNK_FRAMES
    }
}

impl Plugin for ChunkedRateFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("ChunkedRateFixture", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(format!("unknown parameter: {id}"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: u32) -> Result<(), String> {
        self.check_rate(sample_rate, "initialize")?;
        self.residual.clear();
        self.residual_f64.clear();
        self.stream_input_frames = 0;
        self.last_output_frames = 0;
        Ok(())
    }

    fn reset(&mut self) {
        self.residual.clear();
        self.residual_f64.clear();
        self.stream_input_frames = 0;
        self.last_output_frames = 0;
    }

    fn output_sample_rate(&self, _input_rate: u32) -> u32 {
        self.output_rate
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        let pending = self.residual_frames().saturating_add(input_frames);
        let chunks = pending / CHUNK_FRAMES;
        chunks.saturating_mul(self.out_per_block())
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        if !self.publish_envelopes {
            return None;
        }
        // Residual stays in [0, CHUNK), so completed chunks are at most
        // (CHUNK - 1 + n) / CHUNK; non-decreasing in n as required.
        let max_residual = CHUNK_FRAMES.checked_sub(1)?;
        let chunks = max_residual.checked_add(input_frames)? / CHUNK_FRAMES;
        chunks.checked_mul(self.out_per_block())
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }

    fn tail_length(&self) -> TailLength {
        let residual = self.residual_frames();
        let frames = if self.down {
            residual / 2 + residual % 2
        } else {
            self.up_tail_frames(residual)
        };
        TailLength::Finite(frames as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.stream_input_frames == 0 {
            0
        } else {
            self.out_per_block()
        }
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        if !self.publish_envelopes {
            return None;
        }
        // Single-block tail, always within the block bound.
        Some(self.out_per_block())
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(1)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        self.check_rate(ctx.sample_rate, "process")?;
        let expected_input = ctx
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "chunked rate fixture input length overflow".to_string())?;
        if input.len() != expected_input {
            return Err(format!(
                "chunked rate fixture input has {} samples, expected {expected_input}",
                input.len()
            ));
        }
        let declared = self.output_frames_for_input(ctx.num_frames);
        let needed = declared
            .checked_mul(self.channels)
            .ok_or_else(|| "chunked rate fixture output length overflow".to_string())?;
        if output.len() < needed {
            return Err(format!(
                "chunked rate fixture output holds {} samples, need {needed}",
                output.len()
            ));
        }
        if !self.residual_f64.is_empty() {
            if !self.residual.is_empty() {
                // Both carries full is unorderable content: fail closed
                // rather than drop or reorder retained audio.
                return Err(
                    "chunked rate fixture refused mixed-precision carry without reset".into(),
                );
            }
            // The host drain-consume phase legitimately calls f32 process
            // after an f64 stream (drain is f32-only by design), so convert
            // the older f64 carry across the documented bridge and continue
            // in f32, preserving order and content. The pre-carry frame
            // count is unchanged, so the declared production above still
            // binds. Dyadic-exact here; asserted lossless.
            for &sample in &self.residual_f64 {
                debug_assert!(
                    (sample as f32) as f64 == sample,
                    "f64 carry must cross into f32 processing losslessly"
                );
            }
            self.residual
                .extend(self.residual_f64.iter().map(|&sample| sample as f32));
            self.residual_f64.clear();
        }
        let out_per_block = self.out_per_block();
        let mut produced = 0;
        for frame in input.chunks_exact(self.channels) {
            // Bounded: pending holds fewer than CHUNK frames before the push,
            // so the push never exceeds the construction-time reservation and
            // the callback never allocates.
            self.residual.extend_from_slice(frame);
            if self.residual.len() == CHUNK_FRAMES * self.channels {
                let end = (produced + out_per_block) * self.channels;
                let dst = &mut output[produced * self.channels..end];
                if self.down {
                    for (pair, slot) in self
                        .residual
                        .chunks_exact(2 * self.channels)
                        .zip(dst.chunks_exact_mut(self.channels))
                    {
                        for ch in 0..self.channels {
                            slot[ch] = (pair[ch] + pair[self.channels + ch]) * 0.5;
                        }
                    }
                } else {
                    // Nearest-neighbor upsample: OUT frames per chunk, each
                    // an exact copy of input frame (i * CHUNK) / OUT. OUT >=
                    // CHUNK (asserted at construction) makes the map
                    // surjective, so every input frame reaches the output and
                    // the count is exactly OUT, including the wide block
                    // bound, which plain duplication cannot fill.
                    for (i, slot) in dst.chunks_exact_mut(self.channels).enumerate() {
                        let src_frame = (i * CHUNK_FRAMES) / out_per_block;
                        debug_assert!(
                            src_frame < CHUNK_FRAMES,
                            "upsample source must stay inside the chunk"
                        );
                        let base = src_frame * self.channels;
                        slot.copy_from_slice(&self.residual[base..base + self.channels]);
                    }
                }
                produced += out_per_block;
                self.residual.clear();
            }
        }
        debug_assert_eq!(produced, declared);
        self.stream_input_frames = self
            .stream_input_frames
            .checked_add(ctx.num_frames as u64)
            .ok_or_else(|| "chunked rate fixture stream length overflow".to_string())?;
        self.last_output_frames = produced;
        Ok(produced)
    }

    fn supports_f64(&self) -> bool {
        true
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        self.check_rate(ctx.sample_rate, "process_f64")?;
        let expected_input = ctx
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "chunked rate fixture input length overflow".to_string())?;
        if input.len() != expected_input {
            return Err(format!(
                "chunked rate fixture input has {} samples, expected {expected_input}",
                input.len()
            ));
        }
        let declared = self.output_frames_for_input(ctx.num_frames);
        let needed = declared
            .checked_mul(self.channels)
            .ok_or_else(|| "chunked rate fixture output length overflow".to_string())?;
        if output.len() < needed {
            return Err(format!(
                "chunked rate fixture output holds {} samples, need {needed}",
                output.len()
            ));
        }
        if !self.residual.is_empty() {
            if !self.residual_f64.is_empty() {
                // Both carries full is unorderable content: fail closed
                // rather than drop or reorder retained audio.
                return Err(
                    "chunked rate fixture refused mixed-precision carry without reset".into(),
                );
            }
            // Mirror of the f32 bridge: an f64 call after f32-carried
            // content widens the older carry and continues in f64.
            // Widening is always exact.
            self.residual_f64
                .extend(self.residual.iter().map(|&sample| f64::from(sample)));
            self.residual.clear();
        }
        let out_per_block = self.out_per_block();
        let mut produced = 0;
        for frame in input.chunks_exact(self.channels) {
            self.residual_f64.extend_from_slice(frame);
            if self.residual_f64.len() == CHUNK_FRAMES * self.channels {
                let end = (produced + out_per_block) * self.channels;
                let dst = &mut output[produced * self.channels..end];
                if self.down {
                    for (pair, slot) in self
                        .residual_f64
                        .chunks_exact(2 * self.channels)
                        .zip(dst.chunks_exact_mut(self.channels))
                    {
                        for ch in 0..self.channels {
                            slot[ch] = (pair[ch] + pair[self.channels + ch]) * 0.5;
                        }
                    }
                } else {
                    // Nearest-neighbor upsample, mirroring the f32 path:
                    // output i copies input frame (i * CHUNK) / OUT.
                    for (i, slot) in dst.chunks_exact_mut(self.channels).enumerate() {
                        let src_frame = (i * CHUNK_FRAMES) / out_per_block;
                        debug_assert!(
                            src_frame < CHUNK_FRAMES,
                            "upsample source must stay inside the chunk"
                        );
                        let base = src_frame * self.channels;
                        slot.copy_from_slice(&self.residual_f64[base..base + self.channels]);
                    }
                }
                produced += out_per_block;
                self.residual_f64.clear();
            }
        }
        debug_assert_eq!(produced, declared);
        self.stream_input_frames = self
            .stream_input_frames
            .checked_add(ctx.num_frames as u64)
            .ok_or_else(|| "chunked rate fixture stream length overflow".to_string())?;
        self.last_output_frames = produced;
        Ok(produced)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        self.check_rate(context.sample_rate, "begin_drain")
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.check_rate(ctx.sample_rate, "drain")?;
        if self.stream_input_frames == 0 {
            // Fresh stream: no backend block ever ran, so no tail exists.
            // Completes without touching the (possibly zero-length) output,
            // exactly like the resampler flush.
            return Ok(PluginDrainResult::COMPLETE);
        }
        // Fail-closed backstop: both process paths convert the older carry
        // on a precision switch, so both carries full is unreachable
        // through public entry points — but draining it would need an
        // order the state cannot prove, so refuse rather than reorder.
        if !self.residual.is_empty() && !self.residual_f64.is_empty() {
            return Err(
                "chunked rate fixture drain refused mixed-precision tails without reset".into(),
            );
        }
        let residual_frames = self.residual_frames();
        debug_assert!(
            residual_frames < CHUNK_FRAMES,
            "pending input must be smaller than one block"
        );
        let frames = if self.down {
            residual_frames / 2 + residual_frames % 2
        } else {
            self.up_tail_frames(residual_frames)
        };
        debug_assert!(
            frames <= self.out_per_block(),
            "single-block tail must fit the declared drain bound"
        );
        if frames == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let needed = frames
            .checked_mul(self.channels)
            .ok_or_else(|| "chunked rate fixture drain length overflow".to_string())?;
        if output.len() < needed {
            return Err(format!(
                "chunked rate fixture drain output holds {} samples, need {needed}",
                output.len()
            ));
        }
        if self.residual_f64.is_empty() {
            emit_down_or_up_tail(
                &self.residual,
                output,
                self.channels,
                residual_frames,
                frames,
                self.down,
            );
            self.residual.clear();
        } else {
            for &sample in self.residual_f64.iter() {
                debug_assert!(
                    (sample as f32) as f64 == sample,
                    "f64 tail must cross the drain bridge losslessly"
                );
            }
            emit_down_or_up_tail(
                &self.residual_f64,
                output,
                self.channels,
                residual_frames,
                frames,
                self.down,
            );
            self.residual_f64.clear();
        }
        self.last_output_frames = frames;
        Ok(PluginDrainResult {
            frames,
            complete: true,
        })
    }
}

/// Emit one residual tail into the f32 drain destination.
///
/// Down mode averages leftover pairs and copies one unpaired frame unchanged;
/// up mode resamples the leftover with the process nearest-neighbor pattern,
/// emitting `tail_frames` (the shared ceil formula, always positive here)
/// covering every residual frame. Arithmetic runs in f64 and every fixture
/// value is dyadic-exact, so the f32 path reproduces its own process
/// arithmetic bit-exactly and the f64 path crosses the documented drain bridge
/// with the same `as f32` cast as the host retention handoff.
fn emit_down_or_up_tail<T: Copy + Into<f64>>(
    residual: &[T],
    output: &mut [f32],
    channels: usize,
    residual_frames: usize,
    tail_frames: usize,
    down: bool,
) {
    if down {
        let pairs = residual_frames / 2;
        for pair in 0..pairs {
            for ch in 0..channels {
                let a: f64 = residual[(2 * pair) * channels + ch].into();
                let b: f64 = residual[(2 * pair + 1) * channels + ch].into();
                output[pair * channels + ch] = ((a + b) * 0.5) as f32;
            }
        }
        if residual_frames % 2 == 1 {
            let base = (residual_frames - 1) * channels;
            for ch in 0..channels {
                let held: f64 = residual[base + ch].into();
                output[pairs * channels + ch] = held as f32;
            }
        }
    } else {
        // Nearest-neighbor tail: output frame i copies residual frame
        // (i * residual) / tail. tail_frames >= residual (block bound >=
        // CHUNK) makes the map surjective, so every leftover frame is
        // covered; the caller guarantees tail_frames > 0.
        debug_assert!(tail_frames > 0, "up tail needs a positive frame count");
        for frame in 0..tail_frames {
            let src = (frame * residual_frames) / tail_frames;
            debug_assert!(
                src < residual_frames,
                "tail source must stay inside the residual"
            );
            for ch in 0..channels {
                let sample: f64 = residual[src * channels + ch].into();
                output[frame * channels + ch] = sample as f32;
            }
        }
    }
}

/// Dyadic-exact gain with native f64 support.
///
/// Used for the twin branches' distinguishing mid-graph stage (0.5 at 24 kHz)
/// and as unity source/sink plumbing. Clock agnostic: every rate initializes,
/// frame geometry is the identity, and no tail is declared.
struct ExactGainFixture {
    channels: usize,
    factor: f32,
    last_output_frames: usize,
    publish_envelopes: bool,
}

impl ExactGainFixture {
    fn new(channels: usize, factor: f32) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        Self {
            channels,
            factor,
            last_output_frames: 0,
            publish_envelopes: false,
        }
    }

    /// Twin with honest stream-independent envelopes published, modeling
    /// an updated plugin on identical DSP.
    fn with_envelopes(mut self) -> Self {
        self.publish_envelopes = true;
        self
    }
}

impl Plugin for ExactGainFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("ExactGainFixture", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(format!("unknown parameter: {id}"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, _sample_rate: u32) -> Result<(), String> {
        self.last_output_frames = 0;
        Ok(())
    }

    fn reset(&mut self) {
        self.last_output_frames = 0;
    }

    fn output_sample_rate(&self, input_rate: u32) -> u32 {
        input_rate
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        // Every success path writes exactly `num_frames`, both precisions.
        self.publish_envelopes.then_some(input_frames)
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // No tail: drain always completes with zero frames.
        self.publish_envelopes.then_some(0)
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        let expected = ctx
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "gain fixture length overflow".to_string())?;
        if input.len() != expected {
            return Err(format!(
                "gain fixture input has {} samples, expected {expected}",
                input.len()
            ));
        }
        if output.len() < expected {
            return Err(format!(
                "gain fixture output holds {} samples, need {expected}",
                output.len()
            ));
        }
        for (dst, &src) in output[..expected].iter_mut().zip(input.iter()) {
            *dst = src * self.factor;
        }
        self.last_output_frames = ctx.num_frames;
        Ok(ctx.num_frames)
    }

    fn supports_f64(&self) -> bool {
        true
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        let expected = ctx
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "gain fixture length overflow".to_string())?;
        if input.len() != expected {
            return Err(format!(
                "gain fixture input has {} samples, expected {expected}",
                input.len()
            ));
        }
        if output.len() < expected {
            return Err(format!(
                "gain fixture output holds {} samples, need {expected}",
                output.len()
            ));
        }
        let factor = f64::from(self.factor);
        for (dst, &src) in output[..expected].iter_mut().zip(input.iter()) {
            *dst = src * factor;
        }
        self.last_output_frames = ctx.num_frames;
        Ok(ctx.num_frames)
    }
}

/// One raw stage process call with diligent actual-count readback.
fn node_process(stage: &mut dyn Plugin, rate: u32, input: &[f32]) -> Vec<f32> {
    let channels = stage.input_channels();
    assert_eq!(
        input.len() % channels,
        0,
        "oracle input must be whole frames"
    );
    let frames = input.len() / channels;
    let capacity = stage.output_frames_for_input(frames);
    let mut block = vec![f32::NAN; capacity * channels];
    let returned = stage
        .process(input, &mut block, &ProcessContext::new(rate, frames))
        .unwrap();
    let actual = stage.last_output_frames().unwrap_or(returned);
    assert!(
        actual <= capacity,
        "oracle stage overproduced its bound: {actual} > {capacity}"
    );
    block[..actual * channels].to_vec()
}

/// f64 twin of `node_process`: one raw stage block through `process_f64`.
fn node_process_f64(stage: &mut dyn Plugin, rate: u32, input: &[f64]) -> Vec<f64> {
    assert!(
        stage.supports_f64(),
        "f64 oracle stage must support process_f64"
    );
    let channels = stage.input_channels();
    assert_eq!(
        input.len() % channels,
        0,
        "oracle input must be whole frames"
    );
    let frames = input.len() / channels;
    let capacity = stage.output_frames_for_input(frames);
    let mut block = vec![f64::NAN; capacity * channels];
    let returned = stage
        .process_f64(input, &mut block, &ProcessContext::new(rate, frames))
        .unwrap();
    let actual = stage.last_output_frames().unwrap_or(returned);
    assert!(
        actual <= capacity,
        "oracle stage overproduced its bound: {actual} > {capacity}"
    );
    block[..actual * channels].to_vec()
}

/// One raw stage full drain with the reference loop shape.
fn node_drain(stage: &mut dyn Plugin, rate: u32) -> Vec<f32> {
    stage.begin_drain(&ProcessContext::new(rate, 0)).unwrap();
    let capacity = stage.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity * stage.input_channels()];
    // Declared call bound plus the structural completion handshake: a stage
    // that needs more calls than it declared is a production defect, not a
    // test-budget problem. Stages that decline to declare fail closed here.
    let bound = stage
        .drain_call_bound()
        .map(|calls| usize::try_from(calls.get()).expect("drain bound fits address space"))
        .expect("oracle stage must declare a drain call bound");
    let mut output = Vec::new();
    for _ in 0..bound
        .checked_add(1)
        .expect("drain bound plus handshake fits")
    {
        block.fill(f32::NAN);
        let result = stage
            .drain(&mut block, &ProcessContext::new(rate, 0))
            .unwrap();
        assert!(result.frames <= capacity);
        output.extend_from_slice(&block[..result.frames * stage.input_channels()]);
        if result.complete {
            return output;
        }
    }
    panic!("oracle drain exceeded its bounded call allowance");
}

/// Lossless twin-diamond oracle over raw fixtures.
///
/// Branch A runs down → 0.5 gain → up, branch B down → up; both share one
/// chunk grid, so per-block productions match exactly and the join carries
/// zero skew. Each block joins aligned prefixes; the finish cascades raw
/// native drains through downstream raw stages and joins retained queues plus
/// tails with zero-padding for the exhausted side, mirroring the drain
/// scheduler's EOF semantics. All fixtures declare zero latency, so no
/// compensation enters.
struct TwinDiamondOracle {
    down_a: ChunkedRateFixture,
    gain_a: ExactGainFixture,
    up_a: ChunkedRateFixture,
    down_b: ChunkedRateFixture,
    up_b: ChunkedRateFixture,
    queue_a: VecDeque<f32>,
    queue_b: VecDeque<f32>,
    queue_a64: VecDeque<f64>,
    queue_b64: VecDeque<f64>,
    channels: usize,
    a_counts: Vec<usize>,
    b_counts: Vec<usize>,
    a_nonzero: bool,
    b_nonzero: bool,
    ab_diverged: bool,
}

impl TwinDiamondOracle {
    fn new(channels: usize) -> Self {
        Self::new_inner(channels, false)
    }

    /// `wide_up_block` selects the 20-frame up block bound on both
    /// branches, matching the session-freeze host geometry exactly.
    fn new_inner(channels: usize, wide_up_block: bool) -> Self {
        let mut down_a = ChunkedRateFixture::down_48_to_24(channels);
        let mut gain_a = ExactGainFixture::new(channels, 0.5);
        let mut up_a = if wide_up_block {
            ChunkedRateFixture::up_24_to_48_wide_block(channels)
        } else {
            ChunkedRateFixture::up_24_to_48(channels)
        };
        let mut down_b = ChunkedRateFixture::down_48_to_24(channels);
        let mut up_b = if wide_up_block {
            ChunkedRateFixture::up_24_to_48_wide_block(channels)
        } else {
            ChunkedRateFixture::up_24_to_48(channels)
        };
        down_a.initialize(48_000).unwrap();
        gain_a.initialize(24_000).unwrap();
        up_a.initialize(24_000).unwrap();
        down_b.initialize(48_000).unwrap();
        up_b.initialize(24_000).unwrap();
        Self {
            down_a,
            gain_a,
            up_a,
            down_b,
            up_b,
            queue_a: VecDeque::new(),
            queue_b: VecDeque::new(),
            queue_a64: VecDeque::new(),
            queue_b64: VecDeque::new(),
            channels,
            a_counts: Vec::new(),
            b_counts: Vec::new(),
            a_nonzero: false,
            b_nonzero: false,
            ab_diverged: false,
        }
    }

    /// Feed one input block; returns the retained aligned join for the block.
    fn feed_block(&mut self, block: &[f32]) -> Vec<f32> {
        let a1 = node_process(&mut self.down_a, 48_000, block);
        let g = node_process(&mut self.gain_a, 24_000, &a1);
        let a2 = node_process(&mut self.up_a, 24_000, &g);
        let b1 = node_process(&mut self.down_b, 48_000, block);
        let b2 = node_process(&mut self.up_b, 24_000, &b1);
        self.queue_a.extend(a2.iter().copied());
        self.queue_b.extend(b2.iter().copied());
        self.a_counts.push(a2.len() / self.channels);
        self.b_counts.push(b2.len() / self.channels);
        self.join_min_prefix()
    }

    /// f64 twin of [`Self::feed_block`].
    fn feed_block_f64(&mut self, block: &[f64]) -> Vec<f64> {
        let a1 = node_process_f64(&mut self.down_a, 48_000, block);
        let g = node_process_f64(&mut self.gain_a, 24_000, &a1);
        let a2 = node_process_f64(&mut self.up_a, 24_000, &g);
        let b1 = node_process_f64(&mut self.down_b, 48_000, block);
        let b2 = node_process_f64(&mut self.up_b, 24_000, &b1);
        self.queue_a64.extend(a2.iter().copied());
        self.queue_b64.extend(b2.iter().copied());
        self.a_counts.push(a2.len() / self.channels);
        self.b_counts.push(b2.len() / self.channels);
        self.join_min_prefix_f64()
    }

    fn join_min_prefix(&mut self) -> Vec<f32> {
        let joined_frames =
            (self.queue_a.len() / self.channels).min(self.queue_b.len() / self.channels);
        let mut joined = Vec::with_capacity(joined_frames * self.channels);
        for _ in 0..joined_frames * self.channels {
            let a = self.queue_a.pop_front().unwrap();
            let b = self.queue_b.pop_front().unwrap();
            if a != 0.0 {
                self.a_nonzero = true;
            }
            if b != 0.0 {
                self.b_nonzero = true;
            }
            if a != b {
                self.ab_diverged = true;
            }
            joined.push(a + b);
        }
        joined
    }

    fn join_min_prefix_f64(&mut self) -> Vec<f64> {
        let joined_frames =
            (self.queue_a64.len() / self.channels).min(self.queue_b64.len() / self.channels);
        let mut joined = Vec::with_capacity(joined_frames * self.channels);
        for _ in 0..joined_frames * self.channels {
            let a = self.queue_a64.pop_front().unwrap();
            let b = self.queue_b64.pop_front().unwrap();
            if a != 0.0 {
                self.a_nonzero = true;
            }
            if b != 0.0 {
                self.b_nonzero = true;
            }
            if a != b {
                self.ab_diverged = true;
            }
            joined.push(a + b);
        }
        joined
    }

    /// Join retained queues plus native branch tails with zero-padding.
    fn finish(&mut self) -> Vec<f32> {
        let a_tail = node_drain(&mut self.down_a, 48_000);
        if !a_tail.is_empty() {
            let g = node_process(&mut self.gain_a, 24_000, &a_tail);
            let fed = node_process(&mut self.up_a, 24_000, &g);
            self.queue_a.extend(fed.iter().copied());
        }
        let au_tail = node_drain(&mut self.up_a, 24_000);
        self.queue_a.extend(au_tail.iter().copied());
        let b_tail = node_drain(&mut self.down_b, 48_000);
        if !b_tail.is_empty() {
            let fed = node_process(&mut self.up_b, 24_000, &b_tail);
            self.queue_b.extend(fed.iter().copied());
        }
        let bu_tail = node_drain(&mut self.up_b, 24_000);
        self.queue_b.extend(bu_tail.iter().copied());
        let joined_frames =
            (self.queue_a.len() / self.channels).max(self.queue_b.len() / self.channels);
        let mut joined = Vec::with_capacity(joined_frames * self.channels);
        for _ in 0..joined_frames * self.channels {
            let a = self.queue_a.pop_front().unwrap_or(0.0);
            let b = self.queue_b.pop_front().unwrap_or(0.0);
            if a != 0.0 {
                self.a_nonzero = true;
            }
            if b != 0.0 {
                self.b_nonzero = true;
            }
            if a != b {
                self.ab_diverged = true;
            }
            joined.push(a + b);
        }
        joined
    }

    /// f64-stream finish: cast retention across the drain bridge first (the
    /// host joins post-cast), proving each cast lossless, then the shared
    /// raw-drain cascade and f32 max-pad join.
    fn finish_f64(&mut self) -> Vec<f32> {
        for sample in self.queue_a64.drain(..) {
            let cast = sample as f32;
            assert!(
                (cast as f64) == sample,
                "f64 branch-A retention must cross the drain bridge losslessly"
            );
            self.queue_a.push_back(cast);
        }
        for sample in self.queue_b64.drain(..) {
            let cast = sample as f32;
            assert!(
                (cast as f64) == sample,
                "f64 branch-B retention must cross the drain bridge losslessly"
            );
            self.queue_b.push_back(cast);
        }
        self.finish()
    }
}

/// Twin-converter diamond: unity source fan-out to down → 0.5 gain → up and
/// down → up branches, joined at a unity sink. Seven nodes, zero latencies;
/// insertion order puts the first converter at node id 1 like the R7 failure.
fn build_twin_host() -> (DawHost, usize) {
    build_twin_host_inner(false, false)
}

/// `wide_up_block` selects the 20-frame up block bound on both branches
/// (session-freeze geometry); `publish_envelopes` models updated plugins
/// on identical DSP. Both branches stay identical so the join carries
/// zero skew in every combination.
fn build_twin_host_inner(wide_up_block: bool, publish_envelopes: bool) -> (DawHost, usize) {
    const CHANNELS: usize = 2;
    let wrap_gain = |factor: f32| {
        let fixture = ExactGainFixture::new(CHANNELS, factor);
        if publish_envelopes {
            fixture.with_envelopes()
        } else {
            fixture
        }
    };
    let wrap_down = || {
        let fixture = ChunkedRateFixture::down_48_to_24(CHANNELS);
        if publish_envelopes {
            fixture.with_envelopes()
        } else {
            fixture
        }
    };
    let wrap_up = || {
        let fixture = if wide_up_block {
            ChunkedRateFixture::up_24_to_48_wide_block(CHANNELS)
        } else {
            ChunkedRateFixture::up_24_to_48(CHANNELS)
        };
        if publish_envelopes {
            fixture.with_envelopes()
        } else {
            fixture
        }
    };
    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(wrap_gain(1.0)))
        .unwrap();
    let down_a = host
        .add_node("branch-a-down".to_string(), Box::new(wrap_down()))
        .unwrap();
    let gain_a = host
        .add_node_at_rate(
            "branch-a-gain".to_string(),
            Box::new(wrap_gain(0.5)),
            24_000,
        )
        .unwrap();
    let up_a = host
        .add_node_at_rate("branch-a-up".to_string(), Box::new(wrap_up()), 24_000)
        .unwrap();
    let down_b = host
        .add_node("branch-b-down".to_string(), Box::new(wrap_down()))
        .unwrap();
    let up_b = host
        .add_node_at_rate("branch-b-up".to_string(), Box::new(wrap_up()), 24_000)
        .unwrap();
    let sink = host
        .add_node("sink".to_string(), Box::new(wrap_gain(1.0)))
        .unwrap();
    for (from, to) in [
        (source, down_a),
        (down_a, gain_a),
        (gain_a, up_a),
        (up_a, sink),
        (source, down_b),
        (down_b, up_b),
        (up_b, sink),
    ] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(48_000), 48_000);
    (host, down_a)
}

/// Exact dyadic stereo fixture: multiples of 1/8, so every gain, average, and
/// join sum the graph computes is exact and the oracle comparison is bitwise.
fn dyadic_stereo_input(frames: usize, channels: usize) -> Vec<f32> {
    let mut input = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        for ch in 0..channels {
            let step = ((frame * channels + ch) % 32) as f32 / 8.0 - 1.5;
            input.push(step);
        }
    }
    input
}

/// Derived host-drain hang guard: every round consumes queued audio, emits
/// output, or completes (scheduler liveness); total work scales with stream
/// frames times graph nodes. Overrun means production no-progress, never a
/// larger arbitrary allowance.
fn host_drain_budget(stream_frames: usize, tail_frames: usize, nodes: usize) -> usize {
    (stream_frames + tail_frames + 1) * (nodes + 1) + 1
}

/// Irregular blocks, 41 frames total: five chunk crossings plus an odd residue,
/// so converters idle some blocks, fire others, and retain tails for drain.
const BLOCKS: [usize; 6] = [1, 7, 8, 9, 3, 13];

/// Straddle blocks, 39 frames total: the up converters end the stream with
/// an empty residual, and the first drain round feeds each exactly the 4
/// down-tail frames, walking the residual 0 → 4 across the chunk multiple
/// the 20-frame quantum straddles — so the live derivation provably rises
/// 40 → 60 mid-drain while the session bound stays frozen.
const STRADDLE_BLOCKS: [usize; 6] = [1, 7, 8, 9, 3, 11];

#[test]
fn state_dependent_drain_bounds_refresh_losslessly_f32() {
    const CHANNELS: usize = 2;
    const NODES: usize = 7;
    let total: usize = BLOCKS.iter().sum();
    assert_eq!(total, 41);
    let input = dyadic_stereo_input(total, CHANNELS);
    let (mut host, down_a) = build_twin_host();
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );
    let fresh_bound = host.drain_output_frames_max();

    // Independent lossless oracle over raw fixtures first.
    let mut oracle = TwinDiamondOracle::new(CHANNELS);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &BLOCKS {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        offset += frames;
    }
    let process_frames = expected.len() / CHANNELS;
    expected.extend_from_slice(&oracle.finish());
    let drain_frames = expected.len() / CHANNELS - process_frames;

    // Activation proofs: genuinely variable branches, both contribute nonzero
    // joined content, the mid-graph gain distinguishes them, and the drain
    // carries the residual tails.
    assert!(
        oracle.a_counts.contains(&0) && oracle.b_counts.contains(&0),
        "converter branches must idle some blocks"
    );
    assert!(
        oracle.a_counts.iter().any(|&count| count > 0)
            && oracle.b_counts.iter().any(|&count| count > 0),
        "converter branches must fire some blocks"
    );
    let a_min = *oracle.a_counts.iter().min().unwrap();
    let a_max = *oracle.a_counts.iter().max().unwrap();
    assert!(
        a_max > a_min,
        "converter branch counts must vary ({a_min}..={a_max})"
    );
    assert!(oracle.a_nonzero, "branch A content must enter the join");
    assert!(oracle.b_nonzero, "branch B content must enter the join");
    assert!(
        oracle.ab_diverged,
        "twin branches must join distinct content"
    );
    assert!(drain_frames > 0, "drain must carry tail content");

    // Warm-up outside the probe: first touch sizes every queue, delay, and
    // scratch buffer at the probed block sizes.
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
    }
    host.reset();
    // Steady-state probe: block sizes repeat the warm-up, so every buffer
    // is already at size. Any allocation inside is a realtime defect.
    // Collection stays outside the counter: growing the oracle vectors is
    // test harness work, not host behavior.
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        let mut actual = 0;
        assert_no_allocs_or_deallocs("state-dependent diamond process", || {
            host.process(block, &mut out).unwrap();
            actual = host
                .last_output_frames()
                .expect("diamond tracks production");
            assert!(actual <= capacity, "host overran its declared output bound");
        });
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        actual_counts, oracle.a_counts,
        "host per-block production must match the oracle branch counts"
    );

    // The R7 mechanism: live declarations outgrow the fresh zeros, so the
    // post-process bound exceeds the build-time one. The up-converter floor
    // (its live 16-frame drain bound propagates through the identity sink)
    // lower-bounds the refreshed value.
    let post_bound = host.drain_output_frames_max();
    assert!(
        fresh_bound < post_bound,
        "live declarations must outgrow the fresh zeros (fresh {fresh_bound}, post {post_bound})"
    );
    assert!(
        post_bound >= UP_OUT_PER_BLOCK,
        "refreshed bound {post_bound} must cover the live up-converter drain bound"
    );

    // Drain sized from the post-process bound. The bound stays constant
    // across drain here: pending input stays below one block, so the
    // chunk-quantized re-derivation agrees every round and once-sized
    // callers are safe; any growth would fail loudly below.
    let mut drain_out = vec![0.0f32; post_bound * CHANNELS];
    let mut drain_calls = 0;
    let live_tails = 2 * (DOWN_OUT_PER_BLOCK + UP_OUT_PER_BLOCK);
    let budget = host_drain_budget(total, live_tails, NODES);
    loop {
        let live = host.drain_output_frames_max();
        assert_eq!(
            live, post_bound,
            "bound must stay constant across drain for once-sized callers"
        );
        let step = host.drain(&mut drain_out).unwrap();
        assert!(
            step.frames <= post_bound,
            "drain emitted {} frames past its {}-frame bound",
            step.frames,
            post_bound
        );
        produced.extend_from_slice(&drain_out[..step.frames * CHANNELS]);
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }

    assert_eq!(
        produced.len(),
        expected.len(),
        "whole-stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "twin-converter diamond must match the oracle bitwise"
    );
    println!(
        "state-dependent f32: fresh bound {fresh_bound}, post bound {post_bound}, \
         drain {drain_frames} frames in {drain_calls} calls, whole {} frames, bitwise match",
        produced.len() / CHANNELS,
    );
}

#[test]
fn state_dependent_drain_bounds_refresh_losslessly_f64() {
    const CHANNELS: usize = 2;
    const NODES: usize = 7;
    let total: usize = BLOCKS.iter().sum();
    assert_eq!(total, 41);
    let input32 = dyadic_stereo_input(total, CHANNELS);
    let input: Vec<f64> = input32.iter().map(|&sample| f64::from(sample)).collect();
    let (mut host, down_a) = build_twin_host();
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );
    let fresh_bound = host.drain_output_frames_max();

    // Independent oracles: f64 over native f64 stages, f32 twin proving both
    // precisions share one chunk grid (blocking depends on counts, never on
    // values, so the grids must agree exactly).
    let mut oracle = TwinDiamondOracle::new(CHANNELS);
    let mut oracle32 = TwinDiamondOracle::new(CHANNELS);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &BLOCKS {
        expected.extend_from_slice(
            &oracle.feed_block_f64(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        oracle32.feed_block(&input32[offset * CHANNELS..(offset + frames) * CHANNELS]);
        offset += frames;
    }
    assert_eq!(
        oracle.a_counts, oracle32.a_counts,
        "f32/f64 chunk grids must agree on branch A"
    );
    assert_eq!(
        oracle.b_counts, oracle32.b_counts,
        "f32/f64 chunk grids must agree on branch B"
    );
    let expected_tail = oracle.finish_f64();
    let drain_frames = expected_tail.len() / CHANNELS;

    assert!(
        oracle.a_counts.contains(&0) && oracle.b_counts.contains(&0),
        "converter branches must idle some blocks"
    );
    assert!(
        oracle.a_counts.iter().any(|&count| count > 0)
            && oracle.b_counts.iter().any(|&count| count > 0),
        "converter branches must fire some blocks"
    );
    let a_min = *oracle.a_counts.iter().min().unwrap();
    let a_max = *oracle.a_counts.iter().max().unwrap();
    assert!(
        a_max > a_min,
        "converter branch counts must vary ({a_min}..={a_max})"
    );
    assert!(oracle.a_nonzero, "branch A content must enter the join");
    assert!(oracle.b_nonzero, "branch B content must enter the join");
    assert!(
        oracle.ab_diverged,
        "twin branches must join distinct content"
    );
    assert!(drain_frames > 0, "drain must carry tail content");

    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f64; capacity * CHANNELS];
        host.process_f64(block, &mut out).unwrap();
    }
    host.reset();
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f64; capacity * CHANNELS];
        let mut actual = 0;
        assert_no_allocs_or_deallocs("state-dependent diamond process_f64", || {
            host.process_f64(block, &mut out).unwrap();
            actual = host
                .last_output_frames()
                .expect("diamond tracks production");
            assert!(actual <= capacity, "host overran its declared output bound");
        });
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        actual_counts, oracle.a_counts,
        "host per-block production must match the oracle branch counts"
    );
    assert_eq!(
        produced.len(),
        expected.len(),
        "f64 process length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "f64 process content must match the oracle bitwise"
    );

    let post_bound = host.drain_output_frames_max();
    assert!(
        fresh_bound < post_bound,
        "live declarations must outgrow the fresh zeros (fresh {fresh_bound}, post {post_bound})"
    );
    assert!(
        post_bound >= UP_OUT_PER_BLOCK,
        "refreshed bound {post_bound} must cover the live up-converter drain bound"
    );

    // The host drain bridge is f32-only; the oracle finish casts with a
    // round-trip losslessness proof per sample (see `finish_f64`).
    let mut produced_tail = Vec::new();
    let mut drain_out = vec![0.0f32; post_bound * CHANNELS];
    let mut drain_calls = 0;
    let live_tails = 2 * (DOWN_OUT_PER_BLOCK + UP_OUT_PER_BLOCK);
    let budget = host_drain_budget(total, live_tails, NODES);
    loop {
        let live = host.drain_output_frames_max();
        assert_eq!(
            live, post_bound,
            "bound must stay constant across drain for once-sized callers"
        );
        let step = host.drain(&mut drain_out).unwrap();
        assert!(
            step.frames <= post_bound,
            "drain emitted {} frames past its {}-frame bound",
            step.frames,
            post_bound
        );
        produced_tail.extend_from_slice(&drain_out[..step.frames * CHANNELS]);
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }

    assert_eq!(
        produced_tail.len(),
        expected_tail.len(),
        "f64-stream drain length must match the oracle"
    );
    assert_eq!(
        produced_tail, expected_tail,
        "f64-stream drain content must match the oracle bitwise"
    );
    println!(
        "state-dependent f64: fresh bound {fresh_bound}, post bound {post_bound}, \
         drain {drain_frames} frames in {drain_calls} calls, bitwise match",
    );
}

#[test]
fn state_dependent_drain_session_bound_freezes_mid_drain_growth() {
    const CHANNELS: usize = 2;
    const NODES: usize = 7;
    let total: usize = STRADDLE_BLOCKS.iter().sum();
    assert_eq!(total, 39);
    let input = dyadic_stereo_input(total, CHANNELS);
    let (mut host, down_a) = build_twin_host_inner(true, false);
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );
    let fresh_bound = host.drain_output_frames_max();

    // Independent lossless oracle over wide-block raw fixtures.
    let mut oracle = TwinDiamondOracle::new_inner(CHANNELS, true);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &STRADDLE_BLOCKS {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        offset += frames;
    }
    let process_frames = expected.len() / CHANNELS;
    expected.extend_from_slice(&oracle.finish());
    let drain_frames = expected.len() / CHANNELS - process_frames;

    // Same activation shape as the precision legs: genuinely variable
    // branches, both contribute nonzero joined content, the mid-graph gain
    // distinguishes them, and the drain carries the residual tails.
    assert!(
        oracle.a_counts.contains(&0) && oracle.b_counts.contains(&0),
        "converter branches must idle some blocks"
    );
    assert!(
        oracle.a_counts.iter().any(|&count| count > 0)
            && oracle.b_counts.iter().any(|&count| count > 0),
        "converter branches must fire some blocks"
    );
    let a_min = *oracle.a_counts.iter().min().unwrap();
    let a_max = *oracle.a_counts.iter().max().unwrap();
    assert!(
        a_max > a_min,
        "converter branch counts must vary ({a_min}..={a_max})"
    );
    assert!(oracle.a_nonzero, "branch A content must enter the join");
    assert!(oracle.b_nonzero, "branch B content must enter the join");
    assert!(
        oracle.ab_diverged,
        "twin branches must join distinct content"
    );
    assert!(drain_frames > 0, "drain must carry tail content");

    // Plain (non-counter) process: allocation-free steady state is pinned
    // by the precision legs; this leg pins the session freeze.
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(STRADDLE_BLOCKS.len());
    let mut cursor = 0;
    for &frames in &STRADDLE_BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
        let actual = host
            .last_output_frames()
            .expect("diamond tracks production");
        assert!(actual <= capacity, "host overran its declared output bound");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        actual_counts, oracle.a_counts,
        "host per-block production must match the oracle branch counts"
    );

    let post_bound = host.drain_output_frames_max();
    assert!(
        fresh_bound < post_bound,
        "live declarations must outgrow the fresh zeros (fresh {fresh_bound}, post {post_bound})"
    );
    assert!(
        post_bound >= WIDE_UP_OUT_PER_BLOCK,
        "refreshed bound {post_bound} must cover the live wide up-converter drain bound"
    );

    // Drain sized once from the post-process bound. The bound query must
    // stay frozen while the live derivation rises past it; drift refreshes
    // must run on the growth; emission must stay paced to the frozen bound
    // so the once-sized buffer holds to completion.
    let mut drain_out = vec![0.0f32; post_bound * CHANNELS];
    let mut drain_calls = 0;
    let mut max_live = post_bound;
    let live_tails = 2 * (DOWN_OUT_PER_BLOCK + WIDE_UP_OUT_PER_BLOCK);
    let budget = host_drain_budget(total, live_tails, NODES);
    loop {
        let live = host.drain_output_frames_max();
        assert_eq!(
            live, post_bound,
            "bound query must stay frozen for the session"
        );
        let unfrozen = host
            .graph_drain_live_bound_for_test()
            .expect("graph drain exposes its live bound");
        max_live = max_live.max(unfrozen);
        let step = host.drain(&mut drain_out).unwrap();
        assert!(
            step.frames <= post_bound,
            "drain emitted {} frames past its {}-frame frozen bound",
            step.frames,
            post_bound
        );
        produced.extend_from_slice(&drain_out[..step.frames * CHANNELS]);
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }
    // Self-proving growth: without a live derivation observably above the
    // frozen bound this test would pass vacuously on a quiet stream.
    assert!(
        max_live > post_bound,
        "live derivation must observably exceed the frozen bound (max {max_live} vs frozen {post_bound})"
    );
    assert!(
        host.graph_drain_refresh_count_for_test() >= 2,
        "drift refreshes must run mid-session, not only at drain entry"
    );
    assert_eq!(
        produced.len(),
        expected.len(),
        "whole-stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "wide-block diamond must match the oracle bitwise"
    );
    println!(
        "state-dependent freeze: fresh bound {fresh_bound}, frozen {post_bound}, live max {max_live}, \
         drain {drain_frames} frames in {drain_calls} calls, bitwise match",
    );
}

#[test]
fn envelope_prepared_drain_needs_no_refresh_and_no_allocations() {
    const CHANNELS: usize = 2;
    const NODES: usize = 7;
    let total: usize = BLOCKS.iter().sum();
    assert_eq!(total, 41);
    let input = dyadic_stereo_input(total, CHANNELS);
    let (mut host, down_a) = build_twin_host_inner(false, true);
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );

    // The prepared path must actually engage: envelope bound present from
    // build, and the bound query answers it fresh (no zero-to-max growth
    // like the legacy legs). Narrow-twin derivation: up quanta 16 emit
    // 32, sink collects 32.
    let envelope = host
        .graph_drain_envelope_bound_for_test()
        .expect("all-envelope twin must prepare an envelope plan");
    assert_eq!(
        envelope, 32,
        "narrow-twin envelope derivation must match by hand"
    );
    let fresh_bound = host.drain_output_frames_max();
    assert_eq!(
        fresh_bound, envelope,
        "bound query must answer the prepared envelope fresh"
    );

    // Independent lossless oracle over identical DSP (legacy twins:
    // envelopes change sizing, never audio).
    let mut oracle = TwinDiamondOracle::new(CHANNELS);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &BLOCKS {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * CHANNELS..(offset + frames) * CHANNELS]),
        );
        offset += frames;
    }
    let process_frames = expected.len() / CHANNELS;
    expected.extend_from_slice(&oracle.finish());
    let drain_frames = expected.len() / CHANNELS - process_frames;

    // Same activation shape as the legacy legs.
    assert!(
        oracle.a_counts.contains(&0) && oracle.b_counts.contains(&0),
        "converter branches must idle some blocks"
    );
    assert!(
        oracle.a_counts.iter().any(|&count| count > 0)
            && oracle.b_counts.iter().any(|&count| count > 0),
        "converter branches must fire some blocks"
    );
    let a_min = *oracle.a_counts.iter().min().unwrap();
    let a_max = *oracle.a_counts.iter().max().unwrap();
    assert!(
        a_max > a_min,
        "converter branch counts must vary ({a_min}..={a_max})"
    );
    assert!(oracle.a_nonzero, "branch A content must enter the join");
    assert!(oracle.b_nonzero, "branch B content must enter the join");
    assert!(
        oracle.ab_diverged,
        "twin branches must join distinct content"
    );
    assert!(drain_frames > 0, "drain must carry tail content");

    // Plain process: per-block production must match the oracle branch
    // counts exactly (scheduling is live; only sizing is prepared).
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
        let actual = host
            .last_output_frames()
            .expect("diamond tracks production");
        assert!(actual <= capacity, "host overran its declared output bound");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        actual_counts, oracle.a_counts,
        "host per-block production must match the oracle branch counts"
    );

    // The prepared bound never moves: post-process query still answers it.
    let post_bound = host.drain_output_frames_max();
    assert_eq!(
        post_bound, envelope,
        "prepared bound must not move mid-stream (fresh {fresh_bound}, post {post_bound})"
    );

    // Bound queries themselves must not allocate on the prepared path.
    assert_no_allocs_or_deallocs("envelope bound query", || {
        for _ in 0..8 {
            assert_eq!(host.drain_output_frames_max(), envelope);
        }
    });

    // Whole cold drain under the allocation counter: retention transfer,
    // session freeze, and every round allocate nothing on prepared
    // reservations. Buffer sized once outside the counter.
    let mut drain_out = vec![0.0f32; post_bound * CHANNELS];
    let mut drain_calls = 0;
    let live_tails = 2 * (DOWN_OUT_PER_BLOCK + UP_OUT_PER_BLOCK);
    let budget = host_drain_budget(total, live_tails, NODES);
    loop {
        let mut step = PluginDrainResult::COMPLETE;
        assert_no_allocs_or_deallocs("envelope prepared graph drain", || {
            step = host.drain(&mut drain_out).unwrap();
        });
        assert!(
            step.frames <= post_bound,
            "drain emitted {} frames past its {}-frame prepared bound",
            step.frames,
            post_bound
        );
        produced.extend_from_slice(&drain_out[..step.frames * CHANNELS]);
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }
    // Self-proving quiet: the legacy straddle twin refreshes at least
    // twice on drift; the prepared twin must never refresh at all.
    assert_eq!(
        host.graph_drain_refresh_count_for_test(),
        0,
        "prepared drain must never refresh, at entry or mid-drain"
    );
    assert_eq!(
        produced.len(),
        expected.len(),
        "whole-stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "prepared twin must match the oracle bitwise"
    );
    println!(
        "envelope prepared: bound {envelope} still fresh-to-post, {drain_calls} drain calls, \
         0 refreshes, drain {drain_frames} frames, bitwise match",
    );
}

#[test]
fn envelope_prepared_process_allocates_nothing_cold_and_warm() {
    const CHANNELS: usize = 2;
    // Near-max-block 7813-frame blocks (the diamond's own big block):
    // 7813 = 976 * 8 + 5 walks the 8-frame residual through every phase
    // over 8 blocks, so straddle peaks hit every alignment. The counter
    // starts before the very first process call: build-time preparation
    // must cover the cold callback, not merely warmed repeats.
    const BIG_BLOCK: usize = 7813;
    const BIG_BLOCKS: usize = 8;
    let total = BIG_BLOCK * BIG_BLOCKS;
    let input = dyadic_stereo_input(total, CHANNELS);
    let (mut host, down_a) = build_twin_host_inner(false, true);
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );
    let mut oracle = TwinDiamondOracle::new(CHANNELS);

    // End-to-end propagation arithmetic, by hand: down emits 3908,
    // gain passes, up emits 7824 on both branches, sink collects 7824.
    let envelope_7813 = host
        .output_frames_envelope(BIG_BLOCK)
        .expect("all-envelope twin must propagate a process envelope");
    assert_eq!(
        envelope_7813, 7824,
        "host envelope propagation must match by hand"
    );

    // Capacity queries themselves must not allocate, cold or mid-stream.
    assert_no_allocs_or_deallocs("envelope process capacity queries", || {
        for size in [1, 64, BIG_BLOCK, 8192] {
            assert!(host.output_frames_envelope(size).is_some());
            let live = host.output_frames_for_input(size);
            assert!(live <= host.output_frames_envelope(size).unwrap());
        }
        let envelope_bound = host.graph_drain_envelope_bound_for_test().unwrap();
        assert!(host.drain_output_frames_max() <= envelope_bound);
    });

    let mut produced = Vec::new();
    let mut expected = Vec::new();
    let mut cursor = 0;
    for block_index in 0..BIG_BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + BIG_BLOCK) * CHANNELS];
        cursor += BIG_BLOCK;
        expected.extend_from_slice(&oracle.feed_block(block));
        // Callers size from the live declaration; the envelope only
        // prepares host internals.
        let capacity = host.output_frames_for_input(BIG_BLOCK);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        let mut actual = 0;
        assert_no_allocs_or_deallocs("envelope prepared cold process", || {
            host.process(block, &mut out).unwrap();
            actual = host
                .last_output_frames()
                .expect("diamond tracks production");
        });
        assert!(actual <= capacity, "host overran its declared output bound");
        // The envelope property, empirically in every residual state.
        let live_envelope = host
            .output_frames_envelope(BIG_BLOCK)
            .expect("envelope must stay known mid-stream");
        assert_eq!(
            live_envelope, envelope_7813,
            "process envelope must be stream-independent"
        );
        assert!(
            actual <= live_envelope,
            "block {block_index} produced {actual} past its {live_envelope}-frame envelope"
        );
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        produced.len(),
        expected.len(),
        "whole-stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "prepared twin must match the oracle bitwise"
    );

    // Warm: reset keeps prepared capacities, so the first post-reset
    // block (a second cold start) allocates nothing either.
    host.reset();
    let mut oracle = TwinDiamondOracle::new(CHANNELS);
    let mut produced = Vec::new();
    let mut expected = Vec::new();
    let mut cursor = 0;
    for block_index in 0..2 {
        let block = &input[cursor * CHANNELS..(cursor + BIG_BLOCK) * CHANNELS];
        cursor += BIG_BLOCK;
        expected.extend_from_slice(&oracle.feed_block(block));
        let capacity = host.output_frames_for_input(BIG_BLOCK);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        let mut actual = 0;
        assert_no_allocs_or_deallocs("envelope prepared warm process", || {
            host.process(block, &mut out).unwrap();
            actual = host
                .last_output_frames()
                .expect("diamond tracks production");
        });
        assert!(
            actual <= capacity,
            "warm block {block_index} overran its declared output bound"
        );
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(
        produced.len(),
        expected.len(),
        "warm whole-stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "warm twin must match the oracle bitwise"
    );
    println!(
        "envelope process: {BIG_BLOCKS} x {BIG_BLOCK} frames cold plus 2 warm, \
         envelope {envelope_7813}, zero callback allocations, bitwise match",
    );
}

#[test]
fn envelope_prepared_process_f64_allocates_nothing_first_call() {
    const CHANNELS: usize = 2;
    // F4: the f64 guarded loop runs post-reset; this pins the cold FIRST
    // f64 call after build. f64 buffers are prepared in the same build
    // loop as f32, so parity is structural, but only a first-touch probe
    // proves no f64-only first-call growth hides on the bridge path.
    const BIG_BLOCK: usize = 7813;
    let input32 = dyadic_stereo_input(BIG_BLOCK, CHANNELS);
    let input: Vec<f64> = input32.iter().map(|&sample| f64::from(sample)).collect();
    let (mut host, down_a) = build_twin_host_inner(false, true);
    assert_eq!(
        down_a, 1,
        "first converter mirrors the R7 node-1 failure site"
    );
    let mut oracle = TwinDiamondOracle::new(CHANNELS);
    let expected = oracle.feed_block_f64(&input);
    let capacity = host.output_frames_for_input(BIG_BLOCK);
    let mut out = vec![0.0f64; capacity * CHANNELS];
    let mut actual = 0;
    assert_no_allocs_or_deallocs("envelope prepared cold f64 first process", || {
        host.process_f64(&input, &mut out).unwrap();
        actual = host
            .last_output_frames()
            .expect("diamond tracks production");
    });
    assert!(actual <= capacity, "host overran its declared output bound");
    assert_eq!(
        &out[..actual * CHANNELS],
        &expected[..],
        "cold f64 first call must match the oracle bitwise"
    );
    println!(
        "envelope f64 first call: {BIG_BLOCK} frames cold, produced {actual}, \
         zero callback allocations, bitwise match",
    );
}

/// Fixed-tail identity source: copies input to output during process,
/// then emits exactly `tail_frames` constant frames on drain. Honest
/// envelopes throughout, so it can prime a graph drain deterministically.
struct FixedTailFixture {
    channels: usize,
    tail_frames: usize,
    drained: bool,
}

impl FixedTailFixture {
    fn new(channels: usize, tail_frames: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        assert!(tail_frames > 0, "fixture needs a positive tail");
        Self {
            channels,
            tail_frames,
            drained: false,
        }
    }
}

impl Plugin for FixedTailFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("FixedTailFixture", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(format!("unknown parameter: {id}"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn reset(&mut self) {
        self.drained = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let samples = context.num_frames * self.channels;
        output[..samples].copy_from_slice(&input[..samples]);
        Ok(context.num_frames)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.tail_frames as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        if self.drained { 0 } else { self.tail_frames }
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(self.tail_frames)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(1)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if self.drained {
            return Ok(PluginDrainResult::COMPLETE);
        }
        output[..self.tail_frames * self.channels].fill(0.5);
        self.drained = true;
        Ok(PluginDrainResult {
            frames: self.tail_frames,
            complete: true,
        })
    }
}

/// Loose-live publisher: declares twice its envelope (2n live over an n
/// envelope) while producing exactly n. Production-valid, but violates
/// the F2 live-domination invariant on purpose: the behavioral proof
/// that the host enforces it loudly instead of corrupting silently.
struct LooseLiveFixture {
    channels: usize,
}

impl LooseLiveFixture {
    fn new(channels: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        Self { channels }
    }
}

impl Plugin for LooseLiveFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("LooseLiveFixture", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err(format!("unknown parameter: {id}"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let samples = context.num_frames * self.channels;
        output[..samples].copy_from_slice(&input[..samples]);
        Ok(context.num_frames)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames * 2
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(0)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(1)
    }
}

#[test]
fn loose_live_tight_envelope_fails_loud_in_graph_drain() {
    const CHANNELS: usize = 2;
    const TAIL: usize = 8;
    // F2 behavioral enforcement: a production-valid but loose-live
    // publisher (live 2n over envelope n) must break a healthy graph
    // drain loudly, never silently. Fork, non-chain so the branched
    // scheduler runs, no merges: the fixed-tail source feeds the loose
    // branch and an honest identity sink. The envelope plan sizes node
    // 1's holdover at TAIL frames; the first drain round consumes TAIL
    // live frames, declares 2*TAIL, and must fail at the holdover check
    // before the fixture's process can advance.
    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(FixedTailFixture::new(CHANNELS, TAIL)),
        )
        .unwrap();
    let loose = host
        .add_node(
            "loose".to_string(),
            Box::new(LooseLiveFixture::new(CHANNELS)),
        )
        .unwrap();
    let sink = host
        .add_node(
            "sink".to_string(),
            Box::new(ExactGainFixture::new(CHANNELS, 1.0).with_envelopes()),
        )
        .unwrap();
    assert_eq!((source, loose, sink), (0, 1, 2));
    host.add_edge(GraphEdge::new(source, loose)).unwrap();
    host.add_edge(GraphEdge::new(source, sink)).unwrap();
    host.build().unwrap();
    // Non-vacuity: every node publishes, so the envelope plan (not the
    // live plan) sizes the holdovers; on live-sized holdovers this test
    // could not exercise the invariant.
    let envelope_bound = host
        .graph_drain_envelope_bound_for_test()
        .expect("all-envelope fork must prepare an envelope plan");
    assert_eq!(
        envelope_bound, TAIL,
        "envelope plan must size the fork at the tail wave"
    );
    let live_bound = host.drain_output_frames_max();
    assert_eq!(
        live_bound, TAIL,
        "fresh live bound must agree with the tail wave"
    );
    let mut out = vec![f32::NAN; live_bound * CHANNELS];
    let error = host.drain(&mut out).unwrap_err();
    assert!(
        error.contains("smaller than its declared process capacity"),
        "unexpected drain error: {error}"
    );
    assert!(
        error.contains("node 1"),
        "error must name the loose node: {error}"
    );
    println!(
        "loose-live fork: envelope plan {envelope_bound}, live {live_bound}, \
         loud holdover refusal naming node 1, no silent advance",
    );
}
