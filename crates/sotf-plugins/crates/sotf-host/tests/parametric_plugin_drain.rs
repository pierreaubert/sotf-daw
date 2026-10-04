//! Parameterized out-of-place plugins forward explicit EOS and preserve defaults.

// Rust guideline compliant 2026-02-21

use sotf_host::plugin::{PluginDrainResult, PluginInfo, PluginResult};
use sotf_host::{
    ParameterSchema, ParameterSet, ParametricPlugin, ParametricPluginAdapter, Plugin,
    ProcessContext,
};

struct Memoryless;

impl ParametricPlugin for Memoryless {
    fn plugin_info(&self) -> PluginInfo {
        PluginInfo::new("Memoryless", "0", "tests")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameter_schema(&self) -> ParameterSchema {
        Vec::new()
    }
    fn current_values(&self) -> ParameterSet {
        ParameterSet::new()
    }
    fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
        Ok(())
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        for (frame, result) in input.as_chunks::<2>().0.iter().zip(output) {
            *result = frame[0] + frame[1];
        }
        Ok(context.num_frames)
    }
}

struct Retained {
    complete: bool,
    prepared: bool,
}

impl ParametricPlugin for Retained {
    fn plugin_info(&self) -> PluginInfo {
        Memoryless.plugin_info()
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn parameter_schema(&self) -> ParameterSchema {
        Vec::new()
    }
    fn current_values(&self) -> ParameterSet {
        ParameterSet::new()
    }
    fn apply_values(&mut self, _: ParameterSet) -> PluginResult<()> {
        Ok(())
    }
    fn process(
        &mut self,
        _: &[f32],
        _: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        Ok(context.num_frames)
    }
    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if context.sample_rate != 96_000.0 || context.transport.sample_position != 73 {
            return Err("fixture preparation context".into());
        }
        self.prepared = true;
        Ok(())
    }
    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.prepared.then(|| std::num::NonZeroU64::new(1).unwrap())
    }
    fn drain_output_frames_max(&self) -> usize {
        2
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if output.len() < 2
            || context.sample_rate != 96_000.0
            || context.transport.sample_position != 73
        {
            return Err("fixture capacity or context".into());
        }
        if self.complete {
            return Ok(PluginDrainResult::COMPLETE);
        }
        output[..2].copy_from_slice(&[0.25, -0.5]);
        self.complete = true;
        Ok(PluginDrainResult {
            frames: 2,
            complete: true,
        })
    }
}

#[test]
fn default_drain_leaves_output_and_processing_unchanged() {
    let mut plugin = ParametricPluginAdapter::new(Memoryless);
    let context = ProcessContext::new(48_000, 0);
    let mut output = [123.0; 3];
    assert_eq!(plugin.drain_output_frames_max(), 0);
    assert_eq!(
        plugin.drain(&mut output, &context).unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert_eq!(output, [123.0; 3]);
    assert_eq!(
        plugin
            .process(
                &[0.25, -0.5],
                &mut output[..1],
                &ProcessContext::new(48_000, 1)
            )
            .unwrap(),
        1
    );
    assert_eq!(output, [-0.25, 123.0, 123.0]);
}

#[test]
fn explicit_drain_forwards_context_errors_and_output_width() {
    let mut plugin = ParametricPluginAdapter::new(Retained {
        complete: false,
        prepared: false,
    });
    let context = ProcessContext::new(96_000, 0).with_sample_position(73);
    let mut output = [123.0; 3];
    assert!(plugin.drain_call_bound().is_none());
    assert!(plugin.begin_drain(&ProcessContext::new(48_000, 0)).is_err());
    assert!(plugin.drain_call_bound().is_none());
    plugin.begin_drain(&context).unwrap();
    assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);
    assert_eq!(plugin.drain_output_frames_max(), 2);
    assert!(plugin.drain(&mut output[..1], &context).is_err());
    assert!(
        plugin
            .drain(&mut output, &ProcessContext::new(48_000, 0))
            .is_err()
    );
    assert_eq!(output, [123.0; 3]);
    assert_eq!(
        plugin.drain(&mut output, &context).unwrap(),
        PluginDrainResult {
            frames: 2,
            complete: true
        }
    );
    assert_eq!(output, [0.25, -0.5, 123.0]);
    assert_eq!(
        plugin.drain(&mut output, &context).unwrap(),
        PluginDrainResult::COMPLETE
    );
}
