//! Over-reported process counts fail loudly with measured evidence.
//!
//! A plugin that returns past its declared staging (adversarial geometry)
//! must fail naming the node with the measured return-vs-declared
//! evidence — never panic on staging, never silently substitute. Failures
//! and panics keep the passthrough recovery; only over-reporting refuses.
//! Pinned on both precision paths.

use super::super::daw_host::DawHost;

use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginInfo, ProcessContext};

const CHANNELS: usize = 2;
const RATE: u32 = 48_000;

/// Under-declares its live bound by one frame, writes honestly, then
/// over-reports past staging on both precision paths.
struct OverReportingFixture {
    channels: usize,
}

impl Plugin for OverReportingFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("OverReportingFixture", "0.1", "test")
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
        Err("OverReportingFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames.saturating_sub(1)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let honest = output.len().min(input.len());
        output[..honest].copy_from_slice(&input[..honest]);
        Ok(context.num_frames)
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let honest = output.len().min(input.len());
        output[..honest].copy_from_slice(&input[..honest]);
        Ok(context.num_frames)
    }
}

fn lying_host() -> DawHost {
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "lier".to_string(),
        Box::new(OverReportingFixture { channels: CHANNELS }),
    )
    .unwrap();
    host.build().unwrap();
    host
}

#[test]
fn f32_over_report_fails_loud_with_measured_evidence() {
    let mut host = lying_host();
    let input = vec![0.5; 64 * CHANNELS];
    let mut output = vec![0.0; 64 * CHANNELS];
    let error = host.process(&input, &mut output).unwrap_err();
    assert!(
        error.contains("lier")
            && error.contains("returned 64 frames")
            && error.contains("63-frame declared output capacity"),
        "lying f32 process must fail with measured evidence: {error}",
    );
}

#[test]
fn f64_over_report_fails_loud_with_measured_evidence() {
    let mut host = lying_host();
    let input = vec![0.5; 64 * CHANNELS];
    let mut output = vec![0.0; 64 * CHANNELS];
    let error = host.process_f64(&input, &mut output).unwrap_err();
    assert!(
        error.contains("lier")
            && error.contains("returned 64 frames")
            && error.contains("63-frame declared output capacity"),
        "lying f64 process must fail with measured evidence: {error}",
    );
}

/// Probe-coincidence shape: identity at the 100-probe (routes compiled /
/// native), loose 84 declaration with honest 83 production — or a genuine
/// lie one past the declaration when `lie` is set. Count lie only, never
/// a buffer overrun: writes stay inside staging, only the return exceeds.
struct CoincidenceFixture {
    channels: usize,
    lie: bool,
    f64_mode: bool,
}

impl CoincidenceFixture {
    fn declared_frames(&self, input_frames: usize) -> usize {
        if input_frames == 83 { 84 } else { input_frames }
    }
}

impl Plugin for CoincidenceFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("CoincidenceFixture", "0.1", "test")
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
        Err("CoincidenceFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        self.declared_frames(input_frames)
    }

    fn supports_f64(&self) -> bool {
        self.f64_mode
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let honest = output.len().min(input.len());
        output[..honest].copy_from_slice(&input[..honest]);
        if self.lie {
            Ok(self.declared_frames(context.num_frames) + 1)
        } else {
            Ok(context.num_frames)
        }
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let honest = output.len().min(input.len());
        output[..honest].copy_from_slice(&input[..honest]);
        if self.lie {
            Ok(self.declared_frames(context.num_frames) + 1)
        } else {
            Ok(context.num_frames)
        }
    }
}

fn coincidence_host(lie: bool, f64_mode: bool) -> DawHost {
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "coincidence".to_string(),
        Box::new(CoincidenceFixture {
            channels: CHANNELS,
            lie,
            f64_mode,
        }),
    )
    .unwrap();
    host.build().unwrap();
    host
}

#[test]
fn compiled_loose_declaration_composes_with_exact_production() {
    let mut host = coincidence_host(false, false);
    assert!(
        host.can_process_f32_linear_chain(),
        "the 83-anomaly shape must route compiled"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let frames = host.process(&input, &mut output).unwrap();
    assert_eq!(frames, 83, "loose declaration must commit exact production");
    assert_eq!(output, input, "committed content must match the input");
}

#[test]
fn compiled_probe_passing_liar_fails_loud_with_measured_evidence() {
    let mut host = coincidence_host(true, false);
    assert!(
        host.can_process_f32_linear_chain(),
        "the probe-passing liar must route compiled"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let error = host.process(&input, &mut output).unwrap_err();
    assert!(
        error.contains("coincidence")
            && error.contains("returned 85 frames")
            && error.contains("84-frame declared output capacity"),
        "compiled liar must fail with measured evidence: {error}",
    );
}

#[test]
fn f64_native_loose_declaration_composes_with_exact_production() {
    let mut host = coincidence_host(false, true);
    assert!(
        host.can_process_f64_chain_native(),
        "the f64 coincidence shape must route the native chain"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let frames = host.process_f64(&input, &mut output).unwrap();
    assert_eq!(frames, 83, "loose declaration must commit exact production");
    assert_eq!(output, input, "committed content must match the input");
}

#[test]
fn f64_native_liar_fails_loud_with_measured_evidence() {
    let mut host = coincidence_host(true, true);
    assert!(
        host.can_process_f64_chain_native(),
        "the f64 liar must route the native chain"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let error = host.process_f64(&input, &mut output).unwrap_err();
    assert!(
        error.contains("coincidence")
            && error.contains("returned 85 frames")
            && error.contains("84-frame declared output capacity"),
        "f64 liar must fail with measured evidence: {error}",
    );
}

/// Honest-but-loose shape: declares 84 at 83 in-frames (probe-identity
/// at 100, like `CoincidenceFixture`) and PRODUCES the full 84 — within
/// its declaration, past an 83-staged caller. Exercises the fallback's
/// loud-surplus leg (genuine surplus, never silently discarded).
struct SurplusFixture {
    channels: usize,
    f64_mode: bool,
}

impl Plugin for SurplusFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("SurplusFixture", "0.1", "test")
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
        Err("SurplusFixture has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        if input_frames == 83 { 84 } else { input_frames }
    }

    fn supports_f64(&self) -> bool {
        self.f64_mode
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        let copied = input.len().min(output.len());
        let honest = output.len().min(input.len() + self.channels);
        output[..copied].copy_from_slice(&input[..copied]);
        for sample in &mut output[copied..honest] {
            *sample = 0.0;
        }
        Ok(output.len() / self.channels)
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        _context: &ProcessContext,
    ) -> Result<usize, String> {
        let copied = input.len().min(output.len());
        let honest = output.len().min(input.len() + self.channels);
        output[..copied].copy_from_slice(&input[..copied]);
        for sample in &mut output[copied..honest] {
            *sample = 0.0;
        }
        Ok(output.len() / self.channels)
    }
}

fn surplus_host(f64_mode: bool) -> DawHost {
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "surplus".to_string(),
        Box::new(SurplusFixture {
            channels: CHANNELS,
            f64_mode,
        }),
    )
    .unwrap();
    host.build().unwrap();
    host
}

#[test]
fn compiled_true_surplus_fails_loud_naming_actual_production() {
    let mut host = surplus_host(false);
    assert!(
        host.can_process_f32_linear_chain(),
        "the surplus shape must route compiled"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let error = host.process(&input, &mut output).unwrap_err();
    assert!(
        error.contains("f32 output too small")
            && error.contains("need 168 samples")
            && error.contains("got 166"),
        "true surplus must fail loud with actual production: {error}",
    );
}

#[test]
fn f64_native_true_surplus_fails_loud_naming_actual_production() {
    let mut host = surplus_host(true);
    assert!(
        host.can_process_f64_chain_native(),
        "the surplus shape must route the native chain"
    );
    let input = vec![0.5; 83 * CHANNELS];
    let mut output = vec![0.0; 83 * CHANNELS];
    let error = host.process_f64(&input, &mut output).unwrap_err();
    assert!(
        error.contains("f64 output too small")
            && error.contains("need 168 samples")
            && error.contains("got 166"),
        "true surplus must fail loud with actual production: {error}",
    );
}

#[test]
fn parallel_stage_liar_fails_loud_with_measured_evidence() {
    // Two sources share stage 0: 4096 frames x 2 channels x cost 1 x 2
    // nodes = 16384 work units, over the 8192 threshold, so the stage
    // provably engages parallel dispatch (asserted, not assumed).
    let mut host = DawHost::new(CHANNELS, RATE);
    host.add_node(
        "honest".to_string(),
        Box::new(CoincidenceFixture {
            channels: CHANNELS,
            lie: false,
            f64_mode: false,
        }),
    )
    .unwrap();
    host.add_node(
        "plier".to_string(),
        Box::new(CoincidenceFixture {
            channels: CHANNELS,
            lie: true,
            f64_mode: false,
        }),
    )
    .unwrap();
    host.build().unwrap();
    assert!(
        host.cached_frames_identity && host.cached_rate_identity && !host.has_variable_frame_plugin,
        "probe-passing fixtures must keep the identity caches"
    );
    let stage = host
        .stages
        .iter()
        .find(|stage| stage.nodes.len() >= 2)
        .expect("both sources must share one stage");
    let nodes = &host.nodes;
    let costs = &host.cached_parallel_node_costs;
    assert!(
        DawHost::should_parallelize_stage(stage, 4096, nodes, costs),
        "the two-source stage must engage parallel dispatch"
    );
    let input = vec![0.5; 4096 * CHANNELS];
    let mut output = vec![0.0; 4096 * CHANNELS];
    let error = host.process(&input, &mut output).unwrap_err();
    assert!(
        error.contains("plier")
            && error.contains("returned 4097 frames")
            && error.contains("4096-frame declared output capacity"),
        "parallel liar must fail with measured evidence: {error}",
    );
}
