use super::isolated_external_plugin::IsolatedExternalPlugin;
use super::isolated_external_plugin_config::IsolatedExternalPluginConfig;
use super::isolated_external_plugin_config::build_worker_launch_command;
use crate::assert_no_allocs;
use crate::external_plugin::{
    ExternalPluginSandboxMode, ExternalPluginState, PluginDescriptor, plan_external_plugin_hosting,
};
use crate::external_plugin_ipc::SecurePluginSharedMemory;
use crate::external_plugin_ipc::{PluginIpcControlRequest, PluginIpcControlResponse};
use crate::external_plugin_process::{ExternalPluginProcessEvent, ExternalPluginWorkerCommand};
use crate::external_plugin_sandbox::{PluginSandboxLaunchBackend, PluginSandboxPolicy};
use crate::external_plugin_worker::ExternalPluginWorker;
use crate::host::DawHost;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{ParameterEvent, Plugin, PluginInfo, PluginResult, ProcessContext};
use std::time::Duration;

use std::path::Path;

use crate::external_plugin::PluginFormat;

fn descriptor() -> PluginDescriptor {
    PluginDescriptor {
        id: "test.external".into(),
        name: "External Test".into(),
        vendor: "Test".into(),
        version: "0.1".into(),
        format: PluginFormat::Clap,
        path: "/tmp/fake.clap".into(),
        audio_inputs: 2,
        audio_outputs: 2,
        is_instrument: false,
        categories: Vec::new(),
        scan_status: crate::external_plugin::PluginScanStatus::Discovered,
    }
}

struct StatefulScalePlugin {
    value: f32,
    history: f32,
}

impl Plugin for StatefulScalePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Stateful Scale", "0.1", "test")
    }
    fn input_channels(&self) -> usize {
        2
    }
    fn output_channels(&self) -> usize {
        2
    }
    fn latency_samples(&self) -> usize {
        320
    }
    fn parameters(&self) -> Vec<Parameter> {
        vec![Parameter::new_float("value", "Value", 1.0, 0.0, 4.0)]
    }
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        if id.as_str() != "value" {
            return Err("unknown parameter".into());
        }
        self.value = value
            .as_float()
            .ok_or_else(|| "expected float".to_string())?;
        Ok(())
    }
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        (id.as_str() == "value").then_some(ParameterValue::Float(self.value))
    }
    fn save_opaque_state(&self) -> PluginResult<Vec<u8>> {
        Ok(self.value.to_le_bytes().to_vec())
    }
    fn load_opaque_state(&mut self, state: &[u8]) -> PluginResult<()> {
        let bytes: [u8; 4] = state.try_into().map_err(|_| "invalid state".to_string())?;
        self.value = f32::from_le_bytes(bytes);
        Ok(())
    }
    fn tail_length(&self) -> crate::plugin::TailLength {
        if self.value >= 3.5 {
            crate::plugin::TailLength::Infinite
        } else if self.value >= 2.0 {
            crate::plugin::TailLength::Finite(128)
        } else {
            crate::plugin::TailLength::Unknown
        }
    }
    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }
    fn reset(&mut self) {
        self.history = 0.0;
    }
    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        for event in context.parameter_events {
            if event.parameter_id.as_str() != "value" {
                return Err("unknown automated parameter".into());
            }
            self.value = event
                .value
                .as_float()
                .ok_or_else(|| "expected automated float".to_string())?;
        }
        for (source, destination) in input[..context.num_frames * 2]
            .iter()
            .zip(&mut output[..context.num_frames * 2])
        {
            *destination = *source * self.value + self.history;
        }
        self.history = output[context.num_frames * 2 - 1];
        Ok(context.num_frames)
    }
}

const LONG_TAIL_FRAMES: usize = 16_384;
const LONG_TAIL_PIPELINE_FRAMES: usize = 8_192;
const LONG_TAIL_CHANNELS: usize = 2;
const LONG_TAIL_TAPS: [(usize, [f32; LONG_TAIL_CHANNELS]); 5] = [
    (0, [0.5, 0.3]),
    (1, [-0.2, 0.25]),
    (31, [0.1, -0.125]),
    (8_191, [-0.05, 0.075]),
    (LONG_TAIL_FRAMES, [0.8, -0.65]),
];

#[derive(Default)]
struct LongTailProbe {
    zero_input_frames: std::sync::atomic::AtomicUsize,
    zero_input_blocks: std::sync::atomic::AtomicUsize,
    zero_input_attempts: std::sync::atomic::AtomicUsize,
    final_tail_left_bits: std::sync::atomic::AtomicU32,
    final_tail_right_bits: std::sync::atomic::AtomicU32,
}

struct LongFiniteTailPlugin {
    history: Vec<[f32; LONG_TAIL_CHANNELS]>,
    history_write: usize,
    probe: std::sync::Arc<LongTailProbe>,
    process_gate: Option<std::sync::Arc<LongTailProcessGate>>,
    gate_marker: Option<[u32; LONG_TAIL_CHANNELS]>,
    fail_after_zero_input_blocks: Option<usize>,
    failure_injected: bool,
}

impl LongFiniteTailPlugin {
    fn new(probe: std::sync::Arc<LongTailProbe>) -> Self {
        Self::with_gate(probe, None, None)
    }

    fn with_gate(
        probe: std::sync::Arc<LongTailProbe>,
        process_gate: Option<std::sync::Arc<LongTailProcessGate>>,
        gate_marker: Option<[u32; LONG_TAIL_CHANNELS]>,
    ) -> Self {
        Self {
            history: vec![[0.0; LONG_TAIL_CHANNELS]; LONG_TAIL_FRAMES + 1],
            history_write: 0,
            probe,
            process_gate,
            gate_marker,
            fail_after_zero_input_blocks: None,
            failure_injected: false,
        }
    }

    fn fail_after_zero_input_blocks(mut self, successful_blocks: usize) -> Self {
        self.fail_after_zero_input_blocks = Some(successful_blocks);
        self
    }
}

#[derive(Default)]
struct LongTailProcessGate {
    state: std::sync::Mutex<LongTailProcessGateState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct LongTailProcessGateState {
    entered: bool,
    released: bool,
}

impl LongTailProcessGate {
    fn block_worker_until_released(&self) -> PluginResult<()> {
        let mut state = self.state.lock().unwrap();
        state.entered = true;
        self.changed.notify_all();
        let result = self
            .changed
            .wait_timeout_while(state, Duration::from_secs(3), |state| !state.released)
            .unwrap();
        if result.1.timed_out() && !result.0.released {
            return Err("finite-tail test worker gate timed out".into());
        }
        Ok(())
    }

    fn wait_until_entered(&self, timeout: Duration) -> bool {
        let state = self.state.lock().unwrap();
        let Ok((state, wait)) = self
            .changed
            .wait_timeout_while(state, timeout, |state| !state.entered)
        else {
            return false;
        };
        state.entered && !wait.timed_out()
    }

    fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.changed.notify_all();
    }
}

struct TestWorkerThread {
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    process_gate: Option<std::sync::Arc<LongTailProcessGate>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl TestWorkerThread {
    fn spawn(
        worker: ExternalPluginWorker,
        process_gate: Option<std::sync::Arc<LongTailProcessGate>>,
    ) -> Self {
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_running = std::sync::Arc::clone(&running);
        let thread = std::thread::spawn(move || {
            let mut worker = worker;
            while worker_running.load(std::sync::atomic::Ordering::Acquire) {
                if worker.process_one().is_err() {
                    break;
                }
                std::thread::yield_now();
            }
        });
        Self {
            running,
            process_gate,
            thread: Some(thread),
        }
    }
}

impl Drop for TestWorkerThread {
    fn drop(&mut self) {
        if let Some(gate) = &self.process_gate {
            gate.release();
        }
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct DrainObservedPlugin {
    inner: IsolatedExternalPlugin,
    prepare_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    begin_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Plugin for DrainObservedPlugin {
    fn info(&self) -> PluginInfo {
        self.inner.info()
    }

    fn input_channels(&self) -> usize {
        self.inner.input_channels()
    }

    fn output_channels(&self) -> usize {
        self.inner.output_channels()
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.inner.parameters()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        self.inner.set_parameter(id, value)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.inner.get_parameter(id)
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.inner.process(input, output, context)
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn reset_checked(&mut self) -> PluginResult<()> {
        self.inner.reset_checked()
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        self.inner.initialize(sample_rate)
    }

    fn latency_samples(&self) -> usize {
        self.inner.latency_samples()
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        self.inner.tail_length()
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.inner.guarantees_identity_frame_geometry()
    }

    fn drain_output_frames_max(&self) -> usize {
        self.inner.drain_output_frames_max()
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        self.prepare_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.inner.prepare_drain_metadata()
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        self.begin_calls
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        self.inner.begin_drain(context)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        self.inner.drain_call_bound()
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<crate::plugin::PluginDrainResult> {
        self.inner.drain(output, context)
    }
}

/// Stateless width-changing stage that makes the host use its channel-changing
/// drain preflight while preserving every input frame in a known lane mapping.
struct StereoToQuadMapper {
    processed_frames: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Plugin for StereoToQuadMapper {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Stereo to Quad Drain Mapper", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        2
    }

    fn output_channels(&self) -> usize {
        4
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> PluginResult<()> {
        Err(format!("unknown parameter '{id}'"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let input_samples = context.num_frames * 2;
        let output_samples = context.num_frames * 4;
        if input.len() != input_samples || output.len() != output_samples {
            return Err("stereo-to-quad mapper received invalid frame geometry".into());
        }
        let (source_frames, _) = input[..input_samples].as_chunks::<2>();
        let (destination_frames, _) = output[..output_samples].as_chunks_mut::<4>();
        for (source, destination) in source_frames.iter().zip(destination_frames) {
            destination.copy_from_slice(&[
                source[0],
                source[1],
                source[0] * 0.5,
                source[1] * -0.25,
            ]);
        }
        self.processed_frames
            .fetch_add(context.num_frames, std::sync::atomic::Ordering::AcqRel);
        Ok(context.num_frames)
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        crate::plugin::TailLength::Finite(0)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }
}

impl Plugin for LongFiniteTailPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Long Finite Tail", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        2
    }

    fn output_channels(&self) -> usize {
        2
    }

    fn tail_length(&self) -> crate::plugin::TailLength {
        crate::plugin::TailLength::Finite(16_384)
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, id: ParameterId, _value: ParameterValue) -> PluginResult<()> {
        Err(format!("unknown parameter '{id}'"))
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        let samples = context.num_frames * LONG_TAIL_CHANNELS;
        let gate_matches = self.gate_marker.is_some_and(|marker| {
            context.num_frames > 0
                && input[samples - 2].to_bits() == marker[0]
                && input[samples - 1].to_bits() == marker[1]
        });
        if gate_matches && let Some(gate) = &self.process_gate {
            gate.block_worker_until_released()?;
        }

        let zero_input_start = if input[..samples].iter().all(|sample| *sample == 0.0) {
            let attempts = self
                .probe
                .zero_input_attempts
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            if !self.failure_injected && self.fail_after_zero_input_blocks == Some(attempts) {
                self.failure_injected = true;
                return Err("injected second native-tail request failure".into());
            }
            Some(
                self.probe
                    .zero_input_frames
                    .fetch_add(context.num_frames, std::sync::atomic::Ordering::AcqRel),
            )
        } else {
            None
        };
        if zero_input_start.is_some() {
            self.probe
                .zero_input_blocks
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }

        for frame in 0..context.num_frames {
            let mut frame_output = [0.0_f32; LONG_TAIL_CHANNELS];
            for (delay, coefficients) in LONG_TAIL_TAPS {
                for channel in 0..LONG_TAIL_CHANNELS {
                    let sample = if delay == 0 {
                        input[frame * LONG_TAIL_CHANNELS + channel]
                    } else {
                        let history_index =
                            (self.history_write + self.history.len() - delay) % self.history.len();
                        self.history[history_index][channel]
                    };
                    frame_output[channel] += sample * coefficients[channel];
                }
            }
            output[frame * LONG_TAIL_CHANNELS..(frame + 1) * LONG_TAIL_CHANNELS]
                .copy_from_slice(&frame_output);
            if zero_input_start.is_some_and(|start| start + frame + 1 == LONG_TAIL_FRAMES) {
                self.probe.final_tail_left_bits.store(
                    frame_output[0].to_bits(),
                    std::sync::atomic::Ordering::Release,
                );
                self.probe.final_tail_right_bits.store(
                    frame_output[1].to_bits(),
                    std::sync::atomic::Ordering::Release,
                );
            }
            self.history[self.history_write] = [
                input[frame * LONG_TAIL_CHANNELS],
                input[frame * LONG_TAIL_CHANNELS + 1],
            ];
            self.history_write = (self.history_write + 1) % self.history.len();
        }

        Ok(context.num_frames)
    }

    fn reset(&mut self) {
        self.history.fill([0.0; LONG_TAIL_CHANNELS]);
        self.history_write = 0;
        self.probe
            .zero_input_frames
            .store(0, std::sync::atomic::Ordering::Release);
        self.probe
            .zero_input_blocks
            .store(0, std::sync::atomic::Ordering::Release);
        self.probe
            .zero_input_attempts
            .store(0, std::sync::atomic::Ordering::Release);
        self.probe
            .final_tail_left_bits
            .store(0, std::sync::atomic::Ordering::Release);
        self.probe
            .final_tail_right_bits
            .store(0, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(unix)]
#[test]
fn isolated_control_state_and_audio_share_transport_without_stale_sidecar() {
    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut external_descriptor = descriptor();
    external_descriptor.path = plugin_file.path().to_path_buf();
    let mut plugin = IsolatedExternalPlugin::new(
        external_descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/sleep").arg("30"),
            start_worker: false,
            deadline: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .unwrap();
    plugin.ensure_worker_running().unwrap();

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(StatefulScalePlugin {
            value: 1.0,
            history: 0.0,
        }),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, None);

    let parameters = match plugin
        .proxy
        .request_control(&PluginIpcControlRequest::Describe, Duration::from_secs(1))
        .unwrap()
    {
        PluginIpcControlResponse::Description {
            parameters,
            identity_frame_geometry,
            ..
        } => {
            plugin.identity_frame_geometry = identity_frame_geometry;
            parameters
        }
        response => panic!("unexpected description response: {response:?}"),
    };
    plugin.proxy.configure_parameters(
        parameters
            .iter()
            .map(|parameter| parameter.id.clone())
            .collect(),
    );
    plugin.parameter_values = parameters
        .iter()
        .map(|parameter| (parameter.id.clone(), parameter.default_value.clone()))
        .collect();
    plugin.parameters = parameters;

    assert!(
        plugin
            .begin_drain(&ProcessContext::new(48_000, 8_192))
            .is_err()
    );
    plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(4.0))
        .unwrap();
    assert_eq!(plugin.tail_length(), crate::plugin::TailLength::Infinite);
    assert!(
        plugin
            .begin_drain(&ProcessContext::new(48_000, 8_192))
            .is_err()
    );

    plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(2.5))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("value")),
        Some(ParameterValue::Float(2.5))
    );
    assert_eq!(
        plugin.tail_length(),
        crate::plugin::TailLength::Finite(128 + 8_192)
    );
    for (value, expected_tail) in [
        (1.0_f32, crate::plugin::TailLength::Unknown),
        (2.5_f32, crate::plugin::TailLength::Finite(128 + 8_192)),
    ] {
        assert!(matches!(
            plugin
                .proxy
                .request_control(
                    &PluginIpcControlRequest::LoadState {
                        state: value.to_le_bytes().to_vec(),
                    },
                    Duration::from_secs(1),
                )
                .unwrap(),
            PluginIpcControlResponse::Ack
        ));
        assert_eq!(plugin.tail_length(), expected_tail);
    }
    plugin
        .begin_drain(&ProcessContext::new(48_000, 8_192))
        .unwrap();
    let input = vec![0.4_f32; 8_192 * 2];
    let mut output = vec![0.0_f32; 8_192 * 2];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 8_192))
        .unwrap();
    let observer = SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let ready_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while observer.worker_state() != crate::external_plugin_ipc::PluginIpcState::WorkerReady {
        assert!(
            std::time::Instant::now() < ready_deadline,
            "worker did not finish primed block"
        );
        std::thread::yield_now();
    }
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 8_192))
        .unwrap();
    assert!(
        output.iter().all(|sample| (*sample - 1.0).abs() < 1e-6),
        "audio/control interleave timed out or used stale parameter state"
    );

    // The previous callback submits another asynchronous worker block.
    // This test checks metadata from completed automation, so ensure that
    // slot is free before submitting it. Deferred-event recovery is tested
    // separately with an intentionally occupied slot.
    let preceding_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while observer.worker_state() != crate::external_plugin_ipc::PluginIpcState::WorkerReady {
        assert!(
            std::time::Instant::now() < preceding_deadline,
            "worker did not finish the block preceding automation"
        );
        std::thread::yield_now();
    }
    let preceding_sequence = observer.host_sequence();

    let automation = [ParameterEvent::new(
        0,
        ParameterId::from("value"),
        ParameterValue::Float(4.0),
    )];
    plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(48_000, 8_192).with_parameter_events(&automation),
        )
        .unwrap();
    assert_eq!(
        observer.host_sequence(),
        preceding_sequence + 1,
        "automation block must be submitted rather than deferred"
    );
    let tail_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while plugin.tail_length() != crate::plugin::TailLength::Infinite {
        assert!(
            std::time::Instant::now() < tail_deadline,
            "completed automation did not refresh worker tail metadata"
        );
        std::thread::yield_now();
    }
    plugin
        .set_parameter(ParameterId::from("value"), ParameterValue::Float(2.5))
        .unwrap();
    assert_eq!(
        plugin.tail_length(),
        crate::plugin::TailLength::Finite(128 + 8_192)
    );

    plugin.reset_checked().unwrap();
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 8_192))
        .unwrap();
    // Read the completed reset render, rather than its intentionally valid
    // deadline fallback, when checking recursive processing history.
    let reset_ready_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while observer.worker_state() != crate::external_plugin_ipc::PluginIpcState::WorkerReady {
        assert!(
            std::time::Instant::now() < reset_ready_deadline,
            "worker did not finish the primed block after reset"
        );
        std::thread::yield_now();
    }
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 8_192))
        .unwrap();
    assert!(
        output.iter().all(|sample| (*sample - 1.0).abs() < 1e-6),
        "worker reset did not clear recursive processing history"
    );

    let state_bytes = match plugin
        .proxy
        .request_control(&PluginIpcControlRequest::SaveState, Duration::from_secs(1))
        .unwrap()
    {
        PluginIpcControlResponse::State(state) => state,
        response => panic!("unexpected state response: {response:?}"),
    };
    plugin.opaque_state = state_bytes;
    if let Some(supervisor) = plugin.supervisor.as_mut() {
        supervisor.terminate().unwrap();
    }
    plugin.supervisor = None;
    let captured = plugin.capture_worker_state().unwrap();
    assert_eq!(
        f32::from_le_bytes(captured.opaque_state.clone().try_into().unwrap()),
        2.5
    );
    let sidecar = plugin.state_file_path.as_ref().unwrap();
    let persisted: ExternalPluginState =
        serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
    assert_eq!(persisted.opaque_state, captured.opaque_state);
}

#[cfg(unix)]
#[test]
fn isolated_drain_preflight_is_idempotent_across_multiple_native_tail_blocks() {
    use std::sync::Arc;

    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut external_descriptor = descriptor();
    external_descriptor.path = plugin_file.path().to_path_buf();
    let mut plugin = IsolatedExternalPlugin::new(
        external_descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/sleep").arg("30"),
            start_worker: false,
            deadline: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .unwrap();
    plugin.ensure_worker_running().unwrap();

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let probe = Arc::new(LongTailProbe::default());
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(LongFiniteTailPlugin::new(Arc::clone(&probe))),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, None);

    let description = plugin
        .proxy
        .request_control(&PluginIpcControlRequest::Describe, Duration::from_secs(1))
        .unwrap();
    let PluginIpcControlResponse::Description {
        identity_frame_geometry,
        tail_length: crate::external_plugin_ipc::PluginIpcTailLength::Finite(16_384),
        ..
    } = description
    else {
        panic!("unexpected long-tail worker description: {description:?}");
    };
    plugin.identity_frame_geometry = identity_frame_geometry;

    let source = long_tail_input(2 * LONG_TAIL_PIPELINE_FRAMES);
    let actual = run_long_tail_stream(&mut plugin, &source);
    let expected = long_tail_scatter_reference(&source);
    assert_stream_matches_reference(&actual, &expected);
    assert_eq!(
        probe
            .zero_input_frames
            .load(std::sync::atomic::Ordering::Acquire),
        LONG_TAIL_FRAMES,
        "native drain must process exactly its advertised zero-input extent"
    );
    assert_eq!(
        probe
            .zero_input_blocks
            .load(std::sync::atomic::Ordering::Acquire),
        2,
        "native tail must span two full transport blocks"
    );
    assert_eq!(
        f32::from_bits(
            probe
                .final_tail_left_bits
                .load(std::sync::atomic::Ordering::Acquire)
        ),
        actual[actual.len() - 2],
        "final native zero-input frame was not delivered through pipeline flush"
    );
    assert_eq!(
        f32::from_bits(
            probe
                .final_tail_right_bits
                .load(std::sync::atomic::Ordering::Acquire)
        ),
        actual[actual.len() - 1],
        "final native zero-input frame lost the distinct right marker"
    );
    assert!(
        plugin
            .drain(
                &mut vec![0.0; LONG_TAIL_PIPELINE_FRAMES * LONG_TAIL_CHANNELS],
                &ProcessContext::new(48_000, 0)
            )
            .is_err(),
        "terminal drain accepted another step without a fresh begin"
    );

    plugin.reset_checked().unwrap();
    let replay = run_long_tail_stream(&mut plugin, &source);
    assert_eq!(
        replay, actual,
        "reset replay changed process-plus-drain audio"
    );
}

#[cfg(unix)]
#[test]
fn daw_host_short_drain_does_not_wait_for_pending_worker_and_timeout_retries() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut external_descriptor = descriptor();
    external_descriptor.path = plugin_file.path().to_path_buf();
    let mut plugin = IsolatedExternalPlugin::new(
        external_descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/sleep").arg("30"),
            start_worker: false,
            deadline: Duration::from_millis(100),
            worker_startup_timeout: Duration::from_millis(60),
            max_block_frames: LONG_TAIL_PIPELINE_FRAMES as u32,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.ensure_worker_running().unwrap();

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let probe = Arc::new(LongTailProbe::default());
    let gate = Arc::new(LongTailProcessGate::default());
    let input = long_tail_input(LONG_TAIL_PIPELINE_FRAMES);
    let final_marker = [
        input[input.len() - 2].to_bits(),
        input[input.len() - 1].to_bits(),
    ];
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(LongFiniteTailPlugin::with_gate(
            Arc::clone(&probe),
            Some(Arc::clone(&gate)),
            Some(final_marker),
        )),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, Some(Arc::clone(&gate)));

    let description = plugin
        .proxy
        .request_control(&PluginIpcControlRequest::Describe, Duration::from_secs(1))
        .unwrap();
    let PluginIpcControlResponse::Description {
        identity_frame_geometry,
        tail_length: crate::external_plugin_ipc::PluginIpcTailLength::Finite(16_384),
        ..
    } = description
    else {
        panic!("unexpected gated worker description: {description:?}");
    };
    plugin.identity_frame_geometry = identity_frame_geometry;

    let begin_calls = Arc::new(AtomicUsize::new(0));
    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let mut host = DawHost::new(LONG_TAIL_CHANNELS, 48_000);
    host.add_plugin(Box::new(DrainObservedPlugin {
        inner: plugin,
        prepare_calls: Arc::clone(&prepare_calls),
        begin_calls: Arc::clone(&begin_calls),
    }))
    .unwrap();
    host.build().unwrap();

    let mut process_output = vec![f32::NAN; input.len()];
    let processed = host.process(&input, &mut process_output).unwrap();
    assert_eq!(processed, LONG_TAIL_PIPELINE_FRAMES);
    assert!(
        gate.wait_until_entered(Duration::from_secs(1)),
        "worker never entered the final accepted request"
    );

    let drain_capacity = host.drain_output_frames_max();
    assert_eq!(drain_capacity, LONG_TAIL_PIPELINE_FRAMES);
    let fill = 91_337.0_f32;
    let mut short_output = vec![fill; (drain_capacity - 1) * LONG_TAIL_CHANNELS];
    let short_error = host.drain(&mut short_output).unwrap_err();
    assert!(short_error.contains("output too small"), "{short_error}");
    assert!(short_output.iter().all(|sample| *sample == fill));
    assert_eq!(
        begin_calls.load(Ordering::Acquire),
        0,
        "short-capacity preflight reached begin_drain and could wait on the pending worker"
    );
    assert_eq!(prepare_calls.load(Ordering::Acquire), 0);

    let mut drain_output = vec![f32::NAN; drain_capacity * LONG_TAIL_CHANNELS];
    let timeout_started = std::time::Instant::now();
    let timeout_error = host.drain(&mut drain_output).unwrap_err();
    assert!(
        timeout_error.contains("before drain timeout"),
        "expected bounded pending-request timeout, got: {timeout_error}"
    );
    assert!(
        timeout_started.elapsed() < Duration::from_secs(1),
        "metadata wait exceeded its bounded timeout"
    );
    assert_eq!(begin_calls.load(Ordering::Acquire), 1);
    assert_eq!(prepare_calls.load(Ordering::Acquire), 0);

    gate.release();
    let mut actual = process_output;
    let mut drained_frames = 0;
    let mut drain_calls = 0;
    loop {
        drain_output.fill(f32::NAN);
        let result = host.drain(&mut drain_output).unwrap();
        assert!(result.frames <= drain_capacity);
        actual.extend_from_slice(&drain_output[..result.frames * LONG_TAIL_CHANNELS]);
        drained_frames += result.frames;
        drain_calls += 1;
        if result.complete {
            break;
        }
        assert!(drain_calls < 5, "retry drain did not terminate");
    }
    assert_eq!(begin_calls.load(Ordering::Acquire), 2);
    assert_eq!(prepare_calls.load(Ordering::Acquire), 0);
    assert_eq!(drained_frames, LONG_TAIL_FRAMES + LONG_TAIL_PIPELINE_FRAMES);
    assert_eq!(drain_calls, 3);
    assert_stream_matches_reference(&actual, &long_tail_scatter_reference(&input));
}

#[cfg(unix)]
#[test]
fn channel_changing_drain_preflight_waits_before_begin_and_retries_full_route() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut external_descriptor = descriptor();
    external_descriptor.path = plugin_file.path().to_path_buf();
    let mut plugin = IsolatedExternalPlugin::new(
        external_descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/sleep").arg("30"),
            start_worker: false,
            deadline: Duration::from_millis(100),
            worker_startup_timeout: Duration::from_millis(60),
            max_block_frames: LONG_TAIL_PIPELINE_FRAMES as u32,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.ensure_worker_running().unwrap();

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let probe = Arc::new(LongTailProbe::default());
    let gate = Arc::new(LongTailProcessGate::default());
    let input = long_tail_input(LONG_TAIL_PIPELINE_FRAMES);
    let final_marker = [
        input[input.len() - 2].to_bits(),
        input[input.len() - 1].to_bits(),
    ];
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(LongFiniteTailPlugin::with_gate(
            Arc::clone(&probe),
            Some(Arc::clone(&gate)),
            Some(final_marker),
        )),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, Some(Arc::clone(&gate)));

    let description = plugin
        .proxy
        .request_control(&PluginIpcControlRequest::Describe, Duration::from_secs(1))
        .unwrap();
    let PluginIpcControlResponse::Description {
        identity_frame_geometry,
        tail_length: crate::external_plugin_ipc::PluginIpcTailLength::Finite(16_384),
        ..
    } = description
    else {
        panic!("unexpected gated worker description: {description:?}");
    };
    plugin.identity_frame_geometry = identity_frame_geometry;

    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let begin_calls = Arc::new(AtomicUsize::new(0));
    let mapped_frames = Arc::new(AtomicUsize::new(0));
    let mut host = DawHost::new(LONG_TAIL_CHANNELS, 48_000);
    host.add_plugin(Box::new(DrainObservedPlugin {
        inner: plugin,
        prepare_calls: Arc::clone(&prepare_calls),
        begin_calls: Arc::clone(&begin_calls),
    }))
    .unwrap();
    host.add_plugin(Box::new(StereoToQuadMapper {
        processed_frames: Arc::clone(&mapped_frames),
    }))
    .unwrap();
    host.build().unwrap();
    assert_eq!(host.output_channels(), 4);

    let mut process_output = vec![f32::NAN; input.len() / 2 * 4];
    let processed = host.process(&input, &mut process_output).unwrap();
    assert_eq!(processed, LONG_TAIL_PIPELINE_FRAMES);
    assert!(
        gate.wait_until_entered(Duration::from_secs(1)),
        "worker never entered the final accepted request"
    );

    let drain_capacity = host.drain_output_frames_max();
    assert_eq!(drain_capacity, LONG_TAIL_PIPELINE_FRAMES);
    let fill = 91_337.0_f32;
    let mut short_output = vec![fill; (drain_capacity - 1) * host.output_channels()];
    let short_error = host.drain(&mut short_output).unwrap_err();
    assert!(short_error.contains("output too small"), "{short_error}");
    assert!(short_output.iter().all(|sample| *sample == fill));
    assert_eq!(prepare_calls.load(Ordering::Acquire), 0);
    assert_eq!(begin_calls.load(Ordering::Acquire), 0);

    let mut drain_output = vec![f32::NAN; drain_capacity * host.output_channels()];
    let timeout_started = std::time::Instant::now();
    let timeout_error = host.drain(&mut drain_output).unwrap_err();
    assert!(
        timeout_error.contains("drain timeout"),
        "expected bounded metadata-preparation timeout, got: {timeout_error}"
    );
    assert!(timeout_started.elapsed() < Duration::from_secs(1));
    assert_eq!(prepare_calls.load(Ordering::Acquire), 1);
    assert_eq!(
        begin_calls.load(Ordering::Acquire),
        0,
        "metadata timeout must happen before begin_drain mutates plugin state"
    );
    assert!(drain_output.iter().all(|sample| sample.is_nan()));

    gate.release();
    let mut actual = process_output;
    let mut drained_frames = 0;
    let mut drain_calls = 0;
    let mut drain_results = Vec::new();
    loop {
        drain_output.fill(f32::NAN);
        let result = host.drain(&mut drain_output).unwrap();
        assert!(result.frames <= drain_capacity);
        drain_results.push((result.frames, result.complete));
        actual.extend_from_slice(&drain_output[..result.frames * host.output_channels()]);
        drained_frames += result.frames;
        drain_calls += 1;
        if result.complete {
            break;
        }
        assert!(drain_calls < 5, "retry drain did not terminate");
    }
    assert_eq!(prepare_calls.load(Ordering::Acquire), 2);
    assert_eq!(begin_calls.load(Ordering::Acquire), 1);
    assert_eq!(drained_frames, LONG_TAIL_FRAMES + LONG_TAIL_PIPELINE_FRAMES);
    assert_eq!(drain_calls, 4);
    assert_eq!(
        drain_results,
        [
            (LONG_TAIL_PIPELINE_FRAMES, false),
            (LONG_TAIL_PIPELINE_FRAMES, false),
            (LONG_TAIL_PIPELINE_FRAMES, false),
            (0, true),
        ],
        "native/pipeline frames must drain before the empty mapper completion call"
    );
    assert_eq!(
        mapped_frames.load(Ordering::Acquire),
        LONG_TAIL_PIPELINE_FRAMES + drained_frames,
        "downstream mapper did not process every input and drained frame exactly once"
    );
    let expected = stereo_to_quad_reference(&long_tail_scatter_reference(&input));
    assert_stream_matches_reference_with_channels(&actual, &expected, 4);
}

#[cfg(unix)]
#[test]
fn isolated_drain_failure_after_one_native_chunk_is_sticky_until_reset() {
    use std::sync::Arc;

    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut external_descriptor = descriptor();
    external_descriptor.path = plugin_file.path().to_path_buf();
    let mut plugin = IsolatedExternalPlugin::new(
        external_descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/sleep").arg("30"),
            start_worker: false,
            deadline: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .unwrap();
    plugin.ensure_worker_running().unwrap();

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let probe = Arc::new(LongTailProbe::default());
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(LongFiniteTailPlugin::new(Arc::clone(&probe)).fail_after_zero_input_blocks(1)),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, None);

    let description = plugin
        .proxy
        .request_control(&PluginIpcControlRequest::Describe, Duration::from_secs(1))
        .unwrap();
    let PluginIpcControlResponse::Description {
        identity_frame_geometry,
        tail_length: crate::external_plugin_ipc::PluginIpcTailLength::Finite(16_384),
        ..
    } = description
    else {
        panic!("unexpected failure-injection description: {description:?}");
    };
    plugin.identity_frame_geometry = identity_frame_geometry;

    let input = long_tail_input(LONG_TAIL_PIPELINE_FRAMES);
    let mut process_output = vec![f32::NAN; input.len()];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut process_output,
                &ProcessContext::new(48_000, LONG_TAIL_PIPELINE_FRAMES)
            )
            .unwrap(),
        LONG_TAIL_PIPELINE_FRAMES
    );
    plugin
        .proxy
        .wait_for_pending_for_drain(Duration::from_secs(1))
        .unwrap();
    plugin.begin_drain(&ProcessContext::new(48_000, 0)).unwrap();

    let mut tail_chunk = vec![f32::NAN; LONG_TAIL_PIPELINE_FRAMES * LONG_TAIL_CHANNELS];
    let first_tail = plugin
        .drain(&mut tail_chunk, &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(first_tail.frames, LONG_TAIL_PIPELINE_FRAMES);
    assert!(!first_tail.complete);
    assert!(tail_chunk.iter().any(|sample| sample.abs() > 1e-4));
    let mut failed_prefix = process_output.clone();
    failed_prefix.extend_from_slice(&tail_chunk);
    plugin
        .proxy
        .wait_for_pending_for_drain(Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        probe
            .zero_input_blocks
            .load(std::sync::atomic::Ordering::Acquire),
        1,
        "the failure must occur only after one native tail block completed"
    );

    let queued_tail = plugin
        .drain(&mut tail_chunk, &ProcessContext::new(48_000, 0))
        .unwrap();
    assert_eq!(queued_tail.frames, LONG_TAIL_PIPELINE_FRAMES);
    assert!(!queued_tail.complete);
    failed_prefix.extend_from_slice(&tail_chunk);

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let failed_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while worker_shared.worker_state() != crate::external_plugin_ipc::PluginIpcState::WorkerFailed {
        assert!(
            std::time::Instant::now() < failed_deadline,
            "second native-tail request did not publish its injected worker failure"
        );
        std::thread::yield_now();
    }

    let failure = plugin
        .drain(&mut tail_chunk, &ProcessContext::new(48_000, 0))
        .unwrap_err();
    assert!(failure.contains("worker failed during drain"), "{failure}");
    assert_prefix_matches_reference(&failed_prefix, &long_tail_scatter_reference(&input));
    assert_eq!(
        probe
            .zero_input_attempts
            .load(std::sync::atomic::Ordering::Acquire),
        2,
        "second tail request did not reach the injected worker failure"
    );
    assert!(
        plugin
            .drain(&mut tail_chunk, &ProcessContext::new(48_000, 0))
            .unwrap_err()
            .contains("requires reset"),
        "post-mutation drain failure was not sticky"
    );
    assert!(
        plugin
            .process(
                &input,
                &mut process_output,
                &ProcessContext::new(48_000, LONG_TAIL_PIPELINE_FRAMES),
            )
            .unwrap_err()
            .contains("requires reset"),
        "new input was accepted after a mutated drain failed"
    );
    assert!(
        plugin
            .prepare_drain_metadata()
            .unwrap_err()
            .contains("metadata cannot be prepared")
    );
    assert!(
        plugin
            .begin_drain(&ProcessContext::new(48_000, 0))
            .unwrap_err()
            .contains("requires reset")
    );

    plugin.reset_checked().unwrap();
    assert_eq!(
        probe
            .zero_input_blocks
            .load(std::sync::atomic::Ordering::Acquire),
        0,
        "reset did not clear native DSP history/probe state"
    );
    let mut actual =
        Vec::with_capacity((LONG_TAIL_PIPELINE_FRAMES * 2 + LONG_TAIL_FRAMES) * LONG_TAIL_CHANNELS);
    process_output.fill(f32::NAN);
    plugin
        .process(
            &input,
            &mut process_output,
            &ProcessContext::new(48_000, LONG_TAIL_PIPELINE_FRAMES),
        )
        .unwrap();
    actual.extend_from_slice(&process_output);
    plugin
        .proxy
        .wait_for_pending_for_drain(Duration::from_secs(1))
        .unwrap();
    plugin.begin_drain(&ProcessContext::new(48_000, 0)).unwrap();

    let mut drain_calls = 0;
    loop {
        tail_chunk.fill(f32::NAN);
        let result = plugin
            .drain(&mut tail_chunk, &ProcessContext::new(48_000, 0))
            .unwrap();
        assert!(result.frames <= LONG_TAIL_PIPELINE_FRAMES);
        actual.extend_from_slice(&tail_chunk[..result.frames * LONG_TAIL_CHANNELS]);
        drain_calls += 1;
        if drain_calls <= 2 {
            plugin
                .proxy
                .wait_for_pending_for_drain(Duration::from_secs(1))
                .unwrap();
        }
        if result.complete {
            break;
        }
        assert!(drain_calls < 5, "fresh post-reset replay did not terminate");
    }
    assert_eq!(drain_calls, 3);
    assert_eq!(
        probe
            .zero_input_frames
            .load(std::sync::atomic::Ordering::Acquire),
        LONG_TAIL_FRAMES
    );
    assert_eq!(
        probe
            .zero_input_blocks
            .load(std::sync::atomic::Ordering::Acquire),
        2
    );
    assert_stream_matches_reference(&actual, &long_tail_scatter_reference(&input));
}

fn long_tail_input(frames: usize) -> Vec<f32> {
    let mut input = Vec::with_capacity(frames * LONG_TAIL_CHANNELS);
    for frame in 0..frames {
        let left = 0.125 + (frame % 97) as f32 / 512.0;
        let right = -0.25 - (frame % 83) as f32 / 640.0;
        input.extend_from_slice(&[left, right]);
    }
    input[(frames - 1) * LONG_TAIL_CHANNELS] = 0.9375;
    input[(frames - 1) * LONG_TAIL_CHANNELS + 1] = -0.8125;
    input
}

fn long_tail_scatter_reference(input: &[f32]) -> Vec<f32> {
    let input_frames = input.len() / LONG_TAIL_CHANNELS;
    let convolution_frames = input_frames + LONG_TAIL_FRAMES;
    let mut convolution = vec![0.0_f64; convolution_frames * LONG_TAIL_CHANNELS];
    for input_frame in 0..input_frames {
        for (delay, coefficients) in LONG_TAIL_TAPS {
            for channel in 0..LONG_TAIL_CHANNELS {
                let output_frame = input_frame + delay;
                convolution[output_frame * LONG_TAIL_CHANNELS + channel] +=
                    f64::from(input[input_frame * LONG_TAIL_CHANNELS + channel])
                        * f64::from(coefficients[channel]);
            }
        }
    }

    let mut expected = vec![0.0_f32; LONG_TAIL_PIPELINE_FRAMES * LONG_TAIL_CHANNELS];
    expected.extend(convolution.into_iter().map(|sample| sample as f32));
    expected
}

fn stereo_to_quad_reference(stereo: &[f32]) -> Vec<f32> {
    let (stereo_frames, remainder) = stereo.as_chunks::<2>();
    assert!(remainder.is_empty());
    let mut quad = Vec::with_capacity(stereo.len() * 2);
    for frame in stereo_frames {
        quad.extend_from_slice(&[frame[0], frame[1], frame[0] * 0.5, frame[1] * -0.25]);
    }
    quad
}

fn run_long_tail_stream(plugin: &mut IsolatedExternalPlugin, input: &[f32]) -> Vec<f32> {
    let block_sizes = [4_096, 8_192, 4_096];
    assert_eq!(
        input.len() / LONG_TAIL_CHANNELS,
        block_sizes.iter().sum::<usize>()
    );
    let mut actual = Vec::with_capacity(
        (input.len() / LONG_TAIL_CHANNELS + LONG_TAIL_FRAMES + LONG_TAIL_PIPELINE_FRAMES)
            * LONG_TAIL_CHANNELS,
    );
    let mut frame_offset = 0;
    for (block_index, frames) in block_sizes.into_iter().enumerate() {
        let input_start = frame_offset * LONG_TAIL_CHANNELS;
        let input_end = input_start + frames * LONG_TAIL_CHANNELS;
        let mut output = vec![f32::NAN; frames * LONG_TAIL_CHANNELS];
        let processed = plugin
            .process(
                &input[input_start..input_end],
                &mut output,
                &ProcessContext::new(48_000, frames),
            )
            .unwrap();
        assert_eq!(processed, frames, "input callback {block_index}");
        actual.extend_from_slice(&output);
        assert!(
            plugin
                .proxy
                .wait_for_pending_for_drain(Duration::from_secs(1))
                .is_ok(),
            "worker did not publish and resolve input callback {block_index}"
        );
        frame_offset += frames;
    }

    let drain_context = ProcessContext::new(48_000, 0);
    plugin.begin_drain(&drain_context).unwrap();
    assert_eq!(
        plugin.tail_length(),
        crate::plugin::TailLength::Finite((LONG_TAIL_FRAMES + LONG_TAIL_PIPELINE_FRAMES) as u64)
    );
    let mut native_drained = 0usize;
    let mut pipeline_drained = 0usize;
    let mut drain_calls = 0;
    loop {
        // DawHost repeats metadata preparation before each host drain step.
        plugin.prepare_drain_metadata().unwrap();
        let mut output = vec![f32::NAN; LONG_TAIL_PIPELINE_FRAMES * LONG_TAIL_CHANNELS];
        let result = plugin.drain(&mut output, &drain_context).unwrap();
        assert!(result.frames <= LONG_TAIL_PIPELINE_FRAMES);
        actual.extend_from_slice(&output[..result.frames * LONG_TAIL_CHANNELS]);
        drain_calls += 1;
        if drain_calls <= 2 {
            native_drained += result.frames;
            assert!(
                plugin
                    .proxy
                    .wait_for_pending_for_drain(Duration::from_secs(1))
                    .is_ok(),
                "worker did not publish and resolve native-tail block {drain_calls}"
            );
        } else {
            pipeline_drained += result.frames;
        }
        if result.complete {
            assert_eq!(drain_calls, 3, "expected two native and one pipeline chunk");
            break;
        }
        assert!(drain_calls < 5, "finite tail drain did not terminate");
    }
    assert_eq!(native_drained, LONG_TAIL_FRAMES);
    assert_eq!(pipeline_drained, LONG_TAIL_PIPELINE_FRAMES);
    actual
}

fn assert_stream_matches_reference(actual: &[f32], expected: &[f32]) {
    assert_stream_matches_reference_with_channels(actual, expected, LONG_TAIL_CHANNELS);
}

fn assert_prefix_matches_reference(actual: &[f32], expected: &[f32]) {
    assert!(
        actual.len() <= expected.len(),
        "prefix exceeds reference extent"
    );
    assert!(actual.iter().all(|sample| sample.is_finite()));
    let max_residual = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_residual <= 2e-5,
        "max process/failure-prefix residual {max_residual}"
    );
}

fn assert_stream_matches_reference_with_channels(
    actual: &[f32],
    expected: &[f32],
    channels: usize,
) {
    assert_eq!(actual.len(), expected.len(), "full output length differs");
    assert_eq!(actual.len() % channels, 0, "output is not whole frames");
    assert!(actual.iter().all(|sample| sample.is_finite()));
    assert!(expected.iter().all(|sample| sample.is_finite()));
    let max_residual = actual
        .iter()
        .zip(expected)
        .map(|(actual, expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        max_residual <= 2e-5,
        "max full-stream residual {max_residual}"
    );
    let native_tail_start = expected.len() - LONG_TAIL_FRAMES * channels;
    assert!(
        actual[native_tail_start..]
            .iter()
            .any(|sample| sample.abs() > 1e-4),
        "native tail was silent; a dropped final tail block could pass"
    );
    assert!(
        actual[actual.len() - channels].abs() > 1e-3
            && actual[actual.len() - channels + 1].abs() > 1e-3
            && (actual[actual.len() - channels] - actual[actual.len() - channels + 1]).abs() > 1e-3,
        "the final native-tail frame must retain distinct nonzero source-channel markers"
    );
}

#[test]
fn isolated_external_plugin_rejects_unprobed_scanner_channel_metadata() {
    let mut descriptor = descriptor();
    descriptor.audio_inputs = 0;
    descriptor.audio_outputs = 0;
    let error = match IsolatedExternalPlugin::new(
        descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    ) {
        Ok(_) => panic!("unprobed scanner metadata must not define IPC layout"),
        Err(error) => error,
    };
    assert!(error.contains("unprobed channel metadata"));
}

#[test]
fn isolated_external_plugin_times_out_to_passthrough_without_worker() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            max_block_frames: 2,
            deadline: Duration::ZERO,
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    let _frames = plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    assert_eq!(output, vec![0.0; input.len()]);
    let frames = plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();

    assert_eq!(frames, 2);
    assert_eq!(output, input);
}

#[test]
fn isolated_external_plugin_rejects_launch_failure() {
    let error = match IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new(
                "/definitely/not/a/real/sotf/external/plugin/worker",
            ),
            ..Default::default()
        },
    ) {
        Ok(_) => panic!("launch failure must fail graph construction"),
        Err(error) => error,
    };

    assert!(error.contains("failed to launch isolated external plugin"));
}

#[test]
fn isolated_external_plugin_exposes_worker_poll_state() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert!(matches!(
        plugin.poll_worker().unwrap(),
        Some(ExternalPluginProcessEvent::NotRunning)
    ));
    assert_eq!(plugin.worker_start_count(), 0);
    assert_eq!(plugin.worker_exit_count(), 0);
}

#[test]
fn graph_host_reports_external_worker_with_stable_plugin_instance_id() {
    let plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            plugin_instance_id: Some(91),
            ..Default::default()
        },
    )
    .unwrap();
    let mut host = DawHost::new(2, 48_000);
    let node_id = host
        .add_node("external-graph-node".to_string(), Box::new(plugin))
        .unwrap();
    host.build().unwrap();

    let reports = host.poll_isolated_external_plugin_workers();

    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].node_id, node_id);
    assert_eq!(reports[0].plugin_instance_id, Some(91));
    assert!(matches!(
        reports[0].event,
        Some(ExternalPluginProcessEvent::NotRunning)
    ));
}

#[test]
fn isolated_external_plugin_reports_worker_metadata_without_mutating_graph_latency() {
    let plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(plugin.worker_reported_latency_samples(), None);
    assert_eq!(plugin.latency_samples(), 8_192);
    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    worker_shared.publish_worker_latency_samples(320);
    assert_eq!(plugin.worker_reported_latency_samples(), Some(320));
    assert_eq!(plugin.latency_samples(), 8_192);
}

#[test]
fn worker_latency_handshake_precedes_daw_host_latency_cache() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();
    let shared_path = plugin.proxy.shared_path().to_path_buf();
    let publisher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        let worker_shared = SecurePluginSharedMemory::open_existing(shared_path).unwrap();
        worker_shared.publish_worker_latency_samples(320);
    });

    assert_eq!(
        plugin
            .finalize_worker_latency_metadata(Duration::from_secs(1))
            .unwrap(),
        320
    );
    publisher.join().unwrap();
    assert_eq!(plugin.latency_samples(), 8_192 + 320);

    let worker_shared =
        SecurePluginSharedMemory::open_existing(plugin.proxy.shared_path()).unwrap();
    let worker = ExternalPluginWorker::new(
        worker_shared,
        Box::new(StatefulScalePlugin {
            value: 1.0,
            history: 0.0,
        }),
    )
    .unwrap();
    let _worker_thread = TestWorkerThread::spawn(worker, None);

    let mut host = DawHost::new(2, 48_000);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.build().unwrap();
    assert_eq!(host.total_latency_samples(), 8_192 + 320);
}

#[test]
fn worker_latency_handshake_timeout_is_actionable() {
    let plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    let error = plugin
        .wait_for_worker_latency_metadata(Duration::ZERO)
        .unwrap_err();
    assert!(error.contains("External Test"), "{error}");
    assert!(error.contains("latency metadata"), "{error}");
    assert!(error.contains("within 0 ms"), "{error}");
}

#[test]
fn isolated_external_plugin_exposes_block_failure_counters() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            max_block_frames: 2,
            deadline: Duration::ZERO,
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    assert_eq!(plugin.block_timeout_count(), 1);
    assert_eq!(plugin.block_worker_failure_count(), 0);
    assert_eq!(plugin.block_wrong_sequence_count(), 0);
}

#[test]
fn isolated_external_plugin_quarantines_after_repeated_block_failures() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            max_block_frames: 2,
            deadline: Duration::ZERO,
            start_worker: false,
            max_consecutive_block_failures: 2,
            ..Default::default()
        },
    )
    .unwrap();

    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();

    assert_eq!(
        plugin.launch_error(),
        Some(
            "isolated external plugin 'External Test' worker quarantined after 2 consecutive block failures"
        )
    );
    assert!(plugin.ensure_worker_running_event().is_err());
    output.fill(0.0);
    let context = ProcessContext::new(48_000, 2);
    assert_no_allocs("quarantined external-plugin fallback", || {
        for _ in 0..32 {
            plugin.process(&input, &mut output, &context).unwrap();
        }
    });
    assert_eq!(output, input);
}

#[cfg(unix)]
#[test]
fn isolated_external_plugin_propagates_supervisor_crash_loop_quarantine() {
    let mut plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            worker_command: ExternalPluginWorkerCommand::new("/bin/true"),
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    let mut quarantined = false;
    for _ in 0..500 {
        let _ = plugin.ensure_worker_running_event();
        if plugin.is_worker_quarantined() {
            quarantined = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        quarantined,
        "crash-looping worker must quarantine the plugin"
    );

    let reason = plugin
        .worker_quarantine_reason()
        .expect("quarantine must record a reason");
    assert!(
        reason.contains("quarantined"),
        "unexpected reason: {reason}"
    );
    assert!(plugin.ensure_worker_running_event().is_err());

    // A quarantined plugin renders the local fallback instead of touching
    // the dead worker: no restart is attempted.
    let starts_before = plugin.worker_start_count();
    let input = vec![0.25, -0.5, 1.0, -1.0];
    let mut output = vec![0.0; input.len()];
    let frames = plugin
        .process(&input, &mut output, &ProcessContext::new(48_000, 2))
        .unwrap();
    assert_eq!(frames, 2);
    assert_eq!(plugin.worker_start_count(), starts_before);
}

#[test]
fn isolated_external_plugin_placeholder_state_round_trips() {
    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut descriptor = descriptor();
    descriptor.path = plugin_file.path().to_path_buf();
    let plugin = IsolatedExternalPlugin::new(
        descriptor,
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();
    let mut state = plugin.placeholder_state();
    state.opaque_state = vec![9, 8, 7];

    let json = serde_json::to_string(&state).unwrap();
    let decoded: ExternalPluginState = serde_json::from_str(&json).unwrap();
    let restored = IsolatedExternalPlugin::from_placeholder_state(
        &decoded,
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(decoded.sandbox_mode, ExternalPluginSandboxMode::Isolated);
    assert_eq!(decoded.opaque_state, vec![9, 8, 7]);
    assert_eq!(restored.descriptor(), plugin.descriptor());

    let mut incompatible = decoded;
    incompatible.sandbox_mode = ExternalPluginSandboxMode::InProcess;
    let error = IsolatedExternalPlugin::from_placeholder_state(
        &incompatible,
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .err()
    .expect("in-process state must not restore in isolated host");
    assert!(error.contains("cannot restore isolated plugin"));
}

#[test]
fn isolated_external_plugin_persists_initial_state_for_worker_and_removes_sidecar_on_drop() {
    let plugin_file = tempfile::Builder::new().suffix(".clap").tempfile().unwrap();
    let mut descriptor = descriptor();
    descriptor.path = plugin_file.path().to_path_buf();
    let state = ExternalPluginState::new(
        descriptor,
        ExternalPluginSandboxMode::Isolated,
        vec![9, 8, 7, 6],
    );
    let plugin = IsolatedExternalPlugin::new(
        state.descriptor.clone(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            initial_state: Some(state.clone()),
            ..Default::default()
        },
    )
    .unwrap();

    let state_path = plugin
        .state_file_path
        .clone()
        .expect("initial state must create a worker sidecar");
    let on_disk: ExternalPluginState =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(on_disk, state);
    assert_eq!(plugin.placeholder_state(), state);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    drop(plugin);
    assert!(!state_path.exists());
}

#[test]
fn isolated_external_plugin_reports_same_hosting_plan_as_descriptor() {
    let plugin = IsolatedExternalPlugin::new(
        descriptor(),
        48_000,
        IsolatedExternalPluginConfig {
            start_worker: false,
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(
        plugin.hosting_plan(),
        plan_external_plugin_hosting(plugin.descriptor())
    );
}

#[test]
fn isolated_external_plugin_uses_selected_capability_sandbox_backend() {
    let config = IsolatedExternalPluginConfig {
        capability_sandbox_policy: Some(PluginSandboxPolicy::strict_with_preset_dir(
            "/tmp/sotf-presets",
        )),
        sandbox_launch_backend: PluginSandboxLaunchBackend::MacosAppSandboxHelper,
        sandbox_launcher_command: Some(ExternalPluginWorkerCommand::new(
            "/tmp/sotf-sandbox-helper",
        )),
        start_worker: false,
        ..Default::default()
    };

    let plugin = IsolatedExternalPlugin::new(descriptor(), 48_000, config).unwrap();

    assert_eq!(plugin.worker_start_count(), 0);
}

#[test]
fn isolated_external_plugin_rejects_helper_backend_without_launcher() {
    let config = IsolatedExternalPluginConfig {
        capability_sandbox_policy: Some(PluginSandboxPolicy::strict_with_preset_dir(
            "/tmp/sotf-presets",
        )),
        sandbox_launch_backend: PluginSandboxLaunchBackend::MacosAppSandboxHelper,
        start_worker: false,
        ..Default::default()
    };

    let err = match IsolatedExternalPlugin::new(descriptor(), 48_000, config) {
        Ok(_) => panic!("expected helper backend to require launcher command"),
        Err(err) => err,
    };

    assert!(err.contains("requires a host-owned sandbox launcher command"));
}

#[test]
fn isolated_external_plugin_rejects_windows_backend_without_launcher() {
    let config = IsolatedExternalPluginConfig {
        capability_sandbox_policy: Some(PluginSandboxPolicy::strict_with_preset_dir(
            "/tmp/sotf-presets",
        )),
        sandbox_launch_backend: PluginSandboxLaunchBackend::WindowsAppContainerWorker,
        start_worker: false,
        ..Default::default()
    };

    let err = match IsolatedExternalPlugin::new(descriptor(), 48_000, config) {
        Ok(_) => panic!("expected Windows backend to require launcher command"),
        Err(err) => err,
    };

    assert!(err.contains("requires a host-owned sandbox launcher command"));
}

#[test]
fn sandbox_launcher_command_receives_worker_metadata() {
    let config = IsolatedExternalPluginConfig {
        worker_command: ExternalPluginWorkerCommand::new("/tmp/sotf-worker")
            .arg("--idle-sleep-micros")
            .arg("50")
            .env("SOTF_WORKER_TEST", "1"),
        sandbox_launch_backend: PluginSandboxLaunchBackend::WindowsAppContainerWorker,
        sandbox_launcher_command: Some(ExternalPluginWorkerCommand::new(
            "/tmp/sotf-appcontainer-launcher",
        )),
        start_worker: false,
        ..Default::default()
    };

    let command =
        build_worker_launch_command(&config, "{\"id\":\"test\"}".to_string(), None, Vec::new())
            .unwrap();

    assert_eq!(
        command.program(),
        Path::new("/tmp/sotf-appcontainer-launcher")
    );
    assert_eq!(
        command.command_args(),
        &[
            "--sandbox-worker-binary".to_string(),
            "/tmp/sotf-worker".to_string(),
            "--sandbox-worker-arg".to_string(),
            "--idle-sleep-micros".to_string(),
            "--sandbox-worker-arg".to_string(),
            "50".to_string(),
            "--sandbox-worker-env".to_string(),
            "SOTF_WORKER_TEST=1".to_string(),
            "--descriptor-json".to_string(),
            "{\"id\":\"test\"}".to_string(),
        ]
    );
}

#[test]
fn sandbox_launcher_command_receives_external_state_sidecar() {
    let config = IsolatedExternalPluginConfig {
        worker_command: ExternalPluginWorkerCommand::new("/tmp/sotf-worker"),
        sandbox_launch_backend: PluginSandboxLaunchBackend::WindowsAppContainerWorker,
        sandbox_launcher_command: Some(ExternalPluginWorkerCommand::new(
            "/tmp/sotf-appcontainer-launcher",
        )),
        start_worker: false,
        ..Default::default()
    };
    let state_path = Path::new("/tmp/sotf-external-state.json");

    let command = build_worker_launch_command(
        &config,
        "{\"id\":\"test\"}".to_string(),
        Some(state_path),
        Vec::new(),
    )
    .unwrap();

    assert_eq!(
        command.command_args(),
        &[
            "--sandbox-worker-binary".to_string(),
            "/tmp/sotf-worker".to_string(),
            "--descriptor-json".to_string(),
            "{\"id\":\"test\"}".to_string(),
            "--external-state-file".to_string(),
            state_path.display().to_string(),
            "--sandbox-read-path".to_string(),
            state_path.display().to_string(),
        ]
    );
}

#[test]
fn isolated_external_plugin_rejects_capability_policy_for_process_only_backend() {
    let config = IsolatedExternalPluginConfig {
        capability_sandbox_policy: Some(PluginSandboxPolicy::strict_with_preset_dir(
            "/tmp/sotf-presets",
        )),
        sandbox_launch_backend: PluginSandboxLaunchBackend::ProcessIsolationOnly {
            platform: "test-process-only",
        },
        start_worker: false,
        ..Default::default()
    };

    let err = match IsolatedExternalPlugin::new(descriptor(), 48_000, config) {
        Ok(_) => panic!("expected process-only backend to reject strict capability policy"),
        Err(err) => err,
    };

    assert!(err.contains("cannot satisfy required policy"));
}
