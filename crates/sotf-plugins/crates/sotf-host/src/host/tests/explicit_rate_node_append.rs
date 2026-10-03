//! Focused D5 tests: explicit-rate graph-node insertion and pure-graph order refresh.
//!
//! Covers the scoped [`DawHost::add_node_at_rate`] API plus the `chain_built`
//! refresh rule in [`DawHost::build`]: pure-graph hosts recompute `chain_nodes`
//! from the stage layers exactly when node membership changes, while
//! chain-built hosts keep their insertion order untouched.
//!
//! The strict rate fixtures below reject every initialization and realtime
//! rate but their own input rate, so mid-graph placement genuinely requires
//! explicit-rate insertion; [`DawHost::add_node`] alone cannot admit such a
//! stage (proven by `plain_add_node_rejects_mid_graph_rate_mismatch`).

use super::super::{daw_host::DawHost, graph_edge::GraphEdge};
use super::scaler_plugin::ScalerPlugin;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use crate::test_utils::assert_no_allocs_or_deallocs;
use std::collections::VecDeque;

/// Strict integer-rate test converter with exact small-signal arithmetic.
///
/// Down mode (48 kHz in, 24 kHz out) averages adjacent frame pairs with
/// `(a + b) * 0.5` and carries one unpaired frame across blocks; the drain
/// emits a leftover carried frame unchanged. Up mode (24 kHz in, 48 kHz out)
/// repeats every frame twice and keeps no residual. Initialization and every
/// realtime entry point reject any rate but the configured input rate, so the
/// fixture can only sit mid-graph when the host inserts it at its explicit
/// input rate.
struct StrictRateFixture {
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    down: bool,
    carry: Vec<f32>,
    carry_f64: Vec<f64>,
    last_output_frames: usize,
}

impl StrictRateFixture {
    fn down_48_to_24(channels: usize) -> Self {
        Self::new(channels, 48_000, 24_000)
    }

    fn up_24_to_48(channels: usize) -> Self {
        Self::new(channels, 24_000, 48_000)
    }

    fn new(channels: usize, input_rate: u32, output_rate: u32) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        assert!(
            input_rate == 2 * output_rate || output_rate == 2 * input_rate,
            "fixture only models exact 2:1 rate steps"
        );
        Self {
            channels,
            input_rate,
            output_rate,
            down: input_rate > output_rate,
            carry: Vec::with_capacity(channels),
            carry_f64: Vec::with_capacity(channels),
            last_output_frames: 0,
        }
    }

    fn check_rate(&self, rate: u32, entry: &str) -> Result<(), String> {
        if rate == self.input_rate {
            Ok(())
        } else {
            Err(format!(
                "strict rate fixture {entry} requires init at {} Hz, got {rate} Hz",
                self.input_rate
            ))
        }
    }

    fn carried_frames(&self) -> usize {
        self.carry.len() / self.channels
    }
}

impl Plugin for StrictRateFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("StrictRateFixture", "0.1", "test")
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
        self.carry.clear();
        self.carry_f64.clear();
        self.last_output_frames = 0;
        Ok(())
    }

    fn reset(&mut self) {
        self.carry.clear();
        self.carry_f64.clear();
        self.last_output_frames = 0;
    }

    fn output_sample_rate(&self, _input_rate: u32) -> u32 {
        self.output_rate
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        if self.down {
            let held = self
                .carried_frames()
                .max(self.carry_f64.len() / self.channels);
            (held + input_frames) / 2
        } else {
            input_frames * 2
        }
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }

    fn tail_length(&self) -> TailLength {
        if self.down && (!self.carry.is_empty() || !self.carry_f64.is_empty()) {
            TailLength::Finite(1)
        } else {
            TailLength::Finite(0)
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        usize::from(self.down)
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
            .ok_or_else(|| "strict rate fixture input length overflow".to_string())?;
        if input.len() != expected_input {
            return Err(format!(
                "strict rate fixture input has {} samples, expected {expected_input}",
                input.len()
            ));
        }
        let produced = self.output_frames_for_input(ctx.num_frames);
        let needed = produced
            .checked_mul(self.channels)
            .ok_or_else(|| "strict rate fixture output length overflow".to_string())?;
        if output.len() < needed {
            return Err(format!(
                "strict rate fixture output holds {} samples, need {needed}",
                output.len()
            ));
        }
        if self.down {
            let mut out_frame = 0;
            let mut in_frame = 0;
            if !self.carry.is_empty() {
                for ch in 0..self.channels {
                    output[ch] = (self.carry[ch] + input[ch]) * 0.5;
                }
                self.carry.clear();
                out_frame = 1;
                in_frame = 1;
            }
            while in_frame + 1 < ctx.num_frames {
                for ch in 0..self.channels {
                    let a = input[in_frame * self.channels + ch];
                    let b = input[(in_frame + 1) * self.channels + ch];
                    output[out_frame * self.channels + ch] = (a + b) * 0.5;
                }
                out_frame += 1;
                in_frame += 2;
            }
            if in_frame < ctx.num_frames {
                // One unpaired frame carries into the next block (or the
                // drain). Capacity was reserved at construction, so this
                // never allocates on the callback.
                self.carry
                    .extend_from_slice(&input[in_frame * self.channels..]);
            }
            debug_assert_eq!(out_frame, produced);
        } else {
            for frame in 0..ctx.num_frames {
                let span = frame * self.channels..(frame + 1) * self.channels;
                output[2 * frame * self.channels..(2 * frame + 1) * self.channels]
                    .copy_from_slice(&input[span.clone()]);
                output[(2 * frame + 1) * self.channels..(2 * frame + 2) * self.channels]
                    .copy_from_slice(&input[span]);
            }
        }
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
            .ok_or_else(|| "strict rate fixture input length overflow".to_string())?;
        if input.len() != expected_input {
            return Err(format!(
                "strict rate fixture input has {} samples, expected {expected_input}",
                input.len()
            ));
        }
        let produced = if self.down {
            (self.carry_f64.len() / self.channels + ctx.num_frames) / 2
        } else {
            ctx.num_frames * 2
        };
        let needed = produced
            .checked_mul(self.channels)
            .ok_or_else(|| "strict rate fixture output length overflow".to_string())?;
        if output.len() < needed {
            return Err(format!(
                "strict rate fixture output holds {} samples, need {needed}",
                output.len()
            ));
        }
        if self.down {
            let mut out_frame = 0;
            let mut in_frame = 0;
            if !self.carry_f64.is_empty() {
                for ch in 0..self.channels {
                    output[ch] = (self.carry_f64[ch] + input[ch]) * 0.5;
                }
                self.carry_f64.clear();
                out_frame = 1;
                in_frame = 1;
            }
            while in_frame + 1 < ctx.num_frames {
                for ch in 0..self.channels {
                    let a = input[in_frame * self.channels + ch];
                    let b = input[(in_frame + 1) * self.channels + ch];
                    output[out_frame * self.channels + ch] = (a + b) * 0.5;
                }
                out_frame += 1;
                in_frame += 2;
            }
            if in_frame < ctx.num_frames {
                self.carry_f64
                    .extend_from_slice(&input[in_frame * self.channels..]);
            }
            debug_assert_eq!(out_frame, produced);
        } else {
            for frame in 0..ctx.num_frames {
                let span = frame * self.channels..(frame + 1) * self.channels;
                output[2 * frame * self.channels..(2 * frame + 1) * self.channels]
                    .copy_from_slice(&input[span.clone()]);
                output[(2 * frame + 1) * self.channels..(2 * frame + 2) * self.channels]
                    .copy_from_slice(&input[span]);
            }
        }
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
        if !self.down || (self.carry.is_empty() && self.carry_f64.is_empty()) {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !self.carry.is_empty() && !self.carry_f64.is_empty() {
            return Err(
                "strict rate fixture drain refused mixed-precision tails without reset".into(),
            );
        }
        if output.len() < self.channels {
            return Err(format!(
                "strict rate fixture drain output holds {} samples, need {}",
                output.len(),
                self.channels
            ));
        }
        if self.carry_f64.is_empty() {
            output[..self.channels].copy_from_slice(&self.carry);
            self.carry.clear();
        } else {
            for (dst, &src) in output[..self.channels]
                .iter_mut()
                .zip(self.carry_f64.iter())
            {
                *dst = src as f32;
            }
            self.carry_f64.clear();
        }
        Ok(PluginDrainResult {
            frames: 1,
            complete: true,
        })
    }
}

/// Same-rate sample copier. Identity frame geometry (conservatively declared
/// variable, mirroring `ScalerPlugin`, so merge tests stay on the diligent
/// readback path). Used as the diamond source/sink so branch content is
/// attributable.
struct PassthroughFixture {
    channels: usize,
    last_output_frames: usize,
}

impl PassthroughFixture {
    fn new(channels: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        Self {
            channels,
            last_output_frames: 0,
        }
    }
}

impl Plugin for PassthroughFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("PassthroughFixture", "0.1", "test")
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

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != ctx.num_frames * self.channels {
            return Err(format!(
                "passthrough fixture input has {} samples, expected {}",
                input.len(),
                ctx.num_frames * self.channels
            ));
        }
        if output.len() < input.len() {
            return Err(format!(
                "passthrough fixture output holds {} samples, need {}",
                output.len(),
                input.len()
            ));
        }
        output[..input.len()].copy_from_slice(input);
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
        if input.len() != ctx.num_frames * self.channels {
            return Err(format!(
                "passthrough fixture input has {} samples, expected {}",
                input.len(),
                ctx.num_frames * self.channels
            ));
        }
        if output.len() < input.len() {
            return Err(format!(
                "passthrough fixture output holds {} samples, need {}",
                output.len(),
                input.len()
            ));
        }
        output[..input.len()].copy_from_slice(input);
        self.last_output_frames = ctx.num_frames;
        Ok(ctx.num_frames)
    }
}

/// Same-rate verbatim burst fixture: emits input in `chunk`-frame bursts,
/// holding the remainder across blocks (lossless delay) and flushing it at
/// drain. Strict 48 kHz so graph placement is proven.
struct BurstHoldFixture {
    channels: usize,
    chunk: usize,
    residual: Vec<f32>,
    residual_f64: Vec<f64>,
    last_output_frames: usize,
}

impl BurstHoldFixture {
    fn new(channels: usize, chunk: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        assert!(chunk > 0, "fixture needs a nonzero burst chunk");
        Self {
            channels,
            chunk,
            residual: Vec::with_capacity(chunk * channels),
            residual_f64: Vec::with_capacity(chunk * channels),
            last_output_frames: 0,
        }
    }

    fn residual_frames(&self) -> usize {
        self.residual.len() / self.channels
    }

    fn emitted_frames(&self, input_frames: usize) -> usize {
        let held = self
            .residual_frames()
            .max(self.residual_f64.len() / self.channels);
        (held + input_frames) / self.chunk * self.chunk
    }
}

impl Plugin for BurstHoldFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("BurstHoldFixture", "0.1", "test")
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
        if sample_rate != 48_000 {
            return Err(format!(
                "burst hold fixture requires init at 48000 Hz, got {sample_rate} Hz"
            ));
        }
        self.residual.clear();
        self.residual_f64.clear();
        self.last_output_frames = 0;
        Ok(())
    }

    fn reset(&mut self) {
        self.residual.clear();
        self.residual_f64.clear();
        self.last_output_frames = 0;
    }

    fn output_sample_rate(&self, _input_rate: u32) -> u32 {
        48_000
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        self.emitted_frames(input_frames)
    }

    fn last_output_frames(&self) -> Option<usize> {
        Some(self.last_output_frames)
    }

    fn tail_length(&self) -> TailLength {
        let held = self
            .residual_frames()
            .max(self.residual_f64.len() / self.channels);
        TailLength::Finite(held as u64)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.chunk - 1
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
        if ctx.sample_rate != 48_000 {
            return Err(format!(
                "burst hold fixture process requires 48000 Hz, got {} Hz",
                ctx.sample_rate
            ));
        }
        if input.len() != ctx.num_frames * self.channels {
            return Err(format!(
                "burst hold fixture input has {} samples, expected {}",
                input.len(),
                ctx.num_frames * self.channels
            ));
        }
        let emit = self.emitted_frames(ctx.num_frames);
        if output.len() < emit * self.channels {
            return Err(format!(
                "burst hold fixture output holds {} samples, need {}",
                output.len(),
                emit * self.channels
            ));
        }
        self.residual.extend_from_slice(input);
        output[..emit * self.channels].copy_from_slice(&self.residual[..emit * self.channels]);
        self.residual.drain(..emit * self.channels);
        debug_assert!(self.residual_frames() < self.chunk);
        self.last_output_frames = emit;
        Ok(emit)
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
        if ctx.sample_rate != 48_000 {
            return Err(format!(
                "burst hold fixture process_f64 requires a 48000 Hz context, got {} Hz",
                ctx.sample_rate
            ));
        }
        if input.len() != ctx.num_frames * self.channels {
            return Err(format!(
                "burst hold fixture input has {} samples, expected {}",
                input.len(),
                ctx.num_frames * self.channels
            ));
        }
        let held = self.residual_f64.len() / self.channels;
        let emit = (held + ctx.num_frames) / self.chunk * self.chunk;
        let needed = emit * self.channels;
        if output.len() < needed {
            return Err(format!(
                "burst hold fixture output holds {} samples, need {needed}",
                output.len()
            ));
        }
        self.residual_f64.extend_from_slice(input);
        output[..emit * self.channels].copy_from_slice(&self.residual_f64[..emit * self.channels]);
        self.residual_f64.drain(..emit * self.channels);
        debug_assert!(self.residual_f64.len() / self.channels < self.chunk);
        self.last_output_frames = emit;
        Ok(emit)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.sample_rate != 48_000 {
            return Err(format!(
                "burst hold fixture begin_drain requires 48000 Hz, got {} Hz",
                context.sample_rate
            ));
        }
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if ctx.sample_rate != 48_000 {
            return Err(format!(
                "burst hold fixture drain requires 48000 Hz, got {} Hz",
                ctx.sample_rate
            ));
        }
        if self.residual.is_empty() && self.residual_f64.is_empty() {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if !self.residual.is_empty() && !self.residual_f64.is_empty() {
            return Err(
                "burst hold fixture drain refused mixed-precision tails without reset".into(),
            );
        }
        if self.residual_f64.is_empty() {
            if output.len() < self.residual.len() {
                return Err(format!(
                    "burst hold fixture drain output holds {} samples, need {}",
                    output.len(),
                    self.residual.len()
                ));
            }
            let frames = self.residual_frames();
            output[..self.residual.len()].copy_from_slice(&self.residual);
            self.residual.clear();
            return Ok(PluginDrainResult {
                frames,
                complete: true,
            });
        }
        if output.len() < self.residual_f64.len() {
            return Err(format!(
                "burst hold fixture drain output holds {} samples, need {}",
                output.len(),
                self.residual_f64.len()
            ));
        }
        let frames = self.residual_f64.len() / self.channels;
        for (dst, &src) in output[..self.residual_f64.len()]
            .iter_mut()
            .zip(self.residual_f64.iter())
        {
            *dst = src as f32;
        }
        self.residual_f64.clear();
        Ok(PluginDrainResult {
            frames,
            complete: true,
        })
    }
}

/// Whole-stream decimation oracle: adjacent-pair averages with the final odd
/// frame (if any) emitted unchanged, matching the fixture's carry-then-drain
/// lifecycle for every block partition.
fn decimate_oracle(input: &[f32], channels: usize) -> Vec<f32> {
    assert_eq!(
        input.len() % channels,
        0,
        "oracle input must be whole frames"
    );
    let frames = input.len() / channels;
    let mut out = Vec::with_capacity(input.len() / 2 + channels);
    let mut frame = 0;
    while frame + 1 < frames {
        for ch in 0..channels {
            let a = input[frame * channels + ch];
            let b = input[(frame + 1) * channels + ch];
            out.push((a + b) * 0.5);
        }
        frame += 2;
    }
    if frame < frames {
        out.extend_from_slice(&input[frame * channels..]);
    }
    out
}

/// Whole-stream upsampling oracle: every frame repeated twice, no residual.
fn upsample_oracle(mid: &[f32], channels: usize) -> Vec<f32> {
    assert_eq!(mid.len() % channels, 0, "oracle input must be whole frames");
    let mut out = Vec::with_capacity(mid.len() * 2);
    for frame in mid.chunks_exact(channels) {
        out.extend_from_slice(frame);
        out.extend_from_slice(frame);
    }
    out
}

/// Exact dyadic stereo fixture: multiples of 1/8, so every `(a + b) * 0.5`
/// the graph computes is exact and the oracle comparison is bitwise.
fn exact_stereo_input(frames: usize, channels: usize) -> Vec<f32> {
    let mut input = Vec::with_capacity(frames * channels);
    for frame in 0..frames {
        for ch in 0..channels {
            let step = ((frame * channels + ch) % 32) as f32 / 8.0 - 1.5;
            input.push(step);
        }
    }
    input
}

#[test]
fn plain_add_node_rejects_mid_graph_rate_mismatch() {
    // Necessity proof for `add_node_at_rate`: a strict 24 kHz-input converter
    // cannot be admitted after a 48-to-24 kHz stage through the plain API,
    // which always probes the host rate.
    let mut host = DawHost::new(2, 48_000);
    host.add_node(
        "down".to_string(),
        Box::new(StrictRateFixture::down_48_to_24(2)),
    )
    .unwrap();
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(48_000), 24_000);

    let err = host
        .add_node(
            "up".to_string(),
            Box::new(StrictRateFixture::up_24_to_48(2)),
        )
        .expect_err("plain add_node must reject the 24 kHz-input stage at the 48 kHz probe rate");
    assert!(
        err.contains("24000"),
        "rejection should name the required rate, got: {err}"
    );

    // The failed admission leaves no partial node behind.
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(48_000), 24_000);
}

#[test]
fn appended_converter_composes_bit_exact_with_eof() {
    const CHANNELS: usize = 2;
    // Odd total: the 9th frame exercises the decimator carry across the block
    // boundary and through the drain.
    const BLOCKS: [usize; 2] = [3, 6];

    let mut host = DawHost::new(CHANNELS, 48_000);
    let down = host
        .add_node(
            "down".to_string(),
            Box::new(StrictRateFixture::down_48_to_24(CHANNELS)),
        )
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(48_000), 24_000);

    // Mid-graph insertion at the stage's own input rate.
    let up = host
        .add_node_at_rate(
            "up".to_string(),
            Box::new(StrictRateFixture::up_24_to_48(CHANNELS)),
            24_000,
        )
        .unwrap();
    host.add_edge(GraphEdge::new(down, up)).unwrap();
    host.build().unwrap();

    // Pure-graph order refresh: the appended stage joins the derived order,
    // so the composed output clock, tail readback, and latency follow it.
    assert_eq!(host.chain_nodes, vec![down, up]);
    assert_eq!(host.output_sample_rate(48_000), 48_000);
    assert_eq!(host.total_latency_samples(), 0);

    let total_frames: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total_frames, CHANNELS);

    let mut produced: Vec<f32> = Vec::new();
    let mut actual_per_block = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for (block_index, &block_frames) in BLOCKS.iter().enumerate() {
        if block_index == 1 {
            // Repeated mid-stream rebuild: membership is unchanged, so the
            // derived order and negotiated rates must be untouched.
            host.build().unwrap();
            assert_eq!(host.chain_nodes, vec![down, up]);
            assert_eq!(host.output_sample_rate(48_000), 48_000);
        }
        let block = &input[cursor * CHANNELS..(cursor + block_frames) * CHANNELS];
        cursor += block_frames;
        let capacity = host.output_frames_for_input(block_frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
        // Diligent readback through the refreshed order: the composed graph
        // emits at the outer clock, not the mid-graph clock.
        let actual = host
            .last_output_frames()
            .expect("composed graph must report actual output frames");
        actual_per_block.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(actual_per_block, vec![2, 6]);

    let mut drain_out = vec![0.0f32; host.drain_output_frames_max() * CHANNELS];
    let mut drain_frames = 0;
    let mut drain_calls = 0;
    let tails = StrictRateFixture::down_48_to_24(CHANNELS).drain_output_frames_max()
        + StrictRateFixture::up_24_to_48(CHANNELS).drain_output_frames_max();
    let budget = host_drain_budget(BLOCKS.iter().sum(), tails, 4);
    loop {
        let step = host.drain(&mut drain_out).unwrap();
        produced.extend_from_slice(&drain_out[..step.frames * CHANNELS]);
        drain_frames += step.frames;
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }
    assert_eq!(drain_frames, 2);

    let expected = upsample_oracle(&decimate_oracle(&input, CHANNELS), CHANNELS);
    assert_eq!(
        produced.len(),
        expected.len(),
        "composed stream length must match the oracle"
    );
    assert_eq!(
        produced, expected,
        "48-to-24-to-48 composition must match the whole-stream oracle bitwise"
    );
    println!(
        "explicit-rate 48→24→48: blocks {BLOCKS:?} actual {actual_per_block:?}, \
         drain {drain_frames} frames in {drain_calls} calls, whole {} frames, bitwise match",
        produced.len() / CHANNELS,
    );
}

#[test]
fn pure_graph_order_refreshes_only_on_membership_change() {
    let mut host = DawHost::new(2, 48_000);
    let first = host
        .add_node(
            "down-a".to_string(),
            Box::new(StrictRateFixture::down_48_to_24(2)),
        )
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.chain_nodes, vec![first]);

    // Second same-rate-output stage: membership changes, so the order
    // re-derives (within-layer order is not pinned, only membership).
    let second = host
        .add_node(
            "down-b".to_string(),
            Box::new(StrictRateFixture::down_48_to_24(2)),
        )
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.chain_nodes.len(), 2);
    assert!(
        host.chain_nodes.contains(&first) && host.chain_nodes.contains(&second),
        "re-derived order must cover both stages, got {:?}",
        host.chain_nodes
    );

    // Repeated rebuilds without membership change keep the identical order
    // instead of churning on stage-layer iteration order.
    let snapshot = host.chain_nodes.clone();
    for _ in 0..10 {
        host.build().unwrap();
        assert_eq!(host.chain_nodes, snapshot);
    }
}

#[test]
fn chain_built_hosts_keep_chain_order() {
    let mut host = DawHost::new(2, 48_000);
    host.add_plugin(Box::new(ScalerPlugin::new(2, 1.0)))
        .unwrap();
    host.add_plugin(Box::new(ScalerPlugin::new(2, 1.0)))
        .unwrap();
    host.build().unwrap();
    let snapshot = host.chain_nodes.clone();
    assert_eq!(snapshot.len(), 2);

    // A same-rate side node joins the graph but must not disturb the
    // chain-built order, on this build or any later rebuild.
    host.add_node("side".to_string(), Box::new(ScalerPlugin::new(2, 1.0)))
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.chain_nodes, snapshot);
    host.build().unwrap();
    assert_eq!(host.chain_nodes, snapshot);
}

#[test]
fn unwired_explicit_rate_node_fails_build_loudly() {
    let mut host = DawHost::new(2, 48_000);
    host.add_node(
        "down".to_string(),
        Box::new(StrictRateFixture::down_48_to_24(2)),
    )
    .unwrap();
    // No edge: build renegotiates the unwired stage at the host rate, which
    // the strict fixture refuses instead of silently running off-rate.
    host.add_node_at_rate(
        "up".to_string(),
        Box::new(StrictRateFixture::up_24_to_48(2)),
        24_000,
    )
    .unwrap();
    let err = host
        .build()
        .expect_err("build must refuse the inconsistently wired graph");
    assert!(
        err.contains("24000"),
        "build refusal should name the required rate, got: {err}"
    );
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

/// Lossless variable-diamond oracle: the INTENDED join contract (R1: no
/// dropped samples). Per-edge retention queues keep every branch frame across
/// blocks; each block joins aligned prefixes; drain joins retained queues
/// plus native tails with zero-padding for the exhausted side (the drain
/// scheduler's existing EOF semantics). All fixtures are zero-latency, so no
/// compensation delay enters.
/// Derived host-drain hang guard: every round consumes queued audio, emits
/// output, or completes (scheduler liveness); total work scales with stream
/// frames times graph nodes. Fixture topologies declare zero latency, hence
/// zero compensation flush. Overrun means production no-progress, never a
/// larger arbitrary allowance.
fn host_drain_budget(stream_frames: usize, tail_frames: usize, nodes: usize) -> usize {
    (stream_frames + tail_frames + 1) * (nodes + 1) + 1
}

/// Lossless-test diamond: source fan-out to a 2:1 converter pair and a
/// hold-burst, joined at a passthrough sink. Five nodes, zero latencies.
fn build_diamond_host(chunk: usize) -> DawHost {
    const CHANNELS: usize = 2;
    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    let down = host
        .add_node(
            "branch-a-down".to_string(),
            Box::new(StrictRateFixture::down_48_to_24(CHANNELS)),
        )
        .unwrap();
    let up = host
        .add_node_at_rate(
            "branch-a-up".to_string(),
            Box::new(StrictRateFixture::up_24_to_48(CHANNELS)),
            24_000,
        )
        .unwrap();
    let burst = host
        .add_node(
            "branch-b".to_string(),
            Box::new(BurstHoldFixture::new(CHANNELS, chunk)),
        )
        .unwrap();
    let sink = host
        .add_node(
            "sink".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, down)).unwrap();
    host.add_edge(GraphEdge::new(down, up)).unwrap();
    host.add_edge(GraphEdge::new(up, sink)).unwrap();
    host.add_edge(GraphEdge::new(source, burst)).unwrap();
    host.add_edge(GraphEdge::new(burst, sink)).unwrap();
    host.build().unwrap();
    host
}

struct LosslessDiamondOracle {
    down: StrictRateFixture,
    up: StrictRateFixture,
    burst: BurstHoldFixture,
    queue_a: VecDeque<f32>,
    queue_b: VecDeque<f32>,
    channels: usize,
    a_counts: Vec<usize>,
    b_counts: Vec<usize>,
    joined_nonzero: bool,
}

impl LosslessDiamondOracle {
    fn new(channels: usize, chunk: usize) -> Self {
        let mut down = StrictRateFixture::down_48_to_24(channels);
        let mut up = StrictRateFixture::up_24_to_48(channels);
        let mut burst = BurstHoldFixture::new(channels, chunk);
        down.initialize(48_000).unwrap();
        up.initialize(24_000).unwrap();
        burst.initialize(48_000).unwrap();
        Self {
            down,
            up,
            burst,
            queue_a: VecDeque::new(),
            queue_b: VecDeque::new(),
            channels,
            a_counts: Vec::new(),
            b_counts: Vec::new(),
            joined_nonzero: false,
        }
    }

    /// Currently retained queue depth (frames); the rebuild test samples it
    /// mid-stream to prove retention is exercised, not vacuous lockstep.
    fn retained_frames(&self) -> usize {
        (self.queue_a.len() / self.channels).max(self.queue_b.len() / self.channels)
    }

    /// Feed one input block (S copies it to both branches); returns the
    /// retained aligned join for the block.
    fn feed_block(&mut self, block: &[f32]) -> Vec<f32> {
        let a1 = node_process(&mut self.down, 48_000, block);
        let a2 = node_process(&mut self.up, 24_000, &a1);
        let b = node_process(&mut self.burst, 48_000, block);
        self.queue_a.extend(a2.iter().copied());
        self.queue_b.extend(b.iter().copied());
        let joined_frames =
            (self.queue_a.len() / self.channels).min(self.queue_b.len() / self.channels);
        let mut joined = Vec::with_capacity(joined_frames * self.channels);
        for _ in 0..joined_frames * self.channels {
            let sum = self.queue_a.pop_front().unwrap() + self.queue_b.pop_front().unwrap();
            if sum != 0.0 {
                self.joined_nonzero = true;
            }
            joined.push(sum);
        }
        self.a_counts.push(a2.len() / self.channels);
        self.b_counts.push(b.len() / self.channels);
        joined
    }

    /// Join retained queues plus native branch tails with zero-padding.
    fn finish(&mut self) -> Vec<f32> {
        let a1_tail = node_drain(&mut self.down, 48_000);
        if !a1_tail.is_empty() {
            let fed = node_process(&mut self.up, 24_000, &a1_tail);
            self.queue_a.extend(fed.iter().copied());
        }
        let a2_tail = node_drain(&mut self.up, 24_000);
        self.queue_a.extend(a2_tail.iter().copied());
        let b_tail = node_drain(&mut self.burst, 48_000);
        self.queue_b.extend(b_tail.iter().copied());
        let joined_frames =
            (self.queue_a.len() / self.channels).max(self.queue_b.len() / self.channels);
        let mut joined = Vec::with_capacity(joined_frames * self.channels);
        for _ in 0..joined_frames * self.channels {
            let a = self.queue_a.pop_front().unwrap_or(0.0);
            let b = self.queue_b.pop_front().unwrap_or(0.0);
            joined.push(a + b);
        }
        joined
    }
}

#[test]
fn variable_diamond_lossless_join_requires_edge_retention() {
    // ACCEPTANCE: R1 requires nested variable-rate composition "without
    // dropped or inserted samples". The process-phase join retains per-edge
    // output across blocks, joins aligned prefixes, and EOF-pads tails; both
    // whole-stream asserts below pin lossless length and content against the
    // independent retention oracle. Do not weaken to min-prefix matching.
    const CHANNELS: usize = 2;
    const CHUNK: usize = 5;
    const BLOCKS: [usize; 7] = [3, 7, 2, 9, 13, 1, 29];

    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    let down = host
        .add_node(
            "branch-a-down".to_string(),
            Box::new(StrictRateFixture::down_48_to_24(CHANNELS)),
        )
        .unwrap();
    let up = host
        .add_node_at_rate(
            "branch-a-up".to_string(),
            Box::new(StrictRateFixture::up_24_to_48(CHANNELS)),
            24_000,
        )
        .unwrap();
    let burst = host
        .add_node(
            "branch-b".to_string(),
            Box::new(BurstHoldFixture::new(CHANNELS, CHUNK)),
        )
        .unwrap();
    let sink = host
        .add_node(
            "sink".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    for (from, to) in [
        (source, down),
        (down, up),
        (source, burst),
        (up, sink),
        (burst, sink),
    ] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.output_sample_rate(48_000), 48_000);

    let total_frames: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total_frames, CHANNELS);

    // Independent lossless oracle over raw fixtures first.
    let mut oracle = LosslessDiamondOracle::new(CHANNELS, CHUNK);
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

    // Activation proofs (green today and after retention lands): genuinely
    // unequal branches, nonzero join, nonempty drain.
    assert!(
        oracle.b_counts.contains(&0),
        "burst branch must idle some blocks"
    );
    assert!(
        oracle.b_counts.iter().any(|&count| count > 0),
        "burst branch must fire some blocks"
    );
    let a_min = *oracle.a_counts.iter().min().unwrap();
    let a_max = *oracle.a_counts.iter().max().unwrap();
    assert!(
        a_max > a_min,
        "converter branch counts must vary ({a_min}..={a_max})"
    );
    assert!(a_max > 0, "converter branch must produce");
    assert!(oracle.joined_nonzero, "join must carry nonzero content");
    assert!(drain_frames > 0, "drain must carry tail content");

    // Actual run with diligent readback.
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
            .expect("diamond must track actual production");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    let mut drain_out = vec![0.0f32; host.drain_output_frames_max() * CHANNELS];
    let mut drain_calls = 0;
    let tails = StrictRateFixture::down_48_to_24(CHANNELS).drain_output_frames_max()
        + StrictRateFixture::up_24_to_48(CHANNELS).drain_output_frames_max()
        + BurstHoldFixture::new(CHANNELS, CHUNK).drain_output_frames_max();
    let budget = host_drain_budget(BLOCKS.iter().sum(), tails, 5);
    loop {
        let step = host.drain(&mut drain_out).unwrap();
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
    println!(
        "lossless diamond expects {} frames ({} process + {} drain), host emits {}; per-block actual {actual_counts:?}; drain {drain_calls}/{budget} calls",
        expected.len() / CHANNELS,
        process_frames,
        drain_frames,
        produced.len() / CHANNELS,
    );

    // THE acceptance asserts: lossless length, then lossless content.
    assert_eq!(
        produced.len(),
        expected.len(),
        "lossless whole-stream length must match the retention oracle"
    );
    assert_eq!(
        produced, expected,
        "lossless whole-stream content must match the retention oracle bitwise"
    );
}

/// Idle branch fixture: declares honest 1:1 output but emits nothing. A
/// mean-mismatched diamond (idler vs live branch) must trip the retention
/// cap loudly — the cap admits a wave plus alignment, not sustained
/// divergence — never grow silently or truncate.
struct IdleFixture {
    channels: usize,
}

impl IdleFixture {
    fn new(channels: usize) -> Self {
        assert!(channels > 0, "fixture needs at least one channel");
        Self { channels }
    }
}

impl Plugin for IdleFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("IdleFixture", "0.1", "test")
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
        Ok(())
    }

    fn reset(&mut self) {}

    fn last_output_frames(&self) -> Option<usize> {
        Some(0)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(0)
    }

    fn drain_output_frames_max(&self) -> usize {
        0
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        std::num::NonZeroU64::new(1)
    }

    fn process(
        &mut self,
        input: &[f32],
        _output: &mut [f32],
        ctx: &ProcessContext,
    ) -> Result<usize, String> {
        if input.len() != ctx.num_frames * self.channels {
            return Err(format!(
                "idle fixture input has {} samples, expected {}",
                input.len(),
                ctx.num_frames * self.channels
            ));
        }
        Ok(0)
    }

    fn begin_drain(&mut self, _context: &ProcessContext) -> Result<(), String> {
        Ok(())
    }

    fn drain(
        &mut self,
        _output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        Ok(PluginDrainResult::COMPLETE)
    }
}

/// f64 twin of `node_process`: one raw stage block through `process_f64`.
/// The stage must already be initialized; state carries across calls.
fn node_process_f64(plugin: &mut dyn Plugin, rate: u32, input: &[f64]) -> Vec<f64> {
    assert!(
        plugin.supports_f64(),
        "f64 oracle stage must support process_f64"
    );
    let channels = plugin.input_channels();
    assert_eq!(
        channels,
        plugin.output_channels(),
        "f64 oracle stage keeps width"
    );
    let frames = input.len() / channels;
    let max = plugin.output_frames_for_input(frames);
    let mut output = vec![f64::NAN; max * channels];
    let produced = plugin
        .process_f64(input, &mut output, &ProcessContext::new(rate, frames))
        .unwrap();
    output.truncate(produced * channels);
    for &v in &output {
        assert!(v.is_finite(), "f64 oracle stage emitted non-finite audio");
    }
    output
}

/// Drain one host to completion under the derived call budget; appends
/// output to `produced` and returns the call count.
fn drain_host_to_completion(
    host: &mut DawHost,
    produced: &mut Vec<f32>,
    channels: usize,
    stream_frames: usize,
    tail_frames: usize,
    nodes: usize,
) -> usize {
    let mut drain_out = vec![0.0f32; host.drain_output_frames_max() * channels];
    let mut drain_calls = 0;
    let budget = host_drain_budget(stream_frames, tail_frames, nodes);
    loop {
        let step = host.drain(&mut drain_out).unwrap();
        produced.extend_from_slice(&drain_out[..step.frames * channels]);
        drain_calls += 1;
        if step.complete {
            break;
        }
        assert!(
            drain_calls < budget,
            "drain exceeded its derived call budget ({budget})"
        );
    }
    println!("host drain completed in {drain_calls}/{budget} budgeted calls");
    drain_calls
}

/// Drive one full diamond stream (process blocks plus EOF drain) through
/// both the host and the independent retention oracle; assert lossless
/// length and content. Returns `(produced, per_block_actual, drain_calls)`.
fn run_diamond_stream(
    host: &mut DawHost,
    chunk: usize,
    channels: usize,
    blocks: &[usize],
    input: &[f32],
) -> (Vec<f32>, Vec<usize>, usize) {
    let mut oracle = LosslessDiamondOracle::new(channels, chunk);
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in blocks {
        expected.extend_from_slice(
            &oracle.feed_block(&input[offset * channels..(offset + frames) * channels]),
        );
        offset += frames;
    }
    expected.extend_from_slice(&oracle.finish());
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(blocks.len());
    let mut cursor = 0;
    for &frames in blocks {
        let block = &input[cursor * channels..(cursor + frames) * channels];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * channels];
        host.process(block, &mut out).unwrap();
        let actual = host
            .last_output_frames()
            .expect("diamond tracks production");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * channels]);
    }
    let tails = StrictRateFixture::down_48_to_24(channels).drain_output_frames_max()
        + StrictRateFixture::up_24_to_48(channels).drain_output_frames_max()
        + BurstHoldFixture::new(channels, chunk).drain_output_frames_max();
    let drain_calls =
        drain_host_to_completion(host, &mut produced, channels, blocks.iter().sum(), tails, 5);
    assert_eq!(expected.len(), produced.len(), "lossless length");
    assert_eq!(expected, produced, "lossless content");
    (produced, actual_counts, drain_calls)
}

#[test]
fn merge_retention_process_path_allocates_nothing() {
    const CHANNELS: usize = 2;
    const BLOCKS: [usize; 3] = [17, 5, 31];
    let total: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total, CHANNELS);
    let mut host = build_diamond_host(5);
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
    // is already at size. Any allocation inside is a realtime defect in the
    // retained join.
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        assert_no_allocs_or_deallocs("retained merge process", || {
            host.process(block, &mut out).unwrap();
            let actual = host
                .last_output_frames()
                .expect("diamond tracks production");
            assert!(actual <= capacity, "host overran its declared output bound");
        });
    }
    println!("retained merge steady-state process: no heap activity over {BLOCKS:?}");
}

#[test]
fn merge_retention_reset_returns_to_fresh_state() {
    const CHANNELS: usize = 2;
    const CHUNK: usize = 5;
    const BLOCKS: [usize; 7] = [7, 5, 13, 3, 11, 9, 16];
    let total: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total, CHANNELS);
    let mut host = build_diamond_host(CHUNK);
    // Partial run leaves retention behind (the oracle proves it nonzero).
    let mut oracle = LosslessDiamondOracle::new(CHANNELS, CHUNK);
    let mut partial_join = Vec::new();
    let mut cursor = 0;
    for &frames in BLOCKS.iter().take(2) {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        partial_join.extend_from_slice(&oracle.feed_block(block));
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
    }
    assert!(
        !partial_join.is_empty(),
        "partial run must join some audio before reset"
    );
    assert!(
        oracle.retained_frames() > 0,
        "partial run must leave retention behind"
    );
    host.reset();
    // Post-reset the full stream must equal a fresh lossless run exactly.
    let (produced, actual_counts, drain_calls) =
        run_diamond_stream(&mut host, CHUNK, CHANNELS, &BLOCKS, &input);
    let mut fresh = build_diamond_host(CHUNK);
    let (fresh_produced, fresh_counts, _) =
        run_diamond_stream(&mut fresh, CHUNK, CHANNELS, &BLOCKS, &input);
    assert_eq!(
        actual_counts, fresh_counts,
        "reset must restore block cadence"
    );
    assert_eq!(produced, fresh_produced, "reset must restore audio exactly");
    println!("retention reset: post-reset stream matches fresh run, drain {drain_calls} calls");
}

#[test]
fn merge_retention_no_change_rebuild_preserves_queues() {
    const CHANNELS: usize = 2;
    const CHUNK: usize = 5;
    const BLOCKS: [usize; 7] = [7, 5, 13, 3, 11, 9, 16];
    let total: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total, CHANNELS);
    let mut host = build_diamond_host(CHUNK);
    let mut oracle = LosslessDiamondOracle::new(CHANNELS, CHUNK);
    let mut expected = Vec::new();
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for (index, &frames) in BLOCKS.iter().enumerate() {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        expected.extend_from_slice(&oracle.feed_block(block));
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f32; capacity * CHANNELS];
        host.process(block, &mut out).unwrap();
        let actual = host
            .last_output_frames()
            .expect("diamond tracks production");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
        if index == 1 {
            assert!(
                oracle.retained_frames() > 0,
                "rebuild probe needs live retention"
            );
            // No topology change: the rebuild must preserve every retained
            // queue, delay, and position rather than re-seeding them.
            host.build().unwrap();
        }
    }
    expected.extend_from_slice(&oracle.finish());
    let tails = StrictRateFixture::down_48_to_24(CHANNELS).drain_output_frames_max()
        + StrictRateFixture::up_24_to_48(CHANNELS).drain_output_frames_max()
        + BurstHoldFixture::new(CHANNELS, CHUNK).drain_output_frames_max();
    let drain_calls = drain_host_to_completion(&mut host, &mut produced, CHANNELS, total, tails, 5);
    assert_eq!(expected.len(), produced.len(), "lossless length");
    assert_eq!(expected, produced, "lossless content");
    println!(
        "rebuild mid-stream: {} frames lossless, drain {drain_calls}",
        produced.len() / CHANNELS,
    );
    println!("  per-block actual {actual_counts:?}");
}

#[test]
fn merge_retention_overflow_fails_loudly_and_sticks_until_reset() {
    const CHANNELS: usize = 2;
    const BLOCK: usize = 64;
    // Mean-mismatched diamond: a live 1:1 branch against an idle branch.
    // The live edge retains a full block per round with nothing to join
    // against; the derived cap admits a wave plus alignment, then trips.
    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    let live = host
        .add_node(
            "live".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    let idle = host
        .add_node("idle".to_string(), Box::new(IdleFixture::new(CHANNELS)))
        .unwrap();
    let sink = host
        .add_node(
            "sink".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, live)).unwrap();
    host.add_edge(GraphEdge::new(source, idle)).unwrap();
    host.add_edge(GraphEdge::new(live, sink)).unwrap();
    host.add_edge(GraphEdge::new(idle, sink)).unwrap();
    host.build().unwrap();
    let input = exact_stereo_input(BLOCK, CHANNELS);
    let capacity = host.output_frames_for_input(BLOCK);
    let mut out = vec![0.0f32; capacity * CHANNELS];
    let mut rounds = 0;
    let trip = loop {
        rounds += 1;
        if let Err(message) = host.process(&input, &mut out) {
            break message;
        }
        assert!(
            rounds < 65_536,
            "retention absorbed unbounded skew silently for {rounds} rounds"
        );
    };
    println!("retention tripped loudly after {rounds} rounds: {trip}");
    // Sticky poison: process, drain, and build all refuse with the exact
    // documented messages until reset recovers.
    assert_eq!(
        host.process(&input, &mut out).unwrap_err(),
        "host merge retention overflowed; reset the host before further processing"
    );
    assert_eq!(
        host.drain(&mut out).unwrap_err(),
        "host merge retention overflowed; reset the host before further processing or drain"
    );
    assert_eq!(
        host.build().unwrap_err(),
        "host merge retention overflowed; reset the host before rebuilding"
    );
    host.reset();
    host.process(&input, &mut out).unwrap();
    println!("retention poison cleared by reset; processing resumes");
}

#[test]
fn merge_retention_mean_matched_soak_stays_lossless() {
    const CHANNELS: usize = 2;
    const CHUNKS: [usize; 5] = [2, 3, 5, 8, 13];
    const PATTERNS: [[usize; 7]; 3] = [
        [7, 5, 13, 3, 11, 9, 16],
        [1, 1, 1, 64, 1, 31, 17],
        [64, 64, 64, 64, 64, 64, 64],
    ];
    for &chunk in &CHUNKS {
        for pattern in &PATTERNS {
            let total: usize = pattern.iter().sum();
            let input = exact_stereo_input(total, CHANNELS);
            let mut host = build_diamond_host(chunk);
            let (produced, actual_counts, drain_calls) =
                run_diamond_stream(&mut host, chunk, CHANNELS, pattern, &input);
            println!(
                "soak chunk {chunk} pattern {pattern:?}: {} frames, drain {drain_calls}",
                produced.len() / CHANNELS,
            );
            println!("  per-block actual {actual_counts:?}");
        }
    }
}

#[test]
fn unequal_burst_diamond_stays_lossless() {
    const CHANNELS: usize = 2;
    const CHUNK_A: usize = 3;
    const CHUNK_B: usize = 5;
    const BLOCKS: [usize; 9] = [7, 5, 13, 3, 11, 9, 16, 1, 31];
    // Two hold-bursts with coprime chunks: neither branch idles in lockstep
    // with the other, so retention skews both directions across the stream.
    let mut host = DawHost::new(CHANNELS, 48_000);
    let source = host
        .add_node(
            "source".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    let burst_a = host
        .add_node(
            "burst-a".to_string(),
            Box::new(BurstHoldFixture::new(CHANNELS, CHUNK_A)),
        )
        .unwrap();
    let burst_b = host
        .add_node(
            "burst-b".to_string(),
            Box::new(BurstHoldFixture::new(CHANNELS, CHUNK_B)),
        )
        .unwrap();
    let sink = host
        .add_node(
            "sink".to_string(),
            Box::new(PassthroughFixture::new(CHANNELS)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, burst_a)).unwrap();
    host.add_edge(GraphEdge::new(source, burst_b)).unwrap();
    host.add_edge(GraphEdge::new(burst_a, sink)).unwrap();
    host.add_edge(GraphEdge::new(burst_b, sink)).unwrap();
    host.build().unwrap();
    let total: usize = BLOCKS.iter().sum();
    let input = exact_stereo_input(total, CHANNELS);
    // Independent oracle: raw fixtures, retained queues, aligned join.
    let mut stage_a = BurstHoldFixture::new(CHANNELS, CHUNK_A);
    let mut stage_b = BurstHoldFixture::new(CHANNELS, CHUNK_B);
    stage_a.initialize(48_000).unwrap();
    stage_b.initialize(48_000).unwrap();
    let mut queue_a = VecDeque::new();
    let mut queue_b = VecDeque::new();
    let mut a_counts = Vec::new();
    let mut b_counts = Vec::new();
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &BLOCKS {
        let block = &input[offset * CHANNELS..(offset + frames) * CHANNELS];
        offset += frames;
        let emit_a = node_process(&mut stage_a, 48_000, block);
        let emit_b = node_process(&mut stage_b, 48_000, block);
        a_counts.push(emit_a.len() / CHANNELS);
        b_counts.push(emit_b.len() / CHANNELS);
        queue_a.extend(emit_a.iter().copied());
        queue_b.extend(emit_b.iter().copied());
        let join = queue_a.len().min(queue_b.len()) / CHANNELS * CHANNELS;
        expected.extend(
            queue_a
                .drain(..join)
                .zip(queue_b.drain(..join))
                .map(|(a, b)| a + b),
        );
    }
    assert!(
        a_counts.iter().zip(&b_counts).any(|(&a, &b)| a > b),
        "branch A must lead some blocks"
    );
    assert!(
        a_counts.iter().zip(&b_counts).any(|(&a, &b)| b > a),
        "branch B must lead some blocks"
    );
    // EOF drain: retained queues plus native tails, zero-padded on the
    // exhausted side, summed — the drain scheduler's existing EOF semantics.
    let tail_a = node_drain(&mut stage_a, 48_000);
    let tail_b = node_drain(&mut stage_b, 48_000);
    queue_a.extend(tail_a.iter().copied());
    queue_b.extend(tail_b.iter().copied());
    let tail_frames = queue_a.len().max(queue_b.len()) / CHANNELS;
    queue_a.resize(tail_frames * CHANNELS, 0.0);
    queue_b.resize(tail_frames * CHANNELS, 0.0);
    for _ in 0..tail_frames * CHANNELS {
        let a = queue_a.pop_front().unwrap();
        let b = queue_b.pop_front().unwrap();
        expected.push(a + b);
    }
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
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    let tails = BurstHoldFixture::new(CHANNELS, CHUNK_A).drain_output_frames_max()
        + BurstHoldFixture::new(CHANNELS, CHUNK_B).drain_output_frames_max();
    let drain_calls = drain_host_to_completion(&mut host, &mut produced, CHANNELS, total, tails, 4);
    assert_eq!(expected.len(), produced.len(), "lossless length");
    assert_eq!(expected, produced, "lossless content");
    println!(
        "unequal bursts {CHUNK_A}/{CHUNK_B}: {} frames lossless, drain {drain_calls}",
        produced.len() / CHANNELS,
    );
    println!("  per-block actual {actual_counts:?}");
}

#[test]
fn variable_diamond_f64_path_matches_f64_oracle_natively() {
    const CHANNELS: usize = 2;
    const CHUNK: usize = 5;
    // Total 60: even (no converter carry) and a chunk multiple (no burst
    // residue), so the drain phase is the empty-tail handshake and every
    // sample compares through the native f64 process path.
    const BLOCKS: [usize; 7] = [7, 5, 13, 3, 11, 9, 12];
    const TOLERANCE: f64 = 1e-9;
    // Native f64 diamond: every fixture declares supports_f64, so the host
    // runs the native f64 DAG path (no f32 bridge). The independent oracle
    // drives raw fixtures through process_f64 with its own f64 retention
    // queues; both sides must agree to 1e-9 (copies only, mean-free sink).
    let total: usize = BLOCKS.iter().sum();
    let input32 = exact_stereo_input(total, CHANNELS);
    let input: Vec<f64> = input32.iter().map(|&v| f64::from(v)).collect();
    let mut host = build_diamond_host(CHUNK);
    let mut down = StrictRateFixture::down_48_to_24(CHANNELS);
    let mut up = StrictRateFixture::up_24_to_48(CHANNELS);
    let mut burst = BurstHoldFixture::new(CHANNELS, CHUNK);
    down.initialize(48_000).unwrap();
    up.initialize(24_000).unwrap();
    burst.initialize(48_000).unwrap();
    let mut queue_a: VecDeque<f64> = VecDeque::new();
    let mut queue_b: VecDeque<f64> = VecDeque::new();
    let mut expected = Vec::new();
    let mut offset = 0;
    for &frames in &BLOCKS {
        let block = &input[offset * CHANNELS..(offset + frames) * CHANNELS];
        offset += frames;
        let converted = node_process_f64(&mut down, 48_000, block);
        let restored = node_process_f64(&mut up, 24_000, &converted);
        let held = node_process_f64(&mut burst, 48_000, block);
        queue_a.extend(restored.iter().copied());
        queue_b.extend(held.iter().copied());
        let join = queue_a.len().min(queue_b.len()) / CHANNELS * CHANNELS;
        expected.extend(
            queue_a
                .drain(..join)
                .zip(queue_b.drain(..join))
                .map(|(a, b)| a + b),
        );
    }
    let mut produced = Vec::new();
    let mut actual_counts = Vec::with_capacity(BLOCKS.len());
    let mut cursor = 0;
    for &frames in &BLOCKS {
        let block = &input[cursor * CHANNELS..(cursor + frames) * CHANNELS];
        cursor += frames;
        let capacity = host.output_frames_for_input(frames);
        let mut out = vec![0.0f64; capacity * CHANNELS];
        host.process_f64(block, &mut out).unwrap();
        let actual = host
            .last_output_frames()
            .expect("diamond tracks production");
        actual_counts.push(actual);
        produced.extend_from_slice(&out[..actual * CHANNELS]);
    }
    assert_eq!(expected.len(), produced.len(), "f64 lossless length");
    let mut max_deviation = 0.0f64;
    for (index, (&want, &got)) in expected.iter().zip(&produced).enumerate() {
        let deviation = (want - got).abs();
        max_deviation = max_deviation.max(deviation);
        assert!(
            deviation <= TOLERANCE,
            "f64 sample {index} deviates {deviation:e} (want {want}, got {got})"
        );
    }
    println!(
        "f64 diamond: {} frames within 1e-9, max deviation {max_deviation:e}",
        produced.len() / CHANNELS,
    );
    println!("  per-block actual {actual_counts:?}");
    // Empty-tail drain handshake through the documented f32 drain API.
    let process_len = produced.len();
    let tails = StrictRateFixture::down_48_to_24(CHANNELS).drain_output_frames_max()
        + StrictRateFixture::up_24_to_48(CHANNELS).drain_output_frames_max()
        + BurstHoldFixture::new(CHANNELS, CHUNK).drain_output_frames_max();
    let mut produced32 = Vec::new();
    let drain_calls =
        drain_host_to_completion(&mut host, &mut produced32, CHANNELS, total, tails, 5);
    assert!(produced32.is_empty(), "empty-tail drain must emit nothing");
    assert_eq!(produced.len(), process_len);
    // The documented drain API is f32: a lone down-converter run through
    // process_f64 with an odd frame count must surface its f64 tail through
    // drain identically to its f32 twin (widen/narrow roundtrip identity).
    let mut twin32 = StrictRateFixture::down_48_to_24(CHANNELS);
    let mut twin64 = StrictRateFixture::down_48_to_24(CHANNELS);
    twin32.initialize(48_000).unwrap();
    twin64.initialize(48_000).unwrap();
    let probe32 = exact_stereo_input(7, CHANNELS);
    let probe64: Vec<f64> = probe32.iter().map(|&v| f64::from(v)).collect();
    let out32 = node_process(&mut twin32, 48_000, &probe32);
    let out64 = node_process_f64(&mut twin64, 48_000, &probe64);
    assert!(!out32.is_empty() && !out64.is_empty(), "probe must convert");
    let tail32 = node_drain(&mut twin32, 48_000);
    let tail64_as_32 = node_drain(&mut twin64, 48_000);
    assert_eq!(tail32.len(), CHANNELS, "odd probe leaves one tail frame");
    assert_eq!(
        tail32, tail64_as_32,
        "f64 drain tail must match the f32 twin bitwise"
    );
    println!("f64 drain handshake: {drain_calls} calls, twin tails match bitwise");
}
