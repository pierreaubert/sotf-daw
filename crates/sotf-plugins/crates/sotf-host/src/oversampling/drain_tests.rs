use super::{AutoOversampledPlugin, OversampledPlugin};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    InPlacePlugin, InPlacePluginAdapter, Plugin, PluginDrainResult, PluginInfo, ProcessContext,
};
use std::num::NonZeroU64;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct FiniteEcho {
    channels: usize,
    delay: Vec<f32>,
    cursor: usize,
    tail_frames: usize,
    remaining: usize,
    waiting: bool,
    initialized: bool,
    next_position: Option<u64>,
    declared_max: Option<usize>,
    begins: Arc<AtomicUsize>,
    processes: Arc<AtomicUsize>,
    fail_process: Option<usize>,
}

impl FiniteEcho {
    fn new(channels: usize, tail_frames: usize) -> Self {
        Self {
            channels,
            delay: vec![0.0; channels * tail_frames],
            cursor: 0,
            tail_frames,
            remaining: 0,
            waiting: true,
            initialized: false,
            next_position: None,
            declared_max: None,
            begins: Arc::default(),
            processes: Arc::default(),
            fail_process: None,
        }
    }

    fn tick(&mut self, sample: f32) -> f32 {
        if self.delay.is_empty() {
            return sample;
        }
        let output = sample + 0.3 * self.delay[self.cursor];
        self.delay[self.cursor] = sample;
        self.cursor = (self.cursor + 1) % self.delay.len();
        output
    }
}

impl Plugin for FiniteEcho {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Finite echo", "1.0", "Test")
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
        Ok(())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.cursor = 0;
        self.remaining = 0;
        self.waiting = true;
        self.next_position = None;
    }
    fn initialize(&mut self, _: f64) -> Result<(), String> {
        self.initialized = true;
        Ok(())
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let call = self.processes.fetch_add(1, Ordering::Relaxed) + 1;
        if self.fail_process == Some(call) {
            return Err("deliberate setup failure".into());
        }
        for (dst, &src) in output.iter_mut().zip(input) {
            *dst = self.tick(src);
        }
        self.remaining = self.tail_frames;
        self.waiting = true;
        self.next_position = Some(context.transport.sample_position + context.num_frames as u64);
        Ok(context.num_frames)
    }
    fn begin_drain(&mut self, _: &ProcessContext) -> Result<(), String> {
        self.begins.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        if self.remaining == 0 {
            return NonZeroU64::new(2);
        }
        let blocks = self
            .remaining
            .div_ceil(self.drain_output_frames_max().max(1));
        NonZeroU64::new((2 * blocks - usize::from(!self.waiting)) as u64)
    }
    fn drain_output_frames_max(&self) -> usize {
        if self.initialized {
            self.declared_max.unwrap_or(self.tail_frames.min(2053))
        } else {
            0
        }
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        assert_eq!(Some(context.transport.sample_position), self.next_position);
        if self.waiting {
            self.waiting = false;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }
        let frames = self.remaining.min(self.drain_output_frames_max());
        assert!(output.len() >= frames * self.channels);
        for sample in &mut output[..frames * self.channels] {
            *sample = self.tick(0.0);
        }
        self.remaining -= frames;
        self.waiting = true;
        Ok(PluginDrainResult {
            frames,
            complete: self.remaining == 0,
        })
    }
}

struct InPlaceEcho(FiniteEcho);

impl InPlacePlugin for InPlaceEcho {
    fn info(&self) -> PluginInfo {
        self.0.info()
    }
    fn channels(&self) -> usize {
        self.0.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        self.0.parameters()
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        self.0.set_parameter(id, value)
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.0.get_parameter(id)
    }
    fn initialize(&mut self, rate: f64) -> Result<(), String> {
        self.0.initialize(rate)
    }
    fn reset(&mut self) {
        self.0.reset();
    }
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for sample in buffer {
            *sample = self.0.tick(*sample);
        }
        self.0.remaining = self.0.tail_frames;
        self.0.waiting = true;
        self.0.next_position = Some(context.transport.sample_position + context.num_frames as u64);
        Ok(context.num_frames)
    }
    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        self.0.begin_drain(context)
    }
    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        self.0.drain_call_bound()
    }
    fn drain_output_frames_max(&self) -> usize {
        self.0.drain_output_frames_max()
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.0.drain(output, context)
    }
}

fn wrapper(factor: u32, channels: usize, tail: usize, dynamic: bool) -> Box<dyn Plugin> {
    let mut wrapper: Box<dyn Plugin> = if dynamic {
        Box::new(
            AutoOversampledPlugin::new_with_max_frames(
                Box::new(FiniteEcho::new(channels, tail)),
                factor,
                513,
            )
            .unwrap(),
        )
    } else {
        Box::new(InPlacePluginAdapter::new(
            OversampledPlugin::new_with_max_frames(
                InPlaceEcho(FiniteEcho::new(channels, tail)),
                factor,
                channels,
                513,
            )
            .unwrap(),
        ))
    };
    wrapper.initialize(48_000.0).unwrap();
    wrapper
}

#[test]
fn oversampling_drain_matches_zero_padded_finite_stream_reference() {
    for factor in [2, 4] {
        for channels in [1, 2, 6] {
            for tail in [0, 13, 1701, 4109] {
                for programme_frames in [1_usize, 255, 256, 257, 1027] {
                    let mut reference = wrapper(factor, channels, tail, true);
                    let mut input = vec![0.0; (programme_frames + 4096) * channels];
                    for (i, sample) in input[..programme_frames * channels].iter_mut().enumerate() {
                        *sample = (i as f32 * 0.153).sin() * 0.1;
                    }
                    // The last programme frame must survive even when no hop is complete.
                    for ch in 0..channels {
                        input[(programme_frames - 1) * channels + ch] = 0.5 - ch as f32 * 0.03;
                    }
                    let mut expected = vec![0.0; input.len()];
                    for (block, (src, dst)) in input
                        .chunks(256 * channels)
                        .zip(expected.chunks_mut(256 * channels))
                        .enumerate()
                    {
                        reference
                            .process(
                                src,
                                dst,
                                &ProcessContext::new(48_000, src.len() / channels)
                                    .with_sample_position((block * 256) as u64),
                            )
                            .unwrap();
                    }
                    for drain_capacity in [1, 17, 256, 513] {
                        for dynamic in [false, true] {
                            let mut candidate = wrapper(factor, channels, tail, dynamic);
                            let mut rendered = vec![0.0; programme_frames * channels];
                            for (block, (src, dst)) in input[..programme_frames * channels]
                                .chunks(137 * channels)
                                .zip(rendered.chunks_mut(137 * channels))
                                .enumerate()
                            {
                                candidate
                                    .process(
                                        src,
                                        dst,
                                        &ProcessContext::new(48_000, src.len() / channels)
                                            .with_sample_position((block * 137) as u64),
                                    )
                                    .unwrap();
                            }
                            let mut output = vec![0.0; drain_capacity * channels];
                            let mut complete = false;
                            for _ in 0..4096 {
                                let mut result = PluginDrainResult::COMPLETE;
                                crate::assert_no_allocs("oversampling drain", || {
                                    result = candidate
                                        .drain(&mut output, &ProcessContext::new(48_000, 0))
                                        .unwrap();
                                });
                                assert!(
                                    result.frames
                                        <= drain_capacity.min(candidate.drain_output_frames_max())
                                );
                                rendered.extend_from_slice(&output[..result.frames * channels]);
                                if result.complete {
                                    complete = true;
                                    break;
                                }
                            }
                            assert!(complete, "drain failed to terminate");
                            let expected_frames = 256
                                * (programme_frames.div_ceil(256)
                                    + tail.div_ceil(256 * factor as usize)
                                    + 3);
                            assert_eq!(
                                rendered.len(),
                                expected_frames * channels,
                                "deterministic padded length"
                            );
                            let label = format!(
                                "factor={factor}, channels={channels}, tail={tail}, programme={programme_frames}, capacity={drain_capacity}"
                            );
                            let peak_error = rendered
                                .iter()
                                .zip(&expected)
                                .map(|(&a, &b)| (a - b).abs())
                                .fold(0.0_f32, f32::max);
                            assert!(peak_error < 2e-6, "{label}: peak error {peak_error}");
                            let omitted_peak = expected[rendered.len()..]
                                .iter()
                                .map(|x| x.abs())
                                .fold(0.0_f32, f32::max);
                            assert!(omitted_peak < 2e-6, "{label}: omitted peak {omitted_peak}");
                            assert_eq!(
                                candidate
                                    .drain(&mut output, &ProcessContext::new(48_000, 0))
                                    .unwrap(),
                                PluginDrainResult::COMPLETE
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn drain_capacity_failures_preserve_state_and_partial_drain_reset_matches_fresh() {
    for factor in [2, 4] {
        for dynamic in [false, true] {
            let mut candidate = wrapper(factor, 2, 4109, dynamic);
            let mut reference = wrapper(factor, 2, 4109, dynamic);
            let empty = ProcessContext::new(48_000, 0);
            assert_eq!(
                candidate.drain(&mut [], &empty).unwrap(),
                PluginDrainResult::COMPLETE
            );
            candidate.process(&[], &mut [], &empty).unwrap();
            assert_eq!(
                candidate.drain(&mut [], &empty).unwrap(),
                PluginDrainResult::COMPLETE
            );
            for plugin in [&mut candidate, &mut reference] {
                plugin
                    .process(
                        &[0.7, -0.4],
                        &mut [0.0; 2],
                        &ProcessContext::new(48_000, 1).with_sample_position(9000),
                    )
                    .unwrap();
            }
            assert!(candidate.drain(&mut [], &empty).is_err());
            let mut unaligned = [9.0; 3];
            assert!(candidate.drain(&mut unaligned, &empty).is_err());
            assert_eq!(unaligned, [9.0; 3]);
            // Rejected capacity did not start EOS: new input is still accepted.
            for plugin in [&mut candidate, &mut reference] {
                plugin
                    .process(
                        &[0.1; 254],
                        &mut [0.0; 254],
                        &ProcessContext::new(48_000, 127).with_sample_position(9001),
                    )
                    .unwrap();
            }
            let mut actual = [0.0; 34];
            let mut expected = [0.0; 34];
            for _ in 0..31 {
                let a = candidate.drain(&mut actual, &empty).unwrap();
                let b = reference.drain(&mut expected, &empty).unwrap();
                assert_eq!(a, b);
                assert_eq!(&actual[..a.frames * 2], &expected[..b.frames * 2]);
            }
            crate::assert_no_allocs("oversampling reset during drain", || candidate.reset());
            let mut fresh = wrapper(factor, 2, 4109, dynamic);
            let input = [0.15; 514];
            let mut a = [0.0; 514];
            let mut b = [0.0; 514];
            let context = ProcessContext::new(48_000, 257).with_sample_position(31_337);
            candidate.process(&input, &mut a, &context).unwrap();
            fresh.process(&input, &mut b, &context).unwrap();
            assert_eq!(a, b);
            let mut complete = false;
            for _ in 0..1024 {
                let mut result = PluginDrainResult::COMPLETE;
                crate::assert_no_allocs("reset oversampling drain", || {
                    result = candidate.drain(&mut actual, &empty).unwrap();
                });
                let expected_result = fresh.drain(&mut expected, &empty).unwrap();
                assert_eq!(result, expected_result);
                assert_eq!(&actual[..result.frames * 2], &expected[..result.frames * 2]);
                if result.complete {
                    complete = true;
                    break;
                }
            }
            assert!(complete);
            assert!(candidate.process(&input, &mut a, &context).is_err());
        }
    }
}

#[test]
fn failed_inner_drain_requires_reset_and_never_publishes_unprocessed_audio() {
    use super::Oversampler;
    for factor in [2, 4] {
        let mut oversampler = Oversampler::new(factor, 1).unwrap();
        oversampler.process(&mut [1.0], 1, |_, _| {}).unwrap();
        let mut output = [8.0; 256];
        let initial = oversampler
            .drain_with(&mut output, |_, _| unreachable!())
            .unwrap();
        assert_eq!(initial.frames, 255);
        output.fill(8.0);
        assert!(
            oversampler
                .drain_with(&mut output, |_, _| Err("injected inner error".into()))
                .is_err()
        );
        assert_eq!(output, [8.0; 256]);
        assert!(
            oversampler
                .drain_with(&mut output, |_, _| unreachable!())
                .is_err()
        );
        assert_eq!(output, [8.0; 256]);
        assert!(oversampler.process(&mut [1.0], 1, |_, _| {}).is_err());
        oversampler.reset();
        oversampler.process(&mut [1.0], 1, |_, _| {}).unwrap();
        let mut complete = false;
        for _ in 0..20 {
            let result = oversampler
                .drain_with(&mut output, |_, frames| {
                    Ok(match frames {
                        Some(frames) => PluginDrainResult {
                            frames,
                            complete: false,
                        },
                        None => PluginDrainResult::COMPLETE,
                    })
                })
                .unwrap();
            if result.complete {
                complete = true;
                break;
            }
        }
        assert!(complete);
    }
}

#[test]
fn overflowing_inner_drain_bound_is_rejected_before_planar_allocation() {
    let mut echo = FiniteEcho::new(2, 0);
    echo.declared_max = Some(isize::MAX as usize / std::mem::size_of::<f32>() / 2 + 1);
    let mut dynamic = AutoOversampledPlugin::new(Box::new(echo), 2).unwrap();
    assert!(
        dynamic
            .initialize(48_000.0)
            .unwrap_err()
            .contains("capacity overflow")
    );
    let mut echo = FiniteEcho::new(2, 0);
    echo.declared_max = Some(usize::MAX);
    let mut generic = OversampledPlugin::new(InPlaceEcho(echo), 4, 2).unwrap();
    assert!(
        generic
            .initialize(48_000.0)
            .unwrap_err()
            .contains("capacity overflow")
    );
}

#[test]
fn host_forwards_oversampled_finite_tail_and_zero_frame_progress() {
    use crate::host::DawHost;
    for dynamic in [false, true] {
        let mut host = DawHost::new(2, 48_000);
        host.add_plugin(wrapper(4, 2, 4109, dynamic)).unwrap();
        host.build().unwrap();
        let mut direct = wrapper(4, 2, 4109, dynamic);
        let input = [0.7, -0.3];
        let mut actual = [0.0; 2];
        let mut expected = [0.0; 2];
        host.process(&input, &mut actual).unwrap();
        direct
            .process(&input, &mut expected, &ProcessContext::new(48_000, 1))
            .unwrap();
        assert_eq!(actual, expected);
        let mut host_tail = Vec::new();
        let mut direct_tail = Vec::new();
        let mut output = [0.0; 512];
        for _ in 0..128 {
            let mut result = PluginDrainResult::COMPLETE;
            crate::assert_no_allocs("host oversampled drain", || {
                result = host.drain(&mut output).unwrap();
            });
            host_tail.extend_from_slice(&output[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        for _ in 0..128 {
            let result = direct
                .drain(&mut output, &ProcessContext::new(48_000, 0))
                .unwrap();
            direct_tail.extend_from_slice(&output[..result.frames * 2]);
            if result.complete {
                break;
            }
        }
        assert_eq!(host_tail, direct_tail);
        assert_eq!(
            host_tail.len() / 2 + 1,
            256 * (1 + 4109_usize.div_ceil(1024) + 3)
        );
    }
}

#[test]
fn prepared_bounds_cover_all_residual_phases_and_partially_served_child_blocks() {
    let context = ProcessContext::new(48_000, 0);
    for dynamic in [false, true] {
        for factor in [2, 4] {
            for phase in 0..256 {
                let mut plugin = wrapper(factor, 1, 4109, dynamic);
                let frames = 256 + phase;
                let mut output = vec![0.0; frames];
                plugin
                    .process(
                        &vec![0.2; frames],
                        &mut output,
                        &ProcessContext::new(48_000, frames),
                    )
                    .unwrap();
                assert!(plugin.drain_call_bound().is_none());
                plugin.begin_drain(&context).unwrap();
                let initial = plugin.drain_call_bound().unwrap().get();
                plugin.begin_drain(&context).unwrap();
                assert_eq!(plugin.drain_call_bound().unwrap().get(), initial);
                // A tiny prior destination leaves unread wrapper/child cache.
                plugin.drain(&mut [0.0], &context).unwrap();
                let bound = plugin.drain_call_bound().unwrap().get();
                let mut complete = false;
                for _ in 0..bound {
                    if plugin.drain(&mut [0.0; 256], &context).unwrap().complete {
                        complete = true;
                        break;
                    }
                }
                assert!(complete, "dynamic={dynamic} factor={factor} phase={phase}");
                assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
                assert!(plugin.drain(&mut [], &context).unwrap().complete);
            }
        }
    }
}

#[test]
fn begin_is_idempotent_and_capacity_errors_precede_setup() {
    let inner = FiniteEcho::new(2, 13);
    let begins = Arc::clone(&inner.begins);
    let processes = Arc::clone(&inner.processes);
    let mut plugin = AutoOversampledPlugin::new(Box::new(inner), 2).unwrap();
    plugin.initialize(48_000.0).unwrap();
    plugin
        .process(&[0.2; 2], &mut [0.0; 2], &ProcessContext::new(48_000, 1))
        .unwrap();
    let context = ProcessContext::new(48_000, 0);
    assert!(plugin.drain(&mut [0.0], &context).is_err());
    assert!(plugin.begin_drain(&ProcessContext::new(44_100, 0)).is_err());
    assert_eq!(processes.load(Ordering::Relaxed), 0);
    plugin.begin_drain(&context).unwrap();
    assert_eq!(processes.load(Ordering::Relaxed), 2);
    assert_eq!(begins.load(Ordering::Relaxed), 1);
    plugin.begin_drain(&context).unwrap();
    plugin.drain(&mut [0.0; 512], &context).unwrap();
    assert_eq!(processes.load(Ordering::Relaxed), 2);
    assert_eq!(begins.load(Ordering::Relaxed), 1);
}

#[test]
fn partially_failed_setup_is_terminal_until_reset_and_never_replays_audio() {
    let mut inner = FiniteEcho::new(1, 13);
    inner.fail_process = Some(2);
    let processes = Arc::clone(&inner.processes);
    let mut plugin = AutoOversampledPlugin::new(Box::new(inner), 2).unwrap();
    plugin.initialize(48_000.0).unwrap();
    plugin
        .process(&[0.2], &mut [0.0], &ProcessContext::new(48_000, 1))
        .unwrap();
    let context = ProcessContext::new(48_000, 0);
    assert!(
        plugin
            .begin_drain(&context)
            .unwrap_err()
            .contains("deliberate")
    );
    assert_eq!(processes.load(Ordering::Relaxed), 2);
    assert!(plugin.begin_drain(&context).unwrap_err().contains("reset"));
    assert!(
        plugin
            .drain(&mut [0.0; 256], &context)
            .unwrap_err()
            .contains("reset")
    );
    assert_eq!(processes.load(Ordering::Relaxed), 2);
    plugin.reset();
    plugin
        .process(&[0.2], &mut [0.0], &ProcessContext::new(48_000, 1))
        .unwrap();
    plugin.begin_drain(&context).unwrap();
    assert_eq!(processes.load(Ordering::Relaxed), 4);
}

#[test]
fn nested_preparation_reaches_inner_after_both_resampling_filters() {
    let inner = FiniteEcho::new(1, 4109);
    let begins = Arc::clone(&inner.begins);
    let wrapped = AutoOversampledPlugin::new(Box::new(inner), 2).unwrap();
    let mut plugin = AutoOversampledPlugin::new(Box::new(wrapped), 2).unwrap();
    plugin.initialize(48_000.0).unwrap();
    plugin
        .process(&[0.25], &mut [0.0], &ProcessContext::new(48_000, 1))
        .unwrap();
    let context = ProcessContext::new(48_000, 0);
    plugin.begin_drain(&context).unwrap();
    let bound = plugin.drain_call_bound().unwrap().get();
    let mut complete = false;
    for _ in 0..bound {
        if plugin.drain(&mut [0.0; 256], &context).unwrap().complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(begins.load(Ordering::Relaxed), 1);
}
