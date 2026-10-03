//! Every-sample boundedness oracles for hot-reload transitions.

use super::{ProcessingState, handle_processing_command, request};
use crate::engine::{PreparedHostUpdate, ProcessingCommand};
use sotf_plugins::{
    Parameter, ParameterId, ParameterValue, Plugin, PluginHost, PluginInfo, ProcessContext,
};

/// Scripted mono source: emits `values` cyclically, one per output frame.
struct ScriptSignal {
    values: Vec<f32>,
    emitted: usize,
}

impl Plugin for ScriptSignal {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Script signal", "1", "Test")
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
        self.emitted = 0;
    }
    fn output_sample_rate(&self, rate: u32) -> u32 {
        rate
    }
    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }
    fn process(
        &mut self,
        _: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        debug_assert!(!self.values.is_empty());
        let count = context.num_frames.min(output.len());
        for slot in output.iter_mut().take(count) {
            *slot = self.values[self.emitted % self.values.len()];
            self.emitted += 1;
        }
        Ok(count)
    }
}

fn script_host(sample_rate: u32, values: Vec<f32>) -> PluginHost {
    let mut host = PluginHost::new(1, sample_rate);
    host.add_plugin(Box::new(ScriptSignal { values, emitted: 0 }))
        .unwrap();
    host.build().unwrap();
    host
}

fn start_transition(state: &mut ProcessingState, new_host: PluginHost) {
    let prepared = PreparedHostUpdate::prepare(
        new_host,
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

/// Render one full same-rate transition plus settling frames through the
/// real `process_frame` path, returning every emitted sample.
fn render_transition(old_values: Vec<f32>, new_values: Vec<f32>) -> Vec<f32> {
    const RATE: u32 = 48_000;
    let mut state = ProcessingState::new(
        1,
        RATE,
        #[cfg(feature = "streaming")]
        None,
    );
    *state.host = script_host(RATE, old_values);
    start_transition(&mut state, script_host(RATE, new_values));
    let fade_frames = (u64::from(RATE) * 50).div_ceil(1_000) as usize;
    let mut rendered = Vec::new();
    // Mixed block sizes, including empty and partial blocks: the emitted
    // timeline must not depend on the callback partition.
    let partition = [513usize, 0, 128, 1, 257, 3, 127, 0, 64];
    let mut block = 0;
    while rendered.len() < fade_frames + 64 {
        let count = partition[block % partition.len()];
        let input = vec![0.0; count];
        let mut output = vec![f32::NAN; state.output_frames_for_input(count)];
        let actual = state.process_frame(&input, &mut output, count).unwrap();
        rendered.extend_from_slice(&output[..actual]);
        block += 1;
        assert!(block < 100_000);
    }
    assert!(
        state.prev_host.is_none(),
        "transition did not retire after {} emitted frames",
        rendered.len()
    );
    rendered
}

/// f32 rounding slack for bound assertions: the convex invariant is exact
/// in reals, but (1-a) and the two products each round once.
const BOUND_SLACK: f32 = 1e-6;
/// Tolerance for exact-curve checks against f64 oracles.
const CURVE_TOLERANCE: f64 = 5e-7;
/// 50 ms fade at the 48 kHz test rate.
const FADE_FRAMES: usize = 2_400;

fn fade_alpha(frame: usize) -> f64 {
    (frame as f64 / FADE_FRAMES as f64).min(1.0)
}

/// Assert every transition sample stays under `bound` and follows the
/// exact convex curve: transients keep their shape, mixed, never clamped.
fn assert_bounded_curve(old: &[f32], new: &[f32], bound: f32) {
    let rendered = render_transition(old.to_vec(), new.to_vec());
    assert!(rendered.len() >= FADE_FRAMES);
    for (frame, &sample) in rendered.iter().enumerate() {
        assert!(
            sample.abs() <= bound + BOUND_SLACK,
            "frame {frame}: {sample} exceeds the {bound} bound"
        );
        let alpha = fade_alpha(frame);
        let expected = (1.0 - alpha) * f64::from(old[frame % old.len()])
            + alpha * f64::from(new[frame % new.len()]);
        assert!(
            (f64::from(sample) - expected).abs() < CURVE_TOLERANCE,
            "frame {frame}: {sample} != {expected}"
        );
    }
}

#[test]
fn correlated_ceiling_program_holds_its_ceiling_through_rebuild() {
    const CEILING: f32 = 0.25;
    let script = vec![CEILING; 64];
    assert_bounded_curve(&script, &script, CEILING);
    // Mid-fade regression anchor: equal-power mixing emitted
    // sqrt(2) x ceiling = 0.3535.. here; convex mixing holds 0.25.
    let rendered = render_transition(script.clone(), script);
    let mid = rendered[FADE_FRAMES / 2];
    assert!(
        (f64::from(mid) - f64::from(CEILING)).abs() < CURVE_TOLERANCE,
        "mid-fade {mid} must hold the ceiling, not sqrt(2) x ceiling"
    );
}

#[test]
fn opposite_phase_program_cancels_naturally_and_stays_bounded() {
    const LEVEL: f32 = 0.5;
    assert_bounded_curve(&vec![LEVEL; 64], &vec![-LEVEL; 64], LEVEL);
}

#[test]
fn unequal_bounded_programs_stay_under_their_max() {
    // Old program alternates hot/quiet; the new chain starts silent, so the
    // transient follows the fade-out curve — present and scaled, not gated.
    let program: Vec<f32> = (0..64)
        .map(|i| if i % 2 == 0 { 0.9 } else { -0.1 })
        .collect();
    let silent = vec![0.0; 64];
    assert_bounded_curve(&program, &silent, 0.9);
    // Mirror image: silent old chain, hot new chain follows the fade-in.
    assert_bounded_curve(&silent, &program, 0.9);
}
