use super::isolated_external_plugin_config::IsolatedExternalPluginConfig;
use super::isolated_external_plugin_config::build_worker_launch_command;
use crate::external_plugin::{
    ExternalPluginEditorData, ExternalPluginHostingPlan, ExternalPluginSandboxMode,
    ExternalPluginState, NativePluginAudioSetup, PluginDescriptor, PluginDescriptorProbeCache,
    plan_external_plugin_hosting,
};
use crate::external_plugin_host::{ExternalPluginHostBlockStatus, ExternalPluginHostProxy};
use crate::external_plugin_ipc::{
    PluginIpcControlRequest, PluginIpcControlResponse, PluginIpcTailLength,
};
use crate::external_plugin_ipc::{PluginIpcLayout, PluginSandboxRuntimeStatus};
use crate::external_plugin_process::{ExternalPluginProcessEvent, ExternalPluginProcessSupervisor};
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, TailLength,
};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct IsolatedExternalPlugin {
    pub(super) descriptor: PluginDescriptor,
    pub(super) audio_setup: Option<NativePluginAudioSetup>,
    pub(super) plugin_instance_id: Option<usize>,
    pub(super) proxy: ExternalPluginHostProxy,
    pub(super) supervisor: Option<ExternalPluginProcessSupervisor>,
    pub(super) input_channels: usize,
    pub(super) output_channels: usize,
    pub(super) latency_samples: usize,
    pub(super) launch_error: Option<String>,
    pub(super) consecutive_block_failures: u32,
    pub(super) max_consecutive_block_failures: u32,
    pub(super) quarantined: bool,
    pub(super) quarantine_reason: Option<String>,
    pub(super) opaque_state: Vec<u8>,
    pub(super) state_file_path: Option<PathBuf>,
    pub(super) parameters: Vec<Parameter>,
    pub(super) parameter_values: HashMap<ParameterId, ParameterValue>,
    editor_data: Option<Arc<ExternalPluginEditorData>>,
    pub(super) control_timeout: Duration,
    pub(super) identity_frame_geometry: bool,
    drain_zero_input: Vec<f32>,
    drain_started: bool,
    drain_failed: bool,
    remaining_native_tail_frames: u64,
    remaining_pipeline_frames: u64,
    sample_rate: f64,
}

impl IsolatedExternalPlugin {
    /// Resolve scanner-only metadata through a caller-owned quarantined probe
    /// process, then allocate IPC using the validated native bus layout.
    pub fn new_with_quarantined_probe<S: Into<f64>>(
        descriptor: PluginDescriptor,
        sample_rate: S,
        config: IsolatedExternalPluginConfig,
        cache: &mut PluginDescriptorProbeCache,
        probe: impl FnOnce(&PluginDescriptor) -> Result<PluginDescriptor, String>,
    ) -> Result<Self, String> {
        let descriptor = cache.resolve_with_quarantined_probe(&descriptor, probe)?;
        Self::new(descriptor, sample_rate, config)
    }

    pub fn new<S: Into<f64>>(
        descriptor: PluginDescriptor,
        sample_rate: S,
        config: IsolatedExternalPluginConfig,
    ) -> Result<Self, String> {
        let sample_rate = sample_rate.into();
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and positive".into());
        }
        if descriptor.audio_outputs == 0 {
            return Err(format!(
                "isolated external plugin '{}' has unprobed channel metadata; probe the native plugin before allocating IPC",
                descriptor.name
            ));
        }
        if let Some(state) = config.initial_state.as_ref() {
            state.validate()?;
            if state.descriptor != descriptor {
                return Err(format!(
                    "External plugin state targets '{}' at {}, not '{}' at {}",
                    state.descriptor.id,
                    state.descriptor.path.display(),
                    descriptor.id,
                    descriptor.path.display()
                ));
            }
            if state.sandbox_mode != ExternalPluginSandboxMode::Isolated {
                return Err(format!(
                    "External plugin state sandbox mode {:?} cannot restore isolated plugin",
                    state.sandbox_mode
                ));
            }
        }

        if let (Some(config_setup), Some(state_setup)) = (
            config.audio_setup.as_ref(),
            config
                .initial_state
                .as_ref()
                .and_then(|state| state.audio_setup.as_ref()),
        ) && config_setup != state_setup
        {
            return Err(
                "isolated external plugin config audio setup conflicts with its initial state"
                    .to_string(),
            );
        }
        let requested_audio_setup = config.audio_setup.as_ref().or_else(|| {
            config
                .initial_state
                .as_ref()
                .and_then(|state| state.audio_setup.as_ref())
        });
        let effective_audio_setup = NativePluginAudioSetup::for_descriptor_or_legacy_default(
            &descriptor,
            requested_audio_setup,
        )?;

        let (input_channels, output_channels) = if let Some(setup) = effective_audio_setup.as_ref()
        {
            setup.channel_counts()?
        } else {
            (descriptor.audio_inputs, descriptor.audio_outputs.max(1))
        };
        let layout = PluginIpcLayout::new(
            sample_rate,
            config.max_block_frames,
            input_channels as u32,
            output_channels as u32,
        )
        .map_err(|err| format!("invalid isolated external-plugin layout: {err}"))?;
        let proxy = ExternalPluginHostProxy::new(layout, config.deadline)?;
        let drain_zero_input = vec![0.0; config.max_block_frames as usize * input_channels];
        let descriptor_json = serde_json::to_string(&descriptor)
            .map_err(|err| format!("failed to serialize external plugin descriptor: {err}"))?;
        let path = proxy.shared_path().with_extension("state.json");
        let mut launch_state = config.initial_state.clone().unwrap_or_else(|| {
            ExternalPluginState::new(
                descriptor.clone(),
                ExternalPluginSandboxMode::Isolated,
                Vec::new(),
            )
        });
        if launch_state.audio_setup.is_none() {
            launch_state.audio_setup.clone_from(&config.audio_setup);
        }
        write_initial_state_file(&path, &launch_state)?;
        let state_file_path = Some(path);
        let sandbox_args = match &config.capability_sandbox_policy {
            Some(policy) => policy.command_args_for_backend(config.sandbox_launch_backend)?,
            None => config.sandbox_policy.command_args(),
        };
        let command = build_worker_launch_command(
            &config,
            descriptor_json,
            state_file_path.as_deref(),
            sandbox_args,
        )?;
        let mut supervisor =
            ExternalPluginProcessSupervisor::new(command, proxy.shared_path().to_path_buf())?;
        let launch_error = if config.start_worker {
            supervisor.ensure_running().err()
        } else {
            None
        };
        let quarantine_reason = format!(
            "isolated external plugin '{}' worker quarantined after {} consecutive block failures",
            descriptor.name, config.max_consecutive_block_failures
        );

        let mut plugin = Self {
            descriptor,
            audio_setup: requested_audio_setup.cloned(),
            plugin_instance_id: config.plugin_instance_id,
            proxy,
            supervisor: Some(supervisor),
            input_channels,
            output_channels,
            latency_samples: 0,
            quarantined: launch_error.is_some(),
            launch_error,
            consecutive_block_failures: 0,
            max_consecutive_block_failures: config.max_consecutive_block_failures,
            quarantine_reason: Some(quarantine_reason),
            opaque_state: config
                .initial_state
                .as_ref()
                .map(|state| state.opaque_state.clone())
                .unwrap_or_default(),
            state_file_path,
            parameters: Vec::new(),
            parameter_values: HashMap::new(),
            editor_data: None,
            control_timeout: config.worker_startup_timeout,
            identity_frame_geometry: false,
            drain_zero_input,
            drain_started: false,
            drain_failed: false,
            remaining_native_tail_frames: 0,
            remaining_pipeline_frames: 0,
            sample_rate,
        };

        if let Some(error) = plugin.launch_error.take() {
            if let Some(supervisor) = plugin.supervisor.as_mut() {
                let _ = supervisor.terminate();
            }
            return Err(format!(
                "failed to launch isolated external plugin '{}': {error}",
                plugin.descriptor.name
            ));
        }

        if config.start_worker
            && let Err(mut error) =
                plugin.finalize_worker_latency_metadata(config.worker_startup_timeout)
        {
            if let Some(supervisor) = plugin.supervisor.as_mut() {
                if let Ok(Some(ExternalPluginProcessEvent::Exited { status })) = supervisor.poll() {
                    let detail = supervisor
                        .last_stderr()
                        .map(|stderr| format!(": {stderr}"))
                        .unwrap_or_default();
                    error = format!(
                        "isolated external plugin '{}' worker exited with {status} before publishing latency metadata{detail}",
                        plugin.descriptor.name,
                    );
                }
                let _ = supervisor.terminate();
            }
            return Err(error);
        }

        if config.start_worker {
            plugin.refresh_editor_data()?;
        }

        Ok(plugin)
    }

    /// Refresh immutable editor state between worker audio requests.
    ///
    /// # Errors
    /// Returns an IPC or worker error. Cached values are cleared first so a failed
    /// readback cannot masquerade as the result of an accepted native change.
    pub(super) fn refresh_editor_data(&mut self) -> Result<(), String> {
        self.editor_data = None;
        self.parameter_values.clear();
        match self
            .proxy
            .request_control(&PluginIpcControlRequest::Describe, self.control_timeout)?
        {
            PluginIpcControlResponse::Description {
                parameters,
                parameter_values,
                identity_frame_geometry,
                ..
            } => {
                self.proxy.configure_parameters(
                    parameters
                        .iter()
                        .map(|parameter| parameter.id.clone())
                        .collect(),
                );
                self.parameters = parameters;
                self.parameter_values = parameter_values;
                self.identity_frame_geometry = identity_frame_geometry;
                let native_state = match self
                    .proxy
                    .request_control(&PluginIpcControlRequest::SaveState, self.control_timeout)
                {
                    Ok(PluginIpcControlResponse::State(bytes)) => {
                        self.opaque_state = bytes;
                        Ok(self.placeholder_state())
                    }
                    Ok(PluginIpcControlResponse::Error(error)) => Err(error),
                    Ok(_) => Err("external-plugin worker returned invalid state response".into()),
                    Err(error) => Err(error),
                };
                self.editor_data = Some(Arc::new(ExternalPluginEditorData {
                    plugin_instance_id: self.plugin_instance_id,
                    descriptor: self.descriptor.clone(),
                    parameters: self.parameters.clone(),
                    parameter_values: self.parameter_values.clone(),
                    native_state,
                }));
                Ok(())
            }
            PluginIpcControlResponse::Error(error) => Err(error),
            _ => Err("external-plugin worker returned invalid description".to_string()),
        }
    }

    pub fn descriptor(&self) -> &PluginDescriptor {
        &self.descriptor
    }

    pub fn plugin_instance_id(&self) -> Option<usize> {
        self.plugin_instance_id
    }

    pub fn hosting_plan(&self) -> ExternalPluginHostingPlan {
        plan_external_plugin_hosting(&self.descriptor)
    }

    pub fn placeholder_state(&self) -> ExternalPluginState {
        let mut state = ExternalPluginState::new(
            self.descriptor.clone(),
            ExternalPluginSandboxMode::Isolated,
            self.opaque_state.clone(),
        );
        state.audio_setup.clone_from(&self.audio_setup);
        state
    }

    pub fn capture_worker_state(&mut self) -> Result<ExternalPluginState, String> {
        let worker_running = match self.supervisor.as_mut() {
            Some(supervisor) => supervisor.is_running()?,
            None => false,
        };
        if worker_running {
            match self
                .proxy
                .request_control(&PluginIpcControlRequest::SaveState, self.control_timeout)?
            {
                PluginIpcControlResponse::State(state) => self.opaque_state = state,
                PluginIpcControlResponse::Error(error) => return Err(error),
                _ => return Err("external-plugin worker returned invalid state response".into()),
            }
        }
        let state = self.placeholder_state();
        if let Some(path) = self.state_file_path.as_ref() {
            write_initial_state_file(path, &state)?;
        }
        Ok(state)
    }

    pub fn from_placeholder_state<S: Into<f64>>(
        state: &ExternalPluginState,
        sample_rate: S,
        config: IsolatedExternalPluginConfig,
    ) -> Result<Self, String> {
        state.validate()?;
        if state.sandbox_mode != ExternalPluginSandboxMode::Isolated {
            return Err(format!(
                "External plugin state sandbox mode {:?} cannot restore isolated plugin",
                state.sandbox_mode
            ));
        }
        let mut config = config;
        config.initial_state = Some(state.clone());
        config.audio_setup.clone_from(&state.audio_setup);
        Self::new(state.descriptor.clone(), sample_rate, config)
    }

    pub fn launch_error(&self) -> Option<&str> {
        self.launch_error.as_deref()
    }

    pub fn ensure_worker_running(&mut self) -> Result<(), String> {
        self.ensure_worker_running_event().map(|_| ())
    }

    pub fn ensure_worker_running_event(&mut self) -> Result<ExternalPluginProcessEvent, String> {
        if self.quarantined {
            return Err(self.launch_error.clone().unwrap_or_else(|| {
                format!(
                    "isolated external plugin '{}' worker is quarantined",
                    self.descriptor.name
                )
            }));
        }
        let Some(supervisor) = self.supervisor.as_mut() else {
            return Ok(ExternalPluginProcessEvent::NotRunning);
        };
        let result = supervisor.ensure_running();
        let supervisor_quarantined = supervisor.quarantined();
        if let Err(err) = &result {
            if supervisor_quarantined {
                self.quarantine_worker(format!(
                    "isolated external plugin '{}' worker quarantined: {err}",
                    self.descriptor.name
                ));
            } else {
                self.launch_error = Some(err.clone());
            }
        }
        result
    }

    pub fn poll_worker(&mut self) -> Result<Option<ExternalPluginProcessEvent>, String> {
        let Some(supervisor) = self.supervisor.as_mut() else {
            return Ok(Some(ExternalPluginProcessEvent::NotRunning));
        };
        let result = supervisor.poll();
        if supervisor.quarantined() {
            self.sync_supervisor_quarantine();
        }
        result
    }

    /// Engage plugin-level quarantine when the supervisor stopped restarting a
    /// crash-looping worker, so the engine-visible state and audio fallback
    /// reflect the supervisor decision even when only the poll path runs.
    fn sync_supervisor_quarantine(&mut self) {
        if self.quarantined {
            return;
        }
        let exit_count = self
            .supervisor
            .as_ref()
            .map_or(0, ExternalPluginProcessSupervisor::exit_count);
        self.quarantine_worker(format!(
            "isolated external plugin '{}' worker quarantined after repeated quick exits ({exit_count} observed)",
            self.descriptor.name
        ));
    }

    pub fn worker_start_count(&self) -> u64 {
        self.supervisor
            .as_ref()
            .map_or(0, ExternalPluginProcessSupervisor::start_count)
    }

    pub fn worker_exit_count(&self) -> u64 {
        self.supervisor
            .as_ref()
            .map_or(0, ExternalPluginProcessSupervisor::exit_count)
    }

    pub fn worker_launch_failure_count(&self) -> u64 {
        self.supervisor
            .as_ref()
            .map_or(0, ExternalPluginProcessSupervisor::launch_failure_count)
    }

    pub fn is_worker_quarantined(&self) -> bool {
        self.quarantined
            || self
                .supervisor
                .as_ref()
                .is_some_and(ExternalPluginProcessSupervisor::quarantined)
    }

    pub fn worker_quarantine_reason(&self) -> Option<&str> {
        self.launch_error.as_deref()
    }

    /// Latest stderr captured from the worker process, if any. This is where
    /// worker-side sandbox entry failures are reported.
    pub fn last_worker_stderr(&self) -> Option<String> {
        self.supervisor
            .as_ref()
            .and_then(ExternalPluginProcessSupervisor::last_stderr)
            .map(|stderr| stderr.to_string())
    }

    pub fn block_timeout_count(&self) -> u64 {
        self.proxy.timeout_count()
    }

    pub fn block_worker_failure_count(&self) -> u64 {
        self.proxy.worker_failure_count()
    }

    pub fn block_wrong_sequence_count(&self) -> u64 {
        self.proxy.wrong_sequence_count()
    }

    pub fn worker_sandbox_status(&self) -> PluginSandboxRuntimeStatus {
        self.proxy.worker_sandbox_status()
    }

    pub fn worker_reported_latency_samples(&self) -> Option<usize> {
        self.proxy.worker_latency_samples()
    }

    /// Wait for immutable worker metadata on the control/build thread.
    ///
    /// `DawHost::build` caches both total latency and compensation delays, so
    /// construction must not return a running plugin whose latency is still
    /// unknown. This bounded wait never runs from `Plugin::process`.
    pub(super) fn wait_for_worker_latency_metadata(
        &self,
        timeout: Duration,
    ) -> Result<usize, String> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| "external plugin worker startup timeout is too large".to_string())?;
        loop {
            if let Some(latency_samples) = self.worker_reported_latency_samples() {
                return Ok(latency_samples);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(format!(
                    "isolated external plugin '{}' worker did not publish latency metadata within {} ms",
                    self.descriptor.name,
                    timeout.as_millis(),
                ));
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(1)));
        }
    }

    /// Freeze worker latency into both the graph-visible contract and the dry
    /// fallback delay before the plugin can be added to a built host graph.
    pub(super) fn finalize_worker_latency_metadata(
        &mut self,
        timeout: Duration,
    ) -> Result<usize, String> {
        let latency_samples = self.wait_for_worker_latency_metadata(timeout)?;
        self.proxy.configure_fallback_latency(latency_samples)?;
        self.latency_samples = latency_samples;
        Ok(latency_samples)
    }

    pub(super) fn validate_process_buffers(
        &self,
        input: &[f32],
        output: &[f32],
        frames: usize,
    ) -> Result<(), String> {
        let max_frames = self.proxy.pipeline_latency_samples();
        if frames > max_frames {
            return Err(format!(
                "isolated external plugin '{}' received {frames} frames but its configured maximum is {max_frames}",
                self.descriptor.name
            ));
        }
        let expected_input = frames
            .checked_mul(self.input_channels)
            .ok_or_else(|| "isolated external plugin input length overflow".to_string())?;
        let expected_output = frames
            .checked_mul(self.output_channels)
            .ok_or_else(|| "isolated external plugin output length overflow".to_string())?;
        if input.len() < expected_input {
            return Err(format!(
                "isolated external plugin '{}' received {} input samples but expected at least {expected_input}",
                self.descriptor.name,
                input.len()
            ));
        }
        if output.len() < expected_output {
            return Err(format!(
                "isolated external plugin '{}' received {} output samples but expected at least {expected_output}",
                self.descriptor.name,
                output.len()
            ));
        }
        Ok(())
    }

    pub(super) fn write_fallback(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        frames: usize,
    ) -> usize {
        self.proxy.process_fallback(input, output, frames)
    }

    pub(super) fn record_block_status(&mut self, status: ExternalPluginHostBlockStatus) {
        if matches!(
            status,
            ExternalPluginHostBlockStatus::Processed | ExternalPluginHostBlockStatus::Priming
        ) {
            self.consecutive_block_failures = 0;
            return;
        }

        self.consecutive_block_failures = self.consecutive_block_failures.saturating_add(1);
        if self.max_consecutive_block_failures > 0
            && self.consecutive_block_failures >= self.max_consecutive_block_failures
            && let Some(reason) = self.quarantine_reason.take()
        {
            self.quarantine_worker(reason);
        }
    }

    pub(super) fn quarantine_worker(&mut self, reason: String) {
        if self.quarantined {
            return;
        }
        if let Some(supervisor) = self.supervisor.as_ref() {
            let _ = supervisor.request_terminate();
        }
        self.launch_error = Some(reason);
        self.quarantined = true;
        if let Some(reason) = self.launch_error.as_deref() {
            crate::rate_limited_log!(warn, 1, "{reason}");
        }
    }
}

fn write_initial_state_file(path: &Path, state: &ExternalPluginState) -> Result<(), String> {
    let bytes = serde_json::to_vec(state)
        .map_err(|error| format!("failed to serialize external plugin state: {error}"))?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("failed to create external plugin state file: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("failed to write external plugin state file: {error}"))
}

impl Drop for IsolatedExternalPlugin {
    fn drop(&mut self) {
        if let Some(supervisor) = self.supervisor.as_mut() {
            let _ = supervisor.terminate();
        }
        if let Some(path) = self.state_file_path.as_ref() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Plugin for IsolatedExternalPlugin {
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn info(&self) -> PluginInfo {
        PluginInfo::new(
            &self.descriptor.name,
            &self.descriptor.version,
            &self.descriptor.vendor,
        )
    }

    fn cost_class(&self) -> PluginCostClass {
        PluginCostClass::External
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::boundary(PluginCostClass::External, self.latency_samples())
    }

    fn input_channels(&self) -> usize {
        self.input_channels
    }

    fn output_channels(&self) -> usize {
        self.output_channels
    }

    fn latency_samples(&self) -> usize {
        self.latency_samples
            .saturating_add(self.proxy.pipeline_latency_samples())
    }

    fn tail_length(&self) -> TailLength {
        if self.quarantined {
            return TailLength::Unknown;
        }
        let native_tail = match self.proxy.worker_tail_length() {
            PluginIpcTailLength::Finite(frames) => TailLength::Finite(frames),
            PluginIpcTailLength::Infinite => TailLength::Infinite,
            PluginIpcTailLength::Unknown => TailLength::Unknown,
        };
        match native_tail {
            TailLength::Finite(native_frames) => native_frames
                .checked_add(self.proxy.pipeline_latency_samples() as u64)
                .map_or(TailLength::Infinite, TailLength::Finite),
            TailLength::Infinite => TailLength::Infinite,
            TailLength::Unknown => TailLength::Unknown,
        }
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.identity_frame_geometry
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite() || sample_rate != self.sample_rate {
            return Err(format!(
                "isolated external plugin '{}' was prepared at {} Hz, not {sample_rate} Hz",
                self.descriptor.name, self.sample_rate
            ));
        }
        self.reset_checked()?;
        if self.quarantined {
            return Err(self.launch_error.clone().unwrap_or_else(|| {
                format!(
                    "isolated external plugin '{}' could not reset its worker timeline",
                    self.descriptor.name
                )
            }));
        }
        Ok(())
    }

    fn reset(&mut self) {
        let _ = self.reset_checked();
    }

    fn reset_checked(&mut self) -> PluginResult<()> {
        if self.quarantined {
            return Err(self.launch_error.clone().unwrap_or_else(|| {
                format!(
                    "isolated external plugin '{}' worker is quarantined",
                    self.descriptor.name
                )
            }));
        }
        let worker_is_running = self.proxy.worker_latency_samples().is_some();
        let pending_result = self
            .proxy
            .wait_for_pending_for_drain(self.control_timeout)
            .or_else(|wait_error| {
                if self.proxy.discard_failed_pending_for_reset() {
                    Ok(())
                } else {
                    Err(wait_error)
                }
            });
        let reset_result = pending_result.and_then(|()| {
            if !worker_is_running {
                return Ok(());
            }
            match self
                .proxy
                .request_control(&PluginIpcControlRequest::Reset, self.control_timeout)?
            {
                PluginIpcControlResponse::Ack => Ok(()),
                PluginIpcControlResponse::Error(error) => Err(error),
                _ => Err("external-plugin worker returned invalid reset response".into()),
            }
        });
        if let Err(error) = reset_result {
            self.drain_failed = true;
            self.quarantine_worker(format!(
                "isolated external plugin '{}' could not reset native DSP state: {error}",
                self.descriptor.name
            ));
            return Err(error);
        }
        if worker_is_running {
            self.refresh_editor_data()?;
        }
        if let Err(error) = self.proxy.reset_timeline_after_drain() {
            self.drain_failed = true;
            self.quarantine_worker(format!(
                "isolated external plugin '{}' could not reset its IPC timeline: {error}",
                self.descriptor.name
            ));
            return Err(error);
        }
        self.drain_started = false;
        self.drain_failed = false;
        self.remaining_native_tail_frames = 0;
        self.remaining_pipeline_frames = 0;
        Ok(())
    }

    fn parameters(&self) -> Vec<Parameter> {
        self.parameters.clone()
    }

    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
        let parameter = self
            .parameters
            .iter()
            .find(|parameter| parameter.id == id)
            .ok_or_else(|| {
                format!(
                    "isolated external plugin '{}' has no parameter '{id}'",
                    self.descriptor.name
                )
            })?;
        parameter.validate(&value)?;
        match self.proxy.request_control(
            &PluginIpcControlRequest::Set {
                id: id.clone(),
                value: value.clone(),
            },
            self.control_timeout,
        )? {
            PluginIpcControlResponse::Ack => self.refresh_editor_data().map_err(|error| {
                format!("native parameter changed but editor readback failed: {error}")
            }),
            PluginIpcControlResponse::Error(error) => Err(error),
            _ => Err("external-plugin worker returned invalid parameter response".into()),
        }
    }

    fn get_data(&self) -> Option<Arc<dyn std::any::Any + Send + Sync>> {
        if self.quarantined {
            return None;
        }
        self.editor_data
            .as_ref()
            .map(|data| data.clone() as Arc<dyn std::any::Any + Send + Sync>)
    }

    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
        self.parameter_values.get(id).cloned()
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<usize> {
        self.validate_process_buffers(input, output, context.num_frames)?;
        if self.quarantined {
            self.drain_started = false;
            self.remaining_native_tail_frames = 0;
            self.remaining_pipeline_frames = 0;
            return Ok(self.write_fallback(input, output, context.num_frames));
        }
        if self.drain_failed {
            return Err(
                "isolated external-plugin drain requires reset after a prior failure".into(),
            );
        }

        let (frames, status) = self
            .proxy
            .process_block_with_context(input, output, context)
            .map_err(|err| format!("isolated external plugin processing failed: {err}"))?;
        self.record_block_status(status);
        self.drain_started = false;
        self.remaining_native_tail_frames = 0;
        self.remaining_pipeline_frames = 0;

        match status {
            ExternalPluginHostBlockStatus::Priming => {}
            ExternalPluginHostBlockStatus::Processed => {}
            ExternalPluginHostBlockStatus::TimedOut => {
                crate::rate_limited_log!(
                    warn,
                    5,
                    "isolated external plugin '{}' missed block deadline; using passthrough",
                    self.descriptor.name
                );
            }
            ExternalPluginHostBlockStatus::WorkerFailed => {
                crate::rate_limited_log!(
                    warn,
                    5,
                    "isolated external plugin '{}' worker failed; using passthrough",
                    self.descriptor.name
                );
            }
            ExternalPluginHostBlockStatus::WrongSequence => {
                crate::rate_limited_log!(
                    warn,
                    5,
                    "isolated external plugin '{}' returned stale block; using passthrough",
                    self.descriptor.name
                );
            }
        }

        Ok(frames)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.proxy.pipeline_latency_samples()
    }

    fn drain_frames_envelope(&self) -> Option<usize> {
        // Native state across the isolation boundary is unproven;
        // unknown keeps the host's live sizing.
        None
    }

    fn output_frames_envelope(&self, _input_frames: usize) -> Option<usize> {
        // Native production across the isolation boundary is unproven;
        // unknown keeps the host's live sizing.
        None
    }

    fn prepare_drain_metadata(&mut self) -> PluginResult<()> {
        if self.drain_started {
            return Ok(());
        }
        if self.drain_failed {
            return Err(
                "isolated external-plugin metadata cannot be prepared during a drain".into(),
            );
        }
        if self.quarantined {
            return Err(format!(
                "isolated external plugin '{}' cannot prepare metadata for a quarantined worker",
                self.descriptor.name
            ));
        }
        if !self.identity_frame_geometry {
            return Err(format!(
                "isolated external plugin '{}' did not report identity frame geometry",
                self.descriptor.name
            ));
        }
        if self.proxy.emitted_degraded_output() {
            self.drain_failed = true;
            return Err(format!(
                "isolated external plugin '{}' already emitted timeout/failure fallback; refusing to report it as drained DSP audio",
                self.descriptor.name
            ));
        }
        // EOS is the one bounded blocking point: finish work for input already
        // accepted by the worker before the host asks for its final tail bound.
        self.proxy
            .wait_for_pending_for_drain(self.control_timeout)?;
        if self.proxy.emitted_degraded_output() {
            self.drain_failed = true;
            return Err(format!(
                "isolated external plugin '{}' already emitted timeout/failure fallback; refusing to report it as drained DSP audio",
                self.descriptor.name
            ));
        }
        Ok(())
    }

    fn begin_drain(&mut self, _context: &ProcessContext) -> PluginResult<()> {
        if self.drain_started {
            return Ok(());
        }
        if self.drain_failed {
            return Err(
                "isolated external-plugin drain requires reset after a prior failure".into(),
            );
        }
        if self.quarantined {
            return Err(format!(
                "isolated external plugin '{}' cannot drain a quarantined worker",
                self.descriptor.name
            ));
        }
        if !self.identity_frame_geometry {
            return Err(format!(
                "isolated external plugin '{}' did not report identity frame geometry",
                self.descriptor.name
            ));
        }
        if self.proxy.emitted_degraded_output() {
            self.drain_failed = true;
            return Err(format!(
                "isolated external plugin '{}' already emitted timeout/failure fallback; refusing to report it as drained DSP audio",
                self.descriptor.name
            ));
        }
        self.prepare_drain_metadata()?;
        let pipeline_frames = self.proxy.pipeline_latency_samples() as u64;
        let native_tail_frames = match self.tail_length() {
            TailLength::Finite(frames) => frames
                .checked_sub(pipeline_frames)
                .ok_or_else(|| "isolated external-plugin drain extent underflow".to_string())?,
            TailLength::Infinite => {
                return Err(format!(
                    "isolated external plugin '{}' reports an infinite native tail",
                    self.descriptor.name
                ));
            }
            TailLength::Unknown => {
                return Err(format!(
                    "isolated external plugin '{}' has unknown native tail metadata",
                    self.descriptor.name
                ));
            }
        };
        native_tail_frames
            .checked_add(pipeline_frames)
            .ok_or_else(|| "isolated external-plugin drain extent overflow".to_string())?;

        self.remaining_native_tail_frames = native_tail_frames;
        self.remaining_pipeline_frames = pipeline_frames;
        self.drain_started = true;
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if !self.drain_started || self.drain_failed {
            return None;
        }
        let block = u64::try_from(self.drain_output_frames_max()).ok()?.max(1);
        let ceil_calls = |frames: u64| frames / block + u64::from(!frames.is_multiple_of(block));
        std::num::NonZeroU64::new(
            ceil_calls(self.remaining_native_tail_frames)
                .saturating_add(ceil_calls(self.remaining_pipeline_frames))
                .max(1),
        )
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        if !self.drain_started || self.drain_failed {
            return Err("isolated external-plugin drain was not prepared or requires reset".into());
        }
        if self.proxy.emitted_degraded_output() {
            self.drain_failed = true;
            return Err(
                "isolated external-plugin drain encountered previously emitted fallback audio"
                    .into(),
            );
        }
        let max_frames = self.drain_output_frames_max();
        if max_frames == 0 {
            return Err("isolated external-plugin drain requires positive frame capacity".into());
        }
        let channels = self.output_channels;
        if output.len() < max_frames.saturating_mul(channels) {
            return Err(format!(
                "isolated external-plugin drain output is too small: need {} samples, got {}",
                max_frames.saturating_mul(channels),
                output.len()
            ));
        }

        let result = (|| {
            if self.remaining_native_tail_frames > 0 {
                let frames = usize::try_from(self.remaining_native_tail_frames)
                    .unwrap_or(usize::MAX)
                    .min(max_frames);
                let input_samples = frames.checked_mul(self.input_channels).ok_or_else(|| {
                    "isolated external-plugin drain input extent overflow".to_string()
                })?;
                let output_samples = frames.checked_mul(channels).ok_or_else(|| {
                    "isolated external-plugin drain output extent overflow".to_string()
                })?;
                let mut drain_context = *context;
                drain_context.num_frames = frames;
                self.drain_zero_input[..input_samples].fill(0.0);
                self.proxy
                    .wait_for_pending_for_drain(self.control_timeout)?;
                let (processed, status) = self.proxy.process_block_with_context(
                    &self.drain_zero_input[..input_samples],
                    &mut output[..output_samples],
                    &drain_context,
                )?;
                if processed != frames
                    || !matches!(
                        status,
                        ExternalPluginHostBlockStatus::Processed
                            | ExternalPluginHostBlockStatus::Priming
                    )
                    || self.proxy.emitted_degraded_output()
                {
                    Err(format!(
                        "isolated external-plugin native-tail drain returned {processed}/{frames} frames with {status:?}"
                    ))
                } else {
                    self.remaining_native_tail_frames -= frames as u64;
                    Ok(PluginDrainResult {
                        frames,
                        complete: false,
                    })
                }
            } else if self.remaining_pipeline_frames > 0 {
                let frames = usize::try_from(self.remaining_pipeline_frames)
                    .unwrap_or(usize::MAX)
                    .min(max_frames);
                let output_samples = frames
                    .checked_mul(channels)
                    .ok_or_else(|| "isolated external-plugin flush extent overflow".to_string())?;
                self.proxy
                    .wait_for_pending_for_drain(self.control_timeout)?;
                let status = self
                    .proxy
                    .flush_timeline_for_drain(&mut output[..output_samples], frames)?;
                if !matches!(
                    status,
                    ExternalPluginHostBlockStatus::Processed
                        | ExternalPluginHostBlockStatus::Priming
                ) || self.proxy.emitted_degraded_output()
                {
                    Err(format!(
                        "isolated external-plugin pipeline flush returned {status:?}"
                    ))
                } else {
                    self.remaining_pipeline_frames -= frames as u64;
                    let complete = self.remaining_pipeline_frames == 0;
                    if complete {
                        self.drain_started = false;
                    }
                    Ok(PluginDrainResult { frames, complete })
                }
            } else {
                self.drain_started = false;
                Ok(PluginDrainResult::COMPLETE)
            }
        })();

        if result.is_err() {
            self.drain_failed = true;
            self.drain_started = false;
        }
        result
    }
}
