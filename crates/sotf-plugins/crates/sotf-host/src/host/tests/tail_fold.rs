//! Remaining-tail fold proofs: live tails, supports, and drain totals.
//!
//! The host tail fold answers total future emission with no further input
//! from current stream state. These tests pin its contract from both ends:
//!
//! - exactness: delay chains and asymmetric diamonds report the exact
//!   future frame count, and drain emits exactly that many frames with an
//!   independently derived per-frame content oracle (delayed history,
//!   aligned branch sums, compensation-flush order);
//! - honesty: unprovable tails report `Unknown` (never a guess), infinite
//!   tails propagate, loose fixture bounds propagate verbatim and dominate
//!   actuals pre- and mid-drain, and support dominates true emission
//!   everywhere (never via live values);
//! - realtime: warm and cold post-build queries allocate nothing.
//!
//! Counts use `u64` fold values against measured `usize` drain totals; no
//! padding, truncation, or nominal-frame substitution anywhere.

use super::super::daw_host::DawHost;
use super::super::graph_edge::GraphEdge;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use crate::test_utils::assert_no_allocs_or_deallocs;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const RATE: u32 = 48_000;

/// Exact-tail delay line: process swaps through the ring, drain pushes zeros
/// and emits delayed history, tail reports the exact live remainder.
struct DelayFixture {
    channels: usize,
    delay_frames: usize,
    ring: Vec<f32>,
    pos: usize,
    drain_remaining: Option<usize>,
}

impl DelayFixture {
    fn new(channels: usize, delay_frames: usize) -> Self {
        Self {
            channels,
            delay_frames,
            ring: vec![0.0; delay_frames * channels],
            pos: 0,
            drain_remaining: None,
        }
    }

    fn step(&mut self, input: f32) -> f32 {
        if self.ring.is_empty() {
            return input;
        }
        let delayed = std::mem::replace(&mut self.ring[self.pos], input);
        self.pos += 1;
        if self.pos == self.ring.len() {
            self.pos = 0;
        }
        delayed
    }

    fn finite(frames: usize) -> TailLength {
        match u64::try_from(frames) {
            Ok(frames) => TailLength::Finite(frames),
            Err(_) => TailLength::Unknown,
        }
    }
}

impl Plugin for DelayFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("DelayFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("DelayFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() || !input.len().is_multiple_of(self.channels) {
            return Err("DelayFixture requires matched whole-frame buffers".into());
        }
        for (sample_in, sample_out) in input.iter().zip(output.iter_mut()) {
            *sample_out = self.step(*sample_in);
        }
        Ok(input.len() / self.channels)
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() || !input.len().is_multiple_of(self.channels) {
            return Err("DelayFixture requires matched whole-frame buffers".into());
        }
        for (sample_in, sample_out) in input.iter().zip(output.iter_mut()) {
            // The ring is f32 by design; integer-valued test signals round-trip exactly.
            let narrowed = *sample_in as f32;
            *sample_out = self.step(narrowed) as f64;
        }
        Ok(input.len() / self.channels)
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.drain_remaining = None;
    }

    fn latency_samples(&self) -> usize {
        self.delay_frames
    }

    fn drain_output_frames_max(&self) -> usize {
        self.delay_frames
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(self.delay_frames)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !output.len().is_multiple_of(self.channels) {
            return Err("DelayFixture drain requires whole frames".into());
        }
        let remaining = *self.drain_remaining.get_or_insert(self.delay_frames);
        let frames = remaining.min(output.len() / self.channels);
        for sample_out in output[..frames * self.channels].iter_mut() {
            *sample_out = self.step(0.0);
        }
        self.drain_remaining = Some(remaining - frames);
        Ok(PluginDrainResult {
            frames,
            complete: remaining - frames == 0,
        })
    }

    fn tail_length(&self) -> TailLength {
        Self::finite(self.drain_remaining.unwrap_or(self.delay_frames))
    }

    fn tail_support(&self) -> Option<u64> {
        u64::try_from(self.delay_frames).ok()
    }
}

/// Memoryless passthrough with exact zero tails and honest envelopes.
struct PassFixture {
    channels: usize,
}

impl Plugin for PassFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("PassFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("PassFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("PassFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("PassFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn drain_output_frames_max(&self) -> usize {
        0
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(0)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn tail_support(&self) -> Option<u64> {
        Some(0)
    }
}

/// Passthrough that proves nothing: live tail `Unknown`, no support, no
/// drain envelope. The fold must report `Unknown`, never a guess.
struct UnknownFixture {
    channels: usize,
}

impl Plugin for UnknownFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("UnknownFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("UnknownFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("UnknownFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }
}

/// Autonomous tail: used only to pin `Infinite` propagation (never drained).
struct InfiniteFixture {
    channels: usize,
}

impl Plugin for InfiniteFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("InfiniteFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("InfiniteFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("InfiniteFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Infinite
    }
}

/// Loose-but-honest bound: declares a fixed large tail, emits a small exact
/// one in per-call chunks. The fold must propagate the bound verbatim and
/// stay above actuals pre- and mid-drain.
struct OverFixture {
    channels: usize,
    bound: u64,
    actual: usize,
    chunk: usize,
    emitted: usize,
}

impl Plugin for OverFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("OverFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("OverFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("OverFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn reset(&mut self) {
        self.emitted = 0;
    }

    fn drain_output_frames_max(&self) -> usize {
        self.chunk
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(self.chunk)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !output.len().is_multiple_of(self.channels) {
            return Err("OverFixture drain requires whole frames".into());
        }
        let frames = (self.actual - self.emitted).min(output.len() / self.channels);
        output[..frames * self.channels].fill(0.25);
        self.emitted += frames;
        Ok(PluginDrainResult {
            frames,
            complete: self.emitted == self.actual,
        })
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.bound)
    }

    fn tail_support(&self) -> Option<u64> {
        Some(self.bound)
    }
}

/// Incoherent publisher (Infinite live + finite support): the host folds
/// must not trust the finite support — whole-host support stays `None`.
struct InfiniteSupportedFixture {
    channels: usize,
}

impl Plugin for InfiniteSupportedFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("InfiniteSupportedFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("InfiniteSupportedFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("InfiniteSupportedFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Infinite
    }

    fn tail_support(&self) -> Option<u64> {
        Some(7)
    }
}

/// Deferred-arming quota fixture: `Unknown` tail + partial bound until the
/// scripted arming call, then the exact `Finite` remainder + full bound.
/// Models the A/B mask (unprovable while driven, exact once armed) with
/// scripted counts so the host-side single refresh is provable alone.
struct TransitionFixture {
    channels: usize,
    total: usize,
    arm_after: usize,
    partial: u64,
    drained: usize,
    draining: bool,
    drain_calls: Arc<AtomicUsize>,
    /// R7-F3 pin: counts `drain_call_bound` queries (grant + one refresh).
    bound_queries: Arc<AtomicUsize>,
}

impl Plugin for TransitionFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("TransitionFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("TransitionFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("TransitionFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn reset(&mut self) {
        self.drained = 0;
        self.draining = false;
    }

    fn drain_output_frames_max(&self) -> usize {
        1
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(1)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 {
            return Err("TransitionFixture drain requires a zero-frame context".into());
        }
        self.draining = true;
        self.drained = 0;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        self.bound_queries.fetch_add(1, Ordering::Relaxed);
        if !self.draining {
            return None;
        }
        if self.drained >= self.arm_after {
            NonZeroU64::new((self.total - self.drained) as u64 + 1)
        } else {
            NonZeroU64::new(self.partial)
        }
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !self.draining {
            return Err("TransitionFixture was not prepared for drain".into());
        }
        if output.len() != self.channels {
            return Err("TransitionFixture received the wrong drain capacity".into());
        }
        self.drain_calls.fetch_add(1, Ordering::Relaxed);
        output.fill(0.5);
        self.drained += 1;
        Ok(PluginDrainResult {
            frames: 1,
            complete: self.drained == self.total,
        })
    }

    fn tail_length(&self) -> TailLength {
        if self.drained >= self.arm_after {
            TailLength::Finite((self.total - self.drained) as u64)
        } else {
            TailLength::Unknown
        }
    }
}

/// Broken-promise quota fixture: a grant-time `Finite` tail with a bound
/// far below the true work. Exhaustion must trip at exactly the granted
/// budget — the refresh gate never fires for grant-time `Finite` tails.
struct LiarFixture {
    channels: usize,
    bound: u64,
    tail: u64,
    total: usize,
    drained: usize,
    draining: bool,
    drain_calls: Arc<AtomicUsize>,
    /// R7-F3 corollary: grant-time `Finite` tails are queried once, never
    /// refreshed.
    bound_queries: Arc<AtomicUsize>,
}

impl Plugin for LiarFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("LiarFixture", "0.1", "test")
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

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("LiarFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != output.len() {
            return Err("LiarFixture requires matched buffers".into());
        }
        output.copy_from_slice(input);
        Ok(input.len() / self.channels)
    }

    fn drain_output_frames_max(&self) -> usize {
        1
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        Some(1)
    }

    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Some(input_frames)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0 {
            return Err("LiarFixture drain requires a zero-frame context".into());
        }
        self.draining = true;
        self.drained = 0;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        self.bound_queries.fetch_add(1, Ordering::Relaxed);
        NonZeroU64::new(self.bound)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if !self.draining {
            return Err("LiarFixture was not prepared for drain".into());
        }
        if output.len() != self.channels {
            return Err("LiarFixture received the wrong drain capacity".into());
        }
        self.drain_calls.fetch_add(1, Ordering::Relaxed);
        output.fill(0.25);
        self.drained += 1;
        Ok(PluginDrainResult {
            frames: 1,
            complete: self.drained == self.total,
        })
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.tail)
    }
}

/// Unwrap a `Finite` tail or fail naming the expected count.
fn expect_finite(tail: TailLength, expected: u64) -> u64 {
    let TailLength::Finite(frames) = tail else {
        panic!("expected Finite({expected}) tail");
    };
    assert_eq!(frames, expected, "live tail must match exactly");
    frames
}

/// Feed `input` through `host` in the given block sizes; returns all output.
fn process_blocks(host: &mut DawHost, input: &[f32], blocks: &[usize]) -> Vec<f32> {
    let channels = host.output_channels();
    let mut produced = Vec::new();
    let mut cursor = 0;
    for &frames in blocks {
        let frames = frames.min(input.len() / channels - cursor);
        if frames == 0 {
            break;
        }
        let capacity = host.output_frames_for_input(frames);
        let mut output = vec![0.0; capacity * channels];
        let rendered = host
            .process(
                &input[cursor * channels..(cursor + frames) * channels],
                &mut output,
            )
            .unwrap();
        produced.extend_from_slice(&output[..rendered * channels]);
        cursor += rendered;
    }
    assert_eq!(cursor, input.len() / channels, "test must render all input");
    produced
}

/// Drain `host` to completion; budget derives from the queried tail hint
/// plus topology slack. Returns (total frames, samples, call count).
fn drain_host(host: &mut DawHost, tail_hint: u64, nodes: usize) -> (usize, Vec<f32>, usize) {
    let channels = host.output_channels();
    let bound = host.drain_output_frames_max().max(1);
    let bound_u64 = u64::try_from(bound).unwrap();
    let budget = (tail_hint / bound_u64) as usize + nodes + 8;
    let mut collected = Vec::new();
    let mut calls = 0;
    for _ in 0..budget {
        let mut output = vec![f32::NAN; bound * channels];
        let result = host.drain(&mut output).unwrap();
        assert!(result.frames <= bound, "drain emitted past its bound");
        for &sample in &output[..result.frames * channels] {
            assert!(!sample.is_nan(), "declared drain output left unwritten");
        }
        collected.extend_from_slice(&output[..result.frames * channels]);
        calls += 1;
        if result.complete {
            return (collected.len() / channels, collected, calls);
        }
    }
    panic!("drain did not complete within its derived budget of {budget} calls");
}

#[test]
fn chain_delay_tail_is_exact_with_content_oracle() {
    const CHANNELS: usize = 1;
    const DELAY_A: usize = 65;
    const DELAY_B: usize = 33;
    const TAIL: u64 = (DELAY_A + DELAY_B) as u64;
    const INPUT_FRAMES: usize = 200;

    let mut host = DawHost::new(CHANNELS, RATE);
    let delay_a = host
        .add_node(
            "delay a".to_string(),
            Box::new(DelayFixture::new(CHANNELS, DELAY_A)),
        )
        .unwrap();
    let delay_b = host
        .add_node(
            "delay b".to_string(),
            Box::new(DelayFixture::new(CHANNELS, DELAY_B)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(delay_a, delay_b)).unwrap();
    host.build().unwrap();

    // Fresh rings still emit their full length (zeros until primed).
    expect_finite(host.tail_length(), TAIL);
    assert_eq!(
        host.tail_support(),
        Some(TAIL),
        "chain support is exact pre-drain"
    );

    // Integer-valued ramp: exact in f32 at every stage.
    let input: Vec<f32> = (1..=INPUT_FRAMES).map(|n| n as f32).collect();
    let produced = process_blocks(&mut host, &input, &[64, 7, 129]);
    assert_eq!(produced.len(), INPUT_FRAMES);
    // Chain output is the ramp delayed by the summed rings.
    for (index, &sample) in produced.iter().enumerate() {
        let expected = if index < DELAY_A + DELAY_B {
            0.0
        } else {
            (index - DELAY_A - DELAY_B + 1) as f32
        };
        assert_eq!(sample, expected, "process output mismatch at frame {index}");
    }

    // Primed rings hold history but the future count is unchanged.
    expect_finite(host.tail_length(), TAIL);

    // Drain emits exactly the tail: A's history through B's ring (which
    // holds A's delayed output), then B's own tail. Chain drain feeds each
    // upstream tail wave through downstream `process`, so with inputs[i]
    // = i + 1: B's ring holds A_out[167..200] = inputs[102..135]; A's tail
    // is inputs[135..200]; B emits ring, then the first 32 tail frames,
    // then its re-primed ring (inputs[167..200]).
    let (total, samples, calls) = drain_host(&mut host, TAIL, 2);
    assert_eq!(total as u64, TAIL, "drain must emit exactly the tail");
    let mut expected = Vec::with_capacity(TAIL as usize);
    expected.extend((102..135).map(|i| (i + 1) as f32));
    expected.extend((135..167).map(|i| (i + 1) as f32));
    expected.extend((167..200).map(|i| (i + 1) as f32));
    assert_eq!(
        samples, expected,
        "drain content must match the delay oracle"
    );

    let TailLength::Finite(post) = host.tail_length() else {
        panic!("post-drain tail must stay Finite");
    };
    assert_eq!(post, 0, "drained chain holds nothing more");
    println!(
        "chain delay tail: predicted {TAIL}, drained {total} in {calls} calls, post-drain {post}"
    );
}

#[test]
fn diamond_delay_tail_folds_min_plus_compensation() {
    const CHANNELS: usize = 1;
    const DELAY_A: usize = 65;
    const DELAY_B: usize = 33;
    const COMP: usize = DELAY_A - DELAY_B;
    const TAIL: u64 = DELAY_A as u64;
    const INPUT_FRAMES: usize = 100;

    let mut host = DawHost::new(CHANNELS, RATE);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassFixture { channels: CHANNELS }),
        )
        .unwrap();
    let branch_a = host
        .add_node(
            "branch a".to_string(),
            Box::new(DelayFixture::new(CHANNELS, DELAY_A)),
        )
        .unwrap();
    let branch_b = host
        .add_node(
            "branch b".to_string(),
            Box::new(DelayFixture::new(CHANNELS, DELAY_B)),
        )
        .unwrap();
    let join = host
        .add_node(
            "join".to_string(),
            Box::new(PassFixture { channels: CHANNELS }),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, branch_a)).unwrap();
    host.add_edge(GraphEdge::new(source, branch_b)).unwrap();
    host.add_edge(GraphEdge::new(branch_a, join)).unwrap();
    host.add_edge(GraphEdge::new(branch_b, join)).unwrap();
    host.build().unwrap();

    // Branch futures 65/33 plus short-branch compensation 32 fold to the
    // join minimum 65: exact without process, since rings always emit full.
    expect_finite(host.tail_length(), TAIL);

    let input: Vec<f32> = (1..=INPUT_FRAMES).map(|n| n as f32).collect();
    let produced = process_blocks(&mut host, &input, &[64, 36]);
    assert_eq!(produced.len(), INPUT_FRAMES);
    // Compensation aligns the branches: both land 65 late, summed.
    for (index, &sample) in produced.iter().enumerate() {
        let expected = if index < DELAY_A {
            0.0
        } else {
            2.0 * (index - DELAY_A + 1) as f32
        };
        assert_eq!(sample, expected, "join output mismatch at frame {index}");
    }
    expect_finite(host.tail_length(), TAIL);

    // Drain: B's 33-history commits THROUGH its 32-frame compensation
    // ring, which first evicts its own 32-frame process history
    // (B_out[68..100] = inputs[35..67]); the zero-flush then evicts the
    // ring's retained tail remainder. Stream order is history-then-tail,
    // seamlessly contiguous: B→J[k] = inputs[35 + k] for all k, exactly
    // A→J[k], so the join sums to the aligned double ramp
    // 2·inputs[35..100] (measured 72..200 step 2, matching this
    // derivation frame for frame).
    let (total, samples, calls) = drain_host(&mut host, TAIL, 4);
    assert_eq!(total as u64, TAIL, "drain must emit exactly the tail");
    let expected: Vec<f32> = (0..DELAY_A).map(|k| 2.0 * (35 + k + 1) as f32).collect();
    assert_eq!(
        samples, expected,
        "drain content must match the join oracle"
    );

    let TailLength::Finite(post) = host.tail_length() else {
        panic!("post-drain tail must stay Finite");
    };
    assert_eq!(post, 0, "drained diamond holds nothing more");
    let support = host
        .tail_support()
        .expect("diamond support must be provable");
    assert!(
        support >= TAIL,
        "support {support} must dominate live tail {TAIL}"
    );
    println!(
        "diamond delay tail: predicted {TAIL}, drained {total} in {calls} calls, support {support}, post-drain {post}"
    );
    assert_eq!(
        COMP,
        DELAY_A - DELAY_B,
        "short-branch compensation is the latency skew"
    );
}

#[test]
fn diamond_delay_tail_f64_matches_f32_exactly() {
    const CHANNELS: usize = 1;
    const DELAY_A: usize = 65;
    const TAIL: u64 = DELAY_A as u64;
    const INPUT_FRAMES: usize = 100;

    let mut host = DawHost::new(CHANNELS, RATE);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassFixture { channels: CHANNELS }),
        )
        .unwrap();
    let branch_a = host
        .add_node(
            "branch a".to_string(),
            Box::new(DelayFixture::new(CHANNELS, DELAY_A)),
        )
        .unwrap();
    let branch_b = host
        .add_node(
            "branch b".to_string(),
            Box::new(DelayFixture::new(CHANNELS, 33)),
        )
        .unwrap();
    let join = host
        .add_node(
            "join".to_string(),
            Box::new(PassFixture { channels: CHANNELS }),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, branch_a)).unwrap();
    host.add_edge(GraphEdge::new(source, branch_b)).unwrap();
    host.add_edge(GraphEdge::new(branch_a, join)).unwrap();
    host.add_edge(GraphEdge::new(branch_b, join)).unwrap();
    host.build().unwrap();

    // Integer-valued f64 ramp: exact through the f32 rings and the f64->f32
    // retention transfer alike.
    let input: Vec<f64> = (1..=INPUT_FRAMES).map(|n| n as f64).collect();
    let mut produced = Vec::new();
    let mut cursor = 0;
    for &frames in &[64usize, 36] {
        let capacity = host.output_frames_for_input(frames);
        let mut output = vec![0.0; capacity * CHANNELS];
        let rendered = host
            .process_f64(
                &input[cursor * CHANNELS..(cursor + frames) * CHANNELS],
                &mut output,
            )
            .unwrap();
        produced.extend_from_slice(&output[..rendered * CHANNELS]);
        cursor += rendered;
    }
    assert_eq!(cursor, INPUT_FRAMES);
    for (index, &sample) in produced.iter().enumerate() {
        let expected = if index < DELAY_A {
            0.0
        } else {
            2.0 * (index - DELAY_A + 1) as f64
        };
        assert_eq!(
            sample, expected,
            "f64 join output mismatch at frame {index}"
        );
    }

    // Drain is f32-only by design; frames survive the precision step 1:1.
    // Same history-then-tail mechanics as the f32 leg: integer-valued
    // signals round-trip the f32 rings and the f64->f32 retention
    // transfer exactly, so the expected stream is identical.
    expect_finite(host.tail_length(), TAIL);
    let (total, samples, calls) = drain_host(&mut host, TAIL, 4);
    assert_eq!(
        total as u64, TAIL,
        "f64-stream drain must emit exactly the tail"
    );
    let expected: Vec<f32> = (0..DELAY_A).map(|k| 2.0 * (35 + k + 1) as f32).collect();
    assert_eq!(samples, expected, "f64-stream drain content must match");
    println!("diamond f64 tail: predicted {TAIL}, drained {total} in {calls} calls");
}

#[test]
fn unknown_tail_reports_unknown_and_support_none() {
    let mut host = DawHost::new(1, RATE);
    let source = host
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let unknown = host
        .add_node(
            "unknown".to_string(),
            Box::new(UnknownFixture { channels: 1 }),
        )
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    host.add_edge(GraphEdge::new(source, unknown)).unwrap();
    host.add_edge(GraphEdge::new(source, join)).unwrap();
    host.add_edge(GraphEdge::new(unknown, join)).unwrap();
    host.build().unwrap();

    let TailLength::Unknown = host.tail_length() else {
        panic!("unprovable branch tail must report Unknown, never a guess");
    };
    assert_eq!(
        host.tail_support(),
        None,
        "missing support bound must stay None"
    );
}

#[test]
fn bypassed_unknown_node_is_skipped_not_queried() {
    // Correction of the old dangling premise: every sink is an output
    // (`compute_io_nodes`), so disconnection cannot hide an unknown tail.
    // Bypass is the true skip — a bypassed unknown node on the output
    // path contributes nothing (its tail is never queried) and passes
    // its input through unchanged, exactly as drain skips it.
    let mut host = DawHost::new(1, RATE);
    let source = host
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let unknown = host
        .add_node(
            "unknown".to_string(),
            Box::new(UnknownFixture { channels: 1 }),
        )
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    host.add_edge(GraphEdge::new(source, unknown)).unwrap();
    host.add_edge(GraphEdge::new(source, join)).unwrap();
    host.add_edge(GraphEdge::new(unknown, join)).unwrap();
    host.nodes.get_mut(&unknown).unwrap().bypassed = true;
    host.build().unwrap();
    expect_finite(host.tail_length(), 0);
}

#[test]
fn disconnected_unknown_sink_reports_unknown() {
    // Companion pin for the corrected premise: a disconnected unknown
    // node IS a second output (sinks are outputs), so its unprovable
    // tail honestly reports Unknown for the whole host — and support
    // stays None for the same reason.
    let mut host = DawHost::new(1, RATE);
    let source = host
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    host.add_edge(GraphEdge::new(source, join)).unwrap();
    host.add_node(
        "dangling unknown".to_string(),
        Box::new(UnknownFixture { channels: 1 }),
    )
    .unwrap();
    host.build().unwrap();
    let TailLength::Unknown = host.tail_length() else {
        panic!("disconnected unknown sink must report Unknown");
    };
    assert_eq!(host.tail_support(), None);
}

#[test]
fn infinite_tail_propagates() {
    let mut host = DawHost::new(1, RATE);
    let pass = host
        .add_node("pass".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let infinite = host
        .add_node(
            "infinite".to_string(),
            Box::new(InfiniteFixture { channels: 1 }),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(pass, infinite)).unwrap();
    host.build().unwrap();

    let TailLength::Infinite = host.tail_length() else {
        panic!("autonomous tail must propagate Infinite");
    };
}

#[test]
fn infinite_node_with_finite_support_keeps_host_support_none() {
    // R6-F2 companion pin: an Infinite live tail with a finite support
    // bound is incoherent, and both support folds must refuse to trust
    // the finite value — whole-host support stays `None` in chain and
    // graph folds alike, while live still propagates `Infinite`.
    let mut chain = DawHost::new(1, RATE);
    let pass = chain
        .add_node("pass".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let incoherent = chain
        .add_node(
            "incoherent".to_string(),
            Box::new(InfiniteSupportedFixture { channels: 1 }),
        )
        .unwrap();
    chain.add_edge(GraphEdge::new(pass, incoherent)).unwrap();
    chain.build().unwrap();
    let TailLength::Infinite = chain.tail_length() else {
        panic!("chain live must propagate Infinite");
    };
    assert_eq!(
        chain.tail_support(),
        None,
        "chain support must not trust an Infinite node's finite bound"
    );

    let mut diamond = DawHost::new(1, RATE);
    let source = diamond
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let branch = diamond
        .add_node(
            "branch".to_string(),
            Box::new(InfiniteSupportedFixture { channels: 1 }),
        )
        .unwrap();
    let join = diamond
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    diamond.add_edge(GraphEdge::new(source, branch)).unwrap();
    diamond.add_edge(GraphEdge::new(source, join)).unwrap();
    diamond.add_edge(GraphEdge::new(branch, join)).unwrap();
    diamond.build().unwrap();
    let TailLength::Infinite = diamond.tail_length() else {
        panic!("graph live must propagate Infinite");
    };
    assert_eq!(
        diamond.tail_support(),
        None,
        "graph support must not trust an Infinite node's finite bound"
    );
}

#[test]
fn loose_bound_propagates_verbatim_and_dominates_actuals() {
    const BOUND: u64 = 1000;
    const ACTUAL: usize = 7;

    let mut host = DawHost::new(1, RATE);
    host.add_node(
        "over".to_string(),
        Box::new(OverFixture {
            channels: 1,
            bound: BOUND,
            actual: ACTUAL,
            chunk: 3,
            emitted: 0,
        }),
    )
    .unwrap();
    host.build().unwrap();

    // The fold propagates the fixture's honest bound verbatim.
    expect_finite(host.tail_length(), BOUND);
    assert_eq!(host.tail_support(), Some(BOUND));

    // Mid-drain the stale bound still dominates the true remainder.
    let per_call = host.drain_output_frames_max().max(1);
    let mut first = vec![f32::NAN; per_call];
    let step = host.drain(&mut first).unwrap();
    assert_eq!(step.frames, 3);
    assert!(!step.complete);
    let TailLength::Finite(mid) = host.tail_length() else {
        panic!("mid-drain tail must stay Finite");
    };
    assert!(
        mid >= (ACTUAL - step.frames) as u64,
        "mid-drain bound {mid} must dominate the true remainder"
    );

    let (total, samples, _) = drain_host(&mut host, BOUND, 1);
    assert_eq!(
        step.frames + total,
        ACTUAL,
        "manual plus continued drain must emit the true total"
    );
    assert!(samples.iter().all(|&sample| sample == 0.25));
    let TailLength::Finite(post) = host.tail_length() else {
        panic!("post-drain tail must stay Finite");
    };
    assert_eq!(post, 0);
    println!("loose bound: declared {BOUND}, actual {ACTUAL}, mid-drain {mid}, post-drain {post}");
}

#[test]
fn empty_host_tail_is_exact_zero() {
    // No nodes, no edges, no outputs: drain completes immediately with
    // zero frames (the `PathConfig::None` shape), so both queries answer
    // zero rather than Unknown.
    let host = DawHost::new(1, RATE);
    let TailLength::Finite(tail) = host.tail_length() else {
        panic!("empty host must report a finite tail");
    };
    assert_eq!(tail, 0);
    assert_eq!(host.tail_support(), Some(0));
}

#[test]
fn tail_queries_allocate_nothing_post_build() {
    let mut chain = DawHost::new(1, RATE);
    let delay = chain
        .add_node("delay".to_string(), Box::new(DelayFixture::new(1, 65)))
        .unwrap();
    let pass = chain
        .add_node("pass".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    chain.add_edge(GraphEdge::new(delay, pass)).unwrap();
    chain.build().unwrap();

    let mut diamond = DawHost::new(1, RATE);
    let source = diamond
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let branch = diamond
        .add_node("branch".to_string(), Box::new(DelayFixture::new(1, 33)))
        .unwrap();
    let join = diamond
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    diamond.add_edge(GraphEdge::new(source, branch)).unwrap();
    diamond.add_edge(GraphEdge::new(source, join)).unwrap();
    diamond.add_edge(GraphEdge::new(branch, join)).unwrap();
    diamond.build().unwrap();

    // Build presizes the graph scratch, so even the first post-build query
    // (cold) allocates nothing; the chain fold keeps one accumulator.
    assert_no_allocs_or_deallocs("tail fold queries", || {
        for _ in 0..8 {
            let TailLength::Finite(chain_tail) = chain.tail_length() else {
                panic!("chain tail must be Finite");
            };
            assert_eq!(chain_tail, 65);
            assert_eq!(chain.tail_support(), Some(65));
            let TailLength::Finite(diamond_tail) = diamond.tail_length() else {
                panic!("diamond tail must be Finite");
            };
            assert_eq!(diamond_tail, 33);
            assert!(diamond.tail_support().is_some_and(|support| support >= 33));
        }
    });
}

/// Progress-gated host drain loop: rounds that emit nothing and never
/// complete stall the stream, so more than `progress_every` consecutive
/// still rounds fail loudly (a hang guard, never truncation). Returns
/// (total frames, samples, host rounds).
fn drain_host_progress_gated(
    host: &mut DawHost,
    budget: usize,
    progress_every: usize,
) -> (usize, Vec<f32>, usize) {
    let channels = host.output_channels();
    let bound = host.drain_output_frames_max().max(1);
    let mut collected = Vec::new();
    let mut rounds = 0;
    let mut last_progress = 0;
    for _ in 0..budget {
        let mut output = vec![f32::NAN; bound * channels];
        let result = host.drain(&mut output).unwrap();
        assert!(result.frames <= bound, "drain emitted past its bound");
        for &sample in &output[..result.frames * channels] {
            assert!(!sample.is_nan(), "declared drain output left unwritten");
        }
        collected.extend_from_slice(&output[..result.frames * channels]);
        rounds += 1;
        if result.frames > 0 || result.complete {
            last_progress = rounds;
        }
        assert!(
            rounds - last_progress <= progress_every,
            "drain stalled: {progress_every} rounds without progress"
        );
        if result.complete {
            return (collected.len() / channels, collected, rounds);
        }
    }
    panic!("drain did not complete within its {budget}-round budget");
}

#[test]
fn chain_quota_refreshes_once_for_deferred_arming_tail() {
    // Single-refresh proof on the chain path: 5000 content frames at one
    // frame per call exceed BOTH the 4096 unknown fallback AND the
    // scripted partial bound of 4, so completion proves the host granted
    // the re-queried budget exactly when the tail turned Finite — and the
    // exact pump count proves no wasted or repeated work.
    const TOTAL: usize = 5000;
    const PARTIAL: u64 = 4;
    // Twin probe: identical construction pins the grant-time answers the
    // host will see (Unknown tail, partial bound).
    let probe_queries = Arc::new(AtomicUsize::new(0));
    let mut probe = TransitionFixture {
        channels: 1,
        total: TOTAL,
        arm_after: 2,
        partial: PARTIAL,
        drained: 0,
        draining: false,
        drain_calls: Arc::new(AtomicUsize::new(0)),
        bound_queries: Arc::clone(&probe_queries),
    };
    probe.begin_drain(&ProcessContext::new(RATE, 0)).unwrap();
    assert_eq!(probe.tail_length(), TailLength::Unknown);
    assert_eq!(probe.drain_call_bound(), NonZeroU64::new(PARTIAL));
    assert_eq!(
        probe_queries.load(Ordering::Relaxed),
        1,
        "direct probe query must count exactly once (counter non-vacuity)"
    );

    let calls = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(AtomicUsize::new(0));
    let mut host = DawHost::new(1, RATE);
    host.add_node(
        "transition".to_string(),
        Box::new(TransitionFixture {
            channels: 1,
            total: TOTAL,
            arm_after: 2,
            partial: PARTIAL,
            drained: 0,
            draining: false,
            drain_calls: Arc::clone(&calls),
            bound_queries: Arc::clone(&queries),
        }),
    )
    .unwrap();
    host.build().unwrap();
    let TailLength::Unknown = host.tail_length() else {
        panic!("chain tail must be Unknown before the arming transition");
    };
    let (total, samples, _) = drain_host(&mut host, TOTAL as u64, 1);
    assert_eq!(total, TOTAL, "chain drain must emit every content frame");
    assert!(samples.iter().all(|&sample| sample == 0.5));
    assert_eq!(
        calls.load(Ordering::Relaxed),
        TOTAL,
        "plugin must be pumped exactly once per frame"
    );
    assert_eq!(
        queries.load(Ordering::Relaxed),
        2,
        "chain must query the bound exactly twice (grant + one refresh)"
    );
    assert!(
        total > 4096 && total as u64 > PARTIAL,
        "total {total} must exceed both the 4096 fallback and the partial bound {PARTIAL}"
    );
    expect_finite(host.tail_length(), 0);
    println!("chain refresh: {TOTAL} frames past partial bound {PARTIAL}");
}

#[test]
fn graph_quota_refreshes_once_for_deferred_arming_tail() {
    // Same proof on the graph path: S -> T -> J plus an S -> J skip edge
    // (non-linear, so the graph scheduler owns the quota). The skip edge
    // carries nothing (S completes empty), so J sums T's 5000 frames
    // unchanged; completion past both budgets proves the graph-site
    // refresh, and the exact pump count proves single coverage.
    const TOTAL: usize = 5000;
    const PARTIAL: u64 = 4;
    let calls = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(AtomicUsize::new(0));
    let mut host = DawHost::new(1, RATE);
    let source = host
        .add_node("source".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    let transition = host
        .add_node(
            "transition".to_string(),
            Box::new(TransitionFixture {
                channels: 1,
                total: TOTAL,
                arm_after: 2,
                partial: PARTIAL,
                drained: 0,
                draining: false,
                drain_calls: Arc::clone(&calls),
                bound_queries: Arc::clone(&queries),
            }),
        )
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(PassFixture { channels: 1 }))
        .unwrap();
    host.add_edge(GraphEdge::new(source, transition)).unwrap();
    host.add_edge(GraphEdge::new(transition, join)).unwrap();
    host.add_edge(GraphEdge::new(source, join)).unwrap();
    host.build().unwrap();
    let TailLength::Unknown = host.tail_length() else {
        panic!("graph tail must be Unknown before the arming transition");
    };
    let (total, samples, rounds) = drain_host_progress_gated(&mut host, 2 * TOTAL + 64, 16);
    assert_eq!(total, TOTAL, "graph drain must emit every content frame");
    assert!(samples.iter().all(|&sample| sample == 0.5));
    assert_eq!(
        calls.load(Ordering::Relaxed),
        TOTAL,
        "plugin must be pumped exactly once per frame"
    );
    assert_eq!(
        queries.load(Ordering::Relaxed),
        2,
        "graph must query the bound exactly twice (grant + one refresh)"
    );
    assert!(
        total > 4096 && total as u64 > PARTIAL,
        "total {total} must exceed both the 4096 fallback and the partial bound {PARTIAL}"
    );
    expect_finite(host.tail_length(), 0);
    println!("graph refresh: {TOTAL} frames in {rounds} rounds past partial bound {PARTIAL}");
}

#[test]
fn grant_time_finite_bound_exhaustion_trips_without_refresh() {
    // Refresh-gate pin: a grant-time `Finite` tail with an insufficient
    // bound is a broken promise, not deferred arming — exhaustion must
    // trip at exactly the granted budget with the historical message,
    // proving the refresh changed nothing for non-transitioning plugins.
    let calls = Arc::new(AtomicUsize::new(0));
    let queries = Arc::new(AtomicUsize::new(0));
    let mut host = DawHost::new(1, RATE);
    host.add_node(
        "liar".to_string(),
        Box::new(LiarFixture {
            channels: 1,
            bound: 3,
            tail: 100,
            total: 100,
            drained: 0,
            draining: false,
            drain_calls: Arc::clone(&calls),
            bound_queries: Arc::clone(&queries),
        }),
    )
    .unwrap();
    host.build().unwrap();
    expect_finite(host.tail_length(), 100);
    let bound = host.drain_output_frames_max().max(1);
    for _ in 0..3 {
        let mut output = vec![f32::NAN; bound];
        let result = host.drain(&mut output).unwrap();
        assert_eq!(result.frames, 1);
        assert!(!result.complete);
    }
    let mut output = vec![f32::NAN; bound];
    let error = host.drain(&mut output).unwrap_err();
    assert_eq!(
        error,
        "plugin 'liar' drain did not converge within its call bound"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        3,
        "trip must fire before a fourth pump"
    );
    assert_eq!(
        queries.load(Ordering::Relaxed),
        1,
        "grant-time Finite tails are queried once, never refreshed"
    );
}
