//! Stage A: `process_unpadded` truthful-count coverage.
//!
//! `process` pads short same-clock variable blocks up to the input length
//! and returns the padded count, so the return alone cannot distinguish
//! produced audio from padding; consumers inferred actual production from
//! `last_output_frames` plus identity probes (R2 section 5 residual: a
//! silent chain with node latency short-producing on an exact-bound
//! block). `process_unpadded` shares the render core but skips the
//! padding step, so its return is always the actual collected count and
//! output beyond it is untouched. These tests pin: the residual shape
//! itself (silent compensating halving/doubling pair, exact net bounds,
//! short production, latency), identity agreement between both entries,
//! agreement with the reporting channel, and warm allocation silence.

use super::super::{daw_host::DawHost, graph_edge::GraphEdge};
use super::scaler_plugin::ScalerPlugin;

use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginInfo, ProcessContext};
use crate::test_utils::measure_heap_activity;

const STAGE_LATENCY: usize = 2;

/// Same-clock halving chunker: buffers input and emits the even-indexed
/// frames of each complete 16-frame quantum. Lossy by design (a dynamics
/// double for count observability, not an audio path). Declarations are
/// loose but valid upper bounds over the current retention; production
/// never exceeds them. Silent unless `report` is set.
struct HalveFixture {
    channels: usize,
    buffer: Vec<f32>,
    buffer_cap_samples: usize,
    last_output: usize,
    latency: usize,
    report: bool,
}

impl HalveFixture {
    const QUANTUM: usize = 16;
    const MAX_TEST_BLOCK: usize = 100;

    fn new(channels: usize, latency: usize, report: bool) -> Self {
        assert!(channels > 0, "halve fixture needs channels");
        let cap = (Self::QUANTUM + Self::MAX_TEST_BLOCK)
            .checked_mul(channels)
            .expect("halve buffer fits addressable samples");
        Self {
            channels,
            buffer: Vec::with_capacity(cap),
            buffer_cap_samples: cap,
            last_output: 0,
            latency,
            report,
        }
    }

    fn retained_frames(&self) -> usize {
        debug_assert!(self.buffer.len().is_multiple_of(self.channels));
        self.buffer.len() / self.channels
    }

    /// Loose valid upper bound over the current retention.
    fn declare(&self, input_frames: usize) -> usize {
        let total = input_frames.saturating_add(self.retained_frames());
        total.div_ceil(2)
    }

    /// Residual-free upper bound: retention stays below one quantum.
    fn envelope_for(input_frames: usize) -> Option<usize> {
        let total = input_frames.checked_add(Self::QUANTUM - 1)?;
        Some(total.div_ceil(2))
    }
}

impl Plugin for HalveFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("HalveFixture", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "halve needs whole input frames"
        );
        assert!(
            self.buffer.len() + input.len() <= self.buffer_cap_samples,
            "halve buffer cap exhausted"
        );
        self.buffer.extend_from_slice(input);
        let buffered = self.retained_frames();
        let complete = buffered / Self::QUANTUM;
        let emit = complete * 8;
        let need = emit
            .checked_mul(self.channels)
            .ok_or("halve declared extent overflow")?;
        assert!(
            output.len() >= need,
            "halve output must fit declared frames"
        );
        for frame in 0..emit {
            let src = frame * 2 * self.channels;
            output[frame * self.channels..(frame + 1) * self.channels]
                .copy_from_slice(&self.buffer[src..src + self.channels]);
        }
        let consumed = complete * Self::QUANTUM * self.channels;
        self.buffer.drain(..consumed);
        self.last_output = emit;
        Ok(emit)
    }
    fn reset(&mut self) {
        self.buffer.clear();
        self.last_output = 0;
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        self.declare(input_frames)
    }
    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        Self::envelope_for(input_frames)
    }
    fn last_output_frames(&self) -> Option<usize> {
        self.report.then_some(self.last_output)
    }
}

/// Same-clock frame doubler: emits every input frame twice, exactly.
/// Silent: never reports last-output counts.
struct DoubleFixture {
    channels: usize,
    latency: usize,
}

impl DoubleFixture {
    fn new(channels: usize, latency: usize) -> Self {
        assert!(channels > 0, "double fixture needs channels");
        Self { channels, latency }
    }
}

impl Plugin for DoubleFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("DoubleFixture", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![]
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _ctx: &ProcessContext,
    ) -> Result<usize, String> {
        assert!(
            input.len().is_multiple_of(self.channels),
            "double needs whole input frames"
        );
        let produced = input.len() / self.channels * 2;
        let need = produced
            .checked_mul(self.channels)
            .ok_or("double declared extent overflow")?;
        assert!(
            output.len() >= need,
            "double output must fit declared frames"
        );
        for frame in 0..produced {
            let src = (frame / 2) * self.channels;
            let dst = frame * self.channels;
            output[dst..dst + self.channels].copy_from_slice(&input[src..src + self.channels]);
        }
        Ok(produced)
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames.saturating_mul(2)
    }
    fn output_frames_envelope(&self, input_frames: usize) -> Option<usize> {
        input_frames.checked_mul(2)
    }
}

/// Position-sensitive pattern: channel-distinct, exact in f32, finite.
fn pattern_sample(global_sample: usize, channels: usize) -> f32 {
    let frame = global_sample / channels;
    let lane = global_sample % channels;
    frame as f32 + lane as f32 * 0.5
}

fn pattern_frames(frames: usize, channels: usize, start_sample: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|index| pattern_sample(start_sample + index, channels))
        .collect()
}

/// The R2 section 5 residual shape as a compensating pair: net
/// exact-identity bounds (halve then double), chunk-quantized variable
/// production, silence unless `report`, and latency on both stages.
fn build_halve_double_chain(channels: usize, rate: u32, report: bool) -> DawHost {
    let mut host = DawHost::new(channels, rate);
    let down = host
        .add_node(
            "down".to_string(),
            Box::new(HalveFixture::new(channels, STAGE_LATENCY, report)),
        )
        .unwrap();
    let up = host
        .add_node(
            "up".to_string(),
            Box::new(DoubleFixture::new(channels, STAGE_LATENCY)),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(down, up)).unwrap();
    host.build().unwrap();
    host
}

#[test]
fn unpadded_reports_actual_on_silent_latent_short_production() {
    const CHANNELS: usize = 2;
    const RATE: u32 = 48_000;
    // Two identical chains: one driven padded (legacy masking), one
    // unpadded (truthful). State advances identically; only the return
    // contract differs.
    let mut pad = build_halve_double_chain(CHANNELS, RATE, false);
    let mut cut = build_halve_double_chain(CHANNELS, RATE, false);

    // Fresh declarations read exact-identity (the old preflight's probe
    // shape): bound == n here and at 100, yet production shorts below.
    let bound_now = pad.output_frames_for_input(4);
    let bound_probe = pad.output_frames_for_input(100);
    assert_eq!(bound_now, 4, "fresh bound reads exact");
    assert_eq!(bound_probe, 100, "probe reads exact");
    let silent = pad.last_output_frames().is_none();
    assert!(silent, "chain stays silent");
    let latency = pad.total_latency_samples();
    assert_eq!(latency, 2 * STAGE_LATENCY, "latency observed");

    // Sub-quantum block: nothing emitted yet. The padded entry masks the
    // short span as full zeros; the unpadded entry reports actual zero
    // and leaves its whole output untouched.
    let stream = pattern_frames(20, CHANNELS, 0);
    let (first, rest) = stream.split_at(4 * CHANNELS);
    let mut out4_padded = vec![f32::NAN; 4 * CHANNELS];
    let mut out4_actual = vec![f32::NAN; 4 * CHANNELS];
    let returned_padded = pad.process(first, &mut out4_padded).unwrap();
    let returned_actual = cut.process_unpadded(first, &mut out4_actual).unwrap();
    assert_eq!(returned_padded, 4, "padded masks the short block");
    assert_eq!(returned_actual, 0, "unpadded reports actual zero");
    assert!(
        out4_padded.iter().all(|sample| sample.to_bits() == 0),
        "padding is zeros"
    );
    assert!(
        out4_actual.iter().all(|sample| sample.is_nan()),
        "tail stays untouched"
    );

    // Completing block: one quantum emits eight halved frames, doubled
    // to sixteen. Both entries agree past the short span, and content
    // matches the locally derived doubling reference bitwise.
    let mut out16_padded = vec![f32::NAN; 16 * CHANNELS];
    let mut out16_actual = vec![f32::NAN; 16 * CHANNELS];
    let returned_padded = pad.process(rest, &mut out16_padded).unwrap();
    let returned_actual = cut.process_unpadded(rest, &mut out16_actual).unwrap();
    assert_eq!(returned_padded, 16, "full block reports produced");
    assert_eq!(returned_actual, 16, "unpadded agrees past short");
    let mut expected = Vec::with_capacity(16 * CHANNELS);
    for frame in (0..16).step_by(2) {
        let base = frame * CHANNELS;
        expected.extend_from_slice(&stream[base..base + CHANNELS]);
        expected.extend_from_slice(&stream[base..base + CHANNELS]);
    }
    let actual_pad = &out16_padded[..];
    let actual_cut = &out16_actual[..];
    assert_eq!(actual_cut, &expected[..], "doubled content matches");
    assert_eq!(actual_pad, &expected[..], "padded agrees past short");
    println!("unpadded residual: padded 4/16 vs actual 0/16, content bitwise");
}

#[test]
fn unpadded_agrees_with_padded_on_identity_chains() {
    const CHANNELS: usize = 2;
    const RATE: u32 = 48_000;
    const FRAMES: usize = 64;
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "scaler".to_string(),
        Box::new(ScalerPlugin::new(CHANNELS, 1.0)),
    )
    .unwrap();
    host.build().unwrap();
    let input = pattern_frames(FRAMES, CHANNELS, 0);
    let mut out_padded = vec![f32::NAN; FRAMES * CHANNELS];
    let mut out_actual = vec![f32::NAN; FRAMES * CHANNELS];
    let returned_padded = host.process(&input, &mut out_padded).unwrap();
    let returned_actual = host.process_unpadded(&input, &mut out_actual).unwrap();
    assert_eq!(returned_padded, FRAMES, "padded reports full");
    assert_eq!(returned_actual, FRAMES, "unpadded reports full");
    assert_eq!(out_padded, input, "padded passes audio");
    assert_eq!(out_actual, input, "unpadded passes audio");
    let silent = host.last_output_frames().is_none();
    assert!(silent, "identity chain stays silent");
}

#[test]
fn unpadded_matches_reporting_channel_on_variable_chain() {
    const CHANNELS: usize = 2;
    const RATE: u32 = 48_000;
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "halve".to_string(),
        Box::new(HalveFixture::new(CHANNELS, STAGE_LATENCY, true)),
    )
    .unwrap();
    host.build().unwrap();
    let stream = pattern_frames(24, CHANNELS, 0);
    let (first, rest) = stream.split_at(4 * CHANNELS);

    // Sub-quantum block: the unpadded return equals the reported count.
    let mut out_short = vec![f32::NAN; 4 * CHANNELS];
    let returned_short = host.process_unpadded(first, &mut out_short).unwrap();
    let reported_short = host.last_output_frames();
    assert_eq!(returned_short, 0, "short block reports zero");
    assert_eq!(reported_short, Some(0), "report channel agrees");

    // Completing block: one quantum emits eight halved frames, and the
    // tail canary past them proves the untouched-output contract.
    let mut out_full = vec![f32::NAN; 20 * CHANNELS];
    let returned_full = host.process_unpadded(rest, &mut out_full).unwrap();
    let reported_full = host.last_output_frames();
    assert_eq!(returned_full, 8, "quantum emits eight");
    assert_eq!(reported_full, Some(8), "report channel agrees");
    let mut expected = Vec::with_capacity(8 * CHANNELS);
    for frame in (0..16).step_by(2) {
        let base = frame * CHANNELS;
        expected.extend_from_slice(&stream[base..base + CHANNELS]);
    }
    let actual = &out_full[..8 * CHANNELS];
    assert_eq!(actual, &expected[..], "halved content matches");
    let tail = &out_full[8 * CHANNELS..];
    assert!(tail.iter().all(|sample| sample.is_nan()), "tail intact");
}

#[test]
fn unpadded_warm_block_allocates_nothing() {
    const CHANNELS: usize = 2;
    const RATE: u32 = 48_000;
    let mut host = build_halve_double_chain(CHANNELS, RATE, false);
    let prime = pattern_frames(4, CHANNELS, 0);
    let mut out_prime = vec![f32::NAN; 4 * CHANNELS];
    let primed = host.process_unpadded(&prime, &mut out_prime).unwrap();
    assert_eq!(primed, 0, "prime block buffers");
    let block = pattern_frames(16, CHANNELS, 4 * CHANNELS);
    let mut out_block = vec![f32::NAN; 16 * CHANNELS];
    let mut produced = 0;
    let (allocs, deallocs) = measure_heap_activity(|| {
        produced = host.process_unpadded(&block, &mut out_block).unwrap();
    });
    assert_eq!(produced, 16, "measured block produces");
    assert_eq!((allocs, deallocs), (0, 0), "warm unpadded silent");
    println!("unpadded warm block 16: heap ({allocs}, {deallocs})");
}
