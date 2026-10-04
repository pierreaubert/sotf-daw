//! Independent serial EOS work and completion-state fixtures.
// Rust guideline compliant 2026-02-21
use crate::plugin::PluginDrainResult;
use crate::{DawHost, Parameter, ParameterId, ParameterValue, Plugin, PluginInfo, ProcessContext};
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct Calls {
    begin: AtomicUsize,
    drain: AtomicUsize,
    bounds: AtomicUsize,
    process: AtomicUsize,
}

struct WorkPlugin {
    remaining: usize,
    initial: usize,
    bound: Option<u64>,
    emit: bool,
    absorb: bool,
    calls: Arc<Calls>,
    error_once: bool,
    process_error_once: bool,
    rate: Option<u32>,
    expected_rate: Option<u32>,
}

impl WorkPlugin {
    fn new(steps: usize, bound: Option<u64>, emit: bool) -> Self {
        Self {
            remaining: steps,
            initial: steps,
            bound,
            emit,
            absorb: false,
            calls: Arc::default(),
            error_once: false,
            process_error_once: false,
            rate: None,
            expected_rate: None,
        }
    }
}

impl Plugin for WorkPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("EOS work fixture", "1", "test")
    }
    fn input_channels(&self) -> usize {
        1
    }
    fn output_channels(&self) -> usize {
        1
    }
    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        self.rate.map(f64::from).unwrap_or(input_rate)
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![Parameter::new_bool("restart", "Restart", false)]
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> Result<(), String> {
        if id.as_str() == "restart" && value == ParameterValue::Bool(true) {
            self.remaining = self.initial;
            Ok(())
        } else {
            Err("rejected fixture control".into())
        }
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        (id.as_str() == "restart").then_some(ParameterValue::Bool(false))
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        self.calls.process.fetch_add(1, Ordering::Relaxed);
        if std::mem::take(&mut self.process_error_once) {
            return Err("downstream process failure".into());
        }
        self.remaining = self.initial;
        if self.absorb {
            Ok(0)
        } else {
            output[..context.num_frames].copy_from_slice(input);
            Ok(context.num_frames)
        }
    }
    fn reset(&mut self) {
        self.remaining = self.initial;
    }
    fn drain_output_frames_max(&self) -> usize {
        1
    }
    fn begin_drain(&mut self, _: &ProcessContext) -> Result<(), String> {
        self.calls.begin.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        assert!(
            self.calls.begin.load(Ordering::Relaxed) > 0,
            "query preceded preparation"
        );
        self.calls.bounds.fetch_add(1, Ordering::Relaxed);
        self.bound.and_then(NonZeroU64::new)
    }
    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if let Some(expected) = self.expected_rate {
            assert_eq!(context.sample_rate, f64::from(expected));
        }
        if std::mem::take(&mut self.error_once) {
            return Err("retryable fixture failure".into());
        }
        self.calls.drain.fetch_add(1, Ordering::Relaxed);
        if self.remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        self.remaining -= 1;
        if self.emit {
            output[0] = self.remaining as f32 + 1.0;
        }
        Ok(PluginDrainResult {
            frames: usize::from(self.emit),
            complete: self.remaining == 0,
        })
    }
}

#[test]
fn declared_quota_rejects_next_call_without_rearming_or_invoking_dsp() {
    for emit in [false, true] {
        let mut host = DawHost::new(1, 48_000);
        let plugin = WorkPlugin::new(10, Some(3), emit);
        let calls = Arc::clone(&plugin.calls);
        host.add_plugin(Box::new(plugin)).unwrap();
        for _ in 0..3 {
            assert!(!host.drain(&mut [0.0]).unwrap().complete);
        }
        assert!(
            host.drain(&mut [0.0])
                .unwrap_err()
                .contains("did not converge")
        );
        assert_eq!(calls.drain.load(Ordering::Relaxed), 3);
        assert_eq!(calls.begin.load(Ordering::Relaxed), 1);
        assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn completed_prefix_and_terminal_complete_are_not_reentered() {
    let mut host = DawHost::new(1, 48_000);
    let first = WorkPlugin::new(1, Some(1), true);
    let first_calls = Arc::clone(&first.calls);
    let last = WorkPlugin::new(3, Some(3), true);
    let last_calls = Arc::clone(&last.calls);
    host.add_plugin(Box::new(first)).unwrap();
    host.add_plugin(Box::new(last)).unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut []).unwrap().complete);
    assert_eq!(first_calls.drain.load(Ordering::Relaxed), 1);
    assert_eq!(last_calls.drain.load(Ordering::Relaxed), 3);
}

#[test]
fn preparation_follows_capacity_validation_and_precedes_bound_snapshot() {
    let mut host = DawHost::new(1, 48_000);
    let plugin = WorkPlugin::new(1, Some(1), true);
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    assert!(host.drain(&mut []).is_err());
    assert_eq!(calls.begin.load(Ordering::Relaxed), 0);
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 0);
    assert!(host.drain(&mut [0.0]).unwrap().complete);
    assert_eq!(calls.begin.load(Ordering::Relaxed), 1);
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
}

#[test]
fn finite_long_tail_and_aggregate_chain_can_exceed_legacy_global_limit() {
    for known in [false, true] {
        let mut host = DawHost::new(1, 48_000);
        let steps = if known { 5001 } else { 3000 };
        let count = if known { 1 } else { 2 };
        let mut counters = Vec::new();
        for _ in 0..count {
            let plugin = WorkPlugin::new(steps, known.then_some(steps as u64), false);
            counters.push(Arc::clone(&plugin.calls));
            host.add_plugin(Box::new(plugin)).unwrap();
        }
        let mut complete = false;
        for _ in 0..steps * count {
            if host.drain(&mut [0.0]).unwrap().complete {
                complete = true;
                break;
            }
        }
        assert!(complete);
        for calls in counters {
            assert_eq!(calls.drain.load(Ordering::Relaxed), steps);
            assert_eq!(calls.begin.load(Ordering::Relaxed), 1);
        }
    }
}

#[test]
fn unknown_positive_and_zero_output_plugins_stop_after_4096_successes() {
    for emit in [false, true] {
        let mut host = DawHost::new(1, 48_000);
        let plugin = WorkPlugin::new(5000, None, emit);
        let calls = Arc::clone(&plugin.calls);
        host.add_plugin(Box::new(plugin)).unwrap();
        for _ in 0..4096 {
            assert!(!host.drain(&mut [0.0]).unwrap().complete);
        }
        for _ in 0..2 {
            assert!(
                host.drain(&mut [0.0])
                    .unwrap_err()
                    .contains("did not converge")
            );
        }
        assert_eq!(calls.drain.load(Ordering::Relaxed), 4096);
        assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn accepted_active_controls_rearm_but_unrelated_rejected_and_reads_do_not() {
    let mut host = DawHost::new(1, 48_000);
    let first = WorkPlugin::new(1, Some(1), false);
    let first_calls = Arc::clone(&first.calls);
    let active = WorkPlugin::new(8, Some(2), false);
    let calls = Arc::clone(&active.calls);
    host.add_plugin(Box::new(first)).unwrap();
    host.add_plugin(Box::new(active)).unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    host.set_plugin_parameter_immediate(0, "restart", ParameterValue::Bool(true))
        .unwrap();
    assert!(
        host.set_plugin_parameter_immediate(1, "restart", ParameterValue::Bool(false))
            .is_err()
    );
    assert!(
        host.validate_plugin_parameter(1, "restart", &ParameterValue::Bool(true))
            .is_ok()
    );
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut [0.0]).is_err());
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
    assert_eq!(first_calls.drain.load(Ordering::Relaxed), 1);
    // Queued action is accepted at the next EOS boundary; getter remains false.
    host.set_plugin_parameter(1, "restart", ParameterValue::Bool(true))
        .unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut [0.0]).is_err());
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 2);
    assert_eq!(calls.begin.load(Ordering::Relaxed), 1);
}

#[test]
fn native_error_and_invalid_capacity_do_not_burn_success_quota() {
    let mut host = DawHost::new(1, 48_000);
    let mut plugin = WorkPlugin::new(1, Some(1), true);
    plugin.error_once = true;
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    assert!(host.drain(&mut [0.0]).unwrap_err().contains("retryable"));
    assert!(host.drain(&mut []).is_err());
    assert!(host.drain(&mut [0.0]).unwrap().complete);
    assert_eq!(calls.drain.load(Ordering::Relaxed), 1);
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
}

#[test]
fn absorbed_upstream_output_completes_prefix_and_retains_rate_clock() {
    let mut host = DawHost::new(1, 48_000);
    let mut first = WorkPlugin::new(2, Some(2), true);
    first.rate = Some(96_000);
    let first_calls = Arc::clone(&first.calls);
    let mut last = WorkPlugin::new(2, Some(2), false);
    last.absorb = true;
    last.expected_rate = Some(96_000);
    host.add_plugin(Box::new(first)).unwrap();
    host.add_plugin(Box::new(last)).unwrap();
    for _ in 0..3 {
        let result = host.drain(&mut [0.0]).unwrap();
        assert_eq!(result.frames, 0);
        assert!(!result.complete);
    }
    assert!(host.drain(&mut [0.0]).unwrap().complete);
    assert_eq!(first_calls.drain.load(Ordering::Relaxed), 2);
    host.reset();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert_eq!(first_calls.begin.load(Ordering::Relaxed), 2);
}

#[test]
fn rejected_host_rate_and_rebuild_do_not_refresh_quota_but_graph_replacement_does() {
    let mut host = DawHost::new(1, 48_000);
    let plugin = WorkPlugin::new(10, Some(2), false);
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    host.config.sample_rate = 0.0;
    assert!(host.drain(&mut [0.0]).is_err());
    host.config.sample_rate = 48_000.0;
    host.build().unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut [0.0]).is_err());
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
    host.remove_plugin(0).unwrap();
    host.add_plugin(Box::new(WorkPlugin::new(3, Some(3), true)))
        .unwrap();
    for _ in 0..2 {
        assert!(!host.drain(&mut [0.0]).unwrap().complete);
    }
    assert!(host.drain(&mut [0.0]).unwrap().complete);
}

#[test]
fn downstream_error_still_charges_the_successful_native_drain_call() {
    let mut host = DawHost::new(1, 48_000);
    let first = WorkPlugin::new(10, Some(2), true);
    let calls = Arc::clone(&first.calls);
    let mut downstream = WorkPlugin::new(1, Some(1), true);
    downstream.process_error_once = true;
    host.add_plugin(Box::new(first)).unwrap();
    host.add_plugin(Box::new(downstream)).unwrap();
    assert!(host.drain(&mut [0.0]).unwrap_err().contains("downstream"));
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(
        host.drain(&mut [0.0])
            .unwrap_err()
            .contains("did not converge")
    );
    assert_eq!(calls.drain.load(Ordering::Relaxed), 2);
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
}

#[test]
fn accepted_f32_and_f64_input_rearm_a_completed_stream() {
    let mut host = DawHost::new(1, 48_000);
    let plugin = WorkPlugin::new(2, Some(2), true);
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    for epoch in 0..3 {
        match epoch {
            1 => {
                host.process(&[0.25], &mut [0.0]).unwrap();
            }
            2 => {
                host.process_f64(&[0.25], &mut [0.0]).unwrap();
            }
            _ => {}
        }
        assert!(!host.drain(&mut [0.0]).unwrap().complete);
        assert!(host.drain(&mut [0.0]).unwrap().complete);
        assert!(host.drain(&mut []).unwrap().complete);
    }
    assert_eq!(calls.begin.load(Ordering::Relaxed), 3);
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 3);
    assert_eq!(calls.drain.load(Ordering::Relaxed), 6);
}

#[test]
fn only_actual_bypass_changes_reset_the_epoch() {
    let mut host = DawHost::new(1, 48_000);
    let plugin = WorkPlugin::new(10, Some(2), true);
    let calls = Arc::clone(&plugin.calls);
    host.add_plugin(Box::new(plugin)).unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    host.unbypass_plugin(0).unwrap();
    assert!(!host.drain(&mut [0.0]).unwrap().complete);
    assert!(host.drain(&mut [0.0]).is_err());
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 1);
    host.bypass_plugin(0).unwrap();
    assert!(host.drain(&mut [0.0]).unwrap().complete);
    host.unbypass_plugin(0).unwrap();
    for _ in 0..2 {
        assert!(!host.drain(&mut [0.0]).unwrap().complete);
    }
    assert!(host.drain(&mut [0.0]).is_err());
    assert_eq!(calls.bounds.load(Ordering::Relaxed), 2);
}
