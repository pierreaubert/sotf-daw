use super::super::{daw_host::DawHost, graph_edge::GraphEdge};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

// Install the allocator used by assert_no_allocs in the host's unit-test
// executable. Without this, that helper's thread-local counter never advances.
#[global_allocator]
static ALLOCATOR: crate::CountingAlloc = crate::CountingAlloc;

#[test]
fn fractional_graph_refuses_timeline_overflow_before_processing() {
    let rate = 1234.5678_f64;
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, rate);
    host.add_plugin(Box::new(ClockPlugin::new(1, 1, 0, &valid)))
        .unwrap();
    host.build().unwrap();
    host.set_playback_position(u64::MAX - 1).unwrap();
    let before = host.node_input_positions[0];
    let mut output = [0.0_f32; 2];
    let error = host.process(&[0.25, -0.25], &mut output).unwrap_err();
    assert!(error.contains("horizon"), "{error}");
    assert_eq!(host.node_input_positions[0], before);
    assert_eq!(host.automation_state.playback_position as u64, u64::MAX - 1);
}

#[test]
fn fractional_host_rate_reaches_both_clocks_without_integer_rounding() {
    let rate = 1_234.567_8_f64;
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, rate);
    host.add_plugin(Box::new(ClockPlugin::new(2, 1, 0, &valid)))
        .unwrap();
    host.add_plugin(Box::new(ClockPlugin::new(1, 2, 0, &valid)))
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.node_input_sample_rates[0], rate);
    assert_eq!(host.node_input_sample_rates[1], rate * 2.0);
    assert_eq!(host.output_sample_rate(rate).unwrap(), rate);

    for offset in [0.0_f32, 17.0] {
        let input: Vec<f32> = (0..17).map(|index| index as f32 + offset).collect();
        let mut output = [f32::NAN; 17];
        assert_eq!(host.process(&input, &mut output).unwrap(), input.len());
        assert_eq!(output.as_slice(), input.as_slice());
    }
    assert!(valid.load(Ordering::Relaxed));
}

#[test]
fn removed_node_clock_does_not_constrain_the_rebuilt_graph() {
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, 48_000);
    host.add_plugin(Box::new(ClockPlugin::new(1, 1, 0, &valid)))
        .unwrap();
    let removed = 0;
    host.add_plugin(Box::new(ClockPlugin::new(1, 1, 0, &valid)))
        .unwrap();
    host.build().unwrap();
    drop(host.remove_plugin(0).unwrap());
    // Sparse IDs retain their previous metadata slots until the next build.
    // Even an unrepresentable stale value must not enter the active clock.
    host.node_input_sample_rates[removed] = f64::MAX;
    host.node_output_sample_rates[removed] = f64::MAX;
    host.build().unwrap();
    host.node_input_sample_rates[removed] = f64::MAX;
    host.node_output_sample_rates[removed] = f64::MAX;
    let active = host.chain_nodes[0];
    // The rebuilt latency and processing paths must ignore the removed slot.
    assert_eq!(host.path_latency(active), 0);
    let mut output = [0.0_f32; 1];
    assert_eq!(host.process(&[0.25], &mut output).unwrap(), 1);
    assert_eq!(output, [0.25]);
    assert!(valid.load(Ordering::Relaxed));
}

#[test]
fn contracted_output_does_not_hide_upstream_input_position_overflow() {
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, 48_000);
    host.add_plugin(Box::new(ClockPlugin::new(2, 1, 0, &valid)))
        .unwrap();
    host.add_plugin(Box::new(ClockPlugin::new(1, 4, 0, &valid)))
        .unwrap();
    host.build().unwrap();
    // The downsampler emits fewer frames than the upstream node accepts.
    // Its input position, in the 96 kHz clock, is already near u64::MAX.
    host.set_playback_position((u64::MAX - 1) / 2).unwrap();
    let before = host.node_input_positions.clone();
    let mut output = [f32::NAN; 2];
    let error = host.process(&[0.25, 0.5], &mut output).unwrap_err();
    assert!(error.contains("horizon"), "{error}");
    assert_eq!(host.node_input_positions, before);
    assert!(output.iter().all(|sample| sample.is_nan()));
}

#[test]
fn invalid_host_clock_is_refused_before_plugin_initialization() {
    let valid = Arc::new(AtomicBool::new(true));
    for rate in [0.0, f64::NAN, f64::INFINITY] {
        let mut host = DawHost::new(1, rate);
        let plugin = ClockPlugin::new(1, 1, 0, &valid);
        let initialize_calls = plugin.initialize_calls.clone();
        assert!(host.add_plugin(Box::new(plugin)).is_err());
        assert_eq!(initialize_calls.load(Ordering::Relaxed), 0);
        assert!(host.build().is_err());
    }
}

/// Independent integer-rate oracle: repeat or decimate, then delay in the
/// output clock. No production resampler or host delay code is used.
struct ClockPlugin {
    numerator: usize,
    denominator: usize,
    delay: usize,
    history: VecDeque<f64>,
    initialized_rate: f64,
    initialize_calls: Arc<AtomicU64>,
    input_frames: u64,
    phase: usize,
    gain: f64,
    context_valid: Arc<AtomicBool>,
    batch: usize,
    pending: Vec<f64>,
    position_origin: Arc<AtomicU64>,
    has_input: bool,
}
impl ClockPlugin {
    fn new(numerator: usize, denominator: usize, delay: usize, valid: &Arc<AtomicBool>) -> Self {
        Self {
            numerator,
            denominator,
            delay,
            history: vec![0.0; delay].into(),
            initialized_rate: 0.0,
            initialize_calls: Arc::new(AtomicU64::new(0)),
            input_frames: 0,
            phase: 0,
            gain: 1.0,
            context_valid: valid.clone(),
            batch: 1,
            pending: Vec::with_capacity(8),
            position_origin: Arc::new(AtomicU64::new(0)),
            has_input: false,
        }
    }
    fn run(
        &mut self,
        input: impl Iterator<Item = f64>,
        mut write: impl FnMut(usize, f64),
        context: &ProcessContext,
    ) -> usize {
        if context.sample_rate != self.initialized_rate
            || context.transport.sample_position
                != self.input_frames + self.position_origin.load(Ordering::Relaxed)
        {
            self.context_valid.store(false, Ordering::Relaxed);
        }
        self.input_frames += context.num_frames as u64;
        if context.num_frames > 0 {
            self.has_input = true;
        }
        let mut count = 0;
        for sample in input {
            for _ in 0..self.numerator {
                if self.phase == 0 {
                    let sample = sample * self.gain;
                    let output = if self.delay == 0 {
                        sample
                    } else {
                        let delayed = self.history.pop_front().unwrap();
                        self.history.push_back(sample);
                        delayed
                    };
                    self.pending.push(output);
                    if self.pending.len() == self.batch {
                        for sample in self.pending.drain(..) {
                            write(count, sample);
                            count += 1;
                        }
                    }
                }
                self.phase = (self.phase + 1) % self.denominator;
            }
        }
        count
    }
}
impl Plugin for ClockPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Clock oracle", "1", "test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![Parameter::new_float("gain", "Gain", 1.0, 0.0, 4.0)]
    }
    fn set_parameter(&mut self, _: ParameterId, value: ParameterValue) -> Result<(), String> {
        self.gain = value.as_float().ok_or("expected float")? as f64;
        Ok(())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        Some(ParameterValue::Float(self.gain as f32))
    }
    fn initialize(&mut self, rate: f64) -> Result<(), String> {
        self.initialize_calls.fetch_add(1, Ordering::Relaxed);
        self.initialized_rate = rate;
        Ok(())
    }
    fn reset(&mut self) {
        // Draining consumes the delay line; restore its length so the
        // fixture is reusable after reset.
        if self.history.len() != self.delay {
            self.history = vec![0.0; self.delay].into();
        } else {
            self.history.iter_mut().for_each(|value| *value = 0.0);
        }
        self.input_frames = 0;
        self.phase = 0;
        self.pending.clear();
        self.has_input = false;
    }
    fn tail_length(&self) -> TailLength {
        TailLength::Finite((self.batch - 1 + self.delay) as u64)
    }
    fn latency_samples(&self) -> usize {
        self.delay
    }
    fn output_sample_rate(&self, rate: f64) -> f64 {
        rate * self.numerator as f64 / self.denominator as f64
    }
    fn output_frames_for_input(&self, frames: usize) -> usize {
        (frames * self.numerator).div_ceil(self.denominator) + self.batch - 1
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        Ok(self.run(
            input.iter().map(|sample| *sample as f64),
            |index, sample| output[index] = sample as f32,
            context,
        ))
    }
    fn supports_f64(&self) -> bool {
        true
    }
    fn drain_output_frames_max(&self) -> usize {
        self.batch - 1 + self.delay
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if context.sample_rate != self.initialized_rate
            || context.transport.sample_position
                != self.input_frames + self.position_origin.load(Ordering::Relaxed)
            || context.num_frames != 0
        {
            self.context_valid.store(false, Ordering::Relaxed);
        }
        if !self.has_input {
            return Ok(PluginDrainResult::COMPLETE);
        }
        // Retained output-rate samples, oldest first: batched leftovers
        // already passed the delay line, then the delay line itself.
        let mut frames = 0;
        for (target, sample) in output.iter_mut().zip(self.pending.drain(..)) {
            *target = sample as f32;
            frames += 1;
        }
        for (target, sample) in output[frames..].iter_mut().zip(self.history.drain(..)) {
            *target = sample as f32;
            frames += 1;
        }
        Ok(PluginDrainResult {
            frames,
            complete: true,
        })
    }
    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        Ok(self.run(
            input.iter().copied(),
            |index, sample| output[index] = sample,
            context,
        ))
    }
}

fn graph(join: bool) -> (DawHost, Arc<AtomicBool>) {
    let mut host = DawHost::new(1, 48000);
    let valid = Arc::new(AtomicBool::new(true));
    let mut node = |name: &str, up, down, delay| {
        host.add_node(
            name.into(),
            Box::new(ClockPlugin::new(up, down, delay, &valid)),
        )
        .unwrap()
    };
    let a = node("slow 48k", 1, 1, 3);
    let b = node("fast 48k", 1, 1, 1);
    let up = node("to 96k", 2, 1, 2);
    let pass = node("stay 48k", 1, 1, 0);
    let down = node("back to 48k", 1, 2, 1);
    let terminal = node("fast terminal", 1, 1, 0);
    for (from, to) in [(a, up), (up, down), (b, pass), (pass, terminal)] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    if join {
        let merge = host
            .add_node("join".into(), Box::new(ClockPlugin::new(1, 1, 2, &valid)))
            .unwrap();
        host.add_edge(GraphEdge::new(down, merge)).unwrap();
        host.add_edge(GraphEdge::new(terminal, merge)).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.node_input_sample_rates[down], 96000.0);
    assert_eq!(host.node_input_sample_rates[terminal], 48000.0);
    assert_eq!(host.node_latency_from_input[up], 8);
    assert_eq!(host.node_latency_from_input[down], 5);
    assert_eq!(host.total_latency_samples(), if join { 7 } else { 5 });
    assert_eq!(host.output_sample_rate(48000).unwrap(), 48000.0);
    assert_eq!(host.output_frames_for_input(37), 37);
    (host, valid)
}

#[test]
fn mixed_rate_branches_align_in_each_clock_f32_and_f64() {
    for join in [false, true] {
        for native in [false, true] {
            let (mut host, valid) = graph(join);
            let delay = host.total_latency_samples();
            let input: Vec<f64> = (0..256)
                .map(|index| {
                    if index < 200 {
                        ((index * 17 % 31) as f64 - 15.0) * 0.01
                    } else {
                        0.0
                    }
                })
                .collect();
            let mut result = Vec::new();
            let mut position = 0;
            for frames in [1, 17, 3, 64, 7, 51].into_iter().cycle() {
                let frames = frames.min(input.len() - position);
                if frames == 0 {
                    break;
                }
                if native {
                    let mut output = vec![0.0; frames];
                    assert_eq!(
                        host.process_f64(&input[position..position + frames], &mut output)
                            .unwrap(),
                        frames
                    );
                    result.extend(output);
                } else {
                    let block: Vec<_> = input[position..position + frames]
                        .iter()
                        .map(|value| *value as f32)
                        .collect();
                    let mut output = vec![0.0; frames];
                    assert_eq!(host.process(&block, &mut output).unwrap(), frames);
                    result.extend(output.into_iter().map(f64::from));
                }
                position += frames;
            }
            for (index, actual) in result.iter().enumerate() {
                let expected = if index < delay {
                    0.0
                } else {
                    2.0 * input[index - delay]
                };
                assert!(
                    (actual - expected).abs() < 1e-7,
                    "join={join}, native={native}, sample={index}: {actual} vs {expected}"
                );
            }
            assert!(
                valid.load(Ordering::Relaxed),
                "plugin initialized or processed in the wrong clock"
            );
        }
    }
}

#[test]
fn incompatible_rates_are_rejected_before_processing() {
    for join in [false, true] {
        let mut host = DawHost::new(1, 48000);
        let valid = Arc::new(AtomicBool::new(true));
        let a = host
            .add_node("96k".into(), Box::new(ClockPlugin::new(2, 1, 0, &valid)))
            .unwrap();
        let b = host
            .add_node("48k".into(), Box::new(ClockPlugin::new(1, 1, 0, &valid)))
            .unwrap();
        if join {
            let merge = host
                .add_node("join".into(), Box::new(ClockPlugin::new(1, 1, 0, &valid)))
                .unwrap();
            host.add_edge(GraphEdge::new(a, merge)).unwrap();
            host.add_edge(GraphEdge::new(b, merge)).unwrap();
        }
        let error = host.build().unwrap_err();
        assert!(error.contains("incompatible sample rates"), "{error}");
    }
}

#[test]
fn latent_plugins_preserve_parameter_event_offsets() {
    for native in [false, true] {
        let valid = Arc::new(AtomicBool::new(true));
        let mut host = DawHost::new(1, 48000);
        host.add_plugin(Box::new(ClockPlugin::new(1, 1, 5, &valid)))
            .unwrap();
        host.build().unwrap();
        host.set_plugin_parameter_at(0, "gain", ParameterValue::Float(0.25), 7)
            .unwrap();
        // Equal timestamps retain arrival order: the second event wins.
        host.set_plugin_parameter_at(0, "gain", ParameterValue::Float(0.5), 7)
            .unwrap();
        let actual: Vec<f64> = if native {
            let mut output = vec![0.0; 32];
            assert_eq!(host.process_f64(&[1.0; 32], &mut output).unwrap(), 32);
            output
        } else {
            let mut output = vec![0.0; 32];
            assert_eq!(host.process(&[1.0; 32], &mut output).unwrap(), 32);
            output.into_iter().map(f64::from).collect()
        };
        for (index, sample) in actual.iter().enumerate() {
            let expected = if index < 5 {
                0.0
            } else if index < 12 {
                1.0
            } else {
                0.5
            };
            assert_eq!(*sample, expected, "native={native}, sample={index}");
        }
        assert!(valid.load(Ordering::Relaxed));
    }
}

#[test]
fn mixed_rate_processing_and_latent_event_splits_do_not_allocate() {
    let (mut host, _) = graph(true);
    let mut output = [0.0; 97];
    crate::assert_no_allocs("mixed-rate DAG callback", || {
        host.process(&[0.1; 97], &mut output).unwrap();
        host.process(&[0.2; 31], &mut output[..31]).unwrap();
    });
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, 48000);
    host.add_plugin(Box::new(ClockPlugin::new(1, 1, 5, &valid)))
        .unwrap();
    host.build().unwrap();
    host.set_plugin_parameter_at(0, "gain", ParameterValue::Float(0.5), 37)
        .unwrap();
    crate::assert_no_allocs("latent sample-offset event", || {
        host.process(&[0.1; 97], &mut output).unwrap();
    });
}

#[test]
fn bypassed_rate_converter_keeps_downstream_clock_and_frame_count() {
    for native in [false, true] {
        let valid = Arc::new(AtomicBool::new(true));
        let mut host = DawHost::new(1, 48000);
        host.add_plugin(Box::new(ClockPlugin::new(2, 1, 2, &valid)))
            .unwrap();
        host.add_plugin(Box::new(ClockPlugin::new(1, 1, 0, &valid)))
            .unwrap();
        host.bypass_plugin(0).unwrap();
        host.build().unwrap();
        assert_eq!(host.output_sample_rate(48000).unwrap(), 48000.0);
        assert_eq!(host.total_latency_samples(), 0);
        if native {
            let mut output = [0.0_f64; 17];
            assert_eq!(host.process_f64(&[0.25; 17], &mut output).unwrap(), 17);
            assert_eq!(output, [0.25; 17]);
        } else {
            let mut output = [0.0_f32; 17];
            assert_eq!(host.process(&[0.25; 17], &mut output).unwrap(), 17);
            assert_eq!(output, [0.25; 17]);
        }
        assert!(valid.load(Ordering::Relaxed));
    }
}

#[test]
fn fractional_and_buffered_clocks_follow_accepted_frames_through_drain() {
    for batch in [1, 5] {
        // Exercise generic f32, native f64 chain, and native f64 DAG runners.
        for runner in 0..3 {
            let valid = Arc::new(AtomicBool::new(true));
            let mut host = DawHost::new(1, 48000);
            let mut converter = ClockPlugin::new(3, 2, 0, &valid);
            converter.batch = batch;
            host.add_plugin(Box::new(converter)).unwrap();
            host.add_plugin(Box::new(ClockPlugin::new(1, 1, 0, &valid)))
                .unwrap();
            host.build().unwrap();
            let mut position = 0;
            let mut actual = Vec::new();
            for frames in [1, 1, 3, 2, 7, 1, 4] {
                if position == 1 {
                    // Rebuilding unchanged clocks preserves buffered progress.
                    host.build().unwrap();
                }
                // Embedded hosts repeat the continuous position every callback.
                host.set_playback_position(position as u64).unwrap();
                let input: Vec<_> = (position..position + frames).map(|i| i as f64).collect();
                let capacity = host.output_frames_for_input(frames);
                if runner == 0 {
                    let input: Vec<_> = input.iter().map(|v| *v as f32).collect();
                    let mut output = vec![0.0; capacity];
                    let mut count = 0;
                    crate::assert_no_allocs("fractional f32 clock", || {
                        count = host.process(&input, &mut output).unwrap();
                    });
                    actual.extend(output[..count].iter().map(|v| *v as f64));
                } else {
                    let mut output = vec![0.0; capacity];
                    let mut count = 0;
                    crate::assert_no_allocs("fractional f64 clock", || {
                        count = if runner == 1 {
                            host.process_f64(&input, &mut output).unwrap()
                        } else {
                            host.process_f64_graph_native(&input, &mut output, position as u64)
                                .unwrap()
                        };
                    });
                    actual.extend_from_slice(&output[..count]);
                }
                position += frames;
                assert!(
                    valid.load(Ordering::Relaxed),
                    "batch={batch}, runner={runner}, position={position}"
                );
            }
            let mut tail = vec![0.0; host.drain_output_frames_max()];
            for _ in 0..3 {
                let mut result = PluginDrainResult::COMPLETE;
                crate::assert_no_allocs("fractional clock drain", || {
                    result = host.drain(&mut tail).unwrap();
                });
                actual.extend(tail[..result.frames].iter().map(|v| *v as f64));
                if result.complete {
                    break;
                }
            }
            let expected: Vec<_> = (0..position * 3)
                .step_by(2)
                .map(|i| (i / 3) as f64)
                .collect();
            assert_eq!(actual, expected, "batch={batch}, runner={runner}");
            assert!(
                valid.load(Ordering::Relaxed),
                "drain batch={batch}, runner={runner}"
            );
        }
    }
}

#[test]
fn fractional_round_trip_latency_rounds_only_in_the_destination_clock() {
    for middle_delay in [0, 1, 7] {
        let valid = Arc::new(AtomicBool::new(true));
        let mut host = DawHost::new(1, 48000);
        for (up, down, delay) in [(1, 1, 1), (147, 160, middle_delay), (160, 147, 0)] {
            host.add_plugin(Box::new(ClockPlugin::new(up, down, delay, &valid)))
                .unwrap();
        }
        host.build().unwrap();
        let expected = 1 + (middle_delay * 160).div_ceil(147);
        assert_eq!(host.total_latency_samples(), expected);
        assert_eq!(host.node_latency_from_input[2], expected);
    }
}

#[test]
fn nonlinear_graph_drain_delivers_joined_mixed_rate_tail() {
    // The joined mixed-rate graph used to refuse EOS; the branched
    // scheduler now drains it. Process plus drain must equal the ideal
    // delayed stream with exact length and valid clocks.
    for join in [false, true] {
        let (mut host, valid) = graph(join);
        let delay = host.total_latency_samples();
        let input: Vec<f32> = (0..256).map(|frame| frame as f32 + 1.0).collect();
        let mut output = vec![0.0; 256];
        assert_eq!(host.process(&input, &mut output).unwrap(), 256);
        let mut tail = Vec::new();
        for _ in 0..64 {
            let mut chunk = vec![f32::NAN; host.drain_output_frames_max()];
            let result = host.drain(&mut chunk).unwrap();
            assert!(result.frames <= host.drain_output_frames_max());
            tail.extend_from_slice(&chunk[..result.frames]);
            if result.complete {
                break;
            }
        }
        assert_eq!(tail.len(), delay, "join={join}");
        output.extend_from_slice(&tail);
        for (index, &sample) in output.iter().enumerate() {
            let expected = if index < delay {
                0.0
            } else {
                2.0 * (index - delay) as f64 + 2.0
            };
            assert!(
                (sample as f64 - expected).abs() <= 1e-9,
                "join={join} sample {index}: {sample} vs {expected}"
            );
        }
        assert!(valid.load(Ordering::Relaxed), "join={join}");
    }
}

#[test]
fn fractional_merge_carries_actual_integer_compensation_into_the_next_clock() {
    let valid = Arc::new(AtomicBool::new(true));
    let mut host = DawHost::new(1, 48000);
    let mut add = |up, down, delay| {
        host.add_node(
            "latency oracle".into(),
            Box::new(ClockPlugin::new(up, down, delay, &valid)),
        )
        .unwrap()
    };
    let delayed = add(1, 1, 1);
    let slow = add(147, 160, 0);
    let fast = add(147, 160, 0);
    let merge = add(1, 1, 0);
    let terminal = add(160, 147, 0);
    for (from, to) in [
        (delayed, slow),
        (slow, merge),
        (fast, merge),
        (merge, terminal),
    ] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    host.build().unwrap();
    // The fast branch receives one whole 44.1 kHz compensation frame.
    // Its physical delay is 160/147 host frames, requiring a report of two.
    assert_eq!(host.node_latency_from_input[merge], 1);
    assert_eq!(host.node_latency_from_input[terminal], 2);
    assert_eq!(host.total_latency_samples(), 2);
}

#[test]
fn fractional_clock_seek_and_reset_anchor_the_new_transport_position() {
    for native in [false, true] {
        let valid = Arc::new(AtomicBool::new(true));
        let source = ClockPlugin::new(3, 2, 0, &valid);
        let source_origin = source.position_origin.clone();
        let sink = ClockPlugin::new(1, 1, 0, &valid);
        let sink_origin = sink.position_origin.clone();
        let mut host = DawHost::new(1, 48000);
        host.add_plugin(Box::new(source)).unwrap();
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();
        for position in [0, 101, 0] {
            host.reset();
            if position == 0 {
                host.reset_playback_position().unwrap();
            } else {
                host.set_playback_position(position).unwrap();
            }
            source_origin.store(position, Ordering::Relaxed);
            sink_origin.store(position * 3 / 2, Ordering::Relaxed);
            for offset in 0..3 {
                host.set_playback_position(position + offset).unwrap();
                let count = if native {
                    host.process_f64(&[0.25], &mut [0.0; 2]).unwrap()
                } else {
                    host.process(&[0.25], &mut [0.0; 2]).unwrap()
                };
                assert_eq!(count, if offset % 2 == 0 { 2 } else { 1 });
            }
            assert!(
                valid.load(Ordering::Relaxed),
                "native={native}, seek={position}"
            );
        }
    }
}
