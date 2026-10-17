// Rust guideline compliant 2026-02-21

use super::super::daw_host::{DawHost, HostRenderPath};
use super::ZeroCopyAnalyzerHookPlugin;
use crate::plugin::{Plugin, PluginCompileMetadata, PluginCostClass, PluginInfo, ProcessContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Linear, stateless gain that counts its `process` invocations.
struct CountingLinearPlugin {
    channels: usize,
    gain: f32,
    calls: Arc<AtomicUsize>,
}

impl CountingLinearPlugin {
    fn new(channels: usize, gain: f32, calls: Arc<AtomicUsize>) -> Self {
        Self {
            channels,
            gain,
            calls,
        }
    }
}

impl Plugin for CountingLinearPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("CountingLinear", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        self.channels
    }
    fn output_channels(&self) -> usize {
        self.channels
    }
    fn parameters(&self) -> Vec<crate::parameters::Parameter> {
        vec![]
    }
    fn set_parameter(
        &mut self,
        _: crate::parameters::ParameterId,
        _: crate::parameters::ParameterValue,
    ) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(
        &self,
        _: &crate::parameters::ParameterId,
    ) -> Option<crate::parameters::ParameterValue> {
        None
    }
    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::linear_transform(
            PluginCostClass::Scalar,
            None,
            0,
            false,
            false,
            false,
        )
    }
    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        for (dst, src) in output.iter_mut().zip(input.iter()) {
            *dst = *src * self.gain;
        }
        Ok(context.num_frames)
    }
}

fn counting_host(gains: &[f32]) -> (DawHost, Vec<Arc<AtomicUsize>>) {
    let mut host = DawHost::new(2, 48000);
    let mut counters = Vec::new();
    for &gain in gains {
        let calls = Arc::new(AtomicUsize::new(0));
        counters.push(calls.clone());
        host.add_plugin(Box::new(CountingLinearPlugin::new(2, gain, calls)))
            .unwrap();
    }
    host.build().unwrap();
    (host, counters)
}

#[test]
fn render_stats_report_compiled_linear_plan() {
    let (host, _) = counting_host(&[1.0, 2.0, 3.0]);
    let stats = host.render_stats();
    assert_eq!(stats.path, HostRenderPath::CompiledLinear);
    assert_eq!(stats.nodes, 3);
    assert_eq!(stats.ops, 3);
    assert_eq!(stats.bypassed_nodes, 0);
    assert_eq!(stats.silence_skippable_ops, 3);
}

#[test]
fn bypassed_middle_plugin_is_elided_without_a_call() {
    let (mut host, counters) = counting_host(&[2.0, 5.0, 3.0]);
    host.bypass_plugin(1).unwrap();

    let input = vec![1.0_f32; 8];
    let mut output = vec![0.0_f32; 8];
    let frames = host.process(&input, &mut output).unwrap();

    assert_eq!(frames, 4);
    assert_eq!(output, vec![6.0; 8]);
    assert_eq!(counters[0].load(Ordering::Relaxed), 1);
    assert_eq!(counters[1].load(Ordering::Relaxed), 0);
    assert_eq!(counters[2].load(Ordering::Relaxed), 1);
    assert_eq!(host.render_stats().bypassed_nodes, 1);
}

/// Stereo-to-quad duplicator for width-changing chain tests.
struct DuplicateChannelsPlugin;

impl Plugin for DuplicateChannelsPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("DuplicateChannels", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        4
    }
    fn parameters(&self) -> Vec<crate::parameters::Parameter> {
        vec![]
    }
    fn set_parameter(
        &mut self,
        _: crate::parameters::ParameterId,
        _: crate::parameters::ParameterValue,
    ) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(
        &self,
        _: &crate::parameters::ParameterId,
    ) -> Option<crate::parameters::ParameterValue> {
        None
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for frame in 0..context.num_frames {
            let (left, right) = (input[frame * 2], input[frame * 2 + 1]);
            output[frame * 4..frame * 4 + 4].copy_from_slice(&[left, right, left, right]);
        }
        Ok(context.num_frames)
    }
}

#[test]
fn channel_changing_chain_uses_compiled_linear_path() {
    let mut host = DawHost::new(2, 48000);
    let first_calls = Arc::new(AtomicUsize::new(0));
    let last_calls = Arc::new(AtomicUsize::new(0));
    host.add_plugin(Box::new(CountingLinearPlugin::new(
        2,
        2.0,
        first_calls.clone(),
    )))
    .unwrap();
    host.add_plugin(Box::new(DuplicateChannelsPlugin)).unwrap();
    host.add_plugin(Box::new(CountingLinearPlugin::new(
        4,
        3.0,
        last_calls.clone(),
    )))
    .unwrap();
    host.build().unwrap();

    assert_eq!(
        host.render_stats().path,
        HostRenderPath::CompiledLinear,
        "contiguous width-changing chain must not fall back to the graph path"
    );

    let input = vec![0.5_f32, -0.5];
    let mut output = vec![0.0_f32; 4];
    let frames = host.process(&input, &mut output).unwrap();
    assert_eq!(frames, 1);
    assert_eq!(output, vec![3.0, -3.0, 3.0, -3.0]);
}

#[test]
fn silent_block_skips_stateless_linear_plugin() {
    let (mut host, counters) = counting_host(&[2.0]);

    let silent = vec![0.0_f32; 16];
    let mut output = vec![1.0_f32; 16];
    host.process(&silent, &mut output).unwrap();
    assert_eq!(output, vec![0.0; 16]);
    assert_eq!(counters[0].load(Ordering::Relaxed), 0);

    let loud = vec![0.5_f32; 16];
    host.process(&loud, &mut output).unwrap();
    assert_eq!(output, vec![1.0; 16]);
    assert_eq!(counters[0].load(Ordering::Relaxed), 1);
}

#[test]
fn analyzer_stride_invokes_tap_once_per_stride() {
    let tap_calls = Arc::new(AtomicUsize::new(0));
    let regular_calls = Arc::new(AtomicUsize::new(0));
    let gain_calls = Arc::new(AtomicUsize::new(0));

    let mut host = DawHost::new(2, 48000);
    host.add_plugin(Box::new(ZeroCopyAnalyzerHookPlugin {
        regular_calls: regular_calls.clone(),
        tap_calls: tap_calls.clone(),
    }))
    .unwrap();
    host.add_plugin(Box::new(CountingLinearPlugin::new(
        2,
        2.0,
        gain_calls.clone(),
    )))
    .unwrap();
    host.set_analyzer_stride(4).unwrap();
    assert!(host.set_analyzer_stride(0).is_err());
    host.build().unwrap();
    assert_eq!(host.render_stats().analyzer_taps, 1);

    let input = vec![0.25_f32; 8];
    let mut output = vec![0.0_f32; 8];
    for _ in 0..4 {
        host.process(&input, &mut output).unwrap();
    }

    assert_eq!(output, vec![0.5; 8]);
    assert_eq!(tap_calls.load(Ordering::Relaxed), 1);
    assert_eq!(regular_calls.load(Ordering::Relaxed), 0);
    assert_eq!(gain_calls.load(Ordering::Relaxed), 4);
}

#[test]
fn panic_isolation_can_be_disabled_for_trusted_plugins() {
    let (mut host, counters) = counting_host(&[2.0]);
    host.set_panic_isolation_enabled(false).unwrap();

    let input = vec![0.5_f32; 8];
    let mut output = vec![0.0_f32; 8];
    host.process(&input, &mut output).unwrap();

    assert_eq!(output, vec![1.0; 8]);
    assert_eq!(counters[0].load(Ordering::Relaxed), 1);
}

/// Linear plugin that asks for oversampling: the host must not wrap it.
struct LinearPrefersOversamplingPlugin;

impl Plugin for LinearPrefersOversamplingPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("LinearPrefersOversampling", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn parameters(&self) -> Vec<crate::parameters::Parameter> {
        vec![]
    }
    fn set_parameter(
        &mut self,
        _: crate::parameters::ParameterId,
        _: crate::parameters::ParameterValue,
    ) -> Result<(), String> {
        Err("none".into())
    }
    fn get_parameter(
        &self,
        _: &crate::parameters::ParameterId,
    ) -> Option<crate::parameters::ParameterValue> {
        None
    }
    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::linear_transform(
            PluginCostClass::Scalar,
            None,
            0,
            false,
            false,
            false,
        )
    }
    fn preferred_oversampling(&self) -> Option<u32> {
        Some(2)
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        output.copy_from_slice(input);
        Ok(context.num_frames)
    }
}

#[test]
fn linear_plugin_preferred_oversampling_is_not_wrapped() {
    let mut host = DawHost::new(2, 48000);
    host.add_plugin(Box::new(LinearPrefersOversamplingPlugin))
        .unwrap();

    let plugin = host.get_plugin(0).unwrap();
    assert_eq!(plugin.info().name, "LinearPrefersOversampling");
    assert_eq!(plugin.preferred_oversampling(), Some(2));
}
