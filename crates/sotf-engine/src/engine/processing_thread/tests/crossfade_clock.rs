//! Independent clock oracles for live host transitions.

use super::{ProcessingState, handle_processing_command, request};
use crate::engine::{PreparedHostUpdate, ProcessingCommand};
use sotf_plugins::{
    Parameter, ParameterId, ParameterValue, Plugin, PluginHost, PluginInfo, ProcessContext,
};

/// A constant signal generator with rational frame accounting. Optional
/// chunking separates accepted input time from the emitted audio timeline.
struct ClockSignal {
    input_rate: u32,
    output_rate: u32,
    value: f32,
    chunk: usize,
    accepted: usize,
    emitted: usize,
}

impl Plugin for ClockSignal {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Clock signal", "1", "Test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }
    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("No parameters".to_owned())
    }
    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }
    fn reset(&mut self) {
        self.accepted = 0;
        self.emitted = 0;
    }
    fn output_sample_rate(&self, _: u32) -> u32 {
        self.output_rate
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        ((input_frames + self.chunk) * self.output_rate as usize).div_ceil(self.input_rate as usize)
    }
    fn process(
        &mut self,
        _: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        assert_eq!(context.sample_rate, self.input_rate);
        self.accepted += context.num_frames;
        let ready = self.accepted / self.chunk * self.chunk;
        let target = ready * self.output_rate as usize / self.input_rate as usize;
        let count = target - self.emitted;
        output[..count].fill(self.value);
        self.emitted = target;
        Ok(count)
    }
}

fn signal_host(input_rate: u32, output_rate: u32, value: f32, chunk: usize) -> PluginHost {
    let mut host = PluginHost::new(1, input_rate);
    host.add_plugin(Box::new(ClockSignal {
        input_rate,
        output_rate,
        value,
        chunk,
        accepted: 0,
        emitted: 0,
    }))
    .unwrap();
    host.build().unwrap();
    host
}

fn start_transition(state: &mut ProcessingState, output_rate: u32, chunk: usize) {
    let host = signal_host(state.sample_rate, output_rate, 1.0, chunk);
    let prepared = PreparedHostUpdate::prepare(
        host,
        state.sample_rate,
        1,
        state.host.total_latency_samples(),
    )
    .unwrap();
    let (response_tx, _response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(4);
    assert!(
        !handle_processing_command(
            request(ProcessingCommand::CommitHostUpdate(prepared)),
            state,
            &response_tx,
            &event_tx,
        )
        .is_shutdown()
    );
    assert!(state.prev_host.is_some());
}

fn render_transition(
    input_rate: u32,
    output_rate: u32,
    chunk: usize,
    partition: &[usize],
) -> Vec<f32> {
    let mut state = ProcessingState::new(
        1,
        input_rate,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = signal_host(input_rate, input_rate, 0.0, 1);
    start_transition(&mut state, output_rate, chunk);
    let fade_frames = (u64::from(output_rate) * 50).div_ceil(1_000) as usize;
    let mut rendered = Vec::new();
    let mut block = 0;
    while rendered.len() <= fade_frames + 16 {
        let count = partition[block % partition.len()];
        let input = vec![0.0; count];
        let mut output = vec![f32::NAN; state.output_frames_for_input(count)];
        let actual = state.process_frame(&input, &mut output, count).unwrap();
        rendered.extend_from_slice(&output[..actual]);
        assert_eq!(
            state.prev_host.is_some(),
            rendered.len() < fade_frames,
            "retirement at {} emitted frames, rate {output_rate}, partition {partition:?}",
            rendered.len(),
        );
        block += 1;
        assert!(block < 100_000);
    }
    for (frame, &actual) in rendered.iter().enumerate() {
        let elapsed_seconds = frame as f64 / f64::from(output_rate);
        let phase = (elapsed_seconds / 0.050).min(1.0);
        let expected = if input_rate == output_rate {
            phase
        } else {
            (2.0 * phase - 1.0).max(0.0)
        };
        assert!(
            (f64::from(actual) - expected).abs() < 2e-7,
            "frame={frame}, rate={input_rate}->{output_rate}, chunk={chunk}, partition={partition:?}: {actual} != {expected}",
        );
    }
    rendered.truncate(fade_frames + 1);
    assert_eq!(*rendered.last().unwrap(), 1.0);
    rendered
}

#[test]
fn crossfade_is_fifty_milliseconds_for_irregular_and_buffered_callbacks() {
    for (input_rate, output_rate) in [
        (44_100, 44_100),
        (48_000, 48_000),
        (96_000, 96_000),
        (48_000, 96_000),
        (96_000, 48_000),
        (44_100, 48_000),
        (48_000, 44_100),
    ] {
        for chunk in [1, 37] {
            // Same-rate DSP promises full blocks; the host pads a short
            // same-rate result. Exercise non-emitting chunk buffering only
            // where the actual output clock differs from the input clock.
            if chunk > 1 && input_rate == output_rate {
                continue;
            }
            let reference = render_transition(input_rate, output_rate, chunk, &[1]);
            for partition in [&[127][..], &[513], &[128, 1], &[0, 257, 3, 127, 0, 1, 513]] {
                let actual = render_transition(input_rate, output_rate, chunk, partition);
                assert_eq!(
                    actual, reference,
                    "output gain timeline depends on callback partition"
                );
            }
        }
    }
}

#[test]
fn crossfade_new_update_restarts_clock_and_empty_callbacks_preserve_it() {
    let mut state = ProcessingState::new(
        1,
        48_000,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = signal_host(48_000, 48_000, 0.0, 1);
    start_transition(&mut state, 48_000, 1);
    assert_eq!(state.process_frame(&[], &mut [], 0).unwrap(), 0);
    assert!(state.prev_host.is_some());
    assert_eq!(state.crossfade_progress, 0.0);
    let mut output = [0.0; 600];
    state.process_frame(&[0.0; 600], &mut output, 600).unwrap();
    assert_eq!(state.crossfade_progress, 0.25);
    start_transition(&mut state, 48_000, 1);
    assert_eq!(state.crossfade_progress, 0.0);
    assert_eq!(state.process_frame(&[], &mut [], 0).unwrap(), 0);
    assert_eq!(state.crossfade_progress, 0.0);
    state.process_frame(&[0.0; 600], &mut output, 600).unwrap();
    assert_eq!(state.crossfade_progress, 0.25);
    let (response_tx, _response_rx) = std::sync::mpsc::channel();
    let (event_tx, _event_rx) = crossbeam::channel::bounded(4);
    handle_processing_command(
        request(ProcessingCommand::Stop),
        &mut state,
        &response_tx,
        &event_tx,
    );
    assert!(state.prev_host.is_some());
    assert_eq!(state.crossfade_progress, 0.0);
    assert_eq!(state.process_frame(&[], &mut [], 0).unwrap(), 0);
    state.process_frame(&[0.0; 600], &mut output, 600).unwrap();
    assert_eq!(state.crossfade_progress, 0.25);
}
