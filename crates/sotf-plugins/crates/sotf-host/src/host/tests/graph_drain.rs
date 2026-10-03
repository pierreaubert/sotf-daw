// Branched graph end-of-stream scheduler tests.
//
// Every waveform assertion compares host output against an independent
// signal-flow reference computed with plain sample math below. The
// references never call the scheduler, the merge, or the fixtures.

use super::super::daw_host::DawHost;
use super::super::graph_edge::GraphEdge;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginDrainResult, PluginInfo, PluginResult, ProcessContext, TailLength,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct FixtureCalls {
    process: AtomicUsize,
    drain: AtomicUsize,
    begin: AtomicUsize,
    bound_queries: AtomicUsize,
}

fn no_parameters_info(name: &'static str) -> PluginInfo {
    PluginInfo::new(name, "0.1", "test")
}

fn reject_parameters(_: ParameterId, _: ParameterValue) -> Result<(), String> {
    Err("graph drain fixture has no parameters".into())
}

/// Exact pure-delay line with a chunked tail drain.
struct DelayTail {
    delay: usize,
    channels: usize,
    chunk: usize,
    leading_empty: usize,
    guard_begin: bool,
    history: Vec<f32>,
    pos: usize,
    emit: usize,
    empties_sent: usize,
    has_input: bool,
    calls: Arc<FixtureCalls>,
}

impl DelayTail {
    fn new(delay: usize, channels: usize, chunk: usize) -> Self {
        Self {
            delay,
            channels,
            chunk,
            leading_empty: 0,
            guard_begin: false,
            history: vec![0.0; delay * channels],
            pos: 0,
            emit: 0,
            empties_sent: 0,
            has_input: false,
            calls: Arc::new(FixtureCalls::default()),
        }
    }

    /// Legal `{0, incomplete}` drain calls before data starts, for
    /// scheduling tests. Zero by default; existing graphs behave
    /// exactly as before.
    fn with_leading_empty(mut self, calls: usize) -> Self {
        self.leading_empty = calls;
        self
    }

    /// Panic if the quota bound is queried before `begin_drain`,
    /// enforcing the plugin trait lifecycle like `drain_work`.
    fn with_begin_guard(mut self) -> Self {
        self.guard_begin = true;
        self
    }

    fn retained(&self) -> usize {
        if self.has_input { self.delay } else { 0 }
    }
}

impl Plugin for DelayTail {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Delay tail fixture")
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

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.calls.process.fetch_add(1, Ordering::Relaxed);
        if context.num_frames > 0 {
            self.has_input = true;
        }
        if self.history.is_empty() {
            let len = input.len().min(output.len());
            output[..len].copy_from_slice(&input[..len]);
            return Ok(context.num_frames);
        }
        for (input, output) in input
            .chunks_exact(self.channels)
            .zip(output.chunks_exact_mut(self.channels))
        {
            for (&sample, slot) in input.iter().zip(output.iter_mut()) {
                *slot = self.history[self.pos];
                self.history[self.pos] = sample;
                self.pos += 1;
                if self.pos == self.history.len() {
                    self.pos = 0;
                }
            }
        }
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn latency_samples(&self) -> usize {
        self.delay
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.delay as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.delay.min(self.chunk)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.calls.bound_queries.fetch_add(1, Ordering::Relaxed);
        if self.guard_begin {
            assert!(
                self.calls.begin.load(Ordering::Relaxed) > 0,
                "graph drain queried quota before begin"
            );
        }
        let max = self.drain_output_frames_max().max(1) as u64;
        let remaining = self.retained().saturating_sub(self.emit) as u64;
        let empties = self.leading_empty.saturating_sub(self.empties_sent) as u64;
        std::num::NonZeroU64::new(empties.saturating_add(remaining.div_ceil(max)).max(1))
    }

    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        self.calls.begin.fetch_add(1, Ordering::Relaxed);
        self.history.rotate_left(self.pos);
        self.pos = 0;
        self.emit = 0;
        self.empties_sent = 0;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        if self.empties_sent < self.leading_empty {
            self.empties_sent += 1;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }
        let retained = self.retained();
        let frames = retained
            .saturating_sub(self.emit)
            .min(self.drain_output_frames_max())
            .min(output.len() / self.channels);
        let base = self.emit * self.channels;
        output[..frames * self.channels]
            .copy_from_slice(&self.history[base..base + frames * self.channels]);
        self.emit += frames;
        Ok(PluginDrainResult {
            frames,
            complete: self.emit == retained,
        })
    }

    fn reset(&mut self) {
        self.history.iter_mut().for_each(|slot| *slot = 0.0);
        self.pos = 0;
        self.emit = 0;
        self.empties_sent = 0;
        self.has_input = false;
    }
}

/// Exact FIR filter with chunked support flushing. The support length
/// plays the linear-phase-filter role: retained samples must drain
/// after the last input sample, in order, without loss or padding.
struct FirTail {
    kernel: Vec<f32>,
    channels: usize,
    chunk: usize,
    history: Vec<f32>,
    pos: usize,
    emit: usize,
    has_input: bool,
    calls: Arc<FixtureCalls>,
}

impl FirTail {
    fn new(kernel: Vec<f32>, channels: usize, chunk: usize) -> Self {
        assert!(!kernel.is_empty() && channels <= 8);
        let support = kernel.len().saturating_sub(1) * channels;
        Self {
            kernel,
            channels,
            chunk,
            history: vec![0.0; support],
            pos: 0,
            emit: 0,
            has_input: false,
            calls: Arc::new(FixtureCalls::default()),
        }
    }

    fn support(&self) -> usize {
        self.kernel.len().saturating_sub(1)
    }

    fn retained(&self) -> usize {
        if self.has_input { self.support() } else { 0 }
    }

    /// Shift one frame through the line and write the convolution.
    /// History holds past input frames in a ring; `pos` is the oldest
    /// slot, so past frame `x[n-1-j]` sits `(j+1)` frames behind it.
    fn shift_frame(
        kernel: &[f32],
        history: &mut [f32],
        pos: &mut usize,
        channels: usize,
        input: &[f32],
        output: &mut [f32],
    ) {
        for channel in 0..channels {
            let mut acc = kernel[0] * input[channel];
            for (tap, coeff) in kernel.iter().enumerate().skip(1) {
                let back = tap * channels;
                let slot = (*pos + history.len() - back) % history.len().max(1);
                acc += coeff * history[slot + channel];
            }
            output[channel] = acc;
        }
        if !history.is_empty() {
            for channel in 0..channels {
                history[*pos + channel] = input[channel];
            }
            *pos = (*pos + channels) % history.len();
        }
    }
}

impl Plugin for FirTail {
    fn info(&self) -> PluginInfo {
        no_parameters_info("FIR tail fixture")
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

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.calls.process.fetch_add(1, Ordering::Relaxed);
        if context.num_frames > 0 {
            self.has_input = true;
        }
        for (input, output) in input
            .chunks_exact(self.channels)
            .zip(output.chunks_exact_mut(self.channels))
        {
            Self::shift_frame(
                &self.kernel,
                &mut self.history,
                &mut self.pos,
                self.channels,
                input,
                output,
            );
        }
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.support() as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.support().min(self.chunk)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.calls.bound_queries.fetch_add(1, Ordering::Relaxed);
        let max = self.drain_output_frames_max().max(1) as u64;
        let remaining = self.retained().saturating_sub(self.emit) as u64;
        std::num::NonZeroU64::new(remaining.div_ceil(max).max(1))
    }

    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        self.calls.begin.fetch_add(1, Ordering::Relaxed);
        self.emit = 0;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        let retained = self.retained();
        let frames = retained
            .saturating_sub(self.emit)
            .min(self.drain_output_frames_max())
            .min(output.len() / self.channels);
        const ZEROS: [f32; 8] = [0.0; 8];
        for slot in output.chunks_exact_mut(self.channels).take(frames) {
            Self::shift_frame(
                &self.kernel,
                &mut self.history,
                &mut self.pos,
                self.channels,
                &ZEROS[..self.channels],
                slot,
            );
        }
        self.emit += frames;
        Ok(PluginDrainResult {
            frames,
            complete: self.emit == retained,
        })
    }

    fn reset(&mut self) {
        self.history.iter_mut().for_each(|slot| *slot = 0.0);
        self.pos = 0;
        self.emit = 0;
        self.has_input = false;
    }
}

/// Linear keyed join: program passes through and the summed key bus is
/// added with a fixed gain. Linearity keeps the reference exact while
/// the sidechain packing exercises key routing and key EOF policy.
struct KeyedMix {
    program_channels: usize,
    key_channels: usize,
    key_gain: f32,
    calls: Arc<FixtureCalls>,
}

impl KeyedMix {
    fn new(program_channels: usize, key_channels: usize, key_gain: f32) -> Self {
        Self {
            program_channels,
            key_channels,
            key_gain,
            calls: Arc::new(FixtureCalls::default()),
        }
    }
}

impl Plugin for KeyedMix {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Keyed mix fixture")
    }

    fn input_channels(&self) -> usize {
        self.program_channels + self.key_channels
    }

    fn output_channels(&self) -> usize {
        self.program_channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.calls.process.fetch_add(1, Ordering::Relaxed);
        let width = self.program_channels + self.key_channels;
        for (input, output) in input
            .chunks_exact(width)
            .zip(output.chunks_exact_mut(self.program_channels))
        {
            let key: f32 = input[self.program_channels..].iter().sum();
            for (channel, slot) in output.iter_mut().enumerate() {
                *slot = input[channel] + self.key_gain * key;
            }
        }
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.calls.bound_queries.fetch_add(1, Ordering::Relaxed);
        None
    }

    fn drain(
        &mut self,
        _output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        Ok(PluginDrainResult::COMPLETE)
    }
}

/// Stateless lane selector used as a graph input node.
struct Select {
    channels: usize,
    lane: usize,
}

impl Select {
    fn new(channels: usize, lane: usize) -> Self {
        Self { channels, lane }
    }
}

impl Plugin for Select {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Select fixture")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        1
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for (input, slot) in input.chunks_exact(self.channels).zip(output.iter_mut()) {
            *slot = input[self.lane];
        }
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }
}

/// Stateless gain used for audio-join and fan-out branches.
struct Gain {
    channels: usize,
    gain: f32,
}

impl Gain {
    fn new(channels: usize, gain: f32) -> Self {
        Self { channels, gain }
    }
}

impl Plugin for Gain {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Gain fixture")
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

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for (input, output) in input.iter().zip(output.iter_mut()) {
            *output = input * self.gain;
        }
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }
}

/// Drain that never completes, for quota-exhaustion coverage.
struct InfiniteDrain {
    channels: usize,
    calls: Arc<FixtureCalls>,
}

impl InfiniteDrain {
    fn new(channels: usize) -> Self {
        Self {
            channels,
            calls: Arc::new(FixtureCalls::default()),
        }
    }
}

impl Plugin for InfiniteDrain {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Infinite drain fixture")
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

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        output[..input.len()].copy_from_slice(input);
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn drain_output_frames_max(&self) -> usize {
        1
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        output[..self.channels].fill(0.0);
        Ok(PluginDrainResult {
            frames: 1,
            complete: false,
        })
    }
}

/// Passthrough with a declared latency but no drain of its own.
/// Builds output-delay skew cheaply: process stays short while the
/// host reserves and flushes the full compensation extent.
struct LatencyStub {
    latency: usize,
    channels: usize,
}

impl LatencyStub {
    fn new(latency: usize, channels: usize) -> Self {
        Self { latency, channels }
    }
}

impl Plugin for LatencyStub {
    fn info(&self) -> PluginInfo {
        no_parameters_info("Latency stub fixture")
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

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        reject_parameters(id, value)
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let len = input.len().min(output.len());
        output[..len].copy_from_slice(&input[..len]);
        Ok(context.num_frames)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }
}

// ---------------------------------------------------------------------------
// Independent references: plain sample math, no scheduler involvement.
// ---------------------------------------------------------------------------

/// Pure delay of an infinite zero-primed stream, in full precision.
fn delay_reference(input: &[f64], delay: usize, total: usize) -> Vec<f64> {
    (0..total)
        .map(|t| input.get(t.wrapping_sub(delay)).copied().unwrap_or(0.0))
        .collect()
}

/// FIR convolution of an infinite zero-primed stream, in full precision.
fn fir_reference(input: &[f64], kernel: &[f64], total: usize) -> Vec<f64> {
    (0..total)
        .map(|t| {
            kernel
                .iter()
                .enumerate()
                .map(|(tap, coeff)| coeff * input.get(t.wrapping_sub(tap)).copied().unwrap_or(0.0))
                .sum()
        })
        .collect()
}

/// Keyed join of two latency-aligned branches. Branches are delayed to
/// the shared total latency by the host compensation this models.
fn keyed_reference(
    program: &[f64],
    key: &[f64],
    latency: usize,
    key_gain: f64,
    total: usize,
) -> Vec<f64> {
    (0..total)
        .map(|t| {
            program.get(t.wrapping_sub(latency)).copied().unwrap_or(0.0)
                + key_gain * key.get(t.wrapping_sub(latency)).copied().unwrap_or(0.0)
        })
        .collect()
}

fn assert_close(actual: &[f32], expected: &[f64], tolerance: f64, what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: length mismatch");
    for (index, (&actual, &expected)) in actual.iter().zip(expected.iter()).enumerate() {
        let error = (f64::from(actual) - expected).abs();
        assert!(
            error <= tolerance,
            "{what}: sample {index} error {error} exceeds {tolerance} ({actual} vs {expected})"
        );
    }
}

/// Deterministic pseudo-musical mono signal: mixed sines plus a step.
fn program_signal(frames: usize) -> Vec<f64> {
    (0..frames)
        .map(|t| {
            let t = t as f64;
            0.6 * (t * 0.11).sin()
                + 0.3 * (t * 0.031 + 1.0).sin()
                + if t > 40.0 { 0.1 } else { 0.0 }
        })
        .collect()
}

/// Independent key signal, deliberately unlike the program.
fn key_signal(frames: usize) -> Vec<f64> {
    (0..frames)
        .map(|t| {
            let t = t as f64;
            0.4 * (t * 0.23 + 2.0).sin() * (t * 0.007).sin().abs()
        })
        .collect()
}

fn interleave(program: &[f64], key: &[f64]) -> Vec<f32> {
    program
        .iter()
        .zip(key.iter())
        .flat_map(|(&program, &key)| [program as f32, key as f32])
        .collect()
}

/// Render `input` through `host` in the given block sizes, then drain to
/// completion, returning process output, drain output, and per-call
/// drain frame counts.
fn render_and_drain(
    host: &mut DawHost,
    input: &[f32],
    channels: usize,
    blocks: &[usize],
) -> (Vec<f32>, Vec<f32>, Vec<usize>) {
    let mut process_out = Vec::new();
    let mut consumed = 0;
    for &block in blocks {
        let frames = block.min(input.len() / channels - consumed);
        if frames == 0 {
            break;
        }
        let mut output = vec![0.0; frames * host.output_channels()];
        let rendered = host
            .process(
                &input[consumed * channels..(consumed + frames) * channels],
                &mut output,
            )
            .unwrap();
        process_out.extend_from_slice(&output[..rendered * host.output_channels()]);
        consumed += rendered;
    }
    assert_eq!(
        consumed,
        input.len() / channels,
        "test must render all input"
    );

    let mut drain_out = Vec::new();
    let mut calls = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; host.drain_output_frames_max() * host.output_channels()];
        let result = host.drain(&mut output).unwrap();
        assert!(result.frames <= host.drain_output_frames_max());
        drain_out.extend_from_slice(&output[..result.frames * host.output_channels()]);
        calls.push(result.frames);
        if result.complete {
            return (process_out, drain_out, calls);
        }
    }
    panic!("graph drain did not complete within 10000 calls");
}

/// Independently keyed program/key graph: lane-selected sources feed a
/// program delay and a key delay into a linear keyed join.
fn keyed_graph(
    program_delay: usize,
    key_delay: usize,
    chunk: usize,
) -> (
    DawHost,
    Arc<FixtureCalls>,
    Arc<FixtureCalls>,
    Arc<FixtureCalls>,
) {
    keyed_graph_with_empties(program_delay, key_delay, chunk, 0, 0)
}

/// Keyed graph with legal leading `{0, incomplete}` drain calls on the
/// program and key tails, for scheduling tests.
fn keyed_graph_with_empties(
    program_delay: usize,
    key_delay: usize,
    chunk: usize,
    program_empty: usize,
    key_empty: usize,
) -> (
    DawHost,
    Arc<FixtureCalls>,
    Arc<FixtureCalls>,
    Arc<FixtureCalls>,
) {
    let mut host = DawHost::new(2, 48_000);
    let program_select = host
        .add_node("program select".to_string(), Box::new(Select::new(2, 0)))
        .unwrap();
    let key_select = host
        .add_node("key select".to_string(), Box::new(Select::new(2, 1)))
        .unwrap();
    let program_tail = DelayTail::new(program_delay, 1, chunk).with_leading_empty(program_empty);
    let program_calls = Arc::clone(&program_tail.calls);
    let program = host
        .add_node("program delay".to_string(), Box::new(program_tail))
        .unwrap();
    let key_tail = DelayTail::new(key_delay, 1, chunk).with_leading_empty(key_empty);
    let key_calls = Arc::clone(&key_tail.calls);
    let key = host
        .add_node("key delay".to_string(), Box::new(key_tail))
        .unwrap();
    let join_mix = KeyedMix::new(1, 1, 0.5);
    let join_calls = Arc::clone(&join_mix.calls);
    let join = host
        .add_node("keyed join".to_string(), Box::new(join_mix))
        .unwrap();
    host.add_edge(GraphEdge::new(program_select, program))
        .unwrap();
    host.add_edge(GraphEdge::new(key_select, key)).unwrap();
    host.add_edge(GraphEdge::new(program, join)).unwrap();
    host.add_edge(GraphEdge::sidechain(key, join)).unwrap();
    host.build().unwrap();
    (host, program_calls, key_calls, join_calls)
}

#[test]
fn keyed_lookahead_graph_delivers_exact_tail() {
    // Program lookahead (16) exceeds key lookahead (5): the key branch
    // ends first and the join pads key silence for the remaining
    // program frames. Chunked drains force several scheduler rounds.
    let (mut host, program_calls, key_calls, join_calls) = keyed_graph(16, 5, 8);
    assert_eq!(host.output_channels(), 1);
    let frames = 96;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, calls) = render_and_drain(&mut host, &input, 2, &[32, 64]);

    assert_eq!(
        process_out.len(),
        frames,
        "process emits exactly the input frames"
    );
    assert_eq!(drain_out.len(), 16, "drain emits exactly the total latency");
    assert!(
        calls.iter().sum::<usize>() == 16 && calls.len() > 2,
        "tail spans rounds: {calls:?}"
    );
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected = keyed_reference(&program, &key, 16, 0.5, frames + 16);
    assert_close(&total, &expected, 1e-6, "keyed lookahead total");

    // Each tail plugin prepares once and drains its exact bound. The
    // quota bound is queried exactly once per drain quota snapshot,
    // after begin; build-time planning never queries it (trait
    // orders queries after begin_drain, itself after capacity
    // validation), so any reintroduced pre-begin query fails here.
    assert_eq!(program_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(key_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(program_calls.drain.load(Ordering::Relaxed), 2);
    assert_eq!(key_calls.drain.load(Ordering::Relaxed), 1);
    assert_eq!(join_calls.drain.load(Ordering::Relaxed), 1);
    assert_eq!(program_calls.bound_queries.load(Ordering::Relaxed), 1);
    assert_eq!(key_calls.bound_queries.load(Ordering::Relaxed), 1);
    assert_eq!(join_calls.bound_queries.load(Ordering::Relaxed), 1);
    assert!(join_calls.process.load(Ordering::Relaxed) > 0);
}

#[test]
fn key_branch_ending_later_aligns_by_timestamp() {
    // Key lookahead (20) exceeds program lookahead (4): the program
    // branch ends first and the join pads program silence while the key
    // tail still arrives. Mirror of the previous test's EOF order.
    let (mut host, _, _, _) = keyed_graph(4, 20, 8);
    let frames = 96;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[96]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 20);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected = keyed_reference(&program, &key, 20, 0.5, frames + 20);
    assert_close(&total, &expected, 1e-6, "key-later total");
}

#[test]
fn lookahead_and_fir_support_drain_in_series() {
    // DeEsser-shaped program chain: a pure lookahead delay followed by
    // a symmetric FIR whose support must flush after the delay tail.
    // Key lookahead is shorter, so key EOF pads the final FIR frames.
    let mut host = DawHost::new(2, 48_000);
    let program_select = host
        .add_node("program select".to_string(), Box::new(Select::new(2, 0)))
        .unwrap();
    let key_select = host
        .add_node("key select".to_string(), Box::new(Select::new(2, 1)))
        .unwrap();
    let lookahead = host
        .add_node("lookahead".to_string(), Box::new(DelayTail::new(8, 1, 4)))
        .unwrap();
    let kernel = vec![0.25f32, 0.5, 0.25];
    let fir = host
        .add_node(
            "linear phase".to_string(),
            Box::new(FirTail::new(kernel, 1, 2)),
        )
        .unwrap();
    let key = host
        .add_node("key delay".to_string(), Box::new(DelayTail::new(2, 1, 4)))
        .unwrap();
    let join = host
        .add_node("keyed join".to_string(), Box::new(KeyedMix::new(1, 1, 0.5)))
        .unwrap();
    host.add_edge(GraphEdge::new(program_select, lookahead))
        .unwrap();
    host.add_edge(GraphEdge::new(lookahead, fir)).unwrap();
    host.add_edge(GraphEdge::new(fir, join)).unwrap();
    host.add_edge(GraphEdge::new(key_select, key)).unwrap();
    host.add_edge(GraphEdge::sidechain(key, join)).unwrap();
    host.build().unwrap();

    let frames = 80;
    let program = program_signal(frames);
    let key_signal = key_signal(frames);
    let input = interleave(&program, &key_signal);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[80]);

    // Program stream runs 8 delay frames plus 2 FIR support frames past
    // the input; the key stream ends 2 frames earlier against silence.
    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 10);
    let delayed = delay_reference(&program, 8, frames + 10);
    let convolved = fir_reference(&delayed, &[0.25, 0.5, 0.25], frames + 10);
    let key_aligned = delay_reference(&key_signal, 8, frames + 10);
    let expected: Vec<f64> = convolved
        .iter()
        .zip(key_aligned.iter())
        .map(|(&program, &key)| program + 0.5 * key)
        .collect();
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    assert_close(&total, &expected, 1e-6, "lookahead plus FIR total");
}

#[test]
fn final_frame_impulse_lands_at_exact_tail_index() {
    // A cold impulse on the last program frame must surface at the
    // exact tail position with its exact value: pure delays do no
    // arithmetic, so this is bitwise.
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let frames = 64;
    let mut program = vec![0.0; frames];
    program[frames - 1] = 0.5;
    let key = vec![0.0; frames];
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[64]);
    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 16);
    for (index, &sample) in drain_out.iter().enumerate() {
        if index == 15 {
            assert_eq!(sample, 0.5, "impulse value must survive bit-exact");
        } else {
            assert_eq!(sample, 0.0, "only the impulse position is nonzero");
        }
    }

    // Same proof on the key bus: the key gain is binary-exact.
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let program = vec![0.0; frames];
    let mut key = vec![0.0; frames];
    key[frames - 1] = 0.25;
    let input = interleave(&program, &key);
    let (_, drain_out, _) = render_and_drain(&mut host, &input, 2, &[64]);
    assert_eq!(drain_out.len(), 16);
    for (index, &sample) in drain_out.iter().enumerate() {
        if index == 15 {
            assert_eq!(sample, 0.125, "key impulse times gain must be exact");
        } else {
            assert_eq!(sample, 0.0, "only the impulse position is nonzero");
        }
    }
}

#[test]
fn fan_out_delivers_independent_branches() {
    // One source fans out to a delayed branch and a gained branch; the
    // multi-output sum aligns through the output-delay flush.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let delayed = host
        .add_node("delayed".to_string(), Box::new(DelayTail::new(8, 1, 4)))
        .unwrap();
    let direct = host
        .add_node("direct".to_string(), Box::new(Gain::new(1, 0.5)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, delayed)).unwrap();
    host.add_edge(GraphEdge::new(source, direct)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let program = program_signal(frames);
    let input: Vec<f32> = program.iter().map(|&sample| sample as f32).collect();
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 1, &[64]);
    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 8);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected: Vec<f64> = delay_reference(&program, 8, frames + 8)
        .iter()
        .map(|&sample| 1.5 * sample)
        .collect();
    assert_close(&total, &expected, 1e-6, "fan-out total");
}

#[test]
fn fan_in_sums_latency_aligned_branches() {
    // Diamond: one source feeds two delays of different lengths into a
    // summing audio join; compensation aligns the short branch.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let slow = host
        .add_node("slow".to_string(), Box::new(DelayTail::new(10, 1, 4)))
        .unwrap();
    let fast = host
        .add_node("fast".to_string(), Box::new(DelayTail::new(3, 1, 4)))
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, slow)).unwrap();
    host.add_edge(GraphEdge::new(source, fast)).unwrap();
    host.add_edge(GraphEdge::new(slow, join)).unwrap();
    host.add_edge(GraphEdge::new(fast, join)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let program = program_signal(frames);
    let input: Vec<f32> = program.iter().map(|&sample| sample as f32).collect();
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 1, &[64]);
    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 10);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected: Vec<f64> = delay_reference(&program, 10, frames + 10)
        .iter()
        .map(|&sample| 2.0 * sample)
        .collect();
    assert_close(&total, &expected, 1e-6, "fan-in total");
}

#[test]
fn irregular_chunks_match_single_block_reference() {
    let frames = 104;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);

    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let (process_out, drain_out, _) =
        render_and_drain(&mut host, &input, 2, &[1, 3, 7, 2, 13, 5, 64, 9]);
    let mut chunked = process_out;
    chunked.extend_from_slice(&drain_out);

    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[104]);
    let mut single = process_out;
    single.extend_from_slice(&drain_out);

    assert_eq!(chunked.len(), single.len());
    assert_eq!(chunked, single, "block partition must not change output");
}

#[test]
fn capacity_retry_matches_untouched_twin() {
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let (mut twin, _, _, _) = keyed_graph(16, 5, 8);
    let frames = 48;
    let input = interleave(&program_signal(frames), &key_signal(frames));
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let mut twin_out = vec![0.0; frames];
    twin.process(&input, &mut twin_out).unwrap();

    // Too-small storage fails before any plugin state advances.
    let bound = host.drain_output_frames_max();
    assert!(bound > 0);
    let mut short = vec![12345.0; (bound - 1) * host.output_channels()];
    let error = host.drain(&mut short).unwrap_err();
    assert!(error.contains("too small"), "unexpected error: {error}");
    assert!(short.iter().all(|&sample| sample == 12345.0));

    // Retry reproduces the untouched twin bit-exact.
    let mut first = Vec::new();
    let mut second = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound * host.output_channels()];
        let result = host.drain(&mut output).unwrap();
        first.extend_from_slice(&output[..result.frames * host.output_channels()]);
        let mut output = vec![f32::NAN; bound * twin.output_channels()];
        let result = twin.drain(&mut output).unwrap();
        second.extend_from_slice(&output[..result.frames * twin.output_channels()]);
        if result.complete {
            break;
        }
    }
    assert_eq!(first, second);
}

#[test]
fn repeat_complete_reset_and_new_input_rearm() {
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let frames = 48;
    let input = interleave(&program_signal(frames), &key_signal(frames));
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let bound = host.drain_output_frames_max() * host.output_channels();

    let mut first = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound];
        let result = host.drain(&mut output).unwrap();
        first.extend_from_slice(&output[..result.frames * host.output_channels()]);
        if result.complete {
            break;
        }
    }
    assert_eq!(first.len(), 16);

    // Completed streams repeat without requiring storage.
    let repeat = host.drain(&mut []).unwrap();
    assert_eq!(repeat.frames, 0);
    assert!(repeat.complete);
    let mut output = vec![f32::NAN; bound];
    let repeat = host.drain(&mut output).unwrap();
    assert_eq!(repeat.frames, 0);
    assert!(repeat.complete);

    // Reset restores the identical stream.
    host.reset();
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let mut second = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound];
        let result = host.drain(&mut output).unwrap();
        second.extend_from_slice(&output[..result.frames * host.output_channels()]);
        if result.complete {
            break;
        }
    }
    assert_eq!(first, second);

    // Accepted input after completion rearms the full tail.
    host.reset();
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let mut output = vec![f32::NAN; bound];
    let partial = host.drain(&mut output).unwrap();
    assert!(!partial.complete);
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let mut rearmed = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound];
        let result = host.drain(&mut output).unwrap();
        rearmed.extend_from_slice(&output[..result.frames * host.output_channels()]);
        if result.complete {
            break;
        }
    }
    assert_eq!(rearmed.len(), 16);
}

#[test]
fn explicit_build_preserves_drain_progress() {
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let (mut twin, _, _, _) = keyed_graph(16, 5, 8);
    let frames = 48;
    let input = interleave(&program_signal(frames), &key_signal(frames));
    let mut output = vec![0.0; frames];
    host.process(&input, &mut output).unwrap();
    let mut twin_out = vec![0.0; frames];
    twin.process(&input, &mut twin_out).unwrap();

    let bound = host.drain_output_frames_max() * host.output_channels();
    let mut first = Vec::new();
    let mut output = vec![f32::NAN; bound];
    let result = host.drain(&mut output).unwrap();
    assert!(!result.complete);
    first.extend_from_slice(&output[..result.frames * host.output_channels()]);
    host.build().unwrap();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound];
        let result = host.drain(&mut output).unwrap();
        first.extend_from_slice(&output[..result.frames * host.output_channels()]);
        if result.complete {
            break;
        }
    }

    let mut second = Vec::new();
    for _ in 0..10_000 {
        let mut output = vec![f32::NAN; bound];
        let result = twin.drain(&mut output).unwrap();
        second.extend_from_slice(&output[..result.frames * twin.output_channels()]);
        if result.complete {
            break;
        }
    }
    assert_eq!(first, second);
}

#[test]
fn quota_exhaustion_is_loud_after_exactly_4096_calls() {
    // Diamond with one branch that never completes: the scheduler must
    // surface quota exhaustion instead of hanging or dropping.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let endless = InfiniteDrain::new(1);
    let endless_calls = Arc::clone(&endless.calls);
    let infinite = host
        .add_node("infinite".to_string(), Box::new(endless))
        .unwrap();
    let finite = host
        .add_node("finite".to_string(), Box::new(DelayTail::new(4, 1, 4)))
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, infinite)).unwrap();
    host.add_edge(GraphEdge::new(source, finite)).unwrap();
    host.add_edge(GraphEdge::new(infinite, join)).unwrap();
    host.add_edge(GraphEdge::new(finite, join)).unwrap();
    host.build().unwrap();

    let input = vec![0.25; 16];
    let mut output = vec![0.0; 16];
    host.process(&input, &mut output).unwrap();
    let bound = host.drain_output_frames_max() * host.output_channels();
    let mut output = vec![f32::NAN; bound];
    for _ in 0..4096 {
        let result = host.drain(&mut output).unwrap();
        assert!(!result.complete);
    }
    let error = host.drain(&mut output).unwrap_err();
    assert!(
        error.contains("did not converge"),
        "unexpected error: {error}"
    );
    assert_eq!(endless_calls.drain.load(Ordering::Relaxed), 4096);
}

#[test]
fn unsupported_geometry_fails_loudly() {
    // Bypassed width-changing nodes have no defined passthrough.
    let (mut host, _, _, _) = keyed_graph(4, 4, 4);
    host.set_bypass_state(4, true);
    let mut output = vec![0.0; 64];
    let error = host.drain(&mut output).unwrap_err();
    assert!(
        error.contains("cannot bypass width-changing"),
        "unexpected error: {error}"
    );

    // Width-mismatched multiple outputs would truncate a tail.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let narrow = host
        .add_node("narrow".to_string(), Box::new(DelayTail::new(4, 1, 4)))
        .unwrap();
    let wide = host
        .add_node("wide".to_string(), Box::new(KeyedMix::new(2, 0, 0.0)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, narrow)).unwrap();
    host.add_edge(GraphEdge::new(source, wide)).unwrap();
    host.build().unwrap();
    let input = vec![0.25; 16];
    let mut output = vec![0.0; 16 * host.output_channels()];
    host.process(&input, &mut output).unwrap();
    let mut tail = vec![0.0; 64];
    let error = host.drain(&mut tail).unwrap_err();
    assert!(error.contains("output width"), "unexpected error: {error}");
}

#[test]
fn graph_drain_makes_no_allocations() {
    let (mut host, _, _, _) = keyed_graph(16, 5, 8);
    let frames = 64;
    let input = interleave(&program_signal(frames), &key_signal(frames));
    let bound = host.drain_output_frames_max();
    assert!(bound > 0 && bound * host.output_channels() <= 4096);
    crate::assert_no_allocs("graph drain", || {
        let mut process_out = [0.0f32; 64];
        let rendered = host.process(&input, &mut process_out).unwrap();
        assert_eq!(rendered, 64);
        let mut tail = [0.0f32; 4096];
        let mut total = 0;
        for _ in 0..64 {
            let result = host
                .drain(&mut tail[..bound * host.output_channels()])
                .unwrap();
            total += result.frames;
            if result.complete {
                break;
            }
        }
        assert_eq!(total, 16);
    });
}

#[test]
fn multi_output_empty_drain_rounds_stay_aligned() {
    // F1 counterexample 1: one output's first drain call legally
    // returns {0, incomplete}. A per-round-sum scheduler would emit 3
    // staggered frames; timestamp FIFOs emit the aligned 2-frame sum.
    let mut host = DawHost::new(2, 48_000);
    let lane0 = host
        .add_node("lane0".to_string(), Box::new(Select::new(2, 0)))
        .unwrap();
    let lane1 = host
        .add_node("lane1".to_string(), Box::new(Select::new(2, 1)))
        .unwrap();
    let tail0 = host
        .add_node("tail0".to_string(), Box::new(DelayTail::new(2, 1, 1)))
        .unwrap();
    let tail1 = host
        .add_node(
            "tail1".to_string(),
            Box::new(DelayTail::new(2, 1, 1).with_leading_empty(1)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(lane0, tail0)).unwrap();
    host.add_edge(GraphEdge::new(lane1, tail1)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[64]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 2, "aligned tails sum to 2 frames, not 3");
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let delayed0 = delay_reference(&program, 2, frames + 2);
    let delayed1 = delay_reference(&key, 2, frames + 2);
    let expected: Vec<f64> = delayed0
        .iter()
        .zip(delayed1.iter())
        .map(|(&a, &b)| a + b)
        .collect();
    assert_close(&total, &expected, 1e-6, "empty-round multi-output total");
}

#[test]
fn multi_output_unequal_depths_and_chunks_stay_aligned() {
    // F1 counterexample 2: equal 4-frame tails reach the host over
    // different depths (1 vs 3 nodes) and different drain chunking
    // (4 vs 2). Arrival rounds differ; the aligned 4-frame sum must
    // not, and no output may overrun the common length.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let shallow = host
        .add_node("shallow".to_string(), Box::new(DelayTail::new(4, 1, 4)))
        .unwrap();
    let mid1 = host
        .add_node("mid1".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let mid2 = host
        .add_node("mid2".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let deep = host
        .add_node("deep".to_string(), Box::new(DelayTail::new(4, 1, 2)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, shallow)).unwrap();
    host.add_edge(GraphEdge::new(source, mid1)).unwrap();
    host.add_edge(GraphEdge::new(mid1, mid2)).unwrap();
    host.add_edge(GraphEdge::new(mid2, deep)).unwrap();
    host.build().unwrap();

    let frames = 48;
    let signal = program_signal(frames);
    let input: Vec<f32> = signal.iter().map(|&sample| sample as f32).collect();
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 1, &[48]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 4);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let delayed = delay_reference(&signal, 4, frames + 4);
    let expected: Vec<f64> = delayed.iter().map(|&sample| 2.0 * sample).collect();
    assert_close(&total, &expected, 1e-6, "unequal-depth multi-output total");
}

#[test]
fn key_surplus_content_never_extends_program() {
    // F4: the key branch emits 6 FIR frames plus 4 compensation frames
    // while program emits 4. Audio-driven length ends output with
    // program; the 6 surplus key frames are discarded, and the key
    // source still finishes its own drain instead of wedging.
    let mut host = DawHost::new(2, 48_000);
    let program_select = host
        .add_node("program select".to_string(), Box::new(Select::new(2, 0)))
        .unwrap();
    let key_select = host
        .add_node("key select".to_string(), Box::new(Select::new(2, 1)))
        .unwrap();
    let program_tail = DelayTail::new(4, 1, 4);
    let program_calls = Arc::clone(&program_tail.calls);
    let program = host
        .add_node("program delay".to_string(), Box::new(program_tail))
        .unwrap();
    let kernel = vec![1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625];
    let key_fir = FirTail::new(kernel.clone(), 1, 2);
    let key_calls = Arc::clone(&key_fir.calls);
    let key = host
        .add_node("key fir".to_string(), Box::new(key_fir))
        .unwrap();
    let join_mix = KeyedMix::new(1, 1, 0.5);
    let join = host
        .add_node("keyed join".to_string(), Box::new(join_mix))
        .unwrap();
    host.add_edge(GraphEdge::new(program_select, program))
        .unwrap();
    host.add_edge(GraphEdge::new(key_select, key)).unwrap();
    host.add_edge(GraphEdge::new(program, join)).unwrap();
    host.add_edge(GraphEdge::sidechain(key, join)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[64]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 4, "key surplus must not extend program");
    // The 4 consumed post-compensation key frames are the FIR outputs
    // for the last 4 process inputs; the remaining 6 are discarded.
    let kernel64: Vec<f64> = kernel.iter().map(|&coeff| f64::from(coeff)).collect();
    let fir_full = fir_reference(&key, &kernel64, frames + 4);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected: Vec<f64> = (0..frames + 4)
        .map(|t| {
            program.get(t.wrapping_sub(4)).copied().unwrap_or(0.0)
                + 0.5 * fir_full.get(t.wrapping_sub(4)).copied().unwrap_or(0.0)
        })
        .collect();
    assert_close(&total, &expected, 1e-5, "key-surplus total");
    assert_eq!(
        key_calls.drain.load(Ordering::Relaxed),
        3,
        "6 FIR frames drain in chunks of 2"
    );
    assert_eq!(program_calls.drain.load(Ordering::Relaxed), 1);
}

#[test]
fn delayed_key_data_is_awaited_not_padded() {
    // Root qualification: a non-EOF key branch that delivers its
    // frames late (two leading empty drain calls) must be awaited at
    // matching timestamps. Zero-padding it early would corrupt the
    // aligned key contribution; the total must equal the aligned
    // reference exactly.
    let (mut host, _, key_calls, _) = keyed_graph_with_empties(16, 5, 5, 0, 2);
    let frames = 96;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[96]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 16);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected = keyed_reference(&program, &key, 16, 0.5, frames + 16);
    assert_close(&total, &expected, 1e-6, "delayed-key total");
    assert_eq!(
        key_calls.drain.load(Ordering::Relaxed),
        3,
        "2 empty plus 1 data call"
    );
}

#[test]
fn diamond_with_unequal_depths_completes_exact() {
    // F2 liveness walk: branches of depth 1 and 3 reconverge with
    // equal tails. Edge queues hold one wave each; the join aligns
    // them and the graph completes without stalling.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let shallow = host
        .add_node("shallow".to_string(), Box::new(DelayTail::new(6, 1, 2)))
        .unwrap();
    let mid1 = host
        .add_node("mid1".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let mid2 = host
        .add_node("mid2".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let deep = host
        .add_node("deep".to_string(), Box::new(DelayTail::new(6, 1, 2)))
        .unwrap();
    let join = host
        .add_node("join".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, shallow)).unwrap();
    host.add_edge(GraphEdge::new(source, mid1)).unwrap();
    host.add_edge(GraphEdge::new(mid1, mid2)).unwrap();
    host.add_edge(GraphEdge::new(mid2, deep)).unwrap();
    host.add_edge(GraphEdge::new(shallow, join)).unwrap();
    host.add_edge(GraphEdge::new(deep, join)).unwrap();
    host.build().unwrap();

    let frames = 48;
    let signal = program_signal(frames);
    let input: Vec<f32> = signal.iter().map(|&sample| sample as f32).collect();
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 1, &[48]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 6);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let delayed = delay_reference(&signal, 6, frames + 6);
    let expected: Vec<f64> = delayed.iter().map(|&sample| 2.0 * sample).collect();
    assert_close(&total, &expected, 1e-6, "unequal-depth diamond total");
}

#[test]
fn leading_empty_drains_count_quota_and_complete() {
    // F3: legal zero-frame incomplete drain calls consume quota and
    // count as progress; the program tail drains 2 empties plus 2
    // data waves and the total output stays exact.
    let (mut host, program_calls, _, join_calls) = keyed_graph_with_empties(16, 5, 8, 2, 0);
    let frames = 96;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[32, 64]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 16);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected = keyed_reference(&program, &key, 16, 0.5, frames + 16);
    assert_close(&total, &expected, 1e-6, "leading-empty total");
    assert_eq!(program_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(
        program_calls.drain.load(Ordering::Relaxed),
        4,
        "2 empty plus 2 data calls"
    );
    assert_eq!(join_calls.drain.load(Ordering::Relaxed), 1);
}

#[test]
fn declared_tail_beyond_4096_completes_with_post_begin_quota_only() {
    // No 4096-scale lifetime assumption: a declared bound of 5000
    // drains 5000 waves through wave-bounded queues paced by
    // backpressure, and the quota is queried only after begin (the
    // guarded fixture panics on any pre-begin query, including at
    // build time).
    let mut host = DawHost::new(2, 48_000);
    let program_select = host
        .add_node("program select".to_string(), Box::new(Select::new(2, 0)))
        .unwrap();
    let key_select = host
        .add_node("key select".to_string(), Box::new(Select::new(2, 1)))
        .unwrap();
    let program_tail = DelayTail::new(5000, 1, 1).with_begin_guard();
    let program_calls = Arc::clone(&program_tail.calls);
    let program = host
        .add_node("program delay".to_string(), Box::new(program_tail))
        .unwrap();
    let key_tail = DelayTail::new(5, 1, 5);
    let key = host
        .add_node("key delay".to_string(), Box::new(key_tail))
        .unwrap();
    let join_mix = KeyedMix::new(1, 1, 0.5);
    let join = host
        .add_node("keyed join".to_string(), Box::new(join_mix))
        .unwrap();
    host.add_edge(GraphEdge::new(program_select, program))
        .unwrap();
    host.add_edge(GraphEdge::new(key_select, key)).unwrap();
    host.add_edge(GraphEdge::new(program, join)).unwrap();
    host.add_edge(GraphEdge::sidechain(key, join)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let program = program_signal(frames);
    let key = key_signal(frames);
    let input = interleave(&program, &key);
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 2, &[64]);

    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 5000);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let expected = keyed_reference(&program, &key, 5000, 0.5, frames + 5000);
    assert_close(&total, &expected, 1e-6, "long-tail total");
    assert_eq!(program_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(
        program_calls.drain.load(Ordering::Relaxed),
        5000,
        "declared bound 5000 drains exactly 5000 waves"
    );
}

#[test]
fn huge_output_compensation_flushes_without_callback_alloc() {
    // Reviewer memory finding 1: a 300000-frame output-delay skew
    // exceeds the old wave scratch extent, so the plan must size the
    // flush push at build. Process stays 64 frames; the full
    // compensation tail drains exact under the allocation counter.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let slow = host
        .add_node("slow".to_string(), Box::new(LatencyStub::new(300_000, 1)))
        .unwrap();
    let fast = host
        .add_node("fast".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    host.add_edge(GraphEdge::new(source, slow)).unwrap();
    host.add_edge(GraphEdge::new(source, fast)).unwrap();
    host.build().unwrap();

    let frames = 64;
    let signal = program_signal(frames);
    let input: Vec<f32> = signal.iter().map(|&sample| sample as f32).collect();
    let channels = host.output_channels();
    let bound = host.drain_output_frames_max() * channels;
    assert!(bound >= 300_000, "bound covers the flush extent: {bound}");
    let mut process_out = vec![0.0f32; frames];
    let mut drain_out = vec![0.0f32; 300_000];
    let mut drained = 0usize;
    let mut tail = vec![f32::NAN; bound];
    crate::assert_no_allocs("huge output compensation", || {
        let rendered = host.process(&input, &mut process_out).unwrap();
        assert_eq!(rendered, frames);
        for _ in 0..8 {
            let result = host.drain(&mut tail).unwrap();
            let take = result.frames * channels;
            drain_out[drained..drained + take].copy_from_slice(&tail[..take]);
            drained += take;
            if result.complete {
                break;
            }
        }
    });
    assert_eq!(drained, 300_000);
    let mut total = process_out;
    total.extend_from_slice(&drain_out[..drained]);
    // The stub declares its latency without delaying (passthrough), so
    // the host aligns on the declared value: process frames carry the
    // slow branch input, while drain frames carry only the fast
    // branch's full delay-history release.
    let delayed = delay_reference(&signal, 300_000, frames + 300_000);
    let expected: Vec<f64> = delayed
        .iter()
        .enumerate()
        .map(|(t, &late)| late + signal.get(t).copied().unwrap_or(0.0))
        .collect();
    assert_close(&total, &expected, 1e-6, "huge-compensation total");
}

#[test]
fn output_backpressure_retries_without_corrupting_delay() {
    // R3 blocking: a fast wave-carrying output with positive output
    // compensation must retain its wave against a full pending FIFO
    // and retry later against pristine delay history. Feeding the
    // delay before the room check (pre-fix) stores the wave into
    // delay history while discarding the released frames, so the
    // retry queues wrong frames and loses real ones.
    //
    // Forcing schedule: same-round live EOF evaluation cascades
    // the whole slow chain in round 1, so the sibling delivers 1
    // frame per round from the start while round-end emission takes
    // only the common prefix. The fast branch commits 4 frames per
    // round into its cap-8 FIFO (SE 4 plus output compensation 4,
    // from sibling latency 20 minus fast latency 16): the 3rd wave
    // overflows the FIFO and later waves keep retrying against the
    // live 4-frame output delay. The retry counter below proves the
    // full-FIFO path executed (no round-count inference); exact
    // waveform and length prove the forced retries behaved; call
    // counts pin the paced schedule.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let fast_tail = DelayTail::new(16, 1, 4);
    let fast_calls = Arc::clone(&fast_tail.calls);
    let fast = host
        .add_node("fast".to_string(), Box::new(fast_tail))
        .unwrap();
    host.add_edge(GraphEdge::new(source, fast)).unwrap();
    let mut prev = source;
    for depth in 0..10 {
        let stage = host
            .add_node(format!("stage{depth}"), Box::new(Gain::new(1, 1.0)))
            .unwrap();
        host.add_edge(GraphEdge::new(prev, stage)).unwrap();
        prev = stage;
    }
    let slow_tail = DelayTail::new(20, 1, 1);
    let slow_calls = Arc::clone(&slow_tail.calls);
    let slow = host
        .add_node("slow".to_string(), Box::new(slow_tail))
        .unwrap();
    host.add_edge(GraphEdge::new(prev, slow)).unwrap();
    host.build().unwrap();

    let frames = 96;
    let signal = program_signal(frames);
    let input: Vec<f32> = signal.iter().map(|&sample| sample as f32).collect();
    let (process_out, drain_out, _) = render_and_drain(&mut host, &input, 1, &[96]);

    // Output compensation aligns both branches to the max latency
    // (20): the fast 16-frame tail plus 4 compensation frames.
    assert_eq!(process_out.len(), frames);
    assert_eq!(drain_out.len(), 20);
    let mut total = process_out;
    total.extend_from_slice(&drain_out);
    let delayed = delay_reference(&signal, 20, frames + 20);
    let expected: Vec<f64> = delayed.iter().map(|&sample| 2.0 * sample).collect();
    assert_close(&total, &expected, 1e-6, "backpressure total");
    assert_eq!(fast_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(slow_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(
        fast_calls.drain.load(Ordering::Relaxed),
        4,
        "16-frame tail drains in 4 paced waves"
    );
    assert_eq!(
        slow_calls.drain.load(Ordering::Relaxed),
        20,
        "20-frame tail drains one frame per wave"
    );
    assert!(
        host.graph_drain_output_commit_retries() > 0,
        "schedule must force at least one full output-FIFO retry"
    );
}

#[test]
fn multi_output_declared_tail_beyond_4096_drains_wave_bounded() {
    // Declared tails past the 4096 unknown-bound fallback drain on
    // concurrent outputs through the same wave-scale queues: no
    // lifetime-sized reservation anywhere. Both outputs carry
    // 5000-frame tails (chunks 1 and 3). Common-prefix emission takes
    // one frame per round, forcing the three-frame producer to retry
    // when its cap-3 FIFO fills. The per-call bound stays wave-scale (3)
    // while 5000-frame tails flow, the guarded quota is queried only
    // after begin, and the whole paced drain runs under the
    // allocation counter.
    let mut host = DawHost::new(1, 48_000);
    let source = host
        .add_node("source".to_string(), Box::new(Gain::new(1, 1.0)))
        .unwrap();
    let fast_tail = DelayTail::new(5000, 1, 1).with_begin_guard();
    let fast_calls = Arc::clone(&fast_tail.calls);
    let fast = host
        .add_node("fast".to_string(), Box::new(fast_tail))
        .unwrap();
    host.add_edge(GraphEdge::new(source, fast)).unwrap();
    let mut prev = source;
    for depth in 0..5 {
        let stage = host
            .add_node(format!("slow-stage{depth}"), Box::new(Gain::new(1, 1.0)))
            .unwrap();
        host.add_edge(GraphEdge::new(prev, stage)).unwrap();
        prev = stage;
    }
    let slow_tail = DelayTail::new(5000, 1, 3);
    let slow_calls = Arc::clone(&slow_tail.calls);
    let slow = host
        .add_node("slow".to_string(), Box::new(slow_tail))
        .unwrap();
    host.add_edge(GraphEdge::new(prev, slow)).unwrap();
    host.build().unwrap();

    // Wave-scale bound while lifetime-scale tails flow.
    assert_eq!(host.drain_output_frames_max(), 3);

    let frames = 64;
    let signal = program_signal(frames);
    let input: Vec<f32> = signal.iter().map(|&sample| sample as f32).collect();
    let channels = host.output_channels();
    assert_eq!(channels, 1);
    let mut total = [0.0f32; 5064];
    let mut filled = 0usize;
    let mut process_frames = 0usize;
    let mut calls = 0usize;
    crate::assert_no_allocs("multi-output long tail", || {
        let mut process_out = [0.0f32; 64];
        process_frames = host.process(&input, &mut process_out).unwrap();
        total[..process_frames].copy_from_slice(&process_out[..process_frames]);
        filled = process_frames;
        let mut tail = [0.0f32; 3];
        for _ in 0..10_000 {
            let result = host.drain(&mut tail).unwrap();
            let take = result.frames * channels;
            total[filled..filled + take].copy_from_slice(&tail[..take]);
            filled += take;
            calls += 1;
            if result.complete {
                break;
            }
        }
    });
    assert_eq!(process_frames, frames);
    assert_eq!(filled, frames + 5000);
    let delayed = delay_reference(&signal, 5000, frames + 5000);
    let expected: Vec<f64> = delayed.iter().map(|&sample| 2.0 * sample).collect();
    assert_close(&total[..filled], &expected, 1e-6, "multi-output long total");
    assert_eq!(fast_calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(fast_calls.drain.load(Ordering::Relaxed), 5000);
    assert_eq!(
        slow_calls.drain.load(Ordering::Relaxed),
        1667,
        "5000 frames in chunks of 3"
    );
    assert_eq!(fast_calls.bound_queries.load(Ordering::Relaxed), 1);
    assert!(calls >= 5000, "paced over the full tail: {calls} rounds");
}
