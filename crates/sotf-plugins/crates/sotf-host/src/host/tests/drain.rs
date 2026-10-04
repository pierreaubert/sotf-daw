use super::super::daw_host::DawHost;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Calls {
    process: AtomicUsize,
    drain: AtomicUsize,
    process_rate: AtomicUsize,
    drain_rate: AtomicUsize,
}

struct TailPlugin {
    channels: usize,
    gain: f32,
    frame_ratio: usize,
    tail: Vec<f32>,
    cursor: usize,
    calls: Arc<Calls>,
}

impl TailPlugin {
    fn new(channels: usize, gain: f32, tail: Vec<f32>) -> Self {
        Self {
            channels,
            gain,
            frame_ratio: 1,
            tail,
            cursor: 0,
            calls: Arc::default(),
        }
    }
}

impl Plugin for TailPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Tail fixture", "0.1", "test")
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
        Err("tail fixture has no parameters".into())
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
        self.calls
            .process_rate
            .store(context.sample_rate as usize, Ordering::Relaxed);
        for (frame, output) in output.chunks_exact_mut(self.channels).enumerate() {
            let source = (frame / self.frame_ratio) * self.channels;
            for (channel, output) in output.iter_mut().enumerate() {
                *output = input[source + channel] * self.gain;
            }
        }
        Ok(context.num_frames * self.frame_ratio)
    }

    fn output_frames_for_input(&self, frames: usize) -> usize {
        frames * self.frame_ratio
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        input_rate * self.frame_ratio as f64
    }

    fn drain_output_frames_max(&self) -> usize {
        // Return two frames per call to exercise repeated drain steps.
        (self.tail.len() / self.channels).min(2)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        self.calls
            .drain_rate
            .store(context.sample_rate as usize, Ordering::Relaxed);
        let samples = (self.tail.len() - self.cursor).min(output.len());
        output[..samples].copy_from_slice(&self.tail[self.cursor..self.cursor + samples]);
        self.cursor += samples;
        Ok(PluginDrainResult {
            frames: samples / self.channels,
            complete: self.cursor == self.tail.len(),
        })
    }
}

#[test]
fn drain_suppresses_bypassed_upstream_tail() {
    let mut host = DawHost::new(1, 48_000);
    let mut upstream = TailPlugin::new(1, 2.0, vec![99.0; 8]);
    upstream.frame_ratio = 2;
    let upstream_calls = Arc::clone(&upstream.calls);
    let downstream = TailPlugin::new(1, 3.0, vec![0.25]);
    let downstream_calls = Arc::clone(&downstream.calls);
    host.add_plugin(Box::new(upstream)).unwrap();
    host.add_plugin(Box::new(downstream)).unwrap();
    host.bypass_plugin(0).unwrap();
    // Capacity queries also respect bypass before the graph is rebuilt.
    assert_eq!(host.drain_output_frames_max(), 1);

    let mut output = [f32::NAN; 1];
    let result = host.drain(&mut output).unwrap();
    assert_eq!(result.frames, 1);
    assert!(result.complete);
    assert_eq!(output, [0.25]);
    assert_eq!(upstream_calls.drain.load(Ordering::Relaxed), 0);
    assert_eq!(downstream_calls.process.load(Ordering::Relaxed), 0);
    assert_eq!(downstream_calls.drain_rate.load(Ordering::Relaxed), 48_000);
}

#[test]
fn drain_passes_tail_through_bypassed_downstream_unchanged() {
    let mut host = DawHost::new(2, 48_000);
    host.add_plugin(Box::new(TailPlugin::new(
        2,
        1.0,
        vec![0.25, -0.5, 0.75, -1.0, 0.125, -0.25],
    )))
    .unwrap();
    let mut downstream = TailPlugin::new(2, 8.0, vec![99.0; 16]);
    downstream.frame_ratio = 2;
    let calls = Arc::clone(&downstream.calls);
    host.add_plugin(Box::new(downstream)).unwrap();
    host.bypass_plugin(1).unwrap();
    assert_eq!(host.drain_output_frames_max(), 2);

    let mut output = [f32::NAN; 4];
    let first = host.drain(&mut output).unwrap();
    assert_eq!(first.frames, 2);
    assert!(!first.complete);
    assert_eq!(output, [0.25, -0.5, 0.75, -1.0]);
    let last = host.drain(&mut output).unwrap();
    assert_eq!(last.frames, 1);
    assert!(last.complete);
    assert_eq!(output[..2], [0.125, -0.25]);
    assert_eq!(calls.process.load(Ordering::Relaxed), 0);
    assert_eq!(calls.drain.load(Ordering::Relaxed), 0);
}

#[test]
fn drain_preserves_active_downstream_processing_after_bypassed_middle() {
    let mut host = DawHost::new(1, 48_000);
    host.add_plugin(Box::new(TailPlugin::new(1, 1.0, vec![0.25, -0.5])))
        .unwrap();
    let mut middle = TailPlugin::new(1, 8.0, vec![99.0; 8]);
    middle.frame_ratio = 2;
    let middle_calls = Arc::clone(&middle.calls);
    host.add_plugin(Box::new(middle)).unwrap();
    let last = TailPlugin::new(1, 2.0, vec![0.75]);
    let last_calls = Arc::clone(&last.calls);
    host.add_plugin(Box::new(last)).unwrap();
    host.bypass_plugin(1).unwrap();
    assert_eq!(host.drain_output_frames_max(), 2);

    let mut output = [f32::NAN; 2];
    let upstream_tail = host.drain(&mut output).unwrap();
    assert_eq!(upstream_tail.frames, 2);
    assert!(!upstream_tail.complete);
    assert_eq!(output, [0.5, -1.0]);
    let final_tail = host.drain(&mut output).unwrap();
    assert_eq!(final_tail.frames, 1);
    assert!(final_tail.complete);
    assert_eq!(output[0], 0.75);
    assert_eq!(middle_calls.process.load(Ordering::Relaxed), 0);
    assert_eq!(middle_calls.drain.load(Ordering::Relaxed), 0);
    assert_eq!(last_calls.process_rate.load(Ordering::Relaxed), 48_000);
    assert_eq!(last_calls.drain_rate.load(Ordering::Relaxed), 48_000);
}

#[test]
fn drain_all_bypassed_is_immediately_complete() {
    let mut host = DawHost::new(1, 48_000);
    let plugin = TailPlugin::new(1, 1.0, vec![99.0; 8]);
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.bypass_plugin(0).unwrap();
    assert_eq!(host.drain_output_frames_max(), 0);
    let result = host.drain(&mut []).unwrap();
    assert_eq!(result.frames, 0);
    assert!(result.complete);
    assert_eq!(calls.drain.load(Ordering::Relaxed), 0);
}

fn capacity_fixture(channels: usize, frame_ratio: usize) -> (DawHost, Arc<Calls>, Arc<Calls>) {
    let mut host = DawHost::new(channels, 48_000);
    let source = TailPlugin::new(
        channels,
        1.0,
        (1..=3 * channels).map(|sample| sample as f32).collect(),
    );
    let source_calls = Arc::clone(&source.calls);
    host.add_plugin(Box::new(source)).unwrap();
    let mut downstream = TailPlugin::new(channels, 0.5, Vec::new());
    downstream.frame_ratio = frame_ratio;
    let downstream_calls = Arc::clone(&downstream.calls);
    host.add_plugin(Box::new(downstream)).unwrap();
    host.build().unwrap();
    (host, source_calls, downstream_calls)
}

#[test]
fn drain_capacity_error_preserves_tail_for_full_capacity_retry() {
    for channels in [1, 2] {
        for ratio in [1, 2] {
            let (mut host, source_calls, downstream_calls) = capacity_fixture(channels, ratio);
            let (mut untouched, _, _) = capacity_fixture(channels, ratio);
            let samples = host.drain_output_frames_max() * channels;
            let mut too_short = vec![12345.0; samples - channels];
            assert!(host.drain(&mut too_short).is_err());
            assert_eq!(too_short, vec![12345.0; samples - channels]);
            assert_eq!(source_calls.drain.load(Ordering::Relaxed), 0);
            assert_eq!(downstream_calls.process.load(Ordering::Relaxed), 0);

            // Include extra whole frames: successful output must leave the
            // unused destination unchanged, including a short final tail.
            for _ in 0..3 {
                let mut actual = vec![12345.0; samples + channels];
                let mut expected = actual.clone();
                let result = host.drain(&mut actual).unwrap();
                let reference = untouched.drain(&mut expected).unwrap();
                assert_eq!(result.frames, reference.frames);
                assert_eq!(result.complete, reference.complete);
                assert_eq!(actual, expected);
                assert!(
                    actual[result.frames * channels..]
                        .iter()
                        .all(|&x| x == 12345.0)
                );
            }
        }
    }
}

#[test]
fn drain_rejects_partial_output_frames_before_dsp() {
    let (mut host, source_calls, downstream_calls) = capacity_fixture(2, 2);
    let mut output = vec![12345.0; host.drain_output_frames_max() * 2 + 1];
    assert!(host.drain(&mut output).is_err());
    assert!(output.iter().all(|&sample| sample == 12345.0));
    assert_eq!(source_calls.drain.load(Ordering::Relaxed), 0);
    assert_eq!(downstream_calls.process.load(Ordering::Relaxed), 0);
}

#[test]
fn drain_preflight_uses_capacity_after_queued_graph_adoption() {
    let mut host = DawHost::new(1, 48_000);
    let source = TailPlugin::new(1, 1.0, vec![1.0, 2.0]);
    let source_calls = Arc::clone(&source.calls);
    host.add_plugin(Box::new(source)).unwrap();
    host.build().unwrap();
    assert_eq!(host.drain_output_frames_max(), 2);
    let mut downstream = TailPlugin::new(1, 0.5, Vec::new());
    downstream.frame_ratio = 2;
    let downstream_calls = Arc::clone(&downstream.calls);
    host.queue_add_plugin(Box::new(downstream)).unwrap();

    assert!(host.drain(&mut [12345.0; 2]).is_err());
    assert_eq!(source_calls.drain.load(Ordering::Relaxed), 0);
    assert_eq!(downstream_calls.process.load(Ordering::Relaxed), 0);
    assert_eq!(host.drain_output_frames_max(), 4);
    let mut output = [12345.0; 4];
    assert_eq!(host.drain(&mut output).unwrap().frames, 4);
    assert_eq!(output, [0.5, 0.5, 1.0, 1.0]);
}

#[test]
fn drain_requires_advertised_capacity_even_for_short_final_tail() {
    let (mut host, source_calls, _) = capacity_fixture(1, 1);
    assert_eq!(host.drain(&mut [0.0; 2]).unwrap().frames, 2);
    let calls_before = source_calls.drain.load(Ordering::Relaxed);
    // Only one source frame remains, but this host contract requires its
    // conservative maximum rather than guessing the next actual frame count.
    assert!(host.drain(&mut [12345.0; 1]).is_err());
    assert_eq!(source_calls.drain.load(Ordering::Relaxed), calls_before);
    let mut output = [12345.0; 2];
    assert_eq!(host.drain(&mut output).unwrap().frames, 1);
    assert_eq!(output, [1.5, 12345.0]);
}

#[test]
fn drain_empty_graph_and_zero_capacity_complete_without_storage() {
    let mut empty = DawHost::new(2, 48_000);
    assert!(empty.drain(&mut []).unwrap().complete);
    let mut complete = DawHost::new(2, 48_000);
    complete
        .add_plugin(Box::new(TailPlugin::new(2, 1.0, Vec::new())))
        .unwrap();
    assert_eq!(complete.drain_output_frames_max(), 0);
    assert!(complete.drain(&mut []).unwrap().complete);
    assert!(complete.drain(&mut []).unwrap().complete);
}
