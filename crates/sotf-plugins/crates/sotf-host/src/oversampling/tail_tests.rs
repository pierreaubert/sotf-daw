use super::{AutoOversampledPlugin, OversampledPlugin};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    InPlacePlugin, InPlacePluginAdapter, Plugin, PluginInfo, ProcessContext, TailLength,
};

struct FiniteDelay {
    channels: usize,
    delay: usize,
    ring: Vec<f32>,
    cursor: usize,
    declared: TailLength,
}

impl FiniteDelay {
    fn new(channels: usize, delay: usize) -> Self {
        Self {
            channels,
            delay,
            ring: vec![0.0; channels * delay],
            cursor: 0,
            declared: TailLength::Finite(delay as u64),
        }
    }
}

impl InPlacePlugin for FiniteDelay {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Finite delay", "1", "test")
    }
    fn channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("no parameters".into())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn tail_length(&self) -> TailLength {
        self.declared
    }
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.cursor = 0;
    }
    fn process_in_place(
        &mut self,
        buffer: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if self.delay > 0 {
            for sample in buffer {
                std::mem::swap(sample, &mut self.ring[self.cursor]);
                self.cursor = (self.cursor + 1) % self.ring.len();
            }
        }
        Ok(context.num_frames)
    }
}

#[test]
fn native_streaming_tail_bound_covers_every_residual_phase_and_remains_resumable() {
    let channels = 2;
    for factor in [2, 4] {
        for inner_delay in [0, 1, 256 * factor as usize + 13] {
            for dynamic in [false, true] {
                let inner = FiniteDelay::new(channels, inner_delay);
                let mut plugin: Box<dyn Plugin> = if dynamic {
                    Box::new(
                        AutoOversampledPlugin::new_with_max_frames(
                            Box::new(InPlacePluginAdapter::new(inner)),
                            factor,
                            512,
                        )
                        .unwrap(),
                    )
                } else {
                    Box::new(InPlacePluginAdapter::new(
                        OversampledPlugin::new_with_max_frames(inner, factor, channels, 512)
                            .unwrap(),
                    ))
                };
                plugin.initialize(48_000).unwrap();
                let TailLength::Finite(bound) = plugin.tail_length() else {
                    panic!("finite inner must remain finite");
                };
                assert!(bound >= plugin.latency_samples() as u64);
                for phase in 0..256 {
                    plugin.reset();
                    let source_frames = 257 + phase;
                    let total = source_frames + bound as usize + 1024;
                    let mut cursor = 0;
                    let mut peak = 0.0_f32;
                    let partitions = if phase % 2 == 0 {
                        [1, 17, 127, 512]
                    } else {
                        [256, 31, 3, 97]
                    };
                    let mut block = 0;
                    while cursor < total {
                        let frames = partitions[block % partitions.len()].min(total - cursor);
                        let mut input = vec![0.0; frames * channels];
                        let mut output = vec![f32::NAN; input.len()];
                        if (cursor..cursor + frames).contains(&(source_frames - 1)) {
                            let index = (source_frames - 1 - cursor) * channels;
                            input[index] = 0.5;
                            input[index + 1] = -0.25;
                        }
                        let mut context = ProcessContext::new(48_000, frames);
                        context.transport.sample_position = cursor as u64;
                        assert_eq!(
                            plugin.process(&input, &mut output, &context).unwrap(),
                            frames
                        );
                        for (frame, samples) in output.as_chunks::<2>().0.iter().enumerate() {
                            assert!(samples.iter().all(|sample| sample.is_finite()));
                            peak = peak.max(samples[0].abs());
                            assert!((samples[0] + 2.0 * samples[1]).abs() < 2e-6);
                            if cursor + frame >= source_frames + bound as usize {
                                assert!(
                                    samples.iter().all(|sample| sample.abs() < 2e-6),
                                    "factor {factor}, delay {inner_delay}, phase {phase}, dynamic {dynamic}: output beyond {bound}"
                                );
                            }
                        }
                        assert_eq!(plugin.tail_length(), TailLength::Finite(bound));
                        cursor += frames;
                        block += 1;
                    }
                    assert!(peak > 0.1, "reference impulse must emerge");
                    // A tail query/zero-input continuation never enters EOS drain.
                    let mut output = [0.0; 34];
                    assert_eq!(
                        plugin
                            .process(&[0.1; 34], &mut output, &ProcessContext::new(48_000, 17))
                            .unwrap(),
                        17
                    );
                }
            }
        }
    }
}

#[test]
fn tail_queries_preserve_unknown_and_infinite_without_allocating() {
    for tail in [
        TailLength::Unknown,
        TailLength::Infinite,
        TailLength::Finite(13),
    ] {
        let mut inner = FiniteDelay::new(1, 0);
        inner.declared = tail;
        let mut plugin =
            AutoOversampledPlugin::new(Box::new(InPlacePluginAdapter::new(inner)), 4).unwrap();
        plugin.initialize(48_000).unwrap();
        let expected = match tail {
            TailLength::Finite(_) => TailLength::Finite(1280),
            other => other,
        };
        std::thread::spawn(move || {
            crate::test_utils::assert_no_allocs("oversampling tail query", || {
                for _ in 0..128 {
                    assert_eq!(plugin.tail_length(), expected);
                }
            });
        })
        .join()
        .unwrap();
    }
}
