//! Parametric drain forwarding preserves defaults, output layouts, and errors.
// Rust guideline compliant 2026-02-21
use sotf_host::plugin::{InPlacePlugin, PluginDrainResult, PluginInfo, PluginResult};
use sotf_host::{
    ParameterSchema, ParameterSet, ParametricInPlacePlugin, ParametricInPlacePluginAdapter, Plugin,
    ProcessContext,
};

struct NoTail;
impl ParametricInPlacePlugin for NoTail {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("No tail fixture", "0", "tests")
    }
    fn channels(&self) -> usize {
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
    fn process_in_place(&mut self, _: &mut [f32], context: &ProcessContext) -> PluginResult<usize> {
        Ok(context.num_frames)
    }
}
struct Tail {
    calls: usize,
    prepared: bool,
}
impl ParametricInPlacePlugin for Tail {
    fn info(&self) -> PluginInfo {
        NoTail.info()
    }
    fn channels(&self) -> usize {
        1
    }
    fn input_channels(&self) -> usize {
        2
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
    fn process_in_place(&mut self, _: &mut [f32], context: &ProcessContext) -> PluginResult<usize> {
        Ok(context.num_frames)
    }
    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        if context.sample_rate != 96_000.0 {
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
        if context.sample_rate != 96_000.0 || output.len() < 2 {
            return Err("fixture capacity/rate".into());
        }
        if self.calls > 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        self.calls += 1;
        output[..2].copy_from_slice(&[0.25, -0.5]);
        Ok(PluginDrainResult {
            frames: 2,
            complete: true,
        })
    }
}
#[test]
fn absent_override_preserves_zero_tail_through_both_adapters() {
    let mut adapter = ParametricInPlacePluginAdapter::new(NoTail);
    let mut canary = [123.0; 3];
    let context = ProcessContext::new(96_000, 0);
    assert_eq!(Plugin::drain_output_frames_max(&adapter), 0);
    assert_eq!(InPlacePlugin::drain_output_frames_max(&adapter), 0);
    assert_eq!(
        Plugin::drain(&mut adapter, &mut canary, &context).unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert_eq!(
        InPlacePlugin::drain(&mut adapter, &mut canary, &context).unwrap(),
        PluginDrainResult::COMPLETE
    );
    assert_eq!(canary, [123.0; 3]);
}
#[test]
fn both_adapter_traits_forward_drain_errors_and_output_channel_frames() {
    for in_place in [false, true] {
        let mut adapter = ParametricInPlacePluginAdapter::new(Tail {
            calls: 0,
            prepared: false,
        });
        let context = ProcessContext::new(96_000, 0);
        assert!(Plugin::drain_call_bound(&adapter).is_none());
        assert!(InPlacePlugin::drain_call_bound(&adapter).is_none());
        if in_place {
            assert!(
                InPlacePlugin::begin_drain(&mut adapter, &ProcessContext::new(48_000, 0)).is_err()
            );
            InPlacePlugin::begin_drain(&mut adapter, &context).unwrap();
        } else {
            assert!(Plugin::begin_drain(&mut adapter, &ProcessContext::new(48_000, 0)).is_err());
            Plugin::begin_drain(&mut adapter, &context).unwrap();
        }
        assert_eq!(Plugin::drain_call_bound(&adapter).unwrap().get(), 1);
        assert_eq!(InPlacePlugin::drain_call_bound(&adapter).unwrap().get(), 1);
        assert_eq!(Plugin::drain_output_frames_max(&adapter), 2);
        assert_eq!(InPlacePlugin::drain_output_frames_max(&adapter), 2);
        let mut output = [123.0; 3];
        let invoke = |adapter: &mut ParametricInPlacePluginAdapter<Tail>,
                      output: &mut [f32],
                      context: &ProcessContext| {
            if in_place {
                InPlacePlugin::drain(adapter, output, context)
            } else {
                Plugin::drain(adapter, output, context)
            }
        };
        assert_eq!(
            invoke(&mut adapter, &mut output[..1], &context).unwrap_err(),
            "fixture capacity/rate"
        );
        assert_eq!(
            invoke(&mut adapter, &mut output, &ProcessContext::new(48_000, 0)).unwrap_err(),
            "fixture capacity/rate"
        );
        assert_eq!(output, [123.0; 3]);
        assert_eq!(
            invoke(&mut adapter, &mut output, &context).unwrap(),
            PluginDrainResult {
                frames: 2,
                complete: true
            }
        );
        assert_eq!(output, [0.25, -0.5, 123.0]);
        assert_eq!(
            invoke(&mut adapter, &mut output, &context).unwrap(),
            PluginDrainResult::COMPLETE
        );
    }
}
