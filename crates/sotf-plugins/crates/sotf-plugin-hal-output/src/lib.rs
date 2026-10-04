// ============================================================================
// HAL Output Plugin - Writes audio to macOS HAL driver
// ============================================================================

pub mod params;

use sotf_host::parameters::{Parameter, ParameterId, ParameterValue};
use sotf_host::plugin::{
    Plugin, PluginCompileMetadata, PluginCostClass, PluginDrainResult, PluginInfo, PluginResult,
    ProcessContext, SinkAppendFailure, SinkQueueState, SinkServiceFailure, SinkTailPreflightError,
    SinkTransportFormat, SinkTransportRecoveryError, SinkTransportRecoveryStatus,
    SinkTransportRepreparePlan, SinkTransportWaitingReason, TerminalSink,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
thread_local! {
    static FAIL_ZEROED_SAMPLE_LABEL: std::cell::RefCell<Option<&'static str>> = const { std::cell::RefCell::new(None) };
}

fn try_zeroed_samples(samples: usize, label: &str) -> Result<Vec<f32>, SinkTransportRecoveryError> {
    #[cfg(test)]
    let inject_failure = FAIL_ZEROED_SAMPLE_LABEL.with(|requested| {
        let mut requested = requested.borrow_mut();
        if requested.is_some_and(|expected| expected == label) {
            requested.take();
            true
        } else {
            false
        }
    });
    #[cfg(test)]
    if inject_failure {
        return Err(SinkTransportRecoveryError::Preparation(format!(
            "{label}: injected test failure"
        )));
    }

    let mut values = Vec::new();
    values
        .try_reserve_exact(samples)
        .map_err(|error| SinkTransportRecoveryError::Preparation(format!("{label}: {error}")))?;
    values.resize(samples, 0.0);
    Ok(values)
}

#[cfg(all(target_os = "macos", feature = "hal"))]
use driver_hal::HalOutputWriter;

trait HalWriter: Send {
    fn is_connected(&self) -> bool;
    fn write(&mut self, buffer: &[f32]) -> usize;
    fn current_format(&self) -> Result<(u32, u32, u32), String>;
    fn config_changed(&self) -> bool;
    fn clear_config_changed(&self);
    fn set_engine_ready(&self, ready: bool);
    fn reconnect(&mut self) -> Result<(), String>;
    fn reload_cipher(&mut self) -> Result<(), String>;
    fn encryption_key_ready(&self) -> bool;
    fn available_read_frames(&self) -> usize;
    fn flush_audio(&self);
}

#[cfg(all(target_os = "macos", feature = "hal"))]
impl HalWriter for HalOutputWriter {
    fn is_connected(&self) -> bool {
        HalOutputWriter::is_connected(self)
    }

    fn write(&mut self, buffer: &[f32]) -> usize {
        HalOutputWriter::write(self, buffer)
    }

    fn current_format(&self) -> Result<(u32, u32, u32), String> {
        HalOutputWriter::current_format(self).map_err(|error| error.to_string())
    }

    fn config_changed(&self) -> bool {
        HalOutputWriter::config_changed(self)
    }

    fn clear_config_changed(&self) {
        HalOutputWriter::clear_config_changed(self);
    }

    fn set_engine_ready(&self, ready: bool) {
        HalOutputWriter::set_engine_ready(self, ready);
    }

    fn reconnect(&mut self) -> Result<(), String> {
        HalOutputWriter::reconnect(self).map_err(|error| error.to_string())
    }

    fn reload_cipher(&mut self) -> Result<(), String> {
        HalOutputWriter::reload_cipher(self).map_err(|error| error.to_string())
    }

    fn encryption_key_ready(&self) -> bool {
        HalOutputWriter::encryption_key_ready(self)
    }

    fn available_read_frames(&self) -> usize {
        HalOutputWriter::available_read_frames(self)
    }

    fn flush_audio(&self) {
        HalOutputWriter::flush_audio(self);
    }
}

// Static error messages used on the audio hot path. Using constants avoids
// re-formatting a fresh `String` every time an error is reported.
const ERR_INVALID_CHANNEL_COUNT: &str = "Invalid channel count. Must be between 1 and 16";
#[cfg(all(target_os = "macos", feature = "hal"))]
const ERR_HAL_DAEMON_NOT_INITIALIZED: &str =
    "HAL driver not initialized. Ensure daemon initialized HAL before creating plugins";
#[allow(
    dead_code,
    reason = "used on non-macOS / non-hal builds; dead on macOS+hal configurations"
)]
const ERR_HAL_UNSUPPORTED_PLATFORM: &str =
    "HAL output plugin is only supported on macOS with 'hal' feature enabled";
const ERR_NO_ADJUSTABLE_PARAMETERS: &str = "HAL output has no adjustable parameters";
const ERR_HAL_WRITER_NOT_AVAILABLE: &str = "HAL writer not available";
pub const HAL_OUTPUT_TELEMETRY_VERSION: u32 = 2;
const SWIFT_HAL_SAFETY_OFFSET_FRAMES: usize = 0;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration parameters for HalOutputPlugin
pub type HalOutputPluginParams = params::Params;

/// Non-automatable transport state reported by [`HalOutputPlugin::telemetry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalOutputTransportState {
    Uninitialized,
    Servicing,
    Ready,
    Disconnected,
    KeyMismatch,
    Backpressured,
    ConfigurationChanged,
    FormatError,
    PrimingFailed,
}

/// Versioned, lossless transport diagnostics. Unlike plugin parameters these
/// values are not automatable and all lifetime counters retain 64-bit range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HalOutputTelemetry {
    pub version: u32,
    pub state: HalOutputTransportState,
    pub requested_frames: u64,
    pub written_frames: u64,
    pub dropped_frames: u64,
    pub queued_frames: usize,
    pub queue_capacity_frames: usize,
    pub backpressure_events: u64,
    pub connected: bool,
    pub encryption_key_ready: bool,
    pub transport_fill_frames: usize,
    pub target_fill_frames: usize,
    pub device_latency_frames: usize,
    pub safety_offset_frames: usize,
    pub boundary_latency_frames: usize,
}

// ============================================================================
// Plugin Implementation
// ============================================================================

/// HAL Output Plugin - Sink plugin that writes audio to macOS HAL driver
pub struct HalOutputPlugin {
    /// Number of input channels
    channels: usize,

    /// Counter for transport backpressure events (legacy API name retained).
    underrun_counter: Arc<AtomicU64>,

    /// Write success ratio for the last process block as a percentage (0.0–100.0).
    /// 100.0 means all samples were accepted; lower values indicate back-pressure.
    write_success_ratio: f32,

    /// Cached HAL buffer capacity in samples.
    #[cfg_attr(not(all(target_os = "macos", feature = "hal")), allow(dead_code))]
    buffer_capacity: usize,

    /// Cached HAL connection status.
    is_connected: bool,

    /// Cached back-pressure state, useful as a diagnostic signal for downstream UI/host.
    is_backpressured: bool,

    /// Negotiated transport rate. Zero until initialization succeeds.
    sample_rate: u32,

    /// Frame-aligned unwritten tail retained after transport backpressure.
    pending: VecDeque<f32>,
    pending_capacity_samples: usize,

    /// Silence used to establish a deterministic initial shared-ring fill.
    /// Prepared on the control thread during initialization.
    prefill_silence: Vec<f32>,
    /// Frame-aligned staging for traversing wrapped pending samples without allocation.
    drain_staging: Vec<f32>,
    target_fill_frames: usize,
    device_latency_frames: usize,
    safety_offset_frames: usize,

    state: HalOutputTransportState,
    requested_frames: u64,
    written_frames: u64,
    dropped_frames: u64,

    /// HAL output writer
    writer: Option<Box<dyn HalWriter>>,
}

impl HalOutputPlugin {
    /// Create a new HAL output plugin
    pub fn new(channels: usize) -> Result<Self, String> {
        // Validate channels
        if channels == 0 || channels > 16 {
            return Err(ERR_INVALID_CHANNEL_COUNT.to_string());
        }

        #[cfg(all(target_os = "macos", feature = "hal"))]
        {
            let writer =
                HalOutputWriter::new().map(|writer| Box::new(writer) as Box<dyn HalWriter>);

            if writer.is_none() {
                return Err(ERR_HAL_DAEMON_NOT_INITIALIZED.to_string());
            }

            Ok(Self {
                channels,
                underrun_counter: Arc::new(AtomicU64::new(0)),
                write_success_ratio: 100.0,
                buffer_capacity: writer
                    .as_ref()
                    .and_then(|writer| writer.current_format().ok())
                    .map(|(_, _, buffer_frames)| buffer_frames as usize)
                    .unwrap_or(0),
                is_connected: writer.as_ref().is_some_and(|writer| writer.is_connected()),
                is_backpressured: false,
                sample_rate: 0,
                pending: VecDeque::new(),
                pending_capacity_samples: 0,
                prefill_silence: Vec::new(),
                drain_staging: Vec::new(),
                target_fill_frames: 0,
                device_latency_frames: 0,
                safety_offset_frames: 0,
                state: HalOutputTransportState::Uninitialized,
                requested_frames: 0,
                written_frames: 0,
                dropped_frames: 0,
                writer,
            })
        }

        #[cfg(not(all(target_os = "macos", feature = "hal")))]
        {
            Err(ERR_HAL_UNSUPPORTED_PLATFORM.to_string())
        }
    }

    /// Create from configuration parameters
    pub fn from_params(params: HalOutputPluginParams) -> Result<Self, String> {
        Self::new(params.channels)
    }

    /// Get the shared underrun counter (can be read from any thread)
    pub fn underrun_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.underrun_counter)
    }

    /// Get the current underrun count
    pub fn underrun_count(&self) -> u64 {
        self.underrun_counter.load(Ordering::Relaxed)
    }

    pub fn telemetry(&self) -> HalOutputTelemetry {
        HalOutputTelemetry {
            version: HAL_OUTPUT_TELEMETRY_VERSION,
            state: self.state,
            requested_frames: self.requested_frames,
            written_frames: self.written_frames,
            dropped_frames: self.dropped_frames,
            queued_frames: self.pending.len() / self.channels,
            queue_capacity_frames: self.pending_capacity_samples / self.channels,
            backpressure_events: self.underrun_count(),
            connected: self.is_connected,
            encryption_key_ready: self
                .writer
                .as_ref()
                .is_some_and(|writer| writer.encryption_key_ready()),
            transport_fill_frames: self
                .writer
                .as_ref()
                .map_or(0, |writer| writer.available_read_frames()),
            target_fill_frames: self.target_fill_frames,
            device_latency_frames: self.device_latency_frames,
            safety_offset_frames: self.safety_offset_frames,
            boundary_latency_frames: self.latency_samples(),
        }
    }

    /// Perform filesystem/remapping/key work on a control thread. Audio
    /// processing never calls this method implicitly.
    pub fn service_transport(&mut self) -> Result<(), String> {
        {
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
            if !writer.is_connected()
                && let Err(error) = writer.reconnect()
            {
                self.is_connected = false;
                self.state = HalOutputTransportState::Disconnected;
                return Err(error);
            }
        }
        self.quiesce_transport()?;
        {
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
            writer.reload_cipher()?;
        }
        self.validate_transport_format(self.sample_rate)?;
        self.prime_target_fill()?;
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
        writer.clear_config_changed();
        writer.set_engine_ready(true);
        self.is_connected = writer.is_connected();
        self.state = if !self.is_connected {
            HalOutputTransportState::Disconnected
        } else if !writer.encryption_key_ready() {
            HalOutputTransportState::KeyMismatch
        } else {
            HalOutputTransportState::Ready
        };
        Ok(())
    }

    fn recover_prepared_transport(
        &mut self,
        expected: SinkTransportFormat,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        let physical_samples =
            self.buffer_capacity
                .checked_mul(self.channels)
                .ok_or_else(|| {
                    SinkTransportRecoveryError::Preparation(
                        "prepared ring sample extent overflow".into(),
                    )
                })?;
        let prepared = SinkTransportFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            buffer_frames: self.buffer_capacity,
        };
        if self.sample_rate == 0
            || self.channels == 0
            || expected != prepared
            || !self.pending_capacity_samples.is_multiple_of(self.channels)
            || self.pending_capacity_samples < physical_samples
            || self.pending.len() > self.pending_capacity_samples
            || self.pending.capacity() < self.pending_capacity_samples
            || self.drain_staging.capacity() < self.pending_capacity_samples
            || self.prefill_silence.len() < physical_samples
        {
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected,
                actual: Some(prepared),
            });
        }

        let Some(writer) = self.writer.as_mut() else {
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::FormatUnavailable,
            ));
        };
        if !writer.is_connected() && writer.reconnect().is_err() {
            self.is_connected = false;
            self.state = HalOutputTransportState::Disconnected;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::Disconnected,
            ));
        }
        self.is_connected = self
            .writer
            .as_ref()
            .is_some_and(|writer| writer.is_connected());
        if !self.is_connected {
            self.state = HalOutputTransportState::Disconnected;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::Disconnected,
            ));
        }

        let Some(actual) = self.current_transport_format_for_recovery() else {
            self.state = HalOutputTransportState::FormatError;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::FormatUnavailable,
            ));
        };
        if actual != expected {
            self.state = HalOutputTransportState::FormatError;
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected,
                actual: Some(actual),
            });
        }

        if let Some(writer) = self.writer.as_mut()
            && let Err(error) = writer.reload_cipher()
        {
            return Err(SinkTransportRecoveryError::Transport(error));
        }
        let key_ready = self
            .writer
            .as_ref()
            .is_some_and(|writer| writer.encryption_key_ready());
        if !key_ready {
            self.state = HalOutputTransportState::KeyMismatch;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::KeyMismatch,
            ));
        }

        self.quiesce_transport()
            .map_err(SinkTransportRecoveryError::Transport)?;
        self.prime_target_fill()
            .map_err(SinkTransportRecoveryError::Transport)?;

        let Some(actual_after_prime) = self.current_transport_format_for_recovery() else {
            self.state = HalOutputTransportState::FormatError;
            if let Some(writer) = self.writer.as_ref() {
                writer.set_engine_ready(false);
            }
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::FormatUnavailable,
            ));
        };
        if actual_after_prime != expected {
            self.state = HalOutputTransportState::FormatError;
            if let Some(writer) = self.writer.as_ref() {
                writer.set_engine_ready(false);
            }
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected,
                actual: Some(actual_after_prime),
            });
        }

        if let Some(writer) = self.writer.as_ref() {
            writer.clear_config_changed();
        }

        let (connected, key_ready, config_changed) =
            self.writer.as_ref().map_or((false, false, true), |writer| {
                (
                    writer.is_connected(),
                    writer.encryption_key_ready(),
                    writer.config_changed(),
                )
            });
        if !connected {
            self.is_connected = false;
            self.state = HalOutputTransportState::Disconnected;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::Disconnected,
            ));
        }
        if !key_ready {
            self.state = HalOutputTransportState::KeyMismatch;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::KeyMismatch,
            ));
        }
        if config_changed {
            self.state = HalOutputTransportState::ConfigurationChanged;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::ConfigurationChanged,
            ));
        }

        if let Some(writer) = self.writer.as_ref() {
            writer.set_engine_ready(true);
        }
        self.is_connected = true;
        self.state = HalOutputTransportState::Ready;
        Ok(SinkTransportRecoveryStatus::Ready)
    }

    fn terminal_sink_reprepare_plan(
        &self,
    ) -> Result<SinkTransportRepreparePlan, SinkTransportRecoveryError> {
        let prepared_format = SinkTransportFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            buffer_frames: self.buffer_capacity,
        };
        if self.sample_rate == 0
            || self.channels == 0
            || !self.pending.len().is_multiple_of(self.channels)
            || !self.pending_capacity_samples.is_multiple_of(self.channels)
            || self.pending.len() > self.pending_capacity_samples
        {
            return Err(SinkTransportRecoveryError::ContractViolation);
        }
        let Some(target_format) = self.current_transport_format_for_recovery() else {
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected: prepared_format,
                actual: None,
            });
        };
        let pending_frames = self.pending.len() / self.channels;
        let (configuration_changed, connected, key_ready) =
            self.writer
                .as_ref()
                .map_or((false, false, false), |writer| {
                    (
                        writer.config_changed(),
                        writer.is_connected(),
                        writer.encryption_key_ready(),
                    )
                });
        if !connected {
            return Err(SinkTransportRecoveryError::Transport(
                "transport must be connected before preparing a ring-size change".into(),
            ));
        }
        if !key_ready {
            return Err(SinkTransportRecoveryError::Transport(
                "transport key must be ready before preparing a ring-size change".into(),
            ));
        }
        if !configuration_changed {
            return Err(SinkTransportRecoveryError::StalePlan);
        }
        if target_format.sample_rate != prepared_format.sample_rate
            || target_format.channels != prepared_format.channels
            || target_format.buffer_frames == 0
            || target_format.buffer_frames == prepared_format.buffer_frames
        {
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected: prepared_format,
                actual: Some(target_format),
            });
        }
        let queue_capacity_frames = target_format.buffer_frames.max(pending_frames);
        queue_capacity_frames
            .checked_mul(self.channels)
            .ok_or_else(|| {
                SinkTransportRecoveryError::Preparation(
                    "logical queue sample extent overflow".into(),
                )
            })?;
        target_format
            .buffer_frames
            .checked_mul(self.channels)
            .ok_or_else(|| {
                SinkTransportRecoveryError::Preparation(
                    "transport ring sample extent overflow".into(),
                )
            })?;
        let latency_samples = target_format
            .buffer_frames
            .checked_mul(2)
            .and_then(|latency| latency.checked_add(self.safety_offset_frames))
            .ok_or_else(|| {
                SinkTransportRecoveryError::Preparation("sink latency overflow".into())
            })?;
        Ok(SinkTransportRepreparePlan {
            prepared_format,
            target_format,
            pending_frames,
            queue_capacity_frames,
            latency_samples,
            configuration_changed,
        })
    }

    fn reprepare_prepared_transport(
        &mut self,
        plan: SinkTransportRepreparePlan,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        if self.terminal_sink_reprepare_plan()? != plan {
            return Err(SinkTransportRecoveryError::StalePlan);
        }
        let queue_samples = plan
            .queue_capacity_frames
            .checked_mul(self.channels)
            .ok_or_else(|| {
                SinkTransportRecoveryError::Preparation(
                    "logical queue sample extent overflow".into(),
                )
            })?;
        let ring_samples = plan
            .target_format
            .buffer_frames
            .checked_mul(self.channels)
            .ok_or_else(|| {
                SinkTransportRecoveryError::Preparation(
                    "transport ring sample extent overflow".into(),
                )
            })?;
        let mut staged_pending = VecDeque::new();
        staged_pending
            .try_reserve_exact(queue_samples)
            .map_err(|error| {
                SinkTransportRecoveryError::Preparation(format!(
                    "pending queue reservation failed: {error}"
                ))
            })?;
        staged_pending.extend(self.pending.iter().copied());
        let staged_prefill =
            try_zeroed_samples(ring_samples, "target-fill storage reservation failed")?;
        let staged_drain = try_zeroed_samples(queue_samples, "drain staging reservation failed")?;

        {
            let Some(writer) = self.writer.as_mut() else {
                return Ok(SinkTransportRecoveryStatus::Waiting(
                    SinkTransportWaitingReason::FormatUnavailable,
                ));
            };
            if !writer.is_connected() && writer.reconnect().is_err() {
                self.is_connected = false;
                self.state = HalOutputTransportState::Disconnected;
                return Ok(SinkTransportRecoveryStatus::Waiting(
                    SinkTransportWaitingReason::Disconnected,
                ));
            }
        }
        if self.terminal_sink_reprepare_plan()? != plan {
            return Err(SinkTransportRecoveryError::StalePlan);
        }
        if let Some(writer) = self.writer.as_mut() {
            writer
                .reload_cipher()
                .map_err(SinkTransportRecoveryError::Transport)?;
        }
        let (connected, key_ready) = self.writer.as_ref().map_or((false, false), |writer| {
            (writer.is_connected(), writer.encryption_key_ready())
        });
        if !connected {
            self.is_connected = false;
            self.state = HalOutputTransportState::Disconnected;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::Disconnected,
            ));
        }
        if !key_ready {
            self.state = HalOutputTransportState::KeyMismatch;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::KeyMismatch,
            ));
        }
        if self.terminal_sink_reprepare_plan()? != plan {
            return Err(SinkTransportRecoveryError::StalePlan);
        }

        self.quiesce_transport()
            .map_err(SinkTransportRecoveryError::Transport)?;
        let prime_result = (|| -> Result<(), SinkTransportRecoveryError> {
            let writer = self.writer.as_mut().ok_or_else(|| {
                SinkTransportRecoveryError::Transport(ERR_HAL_WRITER_NOT_AVAILABLE.into())
            })?;
            let written = Self::write_frames(writer.as_mut(), &staged_prefill, self.channels)
                .map_err(SinkTransportRecoveryError::Transport)?;
            let observed = writer.available_read_frames();
            if written == plan.target_format.buffer_frames && observed == written {
                Ok(())
            } else {
                Err(SinkTransportRecoveryError::Transport(format!(
                    "HAL output could not establish resized target fill: expected {} frames, wrote {written}, observed {observed}",
                    plan.target_format.buffer_frames
                )))
            }
        })();
        if let Err(error) = prime_result {
            if let Some(writer) = self.writer.as_ref() {
                writer.flush_audio();
            }
            self.state = HalOutputTransportState::PrimingFailed;
            return Err(error);
        }

        let actual = self.current_transport_format_for_recovery();
        if actual != Some(plan.target_format) {
            self.state = HalOutputTransportState::FormatError;
            if let Some(writer) = self.writer.as_ref() {
                writer.set_engine_ready(false);
                writer.flush_audio();
            }
            return Err(SinkTransportRecoveryError::NeedsReprepare {
                expected: plan.target_format,
                actual,
            });
        }
        let (connected, key_ready, configuration_changed) =
            self.writer.as_ref().map_or((false, false, true), |writer| {
                (
                    writer.is_connected(),
                    writer.encryption_key_ready(),
                    writer.config_changed(),
                )
            });
        if !connected {
            self.is_connected = false;
            self.state = HalOutputTransportState::Disconnected;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::Disconnected,
            ));
        }
        if !key_ready {
            self.state = HalOutputTransportState::KeyMismatch;
            return Ok(SinkTransportRecoveryStatus::Waiting(
                SinkTransportWaitingReason::KeyMismatch,
            ));
        }
        if !configuration_changed {
            return Err(SinkTransportRecoveryError::StalePlan);
        }

        // The configuration flag is cleared only after target geometry and
        // readiness have both been verified. All remaining local updates are
        // infallible swaps/assignments; pending audio is byte-for-byte copied.
        if let Some(writer) = self.writer.as_ref() {
            writer.clear_config_changed();
        }
        self.pending = staged_pending;
        self.pending_capacity_samples = queue_samples;
        self.buffer_capacity = plan.target_format.buffer_frames;
        self.prefill_silence = staged_prefill;
        self.drain_staging = staged_drain;
        self.target_fill_frames = plan.target_format.buffer_frames;
        self.device_latency_frames = plan.target_format.buffer_frames;
        self.is_connected = true;
        self.is_backpressured = !self.pending.is_empty();
        self.state = HalOutputTransportState::Ready;
        if let Some(writer) = self.writer.as_ref() {
            writer.set_engine_ready(true);
        }
        Ok(SinkTransportRecoveryStatus::Ready)
    }

    fn current_transport_format_for_recovery(&self) -> Option<SinkTransportFormat> {
        let (sample_rate, channels, buffer_frames) = self.writer.as_ref()?.current_format().ok()?;
        Some(SinkTransportFormat {
            sample_rate,
            channels: channels as usize,
            buffer_frames: buffer_frames as usize,
        })
    }

    fn current_reprepare_plan(
        &self,
    ) -> Result<SinkTransportRepreparePlan, SinkTransportRecoveryError> {
        self.terminal_sink_reprepare_plan()
    }

    fn quiesce_transport(&mut self) -> Result<(), String> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
        writer.set_engine_ready(false);
        writer.flush_audio();
        self.state = HalOutputTransportState::Servicing;
        Ok(())
    }

    fn cancel_pending_for_reinitialize(&mut self) {
        let cancelled_frames = self.pending.len() / self.channels;
        self.dropped_frames = self.dropped_frames.saturating_add(cancelled_frames as u64);
        self.pending.clear();
    }

    fn check_transport_format(&self, sample_rate: u32) -> Result<(u32, u32, u32), String> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
        let format = writer.current_format()?;
        if format.0 != sample_rate || format.1 as usize != self.channels {
            return Err(format!(
                "HAL output format mismatch: plugin={sample_rate} Hz/{} channels, transport={} Hz/{} channels",
                self.channels, format.0, format.1
            ));
        }
        Ok(format)
    }

    fn validate_drain_context(&self, context: &ProcessContext) -> PluginResult<()> {
        if self.sample_rate == 0 {
            return Err("HAL output must be initialized before draining".to_string());
        }
        if context.sample_rate == 0.0 || context.sample_rate != f64::from(self.sample_rate) {
            return Err(format!(
                "HAL output initialized at {} Hz but received {} Hz drain context",
                self.sample_rate, context.sample_rate
            ));
        }
        Ok(())
    }

    fn validate_transport_format(&mut self, sample_rate: u32) -> Result<(), String> {
        let (_, _, buffer_frames) = match self.check_transport_format(sample_rate) {
            Ok(format) => format,
            Err(error) => {
                self.state = HalOutputTransportState::FormatError;
                return Err(error);
            }
        };
        let Some(pending_capacity) = (buffer_frames as usize).checked_mul(self.channels) else {
            self.state = HalOutputTransportState::FormatError;
            return Err("HAL output pending capacity overflow".to_string());
        };
        let staging_capacity = pending_capacity.max(self.pending.len());

        if self.pending.capacity() < staging_capacity
            && let Err(error) = self
                .pending
                .try_reserve_exact(staging_capacity.saturating_sub(self.pending.len()))
        {
            self.state = HalOutputTransportState::FormatError;
            return Err(format!("HAL output pending queue capacity failed: {error}"));
        }
        if self.prefill_silence.len() < pending_capacity
            && let Err(error) = self
                .prefill_silence
                .try_reserve_exact(pending_capacity.saturating_sub(self.prefill_silence.len()))
        {
            self.state = HalOutputTransportState::FormatError;
            return Err(format!(
                "HAL output priming storage capacity failed: {error}"
            ));
        }
        if self.drain_staging.capacity() < staging_capacity
            && let Err(error) = self
                .drain_staging
                .try_reserve_exact(staging_capacity.saturating_sub(self.drain_staging.len()))
        {
            self.state = HalOutputTransportState::FormatError;
            return Err(format!("HAL output drain storage capacity failed: {error}"));
        }

        self.buffer_capacity = buffer_frames as usize;
        // The Swift virtual device reports one device buffer of latency and a
        // zero-frame safety offset through its CoreAudio properties.
        self.target_fill_frames = buffer_frames as usize;
        self.device_latency_frames = buffer_frames as usize;
        self.safety_offset_frames = SWIFT_HAL_SAFETY_OFFSET_FRAMES;
        self.pending_capacity_samples = staging_capacity;
        if self.prefill_silence.len() < pending_capacity {
            self.prefill_silence.resize(pending_capacity, 0.0);
        }
        Ok(())
    }

    fn prime_target_fill(&mut self) -> Result<(), String> {
        let result = (|| -> Result<(), String> {
            let samples = self
                .target_fill_frames
                .checked_mul(self.channels)
                .ok_or_else(|| "HAL output target fill overflow".to_string())?;
            let silence = self
                .prefill_silence
                .get(..samples)
                .ok_or_else(|| "HAL output target-fill storage is not prepared".to_string())?;
            let writer = self
                .writer
                .as_mut()
                .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
            let written = Self::write_frames(writer.as_mut(), silence, self.channels)?;
            let observed = writer.available_read_frames();
            if written == self.target_fill_frames && observed == self.target_fill_frames {
                Ok(())
            } else {
                Err(format!(
                    "HAL output could not establish target fill: expected {} frames, wrote {written}, observed {observed}",
                    self.target_fill_frames
                ))
            }
        })();
        if result.is_err() {
            if let Some(writer) = self.writer.as_ref() {
                writer.flush_audio();
            }
            self.state = HalOutputTransportState::PrimingFailed;
        }
        result
    }

    fn append_pending(&mut self, samples: &[f32]) -> usize {
        debug_assert_eq!(samples.len() % self.channels, 0);
        let available = self
            .pending_capacity_samples
            .saturating_sub(self.pending.len());
        let accepted = samples.len().min(available) / self.channels * self.channels;
        self.pending.extend(samples[..accepted].iter().copied());
        let dropped_frames = (samples.len() - accepted) / self.channels;
        self.dropped_frames = self.dropped_frames.saturating_add(dropped_frames as u64);
        dropped_frames
    }

    fn flush_pending(
        pending: &mut VecDeque<f32>,
        writer: &mut dyn HalWriter,
        channels: usize,
    ) -> Result<usize, String> {
        let mut total_written = 0;
        for _ in 0..2 {
            let requested_samples = {
                let (front, _) = pending.as_slices();
                front.len() / channels * channels
            };
            if requested_samples == 0 {
                break;
            }
            let written = {
                let (front, _) = pending.as_slices();
                Self::write_frames(writer, &front[..requested_samples], channels)?
            };
            pending.drain(..written * channels);
            total_written += written;
            if written * channels < requested_samples {
                break;
            }
        }
        Ok(total_written)
    }

    fn flush_pending_for_drain(
        pending: &mut VecDeque<f32>,
        staging: &mut Vec<f32>,
        writer: &mut dyn HalWriter,
        channels: usize,
        written_total: &mut u64,
    ) -> Result<usize, String> {
        staging.clear();
        let (front, back) = pending.as_slices();
        staging.extend_from_slice(front);
        staging.extend_from_slice(back);
        debug_assert_eq!(staging.len() % channels, 0);

        let (front, _) = pending.as_slices();
        let first_request_samples = (front.len() / channels * channels).max(channels);
        let mut total_written = 0;
        let mut offset_samples = 0;

        for attempt in 0..2 {
            let remaining_samples = staging.len().saturating_sub(offset_samples);
            if remaining_samples == 0 {
                break;
            }
            let requested_samples = if attempt == 0 {
                first_request_samples.min(remaining_samples)
            } else {
                remaining_samples
            };
            let written_frames = Self::write_frames(
                writer,
                &staging[offset_samples..offset_samples + requested_samples],
                channels,
            )?;
            let written_samples = written_frames * channels;
            pending.drain(..written_samples);
            total_written += written_frames;
            offset_samples += written_samples;
            *written_total = written_total.saturating_add(written_frames as u64);
            if written_samples < requested_samples {
                break;
            }
        }

        Ok(total_written)
    }

    fn write_frames(
        writer: &mut dyn HalWriter,
        samples: &[f32],
        channels: usize,
    ) -> Result<usize, String> {
        let requested_frames = samples.len() / channels;
        let written_frames = writer.write(samples);
        if written_frames > requested_frames {
            return Err(format!(
                "HAL output writer returned {written_frames} frames for {requested_frames}-frame request"
            ));
        }
        Ok(written_frames)
    }
}

impl Drop for HalOutputPlugin {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.as_ref() {
            writer.set_engine_ready(false);
        }
    }
}

impl Plugin for HalOutputPlugin {
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn terminal_sink(&self) -> Option<&dyn TerminalSink> {
        Some(self)
    }

    fn terminal_sink_mut(&mut self) -> Option<&mut dyn TerminalSink> {
        Some(self)
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        true
    }

    fn info(&self) -> PluginInfo {
        PluginInfo::new("HAL Output", "1.0.0", "SotF")
            .with_description("Writes audio to macOS HAL driver")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        0 // Sink plugin - no output
    }

    fn compile_metadata(&self) -> PluginCompileMetadata {
        PluginCompileMetadata::boundary(PluginCostClass::External, self.latency_samples())
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> PluginResult<()> {
        Err(ERR_NO_ADJUSTABLE_PARAMETERS.to_string())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> PluginResult<()> {
        if !sample_rate.is_finite()
            || sample_rate <= 0.0
            || sample_rate > f64::from(u32::MAX)
            || sample_rate.fract() != 0.0
        {
            return Err("HAL output transport requires a positive integer sample rate".to_string());
        }
        let sample_rate = sample_rate as u32;
        self.check_transport_format(sample_rate)?;
        self.quiesce_transport()?;
        self.validate_transport_format(sample_rate)?;
        self.prime_target_fill()?;
        self.cancel_pending_for_reinitialize();
        self.sample_rate = sample_rate;
        if let Some(writer) = self.writer.as_ref() {
            writer.clear_config_changed();
            writer.set_engine_ready(true);
            self.is_connected = writer.is_connected();
            self.state = if !self.is_connected {
                HalOutputTransportState::Disconnected
            } else if !writer.encryption_key_ready() {
                HalOutputTransportState::KeyMismatch
            } else {
                HalOutputTransportState::Ready
            };
        }
        Ok(())
    }

    fn reset(&mut self) {
        if self.channels == 0 {
            return;
        }
        let pending_frames = self.pending.len() / self.channels;
        self.pending.clear();
        self.dropped_frames = self.dropped_frames.saturating_add(pending_frames as u64);
        self.is_backpressured = false;
        if self.state == HalOutputTransportState::Backpressured {
            self.state = HalOutputTransportState::Ready;
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        0
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()> {
        self.validate_drain_context(context)
    }

    fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
        if self.pending.is_empty() {
            std::num::NonZeroU64::new(1)
        } else {
            None
        }
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> PluginResult<PluginDrainResult> {
        self.validate_drain_context(context)?;
        if !output.is_empty() {
            return Err(format!(
                "HAL output is a sink and requires an empty drain destination, got {} samples",
                output.len()
            ));
        }
        if self.pending.is_empty() {
            return Ok(PluginDrainResult::COMPLETE);
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(ERR_HAL_WRITER_NOT_AVAILABLE.to_string());
        };
        if matches!(
            self.state,
            HalOutputTransportState::Uninitialized
                | HalOutputTransportState::Servicing
                | HalOutputTransportState::Disconnected
                | HalOutputTransportState::KeyMismatch
                | HalOutputTransportState::ConfigurationChanged
                | HalOutputTransportState::FormatError
                | HalOutputTransportState::PrimingFailed
        ) {
            self.is_backpressured = true;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        if writer.config_changed() {
            self.state = HalOutputTransportState::ConfigurationChanged;
            self.is_backpressured = true;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }
        self.is_connected = writer.is_connected();
        if !self.is_connected {
            self.state = HalOutputTransportState::Disconnected;
            self.is_backpressured = true;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }
        if !writer.encryption_key_ready() {
            self.state = HalOutputTransportState::KeyMismatch;
            self.is_backpressured = true;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }

        let written = Self::flush_pending_for_drain(
            &mut self.pending,
            &mut self.drain_staging,
            writer.as_mut(),
            self.channels,
            &mut self.written_frames,
        );
        if let Err(error) = written {
            self.state = HalOutputTransportState::FormatError;
            self.is_backpressured = true;
            return Err(error);
        }

        if self.pending.is_empty() {
            self.state = HalOutputTransportState::Ready;
            self.is_backpressured = false;
            return Ok(PluginDrainResult::COMPLETE);
        }

        self.state = HalOutputTransportState::Backpressured;
        self.is_backpressured = true;
        self.underrun_counter.fetch_add(1, Ordering::Relaxed);
        Ok(PluginDrainResult {
            frames: 0,
            complete: false,
        })
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        if self.sample_rate == 0 {
            return Err("HAL output must be initialized before processing".to_string());
        }
        if context.sample_rate != f64::from(self.sample_rate) {
            return Err(format!(
                "HAL output initialized at {} Hz but received {} Hz context",
                self.sample_rate, context.sample_rate
            ));
        }
        if !output.is_empty() {
            return Err(format!(
                "HAL output is a sink and requires an empty output buffer, got {} samples",
                output.len()
            ));
        }
        // Verify input buffer size
        let expected_len = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "HAL output frame/channel count overflow".to_string())?;
        if input.len() != expected_len {
            return Err(format!(
                "Input buffer size mismatch: expected {}, got {}",
                expected_len,
                input.len()
            ));
        }

        self.requested_frames = self
            .requested_frames
            .saturating_add(context.num_frames as u64);

        if self.state == HalOutputTransportState::Servicing
            || self.state == HalOutputTransportState::PrimingFailed
            || self.state == HalOutputTransportState::FormatError
        {
            self.append_pending(input);
            self.is_backpressured = true;
            return Ok(context.num_frames);
        }

        if self
            .writer
            .as_ref()
            .is_some_and(|writer| writer.config_changed())
        {
            self.state = HalOutputTransportState::ConfigurationChanged;
            self.is_backpressured = true;
            self.append_pending(input);
            return Ok(context.num_frames);
        }

        let key_ready = {
            let writer = self
                .writer
                .as_ref()
                .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;
            self.is_connected = writer.is_connected();
            writer.encryption_key_ready()
        };
        if !self.is_connected {
            self.state = HalOutputTransportState::Disconnected;
        } else if !key_ready {
            self.state = HalOutputTransportState::KeyMismatch;
        }
        if self.state == HalOutputTransportState::Disconnected
            || self.state == HalOutputTransportState::KeyMismatch
        {
            self.append_pending(input);
            self.is_backpressured = true;
            return Ok(context.num_frames);
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| ERR_HAL_WRITER_NOT_AVAILABLE.to_string())?;

        if !self.pending.is_empty() {
            let written_frames =
                Self::flush_pending(&mut self.pending, writer.as_mut(), self.channels)?;
            self.written_frames = self.written_frames.saturating_add(written_frames as u64);
            if !self.pending.is_empty() {
                self.append_pending(input);
                self.write_success_ratio = 0.0;
                self.is_backpressured = true;
                self.underrun_counter.fetch_add(1, Ordering::Relaxed);
                self.state = HalOutputTransportState::Backpressured;
                return Ok(context.num_frames);
            }
        }

        let written_frames = Self::write_frames(writer.as_mut(), input, self.channels)?;
        self.written_frames = self.written_frames.saturating_add(written_frames as u64);
        self.write_success_ratio = if context.num_frames == 0 {
            100.0
        } else {
            written_frames as f32 / context.num_frames as f32 * 100.0
        };
        self.is_backpressured = written_frames < context.num_frames;
        if self.is_backpressured {
            self.append_pending(&input[written_frames * self.channels..]);
            self.underrun_counter.fetch_add(1, Ordering::Relaxed);
            self.state = HalOutputTransportState::Backpressured;
        } else {
            self.state = HalOutputTransportState::Ready;
        }

        Ok(context.num_frames)
    }

    fn latency_samples(&self) -> usize {
        self.target_fill_frames
            .saturating_add(self.device_latency_frames)
            .saturating_add(self.safety_offset_frames)
    }
}

impl TerminalSink for HalOutputPlugin {
    fn queue_state(&self) -> SinkQueueState {
        if self.channels == 0 {
            return SinkQueueState {
                pending_frames: 0,
                free_prepared_frames: 0,
                capacity_frames: 0,
            };
        }
        SinkQueueState {
            pending_frames: self.pending.len() / self.channels,
            free_prepared_frames: self
                .pending_capacity_samples
                .saturating_sub(self.pending.len())
                / self.channels,
            capacity_frames: self.pending_capacity_samples / self.channels,
        }
    }

    fn prepared_transport_format(&self) -> Option<SinkTransportFormat> {
        (self.sample_rate != 0 && self.channels != 0 && self.buffer_capacity != 0).then_some(
            SinkTransportFormat {
                sample_rate: self.sample_rate,
                channels: self.channels,
                buffer_frames: self.buffer_capacity,
            },
        )
    }

    fn preflight_append(
        &self,
        maximum_frames: usize,
        context: &ProcessContext,
    ) -> Result<(), SinkTailPreflightError> {
        if self.writer.is_none() {
            return Err(SinkTailPreflightError::Unavailable);
        }
        if self.channels == 0
            || self.sample_rate == 0
            || context.sample_rate != f64::from(self.sample_rate)
            || context.num_frames != maximum_frames
            || !self.pending.len().is_multiple_of(self.channels)
            || self.pending.len() > self.pending_capacity_samples
            || self.pending.capacity() < self.pending_capacity_samples
            || matches!(
                self.state,
                HalOutputTransportState::Uninitialized
                    | HalOutputTransportState::Servicing
                    | HalOutputTransportState::ConfigurationChanged
                    | HalOutputTransportState::FormatError
                    | HalOutputTransportState::PrimingFailed
            )
            || self
                .writer
                .as_ref()
                .is_some_and(|writer| writer.config_changed())
        {
            return Err(SinkTailPreflightError::InvalidGeometry);
        }
        let Some(required_samples) = maximum_frames.checked_mul(self.channels) else {
            return Err(SinkTailPreflightError::InvalidGeometry);
        };
        if required_samples
            > self
                .pending_capacity_samples
                .saturating_sub(self.pending.len())
        {
            return Err(SinkTailPreflightError::CapacityExceeded {
                required_frames: maximum_frames,
                free_frames: self
                    .pending_capacity_samples
                    .saturating_sub(self.pending.len())
                    / self.channels,
            });
        }
        Ok(())
    }

    fn append_preflighted(
        &mut self,
        input: &[f32],
        context: &ProcessContext,
    ) -> Result<(), SinkAppendFailure> {
        if self.channels == 0
            || context.sample_rate != f64::from(self.sample_rate)
            || context
                .num_frames
                .checked_mul(self.channels)
                .is_none_or(|samples| input.len() != samples)
            || input.len()
                > self
                    .pending_capacity_samples
                    .saturating_sub(self.pending.len())
            || self.pending.capacity() < self.pending.len().saturating_add(input.len())
        {
            return Err(SinkAppendFailure::ContractViolation);
        }
        for sample in input {
            self.pending.push_back(*sample);
        }
        self.requested_frames = self
            .requested_frames
            .saturating_add(context.num_frames as u64);
        Ok(())
    }

    fn service_pending(
        &mut self,
        context: &ProcessContext,
    ) -> Result<SinkQueueState, SinkServiceFailure> {
        if self.channels == 0
            || self.validate_drain_context(context).is_err()
            || !self.pending.len().is_multiple_of(self.channels)
            || self.pending.len() > self.pending_capacity_samples
            || self.pending.capacity() < self.pending.len()
        {
            return Err(SinkServiceFailure::ContractViolation);
        }
        Plugin::drain(self, &mut [], context).map_err(|_| SinkServiceFailure::ContractViolation)?;
        Ok(self.queue_state())
    }

    fn recover_transport(
        &mut self,
        expected: SinkTransportFormat,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        self.recover_prepared_transport(expected)
    }

    fn reprepare_plan(&self) -> Result<SinkTransportRepreparePlan, SinkTransportRecoveryError> {
        self.current_reprepare_plan()
    }

    fn reprepare_transport(
        &mut self,
        plan: SinkTransportRepreparePlan,
    ) -> Result<SinkTransportRecoveryStatus, SinkTransportRecoveryError> {
        self.reprepare_prepared_transport(plan)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use sotf_host::plugin::TailLength;
    use sotf_host::{CountingAlloc, DawHost, GraphEdge, assert_no_allocs};
    use std::alloc::{GlobalAlloc, Layout};

    thread_local! {
        static DEALLOCATION_COUNTING_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static DEALLOCATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    struct HalOutputCountingAlloc;

    /// # Safety
    /// This allocator delegates storage operations and only records test-thread deallocations.
    unsafe impl GlobalAlloc for HalOutputCountingAlloc {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            unsafe { CountingAlloc.alloc(layout) }
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            let _ = DEALLOCATION_COUNTING_ENABLED.try_with(|enabled| {
                if enabled.get() {
                    let _ = DEALLOCATION_COUNT.try_with(|count| count.set(count.get() + 1));
                }
            });
            unsafe { CountingAlloc.dealloc(pointer, layout) };
        }
    }

    #[global_allocator]
    static ALLOCATOR: HalOutputCountingAlloc = HalOutputCountingAlloc;

    struct DeallocationTrackingGuard;

    impl Drop for DeallocationTrackingGuard {
        fn drop(&mut self) {
            DEALLOCATION_COUNTING_ENABLED.with(|enabled| enabled.set(false));
        }
    }

    fn assert_no_heap_activity(label: &str, operation: impl FnOnce()) {
        DEALLOCATION_COUNT.with(|count| count.set(0));
        DEALLOCATION_COUNTING_ENABLED.with(|enabled| enabled.set(true));
        let guard = DeallocationTrackingGuard;
        assert_no_allocs(label, operation);
        let deallocations = DEALLOCATION_COUNT.with(std::cell::Cell::get);
        drop(guard);
        assert_eq!(deallocations, 0, "{label} deallocated heap storage");
    }

    // Helper: build a minimal plugin without the HAL feature.
    // On non-macOS / non-hal builds new() returns Err, so we test the public
    // interface that is reachable regardless of the HAL feature gate.

    #[test]
    fn new_rejects_zero_channels() {
        match HalOutputPlugin::new(0) {
            Err(e) => assert!(e.contains("Invalid channel count"), "unexpected error: {e}"),
            Ok(_) => panic!("expected Err for 0 channels"),
        }
    }

    #[test]
    fn new_rejects_too_many_channels() {
        match HalOutputPlugin::new(17) {
            Err(e) => assert!(e.contains("Invalid channel count"), "unexpected error: {e}"),
            Ok(_) => panic!("expected Err for 17 channels"),
        }
    }

    #[test]
    fn diagnostics_are_not_automatable_parameters() {
        let plugin = make_test_plugin();
        assert!(plugin.parameters().is_empty());
        assert!(
            plugin
                .get_parameter(&ParameterId::from("underrun_count"))
                .is_none()
        );
    }

    #[test]
    fn latency_samples_reports_cached_buffer_capacity() {
        let plugin = make_test_plugin();
        assert_eq!(plugin.latency_samples(), 0);
    }

    #[test]
    fn process_rejects_mismatched_buffer() {
        let mut plugin = make_test_plugin();
        let ctx = ProcessContext::new(48000, 4);
        // 4 frames * 2 channels = 8 samples; supply 7 instead.
        let input = vec![0.0f32; 7];
        let mut output = vec![];
        let err = plugin.process(&input, &mut output, &ctx).unwrap_err();
        assert!(err.contains("mismatch"), "unexpected error message: {err}");
    }

    #[test]
    fn set_parameter_returns_static_no_adjustable_params_error() {
        let mut plugin = make_test_plugin();
        let err = plugin
            .set_parameter(ParameterId::from("gain_db"), ParameterValue::Float(0.0))
            .unwrap_err();
        assert_eq!(err, ERR_NO_ADJUSTABLE_PARAMETERS);
    }

    #[test]
    fn new_rejects_invalid_channel_count_with_static_error() {
        let err = match HalOutputPlugin::new(0) {
            Err(e) => e,
            Ok(_) => panic!("expected error for 0 channels"),
        };
        assert_eq!(err, ERR_INVALID_CHANNEL_COUNT);
    }

    /// Build a `HalOutputPlugin` directly (without the HAL writer) so tests
    /// can exercise the parameter and validation logic on any platform.
    fn make_test_plugin() -> HalOutputPlugin {
        HalOutputPlugin {
            channels: 2,
            underrun_counter: Arc::new(AtomicU64::new(0)),
            write_success_ratio: 100.0,
            buffer_capacity: 0,
            is_connected: false,
            is_backpressured: false,
            sample_rate: 48_000,
            pending: VecDeque::new(),
            pending_capacity_samples: 0,
            prefill_silence: Vec::new(),
            drain_staging: Vec::new(),
            target_fill_frames: 0,
            device_latency_frames: 0,
            safety_offset_frames: 0,
            state: HalOutputTransportState::Uninitialized,
            requested_frames: 0,
            written_frames: 0,
            dropped_frames: 0,
            writer: None,
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum FakePrimeMutation {
        ChangeFormat((u32, u32, u32)),
        ClearConfiguration,
        Disconnect,
        LoseKey,
    }

    #[derive(Default)]
    struct FakeWriterState {
        scheduled_writes: Vec<usize>,
        writes: Vec<Vec<f32>>,
        accepted_samples: Vec<f32>,
        capture_audio: bool,
        capture_accepted_samples: bool,
        writer_attempts: usize,
        ready: Vec<bool>,
        clear_count: usize,
        reconnect_count: usize,
        reload_count: usize,
        fill_frames: usize,
        priming: bool,
        prime_write: Option<usize>,
        prime_actual_write: Option<usize>,
        prime_mutation: Option<FakePrimeMutation>,
        format_reads: usize,
        format_mutation_on_read: Option<(usize, FakePrimeMutation)>,
        flush_count: usize,
        connected: bool,
        config_changed: bool,
        key_ready: bool,
        transport_format: Option<(u32, u32, u32)>,
        format_error: bool,
    }

    struct FakeWriter {
        format: (u32, u32, u32),
        state: Arc<std::sync::Mutex<FakeWriterState>>,
    }

    impl HalWriter for FakeWriter {
        fn is_connected(&self) -> bool {
            self.state.lock().unwrap().connected
        }

        fn write(&mut self, buffer: &[f32]) -> usize {
            let mut state = self.state.lock().unwrap();
            let channel_count = state.transport_format.unwrap_or(self.format).1 as usize;
            let was_priming = state.priming;
            let frames = if was_priming {
                state
                    .prime_write
                    .take()
                    .unwrap_or(buffer.len() / channel_count)
            } else {
                if state.scheduled_writes.is_empty() {
                    buffer.len() / channel_count
                } else {
                    state.scheduled_writes.remove(0)
                }
            };
            let physically_written_frames = if was_priming {
                state.prime_actual_write.take().unwrap_or(frames)
            } else {
                frames
            };
            if !was_priming {
                state.writer_attempts += 1;
                if state.capture_audio {
                    state.writes.push(buffer.to_vec());
                }
                if state.capture_audio || state.capture_accepted_samples {
                    let accepted_samples = frames.min(buffer.len() / channel_count) * channel_count;
                    state
                        .accepted_samples
                        .extend_from_slice(&buffer[..accepted_samples]);
                }
            }
            state.fill_frames = state.fill_frames.saturating_add(physically_written_frames);
            state.priming = false;
            if was_priming {
                match state.prime_mutation.take() {
                    Some(FakePrimeMutation::ChangeFormat(format)) => {
                        state.transport_format = Some(format);
                    }
                    Some(FakePrimeMutation::ClearConfiguration) => {
                        state.config_changed = false;
                    }
                    Some(FakePrimeMutation::Disconnect) => state.connected = false,
                    Some(FakePrimeMutation::LoseKey) => state.key_ready = false,
                    None => {}
                }
            }
            frames
        }

        fn current_format(&self) -> Result<(u32, u32, u32), String> {
            let mut state = self.state.lock().unwrap();
            state.format_reads += 1;
            let format_reads = state.format_reads;
            if state
                .format_mutation_on_read
                .is_some_and(|(target_read, _)| target_read == format_reads)
                && let Some((_, mutation)) = state.format_mutation_on_read.take()
            {
                match mutation {
                    FakePrimeMutation::ChangeFormat(format) => {
                        state.transport_format = Some(format);
                    }
                    FakePrimeMutation::ClearConfiguration => {
                        state.config_changed = false;
                    }
                    FakePrimeMutation::Disconnect => state.connected = false,
                    FakePrimeMutation::LoseKey => state.key_ready = false,
                }
            }
            if state.format_error {
                return Err("fake transport format unavailable".to_string());
            }
            Ok(state.transport_format.unwrap_or(self.format))
        }

        fn config_changed(&self) -> bool {
            self.state.lock().unwrap().config_changed
        }

        fn clear_config_changed(&self) {
            let mut state = self.state.lock().unwrap();
            state.clear_count += 1;
            state.config_changed = false;
        }

        fn set_engine_ready(&self, ready: bool) {
            self.state.lock().unwrap().ready.push(ready);
        }

        fn reconnect(&mut self) -> Result<(), String> {
            let mut state = self.state.lock().unwrap();
            state.connected = true;
            state.reconnect_count += 1;
            Ok(())
        }

        fn reload_cipher(&mut self) -> Result<(), String> {
            let mut state = self.state.lock().unwrap();
            state.reload_count += 1;
            state.key_ready = true;
            Ok(())
        }

        fn encryption_key_ready(&self) -> bool {
            self.state.lock().unwrap().key_ready
        }
        fn available_read_frames(&self) -> usize {
            self.state.lock().unwrap().fill_frames
        }
        fn flush_audio(&self) {
            let mut state = self.state.lock().unwrap();
            state.fill_frames = 0;
            state.priming = true;
            state.flush_count += 1;
        }
    }

    fn make_plugin_with_writer(
        channels: usize,
        format: (u32, u32, u32),
        writes: Vec<usize>,
    ) -> (HalOutputPlugin, Arc<std::sync::Mutex<FakeWriterState>>) {
        make_plugin_with_writer_state(channels, format, writes, true, false)
    }

    fn make_plugin_with_writer_state(
        channels: usize,
        format: (u32, u32, u32),
        writes: Vec<usize>,
        connected: bool,
        changed: bool,
    ) -> (HalOutputPlugin, Arc<std::sync::Mutex<FakeWriterState>>) {
        let state = Arc::new(std::sync::Mutex::new(FakeWriterState::default()));
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.connected = connected;
            snapshot.config_changed = changed;
            snapshot.key_ready = true;
            snapshot.capture_audio = true;
            snapshot.scheduled_writes = writes;
        }
        let writer = FakeWriter {
            format,
            state: Arc::clone(&state),
        };
        (
            HalOutputPlugin {
                channels,
                underrun_counter: Arc::new(AtomicU64::new(0)),
                write_success_ratio: 100.0,
                buffer_capacity: format.2 as usize,
                is_connected: true,
                is_backpressured: false,
                sample_rate: 0,
                pending: VecDeque::with_capacity(format.1 as usize * format.2 as usize),
                pending_capacity_samples: format.1 as usize * format.2 as usize,
                prefill_silence: Vec::new(),
                drain_staging: Vec::new(),
                target_fill_frames: 0,
                device_latency_frames: 0,
                safety_offset_frames: 0,
                state: HalOutputTransportState::Uninitialized,
                requested_frames: 0,
                written_frames: 0,
                dropped_frames: 0,
                writer: Some(Box::new(writer)),
            },
            state,
        )
    }

    /// A tiny, deterministic no-feedback delay used to test the public host path.
    /// It emits the final `delay_frames` input frames exactly once at EOF.
    #[derive(Debug, Clone, Default, PartialEq)]
    struct FiniteDelayObservation {
        begin_drain_calls: usize,
        drain_calls: usize,
        drain_cursor_frames: usize,
        total_drain_call_bound: usize,
        remaining_drain_calls: usize,
        delay_line: Vec<f32>,
        write_frame: usize,
        has_input: bool,
        drained: bool,
    }

    struct FiniteDelayMarker {
        channels: usize,
        delay_frames: usize,
        drain_chunk_frames: usize,
        delay_line: Vec<f32>,
        write_frame: usize,
        drain_cursor_frames: usize,
        has_input: bool,
        drained: bool,
        fail_after_drain: bool,
        observation: Arc<std::sync::Mutex<FiniteDelayObservation>>,
    }

    impl FiniteDelayMarker {
        fn new(channels: usize, delay_frames: usize) -> Self {
            Self::new_observed(channels, delay_frames).0
        }

        fn new_observed(
            channels: usize,
            delay_frames: usize,
        ) -> (Self, Arc<std::sync::Mutex<FiniteDelayObservation>>) {
            Self::new_chunked_observed(channels, delay_frames, delay_frames)
        }

        fn new_chunked_observed(
            channels: usize,
            delay_frames: usize,
            drain_chunk_frames: usize,
        ) -> (Self, Arc<std::sync::Mutex<FiniteDelayObservation>>) {
            assert!(delay_frames > 0);
            assert!(drain_chunk_frames > 0);
            let observation = Arc::new(std::sync::Mutex::new(FiniteDelayObservation::default()));
            let plugin = Self {
                channels,
                delay_frames,
                drain_chunk_frames,
                delay_line: vec![0.0; channels * delay_frames],
                write_frame: 0,
                drain_cursor_frames: 0,
                has_input: false,
                drained: false,
                fail_after_drain: false,
                observation: Arc::clone(&observation),
            };
            plugin.publish_observation();
            (plugin, observation)
        }

        fn publish_observation(&self) {
            let mut observation = self.observation.lock().unwrap();
            observation.delay_line.clone_from(&self.delay_line);
            observation.write_frame = self.write_frame;
            observation.drain_cursor_frames = self.drain_cursor_frames;
            observation.total_drain_call_bound =
                self.delay_frames.div_ceil(self.drain_chunk_frames);
            observation.remaining_drain_calls = if self.has_input && !self.drained {
                self.delay_frames
                    .saturating_sub(self.drain_cursor_frames)
                    .div_ceil(self.drain_chunk_frames)
            } else {
                0
            };
            observation.has_input = self.has_input;
            observation.drained = self.drained;
        }
    }

    impl Plugin for FiniteDelayMarker {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Finite delay marker", "0.1", "test")
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

        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
            Err("finite delay marker has no parameters".into())
        }

        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }

        fn guarantees_identity_frame_geometry(&self) -> bool {
            true
        }

        fn tail_length(&self) -> TailLength {
            TailLength::Finite(self.delay_frames as u64)
        }

        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            let expected_samples = context.num_frames * self.channels;
            if input.len() != expected_samples || output.len() != expected_samples {
                return Err("finite delay marker received a malformed block".into());
            }
            for (input_frame, output_frame) in input
                .chunks_exact(self.channels)
                .zip(output.chunks_exact_mut(self.channels))
            {
                let start = self.write_frame * self.channels;
                let end = start + self.channels;
                output_frame.copy_from_slice(&self.delay_line[start..end]);
                self.delay_line[start..end].copy_from_slice(input_frame);
                self.write_frame = (self.write_frame + 1) % self.delay_frames;
            }
            self.has_input |= context.num_frames > 0;
            self.drain_cursor_frames = 0;
            self.drained = false;
            self.publish_observation();
            Ok(context.num_frames)
        }

        fn drain_output_frames_max(&self) -> usize {
            if self.has_input && !self.drained {
                self.drain_chunk_frames
                    .min(self.delay_frames.saturating_sub(self.drain_cursor_frames))
            } else {
                0
            }
        }

        fn drain_call_bound(&self) -> Option<std::num::NonZeroU64> {
            std::num::NonZeroU64::new(self.delay_frames.div_ceil(self.drain_chunk_frames) as u64)
        }

        fn begin_drain(&mut self, _: &ProcessContext) -> PluginResult<()> {
            self.observation.lock().unwrap().begin_drain_calls += 1;
            Ok(())
        }

        fn drain(
            &mut self,
            output: &mut [f32],
            _: &ProcessContext,
        ) -> PluginResult<PluginDrainResult> {
            self.observation.lock().unwrap().drain_calls += 1;
            if !self.has_input || self.drained {
                return Ok(PluginDrainResult::COMPLETE);
            }
            let frames = self
                .drain_chunk_frames
                .min(self.delay_frames - self.drain_cursor_frames);
            if output.len() < frames * self.channels {
                return Err("finite delay marker drain destination is too small".into());
            }
            for frame in 0..frames {
                let ring_frame =
                    (self.write_frame + self.drain_cursor_frames + frame) % self.delay_frames;
                let source_start = ring_frame * self.channels;
                let output_start = frame * self.channels;
                output[output_start..output_start + self.channels]
                    .copy_from_slice(&self.delay_line[source_start..source_start + self.channels]);
            }
            self.drain_cursor_frames += frames;
            self.drained = self.drain_cursor_frames == self.delay_frames;
            self.publish_observation();
            if self.fail_after_drain {
                return Err("injected finite-tail failure after producer advancement".into());
            }
            Ok(PluginDrainResult {
                frames,
                complete: self.drained,
            })
        }

        fn reset(&mut self) {
            self.delay_line.fill(0.0);
            self.write_frame = 0;
            self.drain_cursor_frames = 0;
            self.has_input = false;
            self.drained = false;
            self.fail_after_drain = false;
            self.publish_observation();
        }
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct TailMetadataObservation {
        process_calls: usize,
        begin_drain_calls: usize,
        drain_calls: usize,
    }

    struct TailMetadataProbe {
        channels: usize,
        tail: TailLength,
        observation: Arc<std::sync::Mutex<TailMetadataObservation>>,
    }

    impl TailMetadataProbe {
        fn new(
            channels: usize,
            tail: TailLength,
        ) -> (Self, Arc<std::sync::Mutex<TailMetadataObservation>>) {
            let observation = Arc::new(std::sync::Mutex::new(TailMetadataObservation::default()));
            (
                Self {
                    channels,
                    tail,
                    observation: Arc::clone(&observation),
                },
                observation,
            )
        }
    }

    impl Plugin for TailMetadataProbe {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Tail metadata probe", "0.1", "test")
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

        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
            Err("tail metadata probe has no parameters".into())
        }

        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }

        fn guarantees_identity_frame_geometry(&self) -> bool {
            true
        }

        fn tail_length(&self) -> TailLength {
            self.tail
        }

        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            let expected_samples = context.num_frames * self.channels;
            if input.len() != expected_samples || output.len() != expected_samples {
                return Err("tail metadata probe received a malformed block".into());
            }
            output.copy_from_slice(input);
            self.observation.lock().unwrap().process_calls += 1;
            Ok(context.num_frames)
        }

        fn drain_output_frames_max(&self) -> usize {
            0
        }

        fn begin_drain(&mut self, _: &ProcessContext) -> PluginResult<()> {
            self.observation.lock().unwrap().begin_drain_calls += 1;
            Ok(())
        }

        fn drain(&mut self, _: &mut [f32], _: &ProcessContext) -> PluginResult<PluginDrainResult> {
            self.observation.lock().unwrap().drain_calls += 1;
            Ok(PluginDrainResult::COMPLETE)
        }
    }

    type SharedFakeWriterState = Arc<std::sync::Mutex<FakeWriterState>>;
    type SharedTailMetadataObservation = Arc<std::sync::Mutex<TailMetadataObservation>>;

    fn make_width_mismatch_sink_host(
        configured_input_channels: usize,
        node_channels: &[usize],
    ) -> (
        DawHost,
        SharedFakeWriterState,
        Vec<SharedTailMetadataObservation>,
    ) {
        let mut host = DawHost::new(configured_input_channels, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        let mut node_ids = Vec::with_capacity(node_channels.len() + 1);
        let mut observations = Vec::with_capacity(node_channels.len());
        for (index, &channels) in node_channels.iter().enumerate() {
            let (probe, observation) = TailMetadataProbe::new(channels, TailLength::Finite(0));
            node_ids.push(
                host.add_node(format!("probe_{index}"), Box::new(probe))
                    .unwrap(),
            );
            observations.push(observation);
        }
        let sink_channels = *node_channels.last().expect("graph requires a producer");
        let (sink, state) =
            make_plugin_with_writer(sink_channels, (48_000, sink_channels as u32, 64), vec![]);
        node_ids.push(host.add_node("sink".into(), Box::new(sink)).unwrap());
        for edge in node_ids.windows(2) {
            host.add_edge(GraphEdge::new(edge[0], edge[1])).unwrap();
        }
        host.build().unwrap();
        (host, state, observations)
    }

    fn make_sink_host(
        delay_frames: usize,
        sink_capacity_frames: usize,
        writes: Vec<usize>,
    ) -> (
        DawHost,
        Arc<std::sync::Mutex<FakeWriterState>>,
        Arc<std::sync::Mutex<FiniteDelayObservation>>,
    ) {
        make_sink_host_for_channels(2, delay_frames, sink_capacity_frames, writes)
    }

    fn make_sink_host_for_channels(
        channels: usize,
        delay_frames: usize,
        sink_capacity_frames: usize,
        writes: Vec<usize>,
    ) -> (
        DawHost,
        Arc<std::sync::Mutex<FakeWriterState>>,
        Arc<std::sync::Mutex<FiniteDelayObservation>>,
    ) {
        make_chunked_sink_host_for_channels(
            channels,
            delay_frames,
            delay_frames,
            sink_capacity_frames,
            writes,
        )
    }

    fn make_chunked_sink_host_for_channels(
        channels: usize,
        delay_frames: usize,
        drain_chunk_frames: usize,
        sink_capacity_frames: usize,
        writes: Vec<usize>,
    ) -> (
        DawHost,
        Arc<std::sync::Mutex<FakeWriterState>>,
        Arc<std::sync::Mutex<FiniteDelayObservation>>,
    ) {
        let mut host = DawHost::new(channels, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        let (source, observation) =
            FiniteDelayMarker::new_chunked_observed(channels, delay_frames, drain_chunk_frames);
        host.add_plugin(Box::new(source)).unwrap();
        let (sink, state) = make_plugin_with_writer(
            channels,
            (48_000, channels as u32, sink_capacity_frames as u32),
            writes,
        );
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();
        (host, state, observation)
    }

    fn make_terminal_sink_host(source: Box<dyn Plugin>, sink: Box<dyn Plugin>) -> DawHost {
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        host.add_plugin(source).unwrap();
        host.add_plugin(sink).unwrap();
        host.build().unwrap();
        host
    }

    struct PassThroughObserver {
        observed: Arc<std::sync::Mutex<Vec<f32>>>,
    }

    impl Plugin for PassThroughObserver {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Pass-through observer", "0.1", "test")
        }

        fn input_channels(&self) -> usize {
            2
        }

        fn output_channels(&self) -> usize {
            2
        }

        fn parameters(&self) -> Vec<Parameter> {
            Vec::new()
        }

        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
            Err("pass-through observer has no parameters".into())
        }

        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }

        fn guarantees_identity_frame_geometry(&self) -> bool {
            true
        }

        fn tail_length(&self) -> TailLength {
            TailLength::Finite(0)
        }

        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            if input.len() != context.num_frames * 2 || output.len() != input.len() {
                return Err("pass-through observer received malformed audio".into());
            }
            output.copy_from_slice(input);
            self.observed.lock().unwrap().extend_from_slice(input);
            Ok(context.num_frames)
        }
    }

    #[derive(Debug, Default, Clone, PartialEq)]
    struct SinkControlObservation {
        process_calls: usize,
        sample_positions: [u64; 8],
        frame_counts: [usize; 8],
        gains: [f32; 8],
        automation_values: [f32; 8],
        gain_updates: usize,
        automation_updates: usize,
        fail_after_process: bool,
        panic_after_process: bool,
    }

    struct SinkControlObserver {
        gain: f32,
        automation_value: f32,
        observation: Arc<std::sync::Mutex<SinkControlObservation>>,
        flip_transport_after_process: Option<Arc<std::sync::Mutex<FakeWriterState>>>,
        config_change_after_process: Option<Arc<std::sync::Mutex<FakeWriterState>>>,
    }

    impl Plugin for SinkControlObserver {
        fn info(&self) -> PluginInfo {
            PluginInfo::new("Sink control observer", "0.1", "test")
        }

        fn input_channels(&self) -> usize {
            2
        }

        fn output_channels(&self) -> usize {
            2
        }

        fn parameters(&self) -> Vec<Parameter> {
            vec![
                Parameter::new_float("gain", "Gain", 1.0, 0.0, 8.0),
                Parameter::new_float("automation", "Automation", 1.0, 0.0, 8.0),
            ]
        }

        fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()> {
            let ParameterValue::Float(value) = value else {
                return Err("sink control observer expects float parameters".into());
            };
            match id.as_str() {
                "gain" => {
                    self.gain = value;
                    self.observation.lock().unwrap().gain_updates += 1;
                }
                "automation" => {
                    self.automation_value = value;
                    self.observation.lock().unwrap().automation_updates += 1;
                }
                _ => return Err("sink control observer received an unknown parameter".into()),
            }
            Ok(())
        }

        fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue> {
            match id.as_str() {
                "gain" => Some(ParameterValue::Float(self.gain)),
                "automation" => Some(ParameterValue::Float(self.automation_value)),
                _ => None,
            }
        }

        fn guarantees_identity_frame_geometry(&self) -> bool {
            true
        }

        fn tail_length(&self) -> TailLength {
            TailLength::Finite(0)
        }

        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            let expected = context.num_frames * 2;
            if input.len() != expected || output.len() != expected {
                return Err("sink control observer received malformed audio".into());
            }
            for (input, output) in input.iter().zip(output.iter_mut()) {
                *output = *input * self.gain;
            }
            let mut observation = self.observation.lock().unwrap();
            let slot = observation.process_calls;
            if slot >= observation.sample_positions.len() {
                return Err("sink control observer history overflow".into());
            }
            observation.sample_positions[slot] = context.transport.sample_position;
            observation.frame_counts[slot] = context.num_frames;
            observation.gains[slot] = self.gain;
            observation.automation_values[slot] = self.automation_value;
            observation.process_calls += 1;
            let fail_after_process = observation.fail_after_process;
            let panic_after_process = observation.panic_after_process;
            drop(observation);
            if let Some(state) = self.flip_transport_after_process.as_ref() {
                let mut state = state.lock().unwrap();
                state.connected = false;
                state.key_ready = false;
            }
            if let Some(state) = self.config_change_after_process.as_ref() {
                state.lock().unwrap().config_changed = true;
            }
            if fail_after_process {
                return Err("injected source failure after processing".into());
            }
            if panic_after_process {
                panic!("injected source panic after processing");
            }
            Ok(context.num_frames)
        }
    }

    fn make_sink_control_host(
        capacity_frames: usize,
        writes: Vec<usize>,
    ) -> (
        DawHost,
        Arc<std::sync::Mutex<FakeWriterState>>,
        Arc<std::sync::Mutex<SinkControlObservation>>,
    ) {
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        let observation = Arc::new(std::sync::Mutex::new(SinkControlObservation::default()));
        host.add_plugin(Box::new(SinkControlObserver {
            gain: 1.0,
            automation_value: 1.0,
            observation: Arc::clone(&observation),
            flip_transport_after_process: None,
            config_change_after_process: None,
        }))
        .unwrap();
        let (sink, state) = make_plugin_with_writer(2, (48_000, 2, capacity_frames as u32), writes);
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();
        (host, state, observation)
    }

    fn sink_telemetry(host: &DawHost) -> HalOutputTelemetry {
        host.get_plugin(1)
            .unwrap()
            .as_any()
            .and_then(|plugin| plugin.downcast_ref::<HalOutputPlugin>())
            .expect("HAL output exposes read-only telemetry through its plugin handle")
            .telemetry()
    }

    fn sink_plugin(host: &DawHost) -> &HalOutputPlugin {
        host.get_plugin(1)
            .unwrap()
            .as_any()
            .and_then(|plugin| plugin.downcast_ref::<HalOutputPlugin>())
            .expect("test route ends in the HAL output plugin")
    }

    fn sink_pending_samples(host: &DawHost) -> Vec<f32> {
        sink_plugin(host).pending.iter().copied().collect()
    }

    fn assert_local_telemetry_unchanged(before: HalOutputTelemetry, after: HalOutputTelemetry) {
        assert_eq!(after.requested_frames, before.requested_frames);
        assert_eq!(after.written_frames, before.written_frames);
        assert_eq!(after.dropped_frames, before.dropped_frames);
        assert_eq!(after.queued_frames, before.queued_frames);
        assert_eq!(after.queue_capacity_frames, before.queue_capacity_frames);
        assert_eq!(after.backpressure_events, before.backpressure_events);
        assert_eq!(after.target_fill_frames, before.target_fill_frames);
        assert_eq!(after.device_latency_frames, before.device_latency_frames);
        assert_eq!(after.safety_offset_frames, before.safety_offset_frames);
        assert_eq!(
            after.boundary_latency_frames,
            before.boundary_latency_frames
        );
    }

    fn finite_delay_stream(input: &[f32], channels: usize, delay_frames: usize) -> Vec<f32> {
        let mut delay = FiniteDelayMarker::new(channels, delay_frames);
        let context = ProcessContext::new(48_000, input.len() / channels);
        let mut ordinary = vec![0.0; input.len()];
        delay.process(input, &mut ordinary, &context).unwrap();
        let mut expected = ordinary;
        if delay.drain_output_frames_max() > 0 {
            let mut tail = vec![0.0; delay.drain_output_frames_max() * channels];
            let result = delay
                .drain(&mut tail, &ProcessContext::new(48_000, 0))
                .unwrap();
            assert!(result.complete);
            expected.extend_from_slice(&tail[..result.frames * channels]);
        }
        expected
    }

    fn drain_sink_to_completion(host: &mut DawHost, call_limit: usize) {
        for _ in 0..call_limit {
            if host.drain_to_sink().unwrap().complete {
                return;
            }
        }
        panic!("terminal sink did not finish within {call_limit} calls");
    }

    #[test]
    fn public_sink_input_refuses_over_capacity_before_advancing_source() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let (mut host, state, observation) = make_sink_host(2, 2, vec![0]);
        let source_before = observation.lock().unwrap().clone();

        let result = host.process_to_sink(&input);
        let source_after = observation.lock().unwrap().clone();
        eprintln!(
            "oversized sink admission: consumed={:?}, writer_calls={}, source_before={source_before:?}, source_after={source_after:?}",
            result
                .as_ref()
                .ok()
                .map(|result| result.input_frames_consumed),
            state.lock().unwrap().writes.len(),
        );

        assert_eq!(
            source_after, source_before,
            "capacity refusal must precede source processing"
        );
        assert_eq!(
            result.unwrap_err(),
            sotf_host::host::SinkProcessError::RetryablePreflight(
                sotf_host::host::SinkPreflightError::InputExceedsCapacity {
                    input_frames: 4,
                    capacity_frames: 2,
                }
            ),
            "a block larger than total sink capacity must be refused before source work"
        );
        assert!(state.lock().unwrap().writes.is_empty());

        let expected = finite_delay_stream(&input, 2, 2);
        let first_chunk = &input[..4];
        let second_chunk = &input[4..];
        let first_admitted = host.process_to_sink(first_chunk).unwrap();
        assert_eq!(first_admitted.input_frames_consumed, 2);
        assert_eq!(first_admitted.pending_sink_frames, 2);
        let service_only = host.process_to_sink(second_chunk).unwrap();
        assert_eq!(service_only.input_frames_consumed, 0);
        assert_eq!(service_only.pending_sink_frames, 0);
        let second_admitted = host.process_to_sink(second_chunk).unwrap();
        assert_eq!(second_admitted.input_frames_consumed, 2);
        let mut drain_calls = 0;
        loop {
            let result = host.drain_to_sink().unwrap();
            drain_calls += 1;
            if result.complete {
                break;
            }
            assert!(drain_calls < 5);
        }
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn public_sink_input_waits_for_pending_capacity_then_retries_distinct_block_once() {
        let first = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let second = [9.0, -9.0];
        let mut combined = first.to_vec();
        combined.extend_from_slice(&second);
        let expected = finite_delay_stream(&combined, 2, 2);
        let (mut host, state, observation) = make_sink_host(2, 4, vec![0, 0]);

        let first_result = host.process_to_sink(&first).unwrap();
        assert_eq!(first_result.input_frames_consumed, 4);
        assert_eq!(first_result.pending_sink_frames, 4);
        let source_before_retry = observation.lock().unwrap().clone();
        let retry = host.process_to_sink(&second).unwrap();
        let source_after_retry = observation.lock().unwrap().clone();
        eprintln!(
            "full-queue retry: consumed={}, writer_calls={}, source_before={source_before_retry:?}, source_after={source_after_retry:?}",
            retry.input_frames_consumed,
            state.lock().unwrap().writes.len(),
        );

        assert_eq!(
            retry.input_frames_consumed, 0,
            "when the queue is full, the exact input block must remain unconsumed"
        );
        assert_eq!(retry.pending_sink_frames, 4);
        assert_eq!(source_after_retry, source_before_retry);
        while host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state()
            .pending_frames
            > 0
        {
            let service_only = host.process_to_sink(&second).unwrap();
            assert_eq!(service_only.input_frames_consumed, 0);
            assert_eq!(service_only.pending_sink_frames, 0);
        }

        assert_eq!(
            host.process_to_sink(&second).unwrap().input_frames_consumed,
            1
        );
        let mut drain_calls = 0;
        loop {
            let result = host.drain_to_sink().unwrap();
            drain_calls += 1;
            if result.complete {
                break;
            }
            assert!(drain_calls < 8);
        }
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn public_sink_service_only_preserves_controls_clocks_and_requested_telemetry() {
        let first = [0.25, -0.5, 0.75, -1.0];
        let second = [9.0, -9.0];
        let (mut host, state, observation) = make_sink_control_host(2, vec![0, 0, 2]);
        host.set_automation(
            0,
            ParameterId::from("automation"),
            sotf_host::automation::AutomationCurve::Step {
                values: vec![2.0, 4.0],
                samples_per_step: 2,
            },
        )
        .unwrap();

        let first_result = host.process_to_sink(&first).unwrap();
        assert_eq!(first_result.input_frames_consumed, 2);
        assert_eq!(first_result.pending_sink_frames, 2);
        let after_first = observation.lock().unwrap().clone();
        assert_eq!(after_first.process_calls, 1);
        assert_eq!(after_first.sample_positions[0], 0);
        assert_eq!(after_first.frame_counts[0], 2);
        assert_eq!(after_first.gains[0], 1.0);
        assert_eq!(after_first.automation_values[0], 2.0);
        let after_first_telemetry = sink_telemetry(&host);
        assert_eq!(after_first_telemetry.requested_frames, 2);
        assert_eq!(after_first_telemetry.written_frames, 0);
        assert_eq!(after_first_telemetry.dropped_frames, 0);

        host.queue_node_parameter(0, ParameterId::from("gain"), ParameterValue::Float(3.0))
            .unwrap();
        let writes_before_refusal = state.lock().unwrap().writer_attempts;
        let blocked = host.process_to_sink(&second).unwrap();
        assert_eq!(blocked.input_frames_consumed, 0);
        assert_eq!(blocked.pending_sink_frames, 2);
        assert_eq!(*observation.lock().unwrap(), after_first);
        assert_eq!(sink_telemetry(&host).requested_frames, 2);
        assert!(state.lock().unwrap().writer_attempts - writes_before_refusal <= 2);

        let writes_before_empty_service = state.lock().unwrap().writer_attempts;
        let serviced = host.process_to_sink(&[]).unwrap();
        assert_eq!(serviced.input_frames_consumed, 0);
        assert_eq!(serviced.pending_sink_frames, 0);
        assert_eq!(*observation.lock().unwrap(), after_first);
        let after_service = sink_telemetry(&host);
        assert_eq!(after_service.requested_frames, 2);
        assert_eq!(after_service.written_frames, 2);
        assert_eq!(after_service.dropped_frames, 0);
        assert_eq!(after_service.queued_frames, 0);
        assert!(state.lock().unwrap().writer_attempts - writes_before_empty_service <= 2);
        assert_eq!(state.lock().unwrap().accepted_samples, first);

        let writes_before_retry = state.lock().unwrap().writer_attempts;
        let resumed = host.process_to_sink(&second).unwrap();
        assert_eq!(resumed.input_frames_consumed, 1);
        assert_eq!(resumed.pending_sink_frames, 0);
        assert!(state.lock().unwrap().writer_attempts - writes_before_retry <= 2);
        let after_retry = observation.lock().unwrap().clone();
        assert_eq!(after_retry.process_calls, 2);
        assert_eq!(&after_retry.sample_positions[..2], &[0, 2]);
        assert_eq!(&after_retry.frame_counts[..2], &[2, 1]);
        assert_eq!(&after_retry.gains[..2], &[1.0, 3.0]);
        assert_eq!(&after_retry.automation_values[..2], &[2.0, 4.0]);
        assert_eq!(after_retry.gain_updates, 1);
        assert_eq!(after_retry.automation_updates, 2);

        let telemetry = sink_telemetry(&host);
        assert_eq!(telemetry.requested_frames, 3);
        assert_eq!(telemetry.written_frames, 3);
        assert_eq!(telemetry.dropped_frames, 0);
        let expected = [first.as_slice(), &[27.0, -27.0]].concat();
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn public_sink_parameter_subblocks_share_one_bounded_service_phase() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let (mut host, state, observation) = make_sink_control_host(8, vec![]);
        host.queue_node_parameter_at(0, ParameterId::from("gain"), ParameterValue::Float(2.0), 1)
            .unwrap();
        host.queue_node_parameter_at(0, ParameterId::from("gain"), ParameterValue::Float(4.0), 3)
            .unwrap();

        let result = host.process_to_sink(&input).unwrap();
        assert_eq!(result.input_frames_consumed, 4);
        assert_eq!(result.pending_sink_frames, 0);
        let observed = observation.lock().unwrap().clone();
        assert_eq!(observed.process_calls, 3);
        assert_eq!(&observed.sample_positions[..3], &[0, 1, 3]);
        assert_eq!(&observed.frame_counts[..3], &[1, 2, 1]);
        assert_eq!(&observed.gains[..3], &[1.0, 2.0, 4.0]);
        assert!(state.lock().unwrap().writer_attempts <= 2);
        assert_eq!(
            sink_telemetry(&host).requested_frames,
            input.len() as u64 / 2
        );
        assert_eq!(sink_telemetry(&host).written_frames, 4);
        assert_eq!(sink_telemetry(&host).dropped_frames, 0);
        let expected = [0.25, -0.5, 1.5, -2.0, 0.25, -0.5, 2.5, -3.0];
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn public_sink_transport_availability_flip_retains_admitted_audio_without_recovery_io() {
        let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 4), vec![]);
        let observation = Arc::new(std::sync::Mutex::new(SinkControlObservation::default()));
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        host.add_plugin(Box::new(SinkControlObserver {
            gain: 1.0,
            automation_value: 1.0,
            observation: Arc::clone(&observation),
            flip_transport_after_process: Some(Arc::clone(&state)),
            config_change_after_process: None,
        }))
        .unwrap();
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();
        let input = [0.25, -0.5, 0.75, -1.0];
        let initial_io = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.clear_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.flush_count,
            )
        };

        let admitted = host.process_to_sink(&input).unwrap();
        assert_eq!(admitted.input_frames_consumed, 2);
        assert_eq!(admitted.pending_sink_frames, 2);
        assert_eq!(state.lock().unwrap().writer_attempts, 0);
        let retained = sink_telemetry(&host);
        assert_eq!(retained.requested_frames, 2);
        assert_eq!(retained.written_frames, 0);
        assert_eq!(retained.dropped_frames, 0);
        assert_eq!(retained.queued_frames, 2);
        let service = host.process_to_sink(&[]).unwrap();
        assert_eq!(service.input_frames_consumed, 0);
        assert_eq!(service.pending_sink_frames, 2);
        assert!(state.lock().unwrap().accepted_samples.is_empty());
        assert_eq!(state.lock().unwrap().writer_attempts, 0);
        let final_io = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.clear_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.flush_count,
            )
        };
        assert_eq!(
            final_io, initial_io,
            "service must not adopt transport state"
        );
        let telemetry = sink_telemetry(&host);
        assert_eq!(telemetry.requested_frames, 2);
        assert_eq!(telemetry.written_frames, 0);
        assert_eq!(telemetry.dropped_frames, 0);
        assert_eq!(telemetry.queued_frames, 2);
        assert_eq!(observation.lock().unwrap().process_calls, 1);
    }

    #[test]
    fn public_sink_preflight_does_not_adopt_changed_transport_configuration() {
        let input = [0.25, -0.5, 0.75, -1.0];
        let (mut host, state, observation) = make_sink_control_host(4, vec![]);
        let initial_io = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.clear_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.flush_count,
            )
        };
        state.lock().unwrap().config_changed = true;

        assert_eq!(
            host.process_to_sink(&input).unwrap_err(),
            sotf_host::host::SinkProcessError::RetryablePreflight(
                sotf_host::host::SinkPreflightError::Sink(SinkTailPreflightError::InvalidGeometry)
            )
        );
        assert_eq!(observation.lock().unwrap().process_calls, 0);
        let refused = sink_telemetry(&host);
        assert_eq!(refused.requested_frames, 0);
        assert_eq!(refused.written_frames, 0);
        assert_eq!(refused.queued_frames, 0);
        let after_refusal_io = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.clear_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.flush_count,
                snapshot.config_changed,
            )
        };
        assert_eq!(
            after_refusal_io,
            (initial_io.0, initial_io.1, initial_io.2, initial_io.3, true,),
            "audio admission must not service or clear changed transport state"
        );

        state.lock().unwrap().config_changed = false;
        assert_eq!(
            host.process_to_sink(&input).unwrap().input_frames_consumed,
            2
        );
        assert_eq!(state.lock().unwrap().accepted_samples, input);
        let telemetry = sink_telemetry(&host);
        assert_eq!(telemetry.requested_frames, 2);
        assert_eq!(telemetry.written_frames, 2);
        assert_eq!(telemetry.dropped_frames, 0);
    }

    #[test]
    fn public_sink_input_service_retry_and_terminal_calls_do_not_touch_the_heap() {
        let first = [0.25, -0.5, 0.75, -1.0];
        let second = [9.0, -9.0];
        let (mut host, state, observation) = make_sink_control_host(2, vec![0, 0, 2]);
        state.lock().unwrap().capture_audio = false;

        assert_no_heap_activity("host sink initial empty input", || {
            let result = host.process_to_sink(&[]).unwrap();
            assert_eq!(result.input_frames_consumed, 0);
            assert_eq!(result.pending_sink_frames, 0);
        });
        assert_eq!(observation.lock().unwrap().process_calls, 0);
        assert_eq!(state.lock().unwrap().writer_attempts, 0);
        assert_no_heap_activity("host sink admitted input", || {
            let result = host.process_to_sink(&first).unwrap();
            assert_eq!(result.input_frames_consumed, 2);
            assert_eq!(result.pending_sink_frames, 2);
        });
        assert_no_heap_activity("host sink blocked retry", || {
            let result = host.process_to_sink(&second).unwrap();
            assert_eq!(result.input_frames_consumed, 0);
            assert_eq!(result.pending_sink_frames, 2);
        });
        assert_no_heap_activity("host sink empty service", || {
            let result = host.process_to_sink(&[]).unwrap();
            assert_eq!(result.input_frames_consumed, 0);
            assert_eq!(result.pending_sink_frames, 0);
        });
        assert_no_heap_activity("host sink recovered input", || {
            let result = host.process_to_sink(&second).unwrap();
            assert_eq!(result.input_frames_consumed, 1);
            assert_eq!(result.pending_sink_frames, 0);
        });
        assert_no_heap_activity("host sink terminal drain", || {
            assert!(host.drain_to_sink().unwrap().complete);
        });

        let telemetry = sink_telemetry(&host);
        assert_eq!(telemetry.requested_frames, 3);
        assert_eq!(telemetry.written_frames, 3);
        assert_eq!(telemetry.dropped_frames, 0);
        assert_eq!(state.lock().unwrap().writer_attempts, 4);
    }

    #[test]
    fn public_sink_settlement_services_then_applies_queued_edit_without_replay() {
        let first = [0.25, -0.5, 0.75, -1.0];
        let second = [9.0, -9.0];
        let mut expected = first.to_vec();
        expected.extend_from_slice(&second);
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        host.add_plugin(Box::new(PassThroughObserver {
            observed: Arc::clone(&observed),
        }))
        .unwrap();
        let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![0, 0, 2]);
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();

        let first_result = host.process_to_sink(&first).unwrap();
        assert_eq!(first_result.input_frames_consumed, 2);
        assert_eq!(first_result.pending_sink_frames, 2);
        let first_observation = observed.lock().unwrap().clone();
        let writer_calls_before_refusal = state.lock().unwrap().writes.len();

        assert!(host.remove_plugin(1).is_err());
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames,
            2,
            "direct graph mutation must not orphan retained sink frames"
        );

        host.queue_remove_plugin(0).unwrap();
        assert_eq!(
            host.process_to_sink(&second).unwrap_err(),
            sotf_host::host::SinkProcessError::RetryablePreflight(
                sotf_host::host::SinkPreflightError::PendingGraphMutation
            )
        );
        assert_eq!(*observed.lock().unwrap(), first_observation);
        assert_eq!(
            state.lock().unwrap().writes.len(),
            writer_calls_before_refusal
        );
        assert!(state.lock().unwrap().accepted_samples.is_empty());

        // Ordinary build recompiles only the current graph; it does not settle
        // the host-owned mutation queue or disturb the retained marker.
        host.build().unwrap();
        assert!(matches!(
            host.process_to_sink(&second),
            Err(sotf_host::host::SinkProcessError::RetryablePreflight(
                sotf_host::host::SinkPreflightError::PendingGraphMutation
            ))
        ));
        assert_eq!(*observed.lock().unwrap(), first_observation);

        assert_eq!(
            host.settle_terminal_sink_graph_mutations().unwrap(),
            sotf_host::host::SinkGraphSettlementResult::ServiceOnly {
                pending_sink_frames: 2,
            }
        );
        assert_eq!(*observed.lock().unwrap(), first_observation);
        assert_eq!(
            host.settle_terminal_sink_graph_mutations().unwrap(),
            sotf_host::host::SinkGraphSettlementResult::ServiceOnly {
                pending_sink_frames: 0,
            }
        );
        assert_eq!(state.lock().unwrap().accepted_samples, first);
        assert_eq!(
            host.settle_terminal_sink_graph_mutations().unwrap(),
            sotf_host::host::SinkGraphSettlementResult::Settled
        );

        let resumed = host.process_to_sink(&second).unwrap();
        assert_eq!(resumed.input_frames_consumed, 1);
        assert_eq!(resumed.pending_sink_frames, 0);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(*observed.lock().unwrap(), first);
    }

    #[test]
    fn public_sink_settlement_repairs_after_partial_graph_error_without_replaying_input() {
        let first = [0.25, -0.5];
        let second = [9.0, -9.0];
        let (mut host, old_state, observation) = make_sink_control_host(2, vec![0, 1]);

        let admitted = host.process_to_sink(&first).unwrap();
        assert_eq!(admitted.input_frames_consumed, 1);
        assert_eq!(admitted.pending_sink_frames, 1);
        let before_edits = observation.lock().unwrap().clone();
        host.queue_remove_plugin(1).unwrap();
        host.queue_remove_plugin(99).unwrap();

        assert_eq!(
            host.settle_terminal_sink_graph_mutations().unwrap(),
            sotf_host::host::SinkGraphSettlementResult::ServiceOnly {
                pending_sink_frames: 0,
            }
        );
        assert_eq!(old_state.lock().unwrap().accepted_samples, first);
        assert_eq!(*observation.lock().unwrap(), before_edits);

        assert!(matches!(
            host.settle_terminal_sink_graph_mutations(),
            Err(sotf_host::host::SinkGraphSettlementError::Graph(_))
        ));
        assert_eq!(*observation.lock().unwrap(), before_edits);
        assert!(matches!(
            host.process_to_sink(&second),
            Err(sotf_host::host::SinkProcessError::RetryablePreflight(
                sotf_host::host::SinkPreflightError::GraphNotBuilt
            ))
        ));
        assert_eq!(*observation.lock().unwrap(), before_edits);

        let (replacement_sink, replacement_state) =
            make_plugin_with_writer(2, (48_000, 2, 4), vec![]);
        host.add_plugin(Box::new(replacement_sink)).unwrap();
        host.build().unwrap();
        assert_eq!(
            host.settle_terminal_sink_graph_mutations().unwrap(),
            sotf_host::host::SinkGraphSettlementResult::Settled
        );

        let resumed = host.process_to_sink(&second).unwrap();
        assert_eq!(resumed.input_frames_consumed, 1);
        assert_eq!(replacement_state.lock().unwrap().accepted_samples, second);
        assert_eq!(old_state.lock().unwrap().accepted_samples, first);
        let final_observation = observation.lock().unwrap();
        assert_eq!(final_observation.process_calls, 2);
        assert_eq!(&final_observation.sample_positions[..2], &[0, 1]);
    }

    #[test]
    fn ordinary_process_rejects_terminal_sink_before_audio_side_effects() {
        let input = [
            0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75, 1.0, -0.875, 0.5, -0.375, 0.875,
            -0.125, 0.375, -0.625,
        ];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![]);
        let ordinary = host.process(&input, &mut []);
        assert!(ordinary.unwrap_err().contains("process_to_sink"));
        assert!(state.lock().unwrap().accepted_samples.is_empty());
        assert_eq!(observation.lock().unwrap().drain_calls, 0);

        let processed = host.process_to_sink(&input).unwrap();
        assert_eq!(processed.input_frames_consumed, input.len() / 2);
        let mut calls = 0;
        let completion = loop {
            let result = host.drain_to_sink().unwrap();
            calls += 1;
            if result.complete {
                break result;
            }
            assert!(calls < 5, "the prepared sink route must converge");
        };
        assert_eq!(completion.source_tail_frames_handed_to_sink, 0);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
    }

    #[test]
    fn sink_mode_owns_commands_and_requires_reset_after_completion() {
        let mut extracted_sender_host = DawHost::new(2, 48_000);
        assert!(
            extracted_sender_host
                .take_parameter_event_sender()
                .is_some()
        );
        assert!(extracted_sender_host.enable_terminal_sink_mode().is_err());

        let (mut host, state, _) = make_sink_host(4, 8, vec![]);
        assert!(host.take_parameter_event_sender().is_none());
        assert!(host.take_graph_mutation_sender().is_none());

        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        host.process_to_sink(&input).unwrap();
        let first_drain = host.drain_to_sink().unwrap();
        assert!(!first_drain.complete);
        assert_eq!(first_drain.source_tail_frames_handed_to_sink, 4);
        assert!(host.settle_terminal_sink_graph_mutations().is_err());
        assert!(
            host.queue_node_parameter(0, ParameterId::from("gain"), ParameterValue::Float(0.5),)
                .is_err()
        );
        assert!(
            host.set_plugin_parameter_immediate(0, "gain", ParameterValue::Float(0.5))
                .is_err()
        );
        assert!(
            host.set_plugin_preferred_oversampling_enabled(true)
                .is_err()
        );

        let mut calls = 1;
        loop {
            let result = host.drain_to_sink().unwrap();
            calls += 1;
            if result.complete {
                break;
            }
            assert!(calls < 5);
        }
        let accepted_at_eof = state.lock().unwrap().accepted_samples.clone();

        assert!(host.settle_terminal_sink_graph_mutations().is_err());
        assert!(host.process_to_sink(&input).is_err());
        assert!(
            host.queue_node_parameter(0, ParameterId::from("gain"), ParameterValue::Float(0.75),)
                .is_err()
        );
        assert!(
            host.add_plugin(Box::new(FiniteDelayMarker::new(2, 4)))
                .is_err()
        );
        assert_eq!(state.lock().unwrap().accepted_samples, accepted_at_eof);

        host.reset();
        host.process_to_sink(&input).unwrap();
        let mut reset_calls = 0;
        loop {
            let result = host.drain_to_sink().unwrap();
            reset_calls += 1;
            if result.complete {
                break;
            }
            assert!(reset_calls < 5);
        }
    }

    #[test]
    fn public_daw_host_sink_drain_preflights_tail_before_any_source_consumption() {
        let input = [0.25, -0.5, 0.75, -1.0];
        let expected = finite_delay_stream(&input, 2, 4);
        assert!(expected[input.len()..].iter().any(|sample| *sample != 0.0));
        let (mut refused_host, refused_state, refused_observation) = make_sink_host(4, 2, vec![]);
        assert_eq!(
            refused_host
                .process_to_sink(&input)
                .unwrap()
                .input_frames_consumed,
            2
        );
        let source_before = refused_observation.lock().unwrap().clone();
        let refusal = refused_host.drain_to_sink().unwrap_err();
        let source_after = refused_observation.lock().unwrap().clone();
        assert!(matches!(
            refusal,
            sotf_host::host::SinkDrainError::SinkPreflight(
                SinkTailPreflightError::CapacityExceeded {
                    required_frames: 4,
                    ..
                }
            )
        ));
        assert_eq!(source_after.begin_drain_calls, 0);
        assert_eq!(source_after.drain_calls, 0);
        assert_eq!(source_after, source_before);
        assert_eq!(
            refused_state.lock().unwrap().accepted_samples,
            expected[..input.len()]
        );
        assert!(refused_host.remove_plugin(1).is_err());

        let (mut comparator, comparator_state, _) = make_sink_host(4, 8, vec![]);
        comparator.process_to_sink(&input).unwrap();
        let tail = comparator.drain_to_sink().unwrap();
        assert_eq!(tail.source_tail_frames_handed_to_sink, 4);
        assert!(!tail.complete);
        assert!(!comparator.drain_to_sink().unwrap().complete);
        assert!(comparator.drain_to_sink().unwrap().complete);
        assert_eq!(comparator_state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn aud138_unknown_or_infinite_upstream_tail_is_refused_before_source_drain() {
        let mut observed_cases = Vec::new();
        for tail in [TailLength::Unknown, TailLength::Infinite] {
            let mut host = DawHost::new(2, 48_000);
            host.enable_terminal_sink_mode().unwrap();
            host.set_plugin_preferred_oversampling_enabled(false)
                .unwrap();
            let (upstream, upstream_observation) = TailMetadataProbe::new(2, tail);
            host.add_plugin(Box::new(upstream)).unwrap();
            let (source, source_observation) = FiniteDelayMarker::new_observed(2, 4);
            host.add_plugin(Box::new(source)).unwrap();
            let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 16), vec![]);
            host.add_plugin(Box::new(sink)).unwrap();
            host.build().unwrap();

            let input = [0.25, -0.5, 0.75, -1.0];
            host.process_to_sink(&input).unwrap();
            let source_before = source_observation.lock().unwrap().clone();
            let writes_before = state.lock().unwrap().writer_attempts;

            let refused = matches!(
                host.drain_to_sink(),
                Err(sotf_host::host::SinkDrainError::UnsupportedTailMetadata {
                    tail: reported,
                    ..
                }) if reported == tail
            );
            let source_unchanged = *source_observation.lock().unwrap() == source_before;
            let no_source_drain =
                source_before.begin_drain_calls == 0 && source_before.drain_calls == 0;
            let writer_unchanged = state.lock().unwrap().writer_attempts == writes_before;

            // The rejected EOF request leaves the stream Running and does not
            // consume the next block or implicitly reset a recursive node.
            let still_running = refused
                && host.process_to_sink(&input).is_ok()
                && upstream_observation.lock().unwrap().process_calls == 2;
            observed_cases.push((
                tail,
                refused,
                source_unchanged,
                no_source_drain,
                writer_unchanged,
                still_running,
            ));
        }
        assert!(
            observed_cases
                .iter()
                .all(|case| case.1 && case.2 && case.3 && case.4 && case.5),
            "upstream tail preflight cases: {observed_cases:?}"
        );
    }

    #[test]
    fn aud138_unknown_or_infinite_final_tail_is_refused_before_source_drain() {
        let mut observed_cases = Vec::new();
        for tail in [TailLength::Unknown, TailLength::Infinite] {
            let mut host = DawHost::new(2, 48_000);
            host.enable_terminal_sink_mode().unwrap();
            host.set_plugin_preferred_oversampling_enabled(false)
                .unwrap();
            let (source, observation) = TailMetadataProbe::new(2, tail);
            host.add_plugin(Box::new(source)).unwrap();
            let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 16), vec![]);
            host.add_plugin(Box::new(sink)).unwrap();
            host.build().unwrap();

            let input = [0.25, -0.5, 0.75, -1.0];
            host.process_to_sink(&input).unwrap();
            let source_before = observation.lock().unwrap().clone();
            let writes_before = state.lock().unwrap().writer_attempts;

            let refused = matches!(
                host.drain_to_sink(),
                Err(sotf_host::host::SinkDrainError::UnsupportedTailMetadata {
                    tail: reported,
                    ..
                }) if reported == tail
            );
            let source_unchanged = *observation.lock().unwrap() == source_before;
            let no_source_drain =
                source_before.begin_drain_calls == 0 && source_before.drain_calls == 0;
            let writer_unchanged = state.lock().unwrap().writer_attempts == writes_before;
            let still_running = refused
                && host.process_to_sink(&input).is_ok()
                && observation.lock().unwrap().process_calls == 2;
            observed_cases.push((
                tail,
                refused,
                source_unchanged,
                no_source_drain,
                writer_unchanged,
                still_running,
            ));
        }
        assert!(
            observed_cases
                .iter()
                .all(|case| case.1 && case.2 && case.3 && case.4 && case.5),
            "final tail preflight cases: {observed_cases:?}"
        );
    }

    #[test]
    fn aud138_unbuilt_sink_eof_refuses_without_building_or_calling_plugins() {
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        let (source, observation) = TailMetadataProbe::new(2, TailLength::Finite(0));
        host.add_plugin(Box::new(source)).unwrap();
        let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 16), vec![]);
        host.add_plugin(Box::new(sink)).unwrap();

        assert!(matches!(
            host.drain_to_sink(),
            Err(sotf_host::host::SinkDrainError::GraphNotBuilt)
        ));
        assert_eq!(observation.lock().unwrap().begin_drain_calls, 0);
        assert_eq!(observation.lock().unwrap().drain_calls, 0);
        assert_eq!(state.lock().unwrap().writer_attempts, 0);
        assert!(host.process_to_sink(&[0.25, -0.5]).is_err());
        assert_eq!(observation.lock().unwrap().process_calls, 0);
    }

    #[test]
    fn aud138_host_input_width_mismatch_is_refused_before_process_or_eof() {
        let (mut process_host, process_state, process_observations) =
            make_width_mismatch_sink_host(2, &[4]);
        let process_refused = process_host
            .process_to_sink(&[0.25, -0.5, 0.75, -1.0])
            .is_err();
        let process_unchanged = process_observations[0].lock().unwrap().process_calls == 0
            && process_state.lock().unwrap().writer_attempts == 0;

        let (mut drain_host, drain_state, drain_observations) =
            make_width_mismatch_sink_host(2, &[4]);
        let drain_refused = matches!(
            drain_host.drain_to_sink(),
            Err(sotf_host::host::SinkDrainError::InvalidRoute(_))
        );
        let drain_unchanged = {
            let observation = drain_observations[0].lock().unwrap();
            observation.begin_drain_calls == 0
                && observation.drain_calls == 0
                && drain_state.lock().unwrap().writer_attempts == 0
        };
        assert!(
            process_refused && process_unchanged && drain_refused && drain_unchanged,
            "entrance mismatch process={process_refused}/{process_unchanged}, EOF={drain_refused}/{drain_unchanged}"
        );
    }

    #[test]
    fn aud138_adjacent_graph_width_mismatch_is_refused_before_process_or_eof() {
        let (mut process_host, process_state, process_observations) =
            make_width_mismatch_sink_host(2, &[2, 4]);
        let process_refused = process_host
            .process_to_sink(&[0.25, -0.5, 0.75, -1.0])
            .is_err();
        let process_unchanged = process_observations
            .iter()
            .all(|observation| observation.lock().unwrap().process_calls == 0)
            && process_state.lock().unwrap().writer_attempts == 0;

        let (mut drain_host, drain_state, drain_observations) =
            make_width_mismatch_sink_host(2, &[2, 4]);
        let drain_refused = matches!(
            drain_host.drain_to_sink(),
            Err(sotf_host::host::SinkDrainError::InvalidRoute(_))
        );
        let drain_unchanged = drain_observations.iter().all(|observation| {
            let observation = observation.lock().unwrap();
            observation.begin_drain_calls == 0 && observation.drain_calls == 0
        }) && drain_state.lock().unwrap().writer_attempts == 0;
        assert!(
            process_refused && process_unchanged && drain_refused && drain_unchanged,
            "adjacent mismatch process={process_refused}/{process_unchanged}, EOF={drain_refused}/{drain_unchanged}"
        );
    }

    #[test]
    fn aud138_post_reservation_config_change_keeps_the_whole_admitted_block() {
        let (sink, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
        let observation = Arc::new(std::sync::Mutex::new(SinkControlObservation::default()));
        let mut host = DawHost::new(2, 48_000);
        host.enable_terminal_sink_mode().unwrap();
        host.set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        host.add_plugin(Box::new(SinkControlObserver {
            gain: 1.0,
            automation_value: 1.0,
            observation: Arc::clone(&observation),
            flip_transport_after_process: None,
            config_change_after_process: Some(Arc::clone(&state)),
        }))
        .unwrap();
        host.add_plugin(Box::new(sink)).unwrap();
        host.build().unwrap();

        let input = [0.25, -0.5, 0.75, -1.0];
        let result = host.process_to_sink(&input).unwrap();
        assert_eq!(result.input_frames_consumed, 2);
        assert_eq!(result.pending_sink_frames, 2);
        assert_eq!(observation.lock().unwrap().process_calls, 1);
        assert_eq!(sink_telemetry(&host).requested_frames, 2);
        assert_eq!(sink_telemetry(&host).dropped_frames, 0);
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .as_any()
                .and_then(|plugin| plugin.downcast_ref::<HalOutputPlugin>())
                .unwrap()
                .pending
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            input
        );
        assert_eq!(state.lock().unwrap().writer_attempts, 0);
    }

    #[test]
    fn aud138_control_recovery_preserves_pending_audio_and_drains_exact_suffix() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        let processed = host.process_to_sink(&input).unwrap();
        assert_eq!(processed.input_frames_consumed, 4);
        assert_eq!(processed.pending_sink_frames, 4);
        let pending_before = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(pending_before.pending_frames, 4);
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();
        let flushes_before_recovery = state.lock().unwrap().flush_count;
        let source_before = observation.lock().unwrap().clone();
        state.lock().unwrap().connected = false;
        let blocked_eof = host.drain_to_sink().unwrap();
        assert!(!blocked_eof.complete);
        assert_eq!(blocked_eof.source_tail_frames_handed_to_sink, 0);
        let blocked_observation = observation.lock().unwrap().clone();
        assert_eq!(blocked_observation.drain_calls, 0);

        // The device changes its ring size while pending samples and a finite
        // source tail are owned by the prepared host route. A control-thread
        // recovery must refuse both growth and shrink without flushing either.
        state.lock().unwrap().transport_format = Some((48_000, 2, 16));
        assert!(matches!(
            host.recover_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                SinkTransportRecoveryError::NeedsReprepare {
                    expected: SinkTransportFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        buffer_frames: 8,
                    },
                    actual: Some(SinkTransportFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        buffer_frames: 16,
                    }),
                }
            ))
        ));
        let after_growth = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(after_growth, pending_before);
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
        let flushes_after_growth = state.lock().unwrap().flush_count;
        assert_eq!(flushes_after_growth, flushes_before_recovery);

        state.lock().unwrap().transport_format = Some((48_000, 2, 4));
        assert!(matches!(
            host.recover_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                SinkTransportRecoveryError::NeedsReprepare {
                    expected: SinkTransportFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        buffer_frames: 8,
                    },
                    actual: Some(SinkTransportFormat {
                        sample_rate: 48_000,
                        channels: 2,
                        buffer_frames: 4,
                    }),
                }
            ))
        ));
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state(),
            pending_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        let flushes_after_shrink = state.lock().unwrap().flush_count;
        assert_eq!(flushes_after_shrink, flushes_before_recovery);

        // An unavailable format is a retryable waiting state. It must not
        // consume producer EOF state or silently discard the retained queue.
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = None;
            snapshot.format_error = true;
        }
        assert_eq!(
            host.recover_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Waiting(SinkTransportWaitingReason::FormatUnavailable)
        );
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state(),
            pending_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        let flushes_while_unavailable = state.lock().unwrap().flush_count;
        assert_eq!(flushes_while_unavailable, flushes_before_recovery);
        let observation_while_unavailable = observation.lock().unwrap().clone();
        assert_eq!(observation_while_unavailable, source_before);

        state.lock().unwrap().format_error = false;
        assert_eq!(
            host.recover_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state(),
            pending_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
        let flushes_after_recovery = state.lock().unwrap().flush_count;
        assert_eq!(flushes_after_recovery, flushes_before_recovery + 1);
        let observation_after_recovery = observation.lock().unwrap().clone();
        assert_eq!(observation_after_recovery, source_before);

        let mut drain_calls = 0;
        loop {
            let drained = host.drain_to_sink().unwrap();
            drain_calls += 1;
            if drained.complete {
                break;
            }
            assert!(
                drain_calls < 8,
                "recovered route must finish its finite tail"
            );
        }
        let accepted_samples = state.lock().unwrap().accepted_samples.clone();
        assert_eq!(accepted_samples, expected);
        let final_observation = observation.lock().unwrap().clone();
        assert_eq!(final_observation.drain_calls, 1);
        assert!(final_observation.drained);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
    }

    #[test]
    fn aud138_reprepare_growth_preserves_pending_frames_and_full_finite_stream() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        let accepted = host.process_to_sink(&input).unwrap();
        assert_eq!(accepted.input_frames_consumed, 4);
        let queue_before = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(queue_before.pending_frames, 4);
        let source_before = observation.lock().unwrap().clone();
        let telemetry_before = sink_telemetry(&host);
        let old_latency = host.total_latency_samples();

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        let queue_after = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(queue_after.pending_frames, queue_before.pending_frames);
        assert_eq!(queue_after.capacity_frames, 16);
        assert_eq!(host.total_latency_samples(), old_latency + 16);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
        assert_eq!(state.lock().unwrap().flush_count, 2);

        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
    }

    #[test]
    fn aud138_reprepare_shrink_keeps_pending_queue_larger_than_physical_ring() {
        let input = [
            0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75, 1.0, -0.875, 0.5, -0.375,
        ];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        let accepted = host.process_to_sink(&input).unwrap();
        assert_eq!(accepted.input_frames_consumed, 6);
        let queue_before = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(queue_before.pending_frames, 6);
        let source_before = observation.lock().unwrap().clone();
        let old_latency = host.total_latency_samples();

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 4));
            snapshot.config_changed = true;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        let queue_after = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        assert_eq!(queue_after.pending_frames, 6);
        assert_eq!(queue_after.capacity_frames, 6);
        assert_eq!(host.total_latency_samples(), old_latency - 8);
        assert_eq!(observation.lock().unwrap().clone(), source_before);

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.capture_audio = false;
            snapshot.capture_accepted_samples = true;
            snapshot
                .accepted_samples
                .try_reserve_exact(expected.len())
                .unwrap();
            // Keep a complete backlog through the first service attempt, then
            // accept it on the next call before the producer tail is emitted.
            snapshot.scheduled_writes.extend([0, 6]);
        }
        assert_no_heap_activity(
            "shrunken ring services retained backlog and finite tail",
            || {
                drain_sink_to_completion(&mut host, 8);
            },
        );
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
    }

    fn assert_started_drain_reprepare_preserves_stream(target_ring_frames: u32) {
        let input = [
            0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75, 1.0, -0.875, 0.5, -0.375, 0.875,
            -0.125, 0.375, -0.625,
        ];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut twin_host, twin_state, twin_observation) = make_sink_host(4, 8, vec![8, 0]);
        assert_eq!(
            twin_host
                .process_to_sink(&input)
                .unwrap()
                .input_frames_consumed,
            8
        );
        assert!(!twin_host.drain_to_sink().unwrap().complete);
        drain_sink_to_completion(&mut twin_host, 8);
        let twin_samples = twin_state.lock().unwrap().accepted_samples.clone();
        assert_eq!(twin_samples, expected);
        let twin_source = twin_observation.lock().unwrap().clone();
        assert_eq!(twin_source.begin_drain_calls, 1);
        assert_eq!(twin_source.drain_calls, 1);

        let (mut host, state, observation) = make_sink_host(4, 8, vec![8, 0]);
        let accepted = host.process_to_sink(&input).unwrap();
        assert_eq!(accepted.input_frames_consumed, 8);
        assert_eq!(accepted.pending_sink_frames, 0);

        let first_drain = host.drain_to_sink().unwrap();
        assert!(!first_drain.complete);
        assert_eq!(first_drain.source_tail_frames_handed_to_sink, 4);
        let source_before = observation.lock().unwrap().clone();
        assert_eq!(source_before.begin_drain_calls, 1);
        assert_eq!(source_before.drain_calls, 1);
        assert!(source_before.drained);
        let pending_before = sink_pending_samples(&host);
        assert_eq!(pending_before.len(), 4 * 2);
        assert_ne!(
            pending_before
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs())),
            0.0
        );
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        assert_eq!(queue_before.pending_frames, 4);
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, target_ring_frames));
            snapshot.config_changed = true;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        let queue_after = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        assert_eq!(queue_after.pending_frames, queue_before.pending_frames);
        assert_eq!(
            queue_after.capacity_frames,
            (target_ring_frames as usize).max(queue_before.pending_frames)
        );
        assert_eq!(sink_pending_samples(&host), pending_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
        assert_eq!(
            host.total_latency_samples(),
            target_ring_frames as usize * 2
        );
        assert_ne!(latency_before, host.total_latency_samples());

        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(state.lock().unwrap().accepted_samples, twin_samples);
        let source_after = observation.lock().unwrap().clone();
        assert_eq!(source_after.begin_drain_calls, 1);
        assert_eq!(source_after.drain_calls, 1);
        assert!(source_after.drained);
    }

    #[test]
    fn aud138_reprepare_growth_and_shrink_preserve_an_already_started_drain() {
        assert_started_drain_reprepare_preserves_stream(16);
        assert_started_drain_reprepare_preserves_stream(4);
    }

    fn assert_mid_drain_reprepare_preserves_remaining_tail(target_ring_frames: u32) {
        let input = [
            0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75, 1.0, -0.875, 0.5, -0.375, 0.875,
            -0.125, 0.375, -0.625,
        ];
        let delay_frames = 6;
        let drain_chunk_frames = 2;
        let expected = finite_delay_stream(&input, 2, delay_frames);

        let (mut twin, twin_state, twin_observation) =
            make_chunked_sink_host_for_channels(2, delay_frames, drain_chunk_frames, 8, vec![8, 0]);
        assert_eq!(
            twin.process_to_sink(&input).unwrap().input_frames_consumed,
            8
        );
        let twin_first = twin.drain_to_sink().unwrap();
        assert!(!twin_first.complete);
        assert_eq!(
            twin_first.source_tail_frames_handed_to_sink,
            drain_chunk_frames
        );
        drain_sink_to_completion(&mut twin, 8);
        let twin_samples = twin_state.lock().unwrap().accepted_samples.clone();
        assert_eq!(twin_samples, expected);
        let twin_source = twin_observation.lock().unwrap().clone();
        assert_eq!(twin_source.begin_drain_calls, 1);
        assert_eq!(twin_source.drain_calls, 3);
        assert_eq!(twin_source.drain_cursor_frames, delay_frames);
        assert_eq!(twin_source.remaining_drain_calls, 0);
        assert!(twin_source.drained);

        let (mut host, state, observation) =
            make_chunked_sink_host_for_channels(2, delay_frames, drain_chunk_frames, 8, vec![8, 0]);
        assert_eq!(
            host.process_to_sink(&input).unwrap().input_frames_consumed,
            8
        );
        let first_drain = host.drain_to_sink().unwrap();
        assert!(!first_drain.complete);
        assert_eq!(
            first_drain.source_tail_frames_handed_to_sink,
            drain_chunk_frames
        );
        let source_before = observation.lock().unwrap().clone();
        assert_eq!(source_before.begin_drain_calls, 1);
        assert_eq!(source_before.drain_calls, 1);
        assert_eq!(source_before.drain_cursor_frames, drain_chunk_frames);
        assert_eq!(source_before.total_drain_call_bound, 3);
        assert_eq!(source_before.remaining_drain_calls, 2);
        assert!(
            !source_before.drained,
            "producer still owns four tail frames"
        );
        let pending_before = sink_pending_samples(&host);
        assert_eq!(pending_before.len(), drain_chunk_frames * 2);
        assert!(pending_before.iter().any(|sample| *sample != 0.0));
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        let latency_before = host.total_latency_samples();
        let telemetry_before = sink_telemetry(&host);

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, target_ring_frames));
            snapshot.config_changed = true;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        assert_eq!(sink_pending_samples(&host), pending_before);
        assert_eq!(
            sink_plugin(&host)
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames,
            queue_before.pending_frames
        );
        assert_eq!(
            host.total_latency_samples(),
            target_ring_frames as usize * 2
        );
        assert_ne!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );

        // The first post-reprepare call services the already queued samples.
        // It must not advance the producer while flushing that backlog.
        let backlog_flush = host.drain_to_sink().unwrap();
        assert!(!backlog_flush.complete);
        assert_eq!(backlog_flush.source_tail_frames_handed_to_sink, 0);
        assert_eq!(observation.lock().unwrap().clone(), source_before);

        // A writer may accept only part of that already queued suffix. Keep
        // servicing it until the queue is empty; none of these calls may
        // advance the still-active producer.
        for _ in 0..4 {
            if sink_plugin(&host)
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames
                == 0
            {
                break;
            }
            let flush = host.drain_to_sink().unwrap();
            assert!(!flush.complete);
            assert_eq!(flush.source_tail_frames_handed_to_sink, 0);
            assert_eq!(observation.lock().unwrap().clone(), source_before);
        }
        assert_eq!(
            sink_plugin(&host)
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames,
            0,
            "pre-reprepare sink backlog should flush before producer resumes"
        );

        let resumed_drain = host.drain_to_sink().unwrap();
        assert!(!resumed_drain.complete);
        assert_eq!(
            resumed_drain.source_tail_frames_handed_to_sink,
            drain_chunk_frames
        );
        let midway_source = observation.lock().unwrap().clone();
        assert_eq!(midway_source.begin_drain_calls, 1);
        assert_eq!(midway_source.drain_calls, 2);
        assert_eq!(midway_source.drain_cursor_frames, drain_chunk_frames * 2);
        assert_eq!(midway_source.remaining_drain_calls, 1);
        assert!(!midway_source.drained);

        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(state.lock().unwrap().accepted_samples, twin_samples);
        let final_source = observation.lock().unwrap().clone();
        assert_eq!(final_source.begin_drain_calls, 1);
        assert_eq!(final_source.drain_calls, 3);
        assert_eq!(final_source.drain_cursor_frames, delay_frames);
        assert_eq!(final_source.remaining_drain_calls, 0);
        assert!(final_source.drained);
    }

    #[test]
    fn aud138_reprepare_growth_and_shrink_preserve_unfinished_multichunk_tail() {
        assert_mid_drain_reprepare_preserves_remaining_tail(16);
        assert_mid_drain_reprepare_preserves_remaining_tail(4);
    }

    #[test]
    fn aud138_reprepare_priming_failure_preserves_local_stream_for_retry() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        host.process_to_sink(&input).unwrap();
        let queue_before = host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state();
        let source_before = observation.lock().unwrap().clone();
        let latency_before = host.total_latency_samples();
        let telemetry_before = sink_telemetry(&host);
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            snapshot.prime_write = Some(0);
        }

        assert!(matches!(
            host.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                SinkTransportRecoveryError::Transport(_)
            ))
        ));
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state(),
            queue_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );

        state.lock().unwrap().prime_write = None;
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames,
            queue_before.pending_frames
        );
        assert_eq!(observation.lock().unwrap().clone(), source_before);

        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
        assert_eq!(
            sink_telemetry(&host).dropped_frames,
            telemetry_before.dropped_frames
        );
    }

    #[test]
    fn aud138_reprepare_writer_error_cleans_partial_prime_and_retries() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        assert_eq!(host.process_to_sink(&input).unwrap().pending_sink_frames, 4);
        let pending_before = sink_pending_samples(&host);
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        let source_before = observation.lock().unwrap().clone();
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();
        let ready_count_before = state.lock().unwrap().ready.len();
        let flush_count_before = state.lock().unwrap().flush_count;

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            // The fake reports an impossible count after physically accepting
            // a partial prefix, so write_frames returns its contract error.
            snapshot.prime_write = Some(17);
            snapshot.prime_actual_write = Some(3);
        }
        assert!(matches!(
            host.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                SinkTransportRecoveryError::Transport(_)
            ))
        ));
        assert_eq!(
            sink_plugin(&host).state,
            HalOutputTransportState::PrimingFailed
        );
        assert_eq!(
            sink_plugin(&host)
                .pending
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            pending_before
        );
        assert_eq!(
            sink_plugin(&host).terminal_sink().unwrap().queue_state(),
            queue_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_local_telemetry_unchanged(telemetry_before, sink_telemetry(&host));
        {
            let snapshot = state.lock().unwrap();
            assert_eq!(snapshot.flush_count, flush_count_before + 2);
            assert_eq!(snapshot.fill_frames, 0, "failed partial fill was flushed");
            assert!(snapshot.priming, "writer is left in a clean priming state");
            assert!(
                snapshot.ready[ready_count_before..]
                    .iter()
                    .all(|ready| !ready),
                "failure must not publish Ready"
            );
        }

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.prime_write = None;
            snapshot.prime_actual_write = None;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        let final_source = observation.lock().unwrap().clone();
        assert_eq!(final_source.begin_drain_calls, 1);
        assert_eq!(final_source.drain_calls, 1);
    }

    fn assert_prime_mutation_preserves_and_retries(mutation: FakePrimeMutation) {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        assert_eq!(host.process_to_sink(&input).unwrap().pending_sink_frames, 4);
        let pending_before = sink_pending_samples(&host);
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        let source_before = observation.lock().unwrap().clone();
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();
        let ready_count_before = state.lock().unwrap().ready.len();
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            snapshot.prime_mutation = Some(mutation);
        }

        let result = host.reprepare_terminal_sink_transport();
        match mutation {
            FakePrimeMutation::ChangeFormat(_) => assert!(matches!(
                result,
                Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                    SinkTransportRecoveryError::NeedsReprepare {
                        expected: SinkTransportFormat {
                            sample_rate: 48_000,
                            channels: 2,
                            buffer_frames: 16,
                        },
                        actual: Some(SinkTransportFormat {
                            sample_rate: 48_000,
                            channels: 2,
                            buffer_frames: 32,
                        }),
                    }
                ))
            )),
            FakePrimeMutation::ClearConfiguration => assert!(matches!(
                result,
                Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                    SinkTransportRecoveryError::StalePlan
                ))
            )),
            FakePrimeMutation::Disconnect => assert_eq!(
                result.unwrap(),
                SinkTransportRecoveryStatus::Waiting(SinkTransportWaitingReason::Disconnected)
            ),
            FakePrimeMutation::LoseKey => assert_eq!(
                result.unwrap(),
                SinkTransportRecoveryStatus::Waiting(SinkTransportWaitingReason::KeyMismatch)
            ),
        }

        assert_eq!(sink_pending_samples(&host), pending_before);
        assert_eq!(
            sink_plugin(&host).terminal_sink().unwrap().queue_state(),
            queue_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_local_telemetry_unchanged(telemetry_before, sink_telemetry(&host));
        {
            let snapshot = state.lock().unwrap();
            assert!(
                snapshot.ready[ready_count_before..]
                    .iter()
                    .all(|ready| !ready),
                "stale plan or lost readiness must not publish Ready"
            );
        }

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            snapshot.connected = true;
            snapshot.key_ready = true;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        assert_eq!(sink_pending_samples(&host), pending_before);
        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        let final_source = observation.lock().unwrap().clone();
        assert_eq!(final_source.begin_drain_calls, 1);
        assert_eq!(final_source.drain_calls, 1);
    }

    #[test]
    fn aud138_reprepare_rejects_stale_plan_and_lost_readiness_then_retries() {
        for mutation in [
            FakePrimeMutation::ChangeFormat((48_000, 2, 32)),
            FakePrimeMutation::ClearConfiguration,
            FakePrimeMutation::Disconnect,
            FakePrimeMutation::LoseKey,
        ] {
            assert_prime_mutation_preserves_and_retries(mutation);
        }
    }

    fn assert_plan_observation_mutation_is_rejected_and_retried(mutation: FakePrimeMutation) {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        assert_eq!(host.process_to_sink(&input).unwrap().pending_sink_frames, 4);
        let pending_before = sink_pending_samples(&host);
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        let source_before = observation.lock().unwrap().clone();
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();
        let prepared_before = sink_plugin(&host)
            .terminal_sink()
            .unwrap()
            .prepared_transport_format();
        let (format_reads_before, flushes_before, reloads_before, ready_before, fill_before) = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.format_reads,
                snapshot.flush_count,
                snapshot.reload_count,
                snapshot.ready.len(),
                snapshot.fill_frames,
            )
        };
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            // The first format read captures the accepted plan. Mutate on the
            // second read, before host staging can be committed or the writer
            // can be quiesced.
            snapshot.format_mutation_on_read = Some((format_reads_before + 2, mutation));
        }

        let result = host.reprepare_terminal_sink_transport();
        match mutation {
            FakePrimeMutation::ChangeFormat(_) => assert!(
                matches!(
                    &result,
                    Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                        SinkTransportRecoveryError::StalePlan
                    ))
                ),
                "{mutation:?} returned {result:?}"
            ),
            FakePrimeMutation::ClearConfiguration => assert!(
                matches!(
                    &result,
                    Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                        SinkTransportRecoveryError::StalePlan
                    ))
                ),
                "{mutation:?} returned {result:?}"
            ),
            FakePrimeMutation::Disconnect | FakePrimeMutation::LoseKey => assert!(
                matches!(
                    &result,
                    Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                        SinkTransportRecoveryError::Transport(_)
                    ))
                ),
                "{mutation:?} returned {result:?}"
            ),
        }

        assert_eq!(
            state.lock().unwrap().format_reads,
            format_reads_before + 2,
            "mutation must land on the host's pre-commit plan recheck"
        );
        assert_eq!(sink_pending_samples(&host), pending_before);
        assert_eq!(
            sink_plugin(&host).terminal_sink().unwrap().queue_state(),
            queue_before
        );
        assert_eq!(
            sink_plugin(&host)
                .terminal_sink()
                .unwrap()
                .prepared_transport_format(),
            prepared_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_local_telemetry_unchanged(telemetry_before, sink_telemetry(&host));
        {
            let snapshot = state.lock().unwrap();
            assert_eq!(snapshot.flush_count, flushes_before);
            assert_eq!(snapshot.reload_count, reloads_before);
            assert_eq!(snapshot.ready.len(), ready_before);
            assert_eq!(snapshot.fill_frames, fill_before);
        }

        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            snapshot.connected = true;
            snapshot.key_ready = true;
            snapshot.format_mutation_on_read = None;
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        let final_source = observation.lock().unwrap().clone();
        assert_eq!(final_source.begin_drain_calls, 1);
        assert_eq!(final_source.drain_calls, 1);
    }

    #[test]
    fn aud138_reprepare_rejects_plan_observation_mutations_before_quiesce() {
        for mutation in [
            FakePrimeMutation::ChangeFormat((48_000, 2, 32)),
            FakePrimeMutation::ClearConfiguration,
            FakePrimeMutation::Disconnect,
            FakePrimeMutation::LoseKey,
        ] {
            assert_plan_observation_mutation_is_rejected_and_retried(mutation);
        }
    }

    #[test]
    fn aud138_reprepare_staging_failure_preserves_old_state_and_retries() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![0]);
        assert_eq!(host.process_to_sink(&input).unwrap().pending_sink_frames, 4);
        let pending_before = sink_pending_samples(&host);
        let queue_before = sink_plugin(&host).terminal_sink().unwrap().queue_state();
        let source_before = observation.lock().unwrap().clone();
        let telemetry_before = sink_telemetry(&host);
        let latency_before = host.total_latency_samples();
        let prepared_before = sink_plugin(&host)
            .terminal_sink()
            .unwrap()
            .prepared_transport_format();
        let (flushes_before, reloads_before, ready_before, fill_before) = {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
            (
                snapshot.flush_count,
                snapshot.reload_count,
                snapshot.ready.len(),
                snapshot.fill_frames,
            )
        };

        FAIL_ZEROED_SAMPLE_LABEL.with(|requested| {
            *requested.borrow_mut() = Some("drain staging reservation failed");
        });
        assert!(matches!(
            host.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                SinkTransportRecoveryError::Preparation(message)
            )) if message.contains("injected test failure")
        ));
        assert!(FAIL_ZEROED_SAMPLE_LABEL.with(|requested| requested.borrow().is_none()));
        assert_eq!(sink_pending_samples(&host), pending_before);
        assert_eq!(
            sink_plugin(&host).terminal_sink().unwrap().queue_state(),
            queue_before
        );
        assert_eq!(
            sink_plugin(&host)
                .terminal_sink()
                .unwrap()
                .prepared_transport_format(),
            prepared_before
        );
        assert_eq!(host.total_latency_samples(), latency_before);
        assert_eq!(observation.lock().unwrap().clone(), source_before);
        assert_local_telemetry_unchanged(telemetry_before, sink_telemetry(&host));
        {
            let snapshot = state.lock().unwrap();
            assert_eq!(snapshot.flush_count, flushes_before);
            assert_eq!(snapshot.reload_count, reloads_before);
            assert_eq!(snapshot.ready.len(), ready_before);
            assert_eq!(snapshot.fill_frames, fill_before);
        }

        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        drain_sink_to_completion(&mut host, 8);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        let final_source = observation.lock().unwrap().clone();
        assert_eq!(final_source.begin_drain_calls, 1);
        assert_eq!(final_source.drain_calls, 1);
    }

    #[test]
    fn aud138_reprepare_preflight_refuses_invalid_geometry_and_lifecycles() {
        let mut unbuilt = DawHost::new(2, 48_000);
        unbuilt.enable_terminal_sink_mode().unwrap();
        assert!(matches!(
            unbuilt.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::GraphNotBuilt)
        ));

        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let (mut invalid_host, invalid_state, invalid_source) = make_sink_host(4, 8, vec![0]);
        assert_eq!(
            invalid_host
                .process_to_sink(&input)
                .unwrap()
                .pending_sink_frames,
            4
        );
        let pending_before = sink_pending_samples(&invalid_host);
        let queue_before = sink_plugin(&invalid_host)
            .terminal_sink()
            .unwrap()
            .queue_state();
        let latency_before = invalid_host.total_latency_samples();
        let telemetry_before = sink_telemetry(&invalid_host);
        let source_before = invalid_source.lock().unwrap().clone();
        let flushes_before = invalid_state.lock().unwrap().flush_count;

        for target in [
            (44_100, 2, 16),
            (48_000, 6, 16),
            (48_000, 2, 0),
            (48_000, 2, 8),
        ] {
            {
                let mut snapshot = invalid_state.lock().unwrap();
                snapshot.transport_format = Some(target);
                snapshot.config_changed = true;
            }
            assert!(matches!(
                invalid_host.reprepare_terminal_sink_transport(),
                Err(sotf_host::host::TerminalSinkRecoveryError::Sink(
                    SinkTransportRecoveryError::NeedsReprepare { .. }
                ))
            ));
            assert_eq!(sink_pending_samples(&invalid_host), pending_before);
            assert_eq!(
                sink_plugin(&invalid_host)
                    .terminal_sink()
                    .unwrap()
                    .queue_state(),
                queue_before
            );
            assert_eq!(invalid_host.total_latency_samples(), latency_before);
            assert_eq!(invalid_source.lock().unwrap().clone(), source_before);
            assert_local_telemetry_unchanged(telemetry_before, sink_telemetry(&invalid_host));
            assert_eq!(invalid_state.lock().unwrap().flush_count, flushes_before);
        }

        let (mut pending_host, pending_state, _) = make_sink_host(4, 8, vec![0]);
        pending_host.queue_remove_plugin(0).unwrap();
        let pending_flushes_before = pending_state.lock().unwrap().flush_count;
        assert!(matches!(
            pending_host.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::PendingGraphMutation)
        ));
        assert_eq!(
            pending_state.lock().unwrap().flush_count,
            pending_flushes_before
        );

        let (mut complete_host, complete_state, _) = make_sink_host(4, 8, vec![]);
        complete_host.process_to_sink(&input).unwrap();
        drain_sink_to_completion(&mut complete_host, 8);
        let complete_queue = sink_plugin(&complete_host)
            .terminal_sink()
            .unwrap()
            .queue_state();
        let complete_latency = complete_host.total_latency_samples();
        let complete_flushes = complete_state.lock().unwrap().flush_count;
        {
            let mut snapshot = complete_state.lock().unwrap();
            snapshot.transport_format = Some((48_000, 2, 16));
            snapshot.config_changed = true;
        }
        assert!(matches!(
            complete_host.reprepare_terminal_sink_transport(),
            Err(sotf_host::host::TerminalSinkRecoveryError::InvalidLifecycle)
        ));
        assert_eq!(
            sink_plugin(&complete_host)
                .terminal_sink()
                .unwrap()
                .queue_state(),
            complete_queue
        );
        assert_eq!(complete_host.total_latency_samples(), complete_latency);
        assert_eq!(complete_state.lock().unwrap().flush_count, complete_flushes);
    }

    #[test]
    fn aud138_large_reprepare_keeps_following_process_and_drain_allocation_free() {
        // HAL output accepts at most 16 channels, and terminal-sink routes
        // require each upstream node to preserve the host entrance width. This
        // exercises the maximum admitted channel width, not a wider graph.
        let channels = 16;
        let target_ring_frames = 16_384;
        let input_frames = 8_193;
        let tail_frames = 8_192;
        let input = (0..input_frames * channels)
            .map(|index| ((index * 17 % 127) as f32 - 63.0) / 64.0)
            .collect::<Vec<_>>();
        let expected = finite_delay_stream(&input, channels, tail_frames);
        assert!(expected.iter().any(|sample| *sample != 0.0));
        let (mut host, state, _) = make_sink_host_for_channels(channels, tail_frames, 8, vec![]);
        {
            let mut snapshot = state.lock().unwrap();
            snapshot.transport_format = Some((48_000, channels as u32, target_ring_frames));
            snapshot.config_changed = true;
            snapshot.capture_audio = false;
            snapshot.capture_accepted_samples = true;
            snapshot
                .accepted_samples
                .try_reserve_exact(expected.len())
                .unwrap();
        }
        assert_eq!(
            host.reprepare_terminal_sink_transport().unwrap(),
            SinkTransportRecoveryStatus::Ready
        );
        assert_eq!(
            host.total_latency_samples(),
            target_ring_frames as usize * 2
        );
        assert_eq!(
            sink_telemetry(&host).target_fill_frames,
            target_ring_frames as usize
        );

        assert_no_heap_activity("post-reprepare sink process and finite drain", || {
            let processed = host.process_to_sink(&input).unwrap();
            assert_eq!(processed.input_frames_consumed, input_frames);
            assert_eq!(processed.pending_sink_frames, 0);
            drain_sink_to_completion(&mut host, 8);
        });
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
    }

    #[test]
    fn blocked_sink_waits_without_advancing_tail_and_later_hands_off_once() {
        let input = [
            0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75, 1.0, -0.875, 0.5, -0.375, 0.875,
            -0.125, 0.375, -0.625,
        ];
        let expected = finite_delay_stream(&input, 2, 4);
        let (mut host, state, observation) = make_sink_host(4, 8, vec![2, 0, 2, 2]);
        assert_eq!(
            host.process_to_sink(&input).unwrap().input_frames_consumed,
            8
        );
        let before = observation.lock().unwrap().clone();
        assert_eq!(before.begin_drain_calls, 0);
        assert_eq!(before.drain_calls, 0);

        let writes_before = state.lock().unwrap().writes.len();
        let blocked = host.drain_to_sink().unwrap();
        let writes_after = state.lock().unwrap().writes.len();
        assert!(!blocked.complete);
        assert_eq!(blocked.source_tail_frames_handed_to_sink, 0);
        assert!(writes_after - writes_before <= 2);
        assert_eq!(observation.lock().unwrap().drain_calls, 0);
        assert_eq!(
            host.get_plugin(1)
                .unwrap()
                .terminal_sink()
                .unwrap()
                .queue_state()
                .pending_frames,
            6
        );

        while host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state()
            .pending_frames
            > 0
        {
            let writes_before = state.lock().unwrap().writes.len();
            let result = host.drain_to_sink().unwrap();
            let writes_after = state.lock().unwrap().writes.len();
            assert!(!result.complete);
            assert_eq!(result.source_tail_frames_handed_to_sink, 0);
            assert!(writes_after - writes_before <= 2);
            assert_eq!(observation.lock().unwrap().drain_calls, 0);
        }

        let source = host.drain_to_sink().unwrap();
        assert_eq!(source.source_tail_frames_handed_to_sink, 4);
        assert!(!source.complete);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
        while host
            .get_plugin(1)
            .unwrap()
            .terminal_sink()
            .unwrap()
            .queue_state()
            .pending_frames
            > 0
        {
            let before = state.lock().unwrap().writes.len();
            assert!(!host.drain_to_sink().unwrap().complete);
            assert!(state.lock().unwrap().writes.len() - before <= 2);
        }
        assert!(host.drain_to_sink().unwrap().complete);
        assert_eq!(state.lock().unwrap().accepted_samples, expected);
        assert_eq!(observation.lock().unwrap().drain_calls, 1);
    }

    #[derive(Default)]
    struct AppendFailureState {
        fail_append: bool,
        fail_service: bool,
        append_calls: usize,
        accepted_samples: Vec<f32>,
    }

    struct AppendFailureSink {
        channels: usize,
        state: Arc<std::sync::Mutex<AppendFailureState>>,
    }

    impl Plugin for AppendFailureSink {
        fn terminal_sink(&self) -> Option<&dyn TerminalSink> {
            Some(self)
        }

        fn terminal_sink_mut(&mut self) -> Option<&mut dyn TerminalSink> {
            Some(self)
        }

        fn guarantees_identity_frame_geometry(&self) -> bool {
            true
        }

        fn info(&self) -> PluginInfo {
            PluginInfo::new("Append failure sink", "0.1", "test")
        }

        fn input_channels(&self) -> usize {
            self.channels
        }

        fn output_channels(&self) -> usize {
            0
        }

        fn parameters(&self) -> Vec<Parameter> {
            Vec::new()
        }

        fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> PluginResult<()> {
            Err("append failure sink has no parameters".into())
        }

        fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
            None
        }

        fn process(
            &mut self,
            input: &[f32],
            output: &mut [f32],
            context: &ProcessContext,
        ) -> PluginResult<usize> {
            if output.is_empty() && input.len() == context.num_frames * self.channels {
                self.state
                    .lock()
                    .unwrap()
                    .accepted_samples
                    .extend_from_slice(input);
                Ok(context.num_frames)
            } else {
                Err("malformed append failure sink block".into())
            }
        }
    }

    impl TerminalSink for AppendFailureSink {
        fn queue_state(&self) -> SinkQueueState {
            let capacity_frames = 16;
            SinkQueueState {
                pending_frames: 0,
                free_prepared_frames: capacity_frames,
                capacity_frames,
            }
        }

        fn preflight_append(
            &self,
            maximum_frames: usize,
            context: &ProcessContext,
        ) -> Result<(), SinkTailPreflightError> {
            if context.sample_rate != 48_000.0 || context.num_frames != maximum_frames {
                return Err(SinkTailPreflightError::InvalidGeometry);
            }
            if maximum_frames > 16 {
                return Err(SinkTailPreflightError::CapacityExceeded {
                    required_frames: maximum_frames,
                    free_frames: 16,
                });
            }
            Ok(())
        }

        fn append_preflighted(
            &mut self,
            input: &[f32],
            context: &ProcessContext,
        ) -> Result<(), SinkAppendFailure> {
            if input.len() != context.num_frames * self.channels {
                return Err(SinkAppendFailure::ContractViolation);
            }
            let mut state = self.state.lock().unwrap();
            state.append_calls += 1;
            if state.fail_append {
                return Err(SinkAppendFailure::ContractViolation);
            }
            state.accepted_samples.extend_from_slice(input);
            Ok(())
        }

        fn service_pending(
            &mut self,
            context: &ProcessContext,
        ) -> Result<SinkQueueState, SinkServiceFailure> {
            if context.sample_rate != 48_000.0 || context.num_frames != 0 {
                return Err(SinkServiceFailure::ContractViolation);
            }
            if self.state.lock().unwrap().fail_service {
                return Err(SinkServiceFailure::ContractViolation);
            }
            Ok(self.queue_state())
        }
    }

    #[test]
    fn public_sink_post_admission_failures_are_not_replayable() {
        let input = [0.25, -0.5, 0.75, -1.0];
        let retry_input = [9.0, -9.0];

        let (mut producer_host, _, producer_observation) = make_sink_control_host(4, vec![]);
        producer_observation.lock().unwrap().fail_after_process = true;
        let producer_error = producer_host.process_to_sink(&input).unwrap_err();
        assert!(matches!(
            producer_error,
            sotf_host::host::SinkProcessError::ResetRequired {
                cause: sotf_host::host::SinkFailure::UpstreamProcess(_),
                input: sotf_host::host::SinkInputDisposition::Indeterminate,
            }
        ));
        assert!(
            producer_host
                .settle_terminal_sink_graph_mutations()
                .is_err()
        );
        let producer_state = producer_observation.lock().unwrap().clone();
        assert_eq!(producer_state.process_calls, 1);
        assert_eq!(sink_telemetry(&producer_host).requested_frames, 0);
        assert!(matches!(
            producer_host.process_to_sink(&retry_input),
            Err(sotf_host::host::SinkProcessError::Lifecycle(_))
        ));
        assert_eq!(
            producer_observation.lock().unwrap().process_calls,
            producer_state.process_calls,
            "a failed producer block must never be replayed before explicit reset"
        );

        let (mut panic_host, panic_state, panic_observation) = make_sink_control_host(4, vec![]);
        panic_observation.lock().unwrap().panic_after_process = true;
        let panic_error = panic_host.process_to_sink(&input).unwrap_err();
        assert!(matches!(
            panic_error,
            sotf_host::host::SinkProcessError::ResetRequired {
                cause: sotf_host::host::SinkFailure::UpstreamProcess(_),
                input: sotf_host::host::SinkInputDisposition::Indeterminate,
            }
        ));
        assert_eq!(panic_observation.lock().unwrap().process_calls, 1);
        assert_eq!(sink_telemetry(&panic_host).requested_frames, 0);
        assert_eq!(panic_state.lock().unwrap().writer_attempts, 0);
        assert!(panic_state.lock().unwrap().accepted_samples.is_empty());
        assert!(matches!(
            panic_host.process_to_sink(&retry_input),
            Err(sotf_host::host::SinkProcessError::Lifecycle(_))
        ));
        assert_eq!(panic_observation.lock().unwrap().process_calls, 1);

        panic_host.reset();
        panic_observation.lock().unwrap().panic_after_process = false;
        let recovered = panic_host.process_to_sink(&input).unwrap();
        assert_eq!(recovered.input_frames_consumed, 2);
        assert_eq!(recovered.pending_sink_frames, 0);
        assert_eq!(panic_observation.lock().unwrap().process_calls, 2);
        assert_eq!(panic_state.lock().unwrap().accepted_samples, input);
        let panic_recovery_drain = panic_host.drain_to_sink().unwrap();
        assert!(panic_recovery_drain.complete);
        assert_eq!(panic_state.lock().unwrap().accepted_samples, input);

        let append_state = Arc::new(std::sync::Mutex::new(AppendFailureState {
            fail_append: true,
            ..AppendFailureState::default()
        }));
        let append_sink = AppendFailureSink {
            channels: 2,
            state: Arc::clone(&append_state),
        };
        let append_observation = Arc::new(std::sync::Mutex::new(SinkControlObservation::default()));
        let mut append_host = DawHost::new(2, 48_000);
        append_host.enable_terminal_sink_mode().unwrap();
        append_host
            .set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        append_host
            .add_plugin(Box::new(SinkControlObserver {
                gain: 1.0,
                automation_value: 1.0,
                observation: Arc::clone(&append_observation),
                flip_transport_after_process: None,
                config_change_after_process: None,
            }))
            .unwrap();
        append_host.add_plugin(Box::new(append_sink)).unwrap();
        append_host.build().unwrap();

        let append_error = append_host.process_to_sink(&input).unwrap_err();
        assert!(matches!(
            append_error,
            sotf_host::host::SinkProcessError::ResetRequired {
                cause: sotf_host::host::SinkFailure::AppendContractViolation,
                input: sotf_host::host::SinkInputDisposition::Indeterminate,
            }
        ));
        assert!(append_host.settle_terminal_sink_graph_mutations().is_err());
        assert_eq!(append_observation.lock().unwrap().process_calls, 1);
        assert_eq!(append_state.lock().unwrap().append_calls, 1);
        assert!(append_state.lock().unwrap().accepted_samples.is_empty());
        assert!(matches!(
            append_host.process_to_sink(&retry_input),
            Err(sotf_host::host::SinkProcessError::Lifecycle(_))
        ));
        assert_eq!(append_observation.lock().unwrap().process_calls, 1);
        assert_eq!(append_state.lock().unwrap().append_calls, 1);

        let service_state = Arc::new(std::sync::Mutex::new(AppendFailureState {
            fail_service: true,
            ..AppendFailureState::default()
        }));
        let service_sink = AppendFailureSink {
            channels: 2,
            state: Arc::clone(&service_state),
        };
        let service_observation =
            Arc::new(std::sync::Mutex::new(SinkControlObservation::default()));
        let mut service_host = DawHost::new(2, 48_000);
        service_host.enable_terminal_sink_mode().unwrap();
        service_host
            .set_plugin_preferred_oversampling_enabled(false)
            .unwrap();
        service_host
            .add_plugin(Box::new(SinkControlObserver {
                gain: 1.0,
                automation_value: 1.0,
                observation: Arc::clone(&service_observation),
                flip_transport_after_process: None,
                config_change_after_process: None,
            }))
            .unwrap();
        service_host.add_plugin(Box::new(service_sink)).unwrap();
        service_host.build().unwrap();

        let service_error = service_host.process_to_sink(&input).unwrap_err();
        assert!(matches!(
            service_error,
            sotf_host::host::SinkProcessError::ResetRequired {
                cause: sotf_host::host::SinkFailure::ServiceContractViolation,
                input: sotf_host::host::SinkInputDisposition::Admitted { frames: 2 },
            }
        ));
        assert!(service_host.settle_terminal_sink_graph_mutations().is_err());
        assert_eq!(service_observation.lock().unwrap().process_calls, 1);
        assert_eq!(service_state.lock().unwrap().append_calls, 1);
        assert_eq!(service_state.lock().unwrap().accepted_samples, input);
        assert!(matches!(
            service_host.process_to_sink(&retry_input),
            Err(sotf_host::host::SinkProcessError::Lifecycle(_))
        ));
        assert_eq!(service_observation.lock().unwrap().process_calls, 1);
        assert_eq!(service_state.lock().unwrap().append_calls, 1);
        assert_eq!(service_state.lock().unwrap().accepted_samples, input);
    }

    #[test]
    fn producer_and_append_failures_require_reset_without_replaying_tail() {
        let input = [0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75];
        let recovery_input = [-0.75, 0.625, -0.5, 0.375, -0.25, 0.125, 1.0, -0.875];

        let expected_first = finite_delay_stream(&input, 2, 4);
        let (mut failing_source, producer_observation) = FiniteDelayMarker::new_observed(2, 4);
        failing_source.fail_after_drain = true;
        let (producer_sink, producer_state) = make_plugin_with_writer(2, (48_000, 2, 16), vec![]);
        let mut producer_error_host =
            make_terminal_sink_host(Box::new(failing_source), Box::new(producer_sink));

        producer_error_host.process_to_sink(&input).unwrap();
        let accepted_before_error = producer_state.lock().unwrap().accepted_samples.clone();
        assert_eq!(accepted_before_error, expected_first[..input.len()]);
        let producer_error = producer_error_host.drain_to_sink().unwrap_err();
        assert!(matches!(
            producer_error,
            sotf_host::host::SinkDrainError::ResetRequired(message)
                if message.contains("injected finite-tail failure")
        ));
        let after_failed_producer = producer_observation.lock().unwrap().clone();
        assert_eq!(after_failed_producer.drain_calls, 1);
        assert!(after_failed_producer.drained);

        assert!(producer_error_host.drain_to_sink().is_err());
        assert!(
            producer_error_host
                .process_to_sink(&recovery_input)
                .is_err()
        );
        assert_eq!(producer_observation.lock().unwrap().drain_calls, 1);
        assert_eq!(
            producer_state.lock().unwrap().accepted_samples,
            accepted_before_error
        );

        producer_error_host.reset();
        let expected_recovery = finite_delay_stream(&recovery_input, 2, 4);
        producer_error_host
            .process_to_sink(&recovery_input)
            .unwrap();
        let mut recovery_calls = 0;
        loop {
            let result = producer_error_host.drain_to_sink().unwrap();
            recovery_calls += 1;
            if result.complete {
                break;
            }
            assert!(recovery_calls < 5);
        }
        assert_eq!(
            producer_state.lock().unwrap().accepted_samples[accepted_before_error.len()..],
            expected_recovery
        );
        assert_eq!(producer_observation.lock().unwrap().drain_calls, 2);

        let expected_append_first = finite_delay_stream(&input, 2, 4);
        let (append_source, append_observation) = FiniteDelayMarker::new_observed(2, 4);
        let append_state = Arc::new(std::sync::Mutex::new(AppendFailureState::default()));
        let append_sink = AppendFailureSink {
            channels: 2,
            state: Arc::clone(&append_state),
        };
        let mut append_error_host =
            make_terminal_sink_host(Box::new(append_source), Box::new(append_sink));
        append_error_host.process_to_sink(&input).unwrap();
        let accepted_before_append_error = append_state.lock().unwrap().accepted_samples.clone();
        assert_eq!(
            accepted_before_append_error,
            expected_append_first[..input.len()]
        );
        append_state.lock().unwrap().fail_append = true;

        let append_error = append_error_host.drain_to_sink().unwrap_err();
        assert!(matches!(
            append_error,
            sotf_host::host::SinkDrainError::ResetRequired(message)
                if message.contains("append")
        ));
        let after_failed_append = append_observation.lock().unwrap().clone();
        assert_eq!(after_failed_append.drain_calls, 1);
        assert!(after_failed_append.drained);
        let append_calls_after_error = append_state.lock().unwrap().append_calls;

        assert!(append_error_host.drain_to_sink().is_err());
        assert!(append_error_host.process_to_sink(&recovery_input).is_err());
        assert_eq!(append_observation.lock().unwrap().drain_calls, 1);
        assert_eq!(
            append_state.lock().unwrap().append_calls,
            append_calls_after_error
        );
        assert_eq!(
            append_state.lock().unwrap().accepted_samples,
            accepted_before_append_error
        );

        append_error_host.reset();
        append_state.lock().unwrap().fail_append = false;
        let expected_append_recovery = finite_delay_stream(&recovery_input, 2, 4);
        append_error_host.process_to_sink(&recovery_input).unwrap();
        let mut append_recovery_calls = 0;
        loop {
            let result = append_error_host.drain_to_sink().unwrap();
            append_recovery_calls += 1;
            if result.complete {
                break;
            }
            assert!(append_recovery_calls < 5);
        }
        assert_eq!(
            append_state.lock().unwrap().accepted_samples[accepted_before_append_error.len()..],
            expected_append_recovery
        );
        assert_eq!(append_observation.lock().unwrap().drain_calls, 2);
    }

    #[test]
    fn hal_output_reset_cancels_pending_samples_and_counts_drops_without_transport_io() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2]);
        plugin.initialize(48_000.0).unwrap();
        plugin
            .process(
                &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                &mut [],
                &ProcessContext::new(48_000, 4),
            )
            .unwrap();
        let before = plugin.telemetry();
        assert_eq!(before.queued_frames, 2);
        let (
            writes_before,
            fill_before,
            flush_before,
            reconnect_before,
            reload_before,
            ready_before,
        ) = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.writes.len(),
                snapshot.fill_frames,
                snapshot.flush_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.ready.len(),
            )
        };

        assert_no_heap_activity("HAL Output reset with pending samples", || {
            Plugin::reset(&mut plugin)
        });

        let after = plugin.telemetry();
        let transport_unchanged = {
            let snapshot = state.lock().unwrap();
            snapshot.writes.len() == writes_before
                && snapshot.fill_frames == fill_before
                && snapshot.flush_count == flush_before
                && snapshot.reconnect_count == reconnect_before
                && snapshot.reload_count == reload_before
                && snapshot.ready.len() == ready_before
        };
        assert!(
            after.queued_frames == 0
                && after.dropped_frames == before.dropped_frames + 2
                && transport_unchanged,
            "reset should cancel only queued frames, count those drops, and avoid transport I/O; before={before:?}, after={after:?}, transport_unchanged={transport_unchanged}"
        );
    }

    #[test]
    fn public_daw_host_reset_does_not_replay_pre_reset_pending_samples() {
        let mut old_input = [0.0; 16];
        old_input[..8].copy_from_slice(&[0.25, -0.5, 0.75, -1.0, 0.125, -0.25, 0.625, -0.75]);
        let new_input = [0.0; 16];
        let new_stream = finite_delay_stream(&new_input, 2, 4);
        let (mut host, state, _) = make_sink_host(4, 8, vec![6]);
        let process = host.process_to_sink(&old_input);
        let accepted_before = state.lock().unwrap().accepted_samples.clone();
        let old_stream = finite_delay_stream(&old_input, 2, 4);
        assert_eq!(accepted_before, old_stream[..12]);
        let before_reset = sink_telemetry(&host);
        assert_eq!(before_reset.requested_frames, 8);
        assert_eq!(before_reset.written_frames, 6);
        assert_eq!(before_reset.dropped_frames, 0);
        assert_eq!(before_reset.queued_frames, 2);
        let (
            fill_before,
            flush_before,
            reconnect_before,
            reload_before,
            ready_before,
            writes_before,
        ) = {
            let snapshot = state.lock().unwrap();
            (
                snapshot.fill_frames,
                snapshot.flush_count,
                snapshot.reconnect_count,
                snapshot.reload_count,
                snapshot.ready.len(),
                snapshot.writes.len(),
            )
        };

        assert_no_heap_activity("DawHost reset with pending HAL samples", || host.reset());
        let after_reset = sink_telemetry(&host);
        assert_eq!(after_reset.requested_frames, 8);
        assert_eq!(after_reset.written_frames, 6);
        assert_eq!(after_reset.dropped_frames, 2);
        assert_eq!(after_reset.queued_frames, 0);
        let transport_unchanged = {
            let snapshot = state.lock().unwrap();
            snapshot.fill_frames == fill_before
                && snapshot.flush_count == flush_before
                && snapshot.reconnect_count == reconnect_before
                && snapshot.reload_count == reload_before
                && snapshot.ready.len() == ready_before
                && snapshot.writes.len() == writes_before
        };

        state.lock().unwrap().writes.clear();
        let next_process = host.process_to_sink(&new_input);
        let accepted_after = state.lock().unwrap().accepted_samples.clone();
        let expected_after = [accepted_before, new_stream[..new_input.len()].to_vec()].concat();
        let after_resume = sink_telemetry(&host);
        assert!(
            transport_unchanged
                && matches!(process, Ok(result) if result.input_frames_consumed == 8)
                && matches!(next_process, Ok(result) if result.input_frames_consumed == 8)
                && accepted_after == expected_after
                && after_resume.requested_frames == 16
                && after_resume.written_frames == 14
                && after_resume.dropped_frames == 2
                && after_resume.queued_frames == 0,
            "host reset should cancel pending audio while retaining already handed-off ring data and avoiding transport I/O; transport_unchanged={transport_unchanged}, process_failed={}, next_process_failed={}, accepted_after_samples={}, expected_after_samples={}, accepted_after={accepted_after:?}, expected_after={expected_after:?}",
            process.is_err(),
            next_process.is_err(),
            accepted_after.len(),
            expected_after.len()
        );
    }

    #[test]
    fn full_multichannel_writes_are_counted_in_frames() {
        for channels in [1, 2, 6, 16] {
            let frames = 32;
            let (mut plugin, _) =
                make_plugin_with_writer(channels, (48_000, channels as u32, 256), vec![frames]);
            plugin.initialize(48_000.0).unwrap();
            let input = vec![0.25; frames * channels];
            let mut output = Vec::new();
            assert_eq!(
                plugin
                    .process(&input, &mut output, &ProcessContext::new(48_000, frames),)
                    .unwrap(),
                frames
            );
            assert_eq!(plugin.write_success_ratio, 100.0);
            assert!(!plugin.is_backpressured);
            assert_eq!(plugin.underrun_count(), 0);
        }
    }

    #[test]
    fn partial_write_retries_tail_before_new_audio() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 2, 2]);
        plugin.initialize(48_000.0).unwrap();
        let mut output = Vec::new();
        let first: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        plugin
            .process(&first, &mut output, &ProcessContext::new(48_000, 4))
            .unwrap();
        assert_eq!(plugin.write_success_ratio, 50.0);
        assert!(plugin.is_backpressured);

        let second: Vec<f32> = (8..12).map(|sample| sample as f32).collect();
        plugin
            .process(&second, &mut output, &ProcessContext::new(48_000, 2))
            .unwrap();

        let state = state.lock().unwrap();
        assert_eq!(state.writes[0], first);
        assert_eq!(state.writes[1], vec![4.0, 5.0, 6.0, 7.0]);
        assert_eq!(state.writes[2], second);
    }

    #[test]
    fn eof_drain_flushes_the_final_partial_write_to_the_writer() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 2]);
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        let tail = vec![4.0, 5.0, 6.0, 7.0];

        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        assert_eq!(plugin.telemetry().queued_frames, 2);
        assert_eq!(plugin.drain_call_bound(), None);

        let drained = Plugin::drain(&mut plugin, &mut [], &ProcessContext::new(48_000, 0)).unwrap();
        assert_eq!(drained.frames, 0);
        assert!(drained.complete);

        let (accepted_samples, writes) = {
            let snapshot = state.lock().unwrap();
            (snapshot.accepted_samples.clone(), snapshot.writes.clone())
        };
        assert_eq!(
            accepted_samples, input,
            "HAL output cannot report EOF complete before the final frames are accepted"
        );
        assert_eq!(writes, vec![input, tail]);
        assert_eq!(plugin.telemetry().queued_frames, 0);
        assert_eq!(plugin.drain_call_bound(), std::num::NonZeroU64::new(1));
    }

    #[test]
    fn eof_drain_reports_backpressure_and_retries_without_new_input() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 0, 2]);
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        let tail = vec![4.0, 5.0, 6.0, 7.0];

        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        assert_eq!(plugin.telemetry().queued_frames, 2);
        assert_eq!(plugin.drain_call_bound(), None);

        let context = ProcessContext::new(48_000, 0);
        let blocked = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert_eq!(blocked.frames, 0);
        assert!(
            !blocked.complete,
            "HAL output drain must report incomplete while pending frames are backpressured"
        );
        assert_eq!(plugin.telemetry().queued_frames, 2);
        assert_eq!(plugin.drain_call_bound(), None);

        let drained = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert_eq!(drained.frames, 0);
        assert!(drained.complete);
        assert_eq!(plugin.telemetry().queued_frames, 0);
        assert_eq!(plugin.drain_call_bound(), std::num::NonZeroU64::new(1));

        let writes = state.lock().unwrap().writes.clone();
        assert_eq!(writes, vec![input, tail.clone(), tail]);
    }

    #[test]
    fn drain_wraps_frames_across_vecdeque_storage_with_two_writes() {
        let wrapped_pending = VecDeque::<f32>::with_capacity(64);
        let storage_capacity = wrapped_pending.capacity();
        let channels = (2..=16)
            .find(|channels| !storage_capacity.is_multiple_of(*channels))
            .expect("test VecDeque capacity must not be divisible by every channel count");
        let frames = storage_capacity / channels;
        assert!(frames >= 2);
        let first_write_frames = frames - 1;

        let (mut plugin, state) = make_plugin_with_writer(
            channels,
            (48_000, channels as u32, frames as u32),
            vec![first_write_frames, 1],
        );
        plugin.initialize(48_000.0).unwrap();
        plugin.pending = wrapped_pending;
        plugin.pending_capacity_samples = frames * channels;

        let original: Vec<f32> = (0..frames * channels).map(|sample| sample as f32).collect();
        plugin.pending.extend(original.iter().copied());
        plugin.pending.drain(..channels);
        let replacement: Vec<f32> = (original.len()..original.len() + channels)
            .map(|sample| sample as f32)
            .collect();
        plugin.pending.extend(replacement.iter().copied());
        let mut expected = original[channels..].to_vec();
        expected.extend_from_slice(&replacement);

        let (front, back) = plugin.pending.as_slices();
        assert!(!back.is_empty(), "test queue must wrap physically");
        assert_ne!(front.len() % channels, 0, "wrap must split a frame");
        let second_write_frames = frames - first_write_frames;
        assert_eq!(second_write_frames, 1);
        let drained = Plugin::drain(&mut plugin, &mut [], &ProcessContext::new(48_000, 0)).unwrap();
        assert_eq!(drained.frames, 0);
        assert!(drained.complete);
        assert!(plugin.pending.is_empty());

        let (accepted_samples, writes) = {
            let snapshot = state.lock().unwrap();
            (snapshot.accepted_samples.clone(), snapshot.writes.clone())
        };
        assert_eq!(accepted_samples, expected);
        assert_eq!(writes.len(), 2);
        assert_eq!(
            writes[0],
            expected[..first_write_frames * channels].to_vec()
        );
        assert_eq!(
            writes[1],
            expected[first_write_frames * channels..].to_vec()
        );
    }

    #[test]
    fn drain_validation_errors_preserve_pending_and_destination() {
        let (mut plugin, _) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 2]);
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        let pending = plugin.pending.iter().copied().collect::<Vec<_>>();
        let telemetry = plugin.telemetry();

        assert!(Plugin::drain(&mut plugin, &mut [], &ProcessContext::new(44_100, 0)).is_err());
        assert_eq!(plugin.pending.iter().copied().collect::<Vec<_>>(), pending);
        assert_eq!(plugin.telemetry().dropped_frames, telemetry.dropped_frames);

        let mut destination = [7.0];
        assert!(
            Plugin::drain(
                &mut plugin,
                &mut destination,
                &ProcessContext::new(48_000, 0)
            )
            .is_err()
        );
        assert_eq!(destination, [7.0]);
        assert_eq!(plugin.pending.iter().copied().collect::<Vec<_>>(), pending);
        assert_eq!(plugin.telemetry().written_frames, telemetry.written_frames);
    }

    #[test]
    fn drain_gates_and_control_recovery_preserve_pending_samples() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 2]);
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        let expected_pending = vec![4.0, 5.0, 6.0, 7.0];
        let context = ProcessContext::new(48_000, 0);

        for gate in [
            HalOutputTransportState::Servicing,
            HalOutputTransportState::PrimingFailed,
            HalOutputTransportState::FormatError,
        ] {
            plugin.state = gate;
            let result = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
            assert!(!result.complete);
            assert_eq!(
                plugin.pending.iter().copied().collect::<Vec<_>>(),
                expected_pending
            );
        }
        plugin.state = HalOutputTransportState::Backpressured;

        state.lock().unwrap().config_changed = true;
        let blocked = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(!blocked.complete);
        assert_eq!(plugin.state, HalOutputTransportState::ConfigurationChanged);
        assert_eq!(
            plugin.pending.iter().copied().collect::<Vec<_>>(),
            expected_pending
        );
        plugin.service_transport().unwrap();
        assert_eq!(
            plugin.pending.iter().copied().collect::<Vec<_>>(),
            expected_pending
        );

        state.lock().unwrap().connected = false;
        let disconnected = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(!disconnected.complete);
        assert_eq!(plugin.state, HalOutputTransportState::Disconnected);
        state.lock().unwrap().connected = true;
        let still_gated = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(!still_gated.complete, "recovery is control-thread owned");
        plugin.service_transport().unwrap();
        assert_eq!(
            plugin.pending.iter().copied().collect::<Vec<_>>(),
            expected_pending
        );

        state.lock().unwrap().key_ready = false;
        let key_blocked = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(!key_blocked.complete);
        assert_eq!(plugin.state, HalOutputTransportState::KeyMismatch);
        state.lock().unwrap().key_ready = true;
        let key_still_gated = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(!key_still_gated.complete, "key recovery requires service");
        plugin.service_transport().unwrap();
        assert_eq!(
            plugin.pending.iter().copied().collect::<Vec<_>>(),
            expected_pending
        );

        let complete = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        assert!(complete.complete);
        let (accepted_samples, writes) = {
            let snapshot = state.lock().unwrap();
            (snapshot.accepted_samples.clone(), snapshot.writes.clone())
        };
        assert_eq!(accepted_samples, input);
        assert_eq!(writes.len(), 2);
    }

    #[test]
    fn failed_recovery_format_preparation_retains_pending_for_later_service() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 2]);
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        let pending = plugin.pending.iter().copied().collect::<Vec<_>>();
        let dropped_frames = plugin.telemetry().dropped_frames;

        state.lock().unwrap().transport_format = Some((44_100, 2, 8));
        assert!(plugin.service_transport().is_err());
        assert_eq!(plugin.state, HalOutputTransportState::FormatError);
        assert_eq!(plugin.pending.iter().copied().collect::<Vec<_>>(), pending);
        assert_eq!(plugin.telemetry().dropped_frames, dropped_frames);

        state.lock().unwrap().transport_format = None;
        plugin.service_transport().unwrap();
        assert_eq!(plugin.pending.iter().copied().collect::<Vec<_>>(), pending);
        let drained = Plugin::drain(&mut plugin, &mut [], &ProcessContext::new(48_000, 0)).unwrap();
        assert!(drained.complete);
        assert_eq!(state.lock().unwrap().accepted_samples, input);
    }

    #[test]
    fn drain_errors_leave_unaccepted_frames_queued() {
        let (mut unavailable, _) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2]);
        unavailable.initialize(48_000.0).unwrap();
        unavailable
            .process(
                &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                &mut [],
                &ProcessContext::new(48_000, 4),
            )
            .unwrap();
        let pending = unavailable.pending.iter().copied().collect::<Vec<_>>();
        let writer = unavailable.writer.take();
        assert!(Plugin::drain(&mut unavailable, &mut [], &ProcessContext::new(48_000, 0)).is_err());
        assert_eq!(
            unavailable.pending.iter().copied().collect::<Vec<_>>(),
            pending
        );
        unavailable.writer = writer;

        let (mut overreport, _) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 3]);
        overreport.initialize(48_000.0).unwrap();
        overreport
            .process(
                &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                &mut [],
                &ProcessContext::new(48_000, 4),
            )
            .unwrap();
        let pending = overreport.pending.iter().copied().collect::<Vec<_>>();
        assert!(Plugin::drain(&mut overreport, &mut [], &ProcessContext::new(48_000, 0)).is_err());
        assert_eq!(
            overreport.pending.iter().copied().collect::<Vec<_>>(),
            pending
        );
        assert_eq!(overreport.state, HalOutputTransportState::FormatError);
    }

    #[test]
    fn drain_orders_new_input_around_an_incomplete_and_completed_eof() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2, 0, 2, 0, 2]);
        plugin.initialize(48_000.0).unwrap();
        let first: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        let second = vec![8.0, 9.0, 10.0, 11.0];
        let third = vec![12.0, 13.0, 14.0, 15.0];
        let context = ProcessContext::new(48_000, 4);
        plugin.process(&first, &mut [], &context).unwrap();

        let drain_context = ProcessContext::new(48_000, 0);
        let blocked = Plugin::drain(&mut plugin, &mut [], &drain_context).unwrap();
        assert!(!blocked.complete);
        plugin
            .process(&second, &mut [], &ProcessContext::new(48_000, 2))
            .unwrap();
        assert_eq!(plugin.telemetry().queued_frames, 2);
        assert_eq!(plugin.telemetry().dropped_frames, 0);
        assert!(
            Plugin::drain(&mut plugin, &mut [], &drain_context)
                .unwrap()
                .complete
        );

        plugin
            .process(&third, &mut [], &ProcessContext::new(48_000, 2))
            .unwrap();
        assert!(
            Plugin::drain(&mut plugin, &mut [], &drain_context)
                .unwrap()
                .complete
        );
        let accepted_samples = state.lock().unwrap().accepted_samples.clone();
        let mut expected = first;
        expected.extend_from_slice(&second);
        expected.extend_from_slice(&third);
        assert_eq!(accepted_samples, expected);
    }

    #[test]
    fn bounded_queue_drops_newest_complete_frames_and_preserves_oldest_order() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 4), vec![0, 0, 4, 4]);
        plugin.initialize(48_000.0).unwrap();
        let mut output = Vec::new();
        let first: Vec<f32> = (0..8).map(|sample| sample as f32).collect();
        let second: Vec<f32> = (8..16).map(|sample| sample as f32).collect();
        let third: Vec<f32> = (16..24).map(|sample| sample as f32).collect();
        let context = ProcessContext::new(48_000, 4);
        plugin.process(&first, &mut output, &context).unwrap();
        plugin.process(&second, &mut output, &context).unwrap();
        let telemetry = plugin.telemetry();
        assert_eq!(telemetry.queued_frames, 4);
        assert_eq!(telemetry.dropped_frames, 4);
        plugin.process(&third, &mut output, &context).unwrap();

        let writes = state.lock().unwrap().writes.clone();
        assert_eq!(writes[0], first);
        assert_eq!(writes[1], first);
        assert_eq!(writes[2], first);
        assert_eq!(writes[3], third);
        assert_eq!(plugin.telemetry().queued_frames, 0);
        let dropped_frames = plugin.telemetry().dropped_frames;
        assert!(
            Plugin::drain(&mut plugin, &mut [], &ProcessContext::new(48_000, 0))
                .unwrap()
                .complete
        );
        assert_eq!(plugin.telemetry().dropped_frames, dropped_frames);
    }

    #[test]
    fn telemetry_preserves_counters_past_i32_range() {
        let plugin = make_test_plugin();
        let value = i32::MAX as u64 + 42;
        plugin.underrun_counter.store(value, Ordering::Relaxed);
        assert_eq!(plugin.telemetry().backpressure_events, value);
    }

    #[test]
    fn control_thread_service_reconnects_reloads_key_and_reasserts_readiness() {
        let (mut plugin, state) =
            make_plugin_with_writer_state(2, (48_000, 2, 8), vec![], false, false);
        plugin.sample_rate = 48_000;
        plugin.service_transport().unwrap();
        assert_eq!(plugin.telemetry().state, HalOutputTransportState::Ready);
        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.reconnect_count, 1);
        assert_eq!(snapshot.reload_count, 1);
        assert_eq!(snapshot.ready, vec![false, true]);
    }

    #[test]
    fn initialization_primes_negotiated_fill_and_reports_v2_boundary_latency() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
        plugin.initialize(48_000.0).unwrap();

        let telemetry = plugin.telemetry();
        assert_eq!(telemetry.version, HAL_OUTPUT_TELEMETRY_VERSION);
        assert_eq!(telemetry.transport_fill_frames, 8);
        assert_eq!(telemetry.target_fill_frames, 8);
        assert_eq!(telemetry.device_latency_frames, 8);
        assert_eq!(telemetry.safety_offset_frames, 0);
        assert_eq!(telemetry.boundary_latency_frames, 16);
        assert_eq!(plugin.latency_samples(), 16);
        assert_eq!(state.lock().unwrap().ready, vec![false, true]);
    }

    #[test]
    fn failed_priming_is_transactional_and_keeps_transport_quiesced() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
        state.lock().unwrap().prime_write = Some(4);

        let error = plugin.initialize(48_000.0).unwrap_err();

        assert!(error.contains("could not establish target fill"));
        assert_eq!(plugin.sample_rate, 0);
        assert_eq!(plugin.state, HalOutputTransportState::PrimingFailed);
        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.ready, vec![false]);
        assert_eq!(snapshot.fill_frames, 0);
        assert_eq!(snapshot.flush_count, 2);
    }

    #[test]
    fn reservice_preserves_pending_and_reestablishes_target_fill() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
        plugin.initialize(48_000.0).unwrap();
        plugin.pending.extend([1.0, 2.0, 3.0, 4.0]);

        plugin.service_transport().unwrap();

        assert_eq!(
            plugin.pending.iter().copied().collect::<Vec<_>>(),
            [1.0, 2.0, 3.0, 4.0]
        );
        assert_eq!(plugin.telemetry().dropped_frames, 0);
        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.fill_frames, 8);
        assert_eq!(snapshot.flush_count, 2);
        assert_eq!(snapshot.ready, vec![false, true, false, true]);
    }

    #[test]
    fn reinitialize_cancels_pending_only_after_successful_preflight_and_counts_it() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![2]);
        plugin.initialize(48_000.0).unwrap();
        plugin
            .process(
                &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0],
                &mut [],
                &ProcessContext::new(48_000, 4),
            )
            .unwrap();
        assert_eq!(plugin.telemetry().queued_frames, 2);

        state.lock().unwrap().transport_format = Some((44_100, 2, 8));
        assert!(plugin.initialize(48_000.0).is_err());
        assert_eq!(plugin.telemetry().queued_frames, 2);
        assert_eq!(plugin.telemetry().dropped_frames, 0);

        state.lock().unwrap().transport_format = None;
        plugin.initialize(48_000.0).unwrap();
        assert_eq!(plugin.telemetry().queued_frames, 0);
        assert_eq!(plugin.telemetry().dropped_frames, 2);
        assert_eq!(plugin.telemetry().requested_frames, 4);
    }

    #[test]
    fn failed_reservice_cannot_publish_or_write_partial_prime() {
        let (mut plugin, state) = make_plugin_with_writer(2, (48_000, 2, 8), vec![]);
        plugin.initialize(48_000.0).unwrap();
        state.lock().unwrap().prime_write = Some(4);

        assert!(plugin.service_transport().is_err());
        plugin
            .process(&[0.25; 8], &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();

        assert_eq!(plugin.state, HalOutputTransportState::PrimingFailed);
        assert_eq!(plugin.pending.len(), 8);
        let snapshot = state.lock().unwrap();
        assert_eq!(snapshot.ready, vec![false, true, false]);
        assert_eq!(snapshot.fill_frames, 0);
        assert!(snapshot.writes.is_empty());
    }

    #[test]
    fn config_change_is_quiesced_until_control_thread_service() {
        let (mut plugin, state) =
            make_plugin_with_writer_state(2, (48_000, 2, 8), vec![], true, true);
        plugin.initialize(48_000.0).unwrap();
        state.lock().unwrap().config_changed = true;
        let input = vec![0.0; 8];
        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 4))
            .unwrap();
        assert_eq!(
            plugin.telemetry().state,
            HalOutputTransportState::ConfigurationChanged
        );
        assert!(state.lock().unwrap().writes.is_empty());
        plugin.service_transport().unwrap();
        assert_eq!(plugin.telemetry().state, HalOutputTransportState::Ready);
    }

    #[test]
    fn invalid_transport_frame_count_is_rejected_without_losing_writer() {
        let (mut plugin, _) = make_plugin_with_writer(2, (48_000, 2, 8), vec![5, 4]);
        plugin.initialize(48_000.0).unwrap();
        let context = ProcessContext::new(48_000, 4);
        assert!(plugin.process(&[0.0; 8], &mut [], &context).is_err());
        assert!(plugin.process(&[0.0; 8], &mut [], &context).is_ok());
    }

    #[test]
    fn maximum_channel_full_and_backpressured_callbacks_allocate_nothing() {
        struct NoAllocWriter {
            writes: [usize; 4],
            next: usize,
            priming: std::cell::Cell<bool>,
            fill_frames: std::cell::Cell<usize>,
        }
        impl HalWriter for NoAllocWriter {
            fn is_connected(&self) -> bool {
                true
            }
            fn write(&mut self, _buffer: &[f32]) -> usize {
                if self.priming.replace(false) {
                    self.fill_frames.set(8);
                    return 8;
                }
                let value = self.writes[self.next];
                self.next += 1;
                value
            }
            fn current_format(&self) -> Result<(u32, u32, u32), String> {
                Ok((48_000, 16, 8))
            }
            fn config_changed(&self) -> bool {
                false
            }
            fn clear_config_changed(&self) {}
            fn set_engine_ready(&self, _ready: bool) {}
            fn reconnect(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn reload_cipher(&mut self) -> Result<(), String> {
                Ok(())
            }
            fn encryption_key_ready(&self) -> bool {
                true
            }
            fn available_read_frames(&self) -> usize {
                self.fill_frames.get()
            }
            fn flush_audio(&self) {
                self.fill_frames.set(0);
                self.priming.set(true);
            }
        }
        let mut plugin = make_test_plugin();
        plugin.channels = 16;
        plugin.sample_rate = 0;
        plugin.writer = Some(Box::new(NoAllocWriter {
            writes: [8, 0, 8, 8],
            next: 0,
            priming: std::cell::Cell::new(false),
            fill_frames: std::cell::Cell::new(0),
        }));
        plugin.initialize(48_000.0).unwrap();
        let input = [0.25; 8 * 16];
        let context = ProcessContext::new(48_000, 8);
        assert_no_allocs("HAL output full write", || {
            plugin.process(&input, &mut [], &context).unwrap();
        });
        assert_no_allocs("HAL output queued write", || {
            plugin.process(&input, &mut [], &context).unwrap();
        });
        assert_no_allocs("HAL output queue recovery", || {
            plugin.process(&input, &mut [], &context).unwrap();
        });
    }

    #[test]
    fn first_blocked_retry_and_terminal_drains_allocate_nothing_or_shrink_storage() {
        struct DrainNoAllocWriter {
            writes: [usize; 3],
            next: usize,
            priming: std::cell::Cell<bool>,
            fill_frames: std::cell::Cell<usize>,
        }

        impl HalWriter for DrainNoAllocWriter {
            fn is_connected(&self) -> bool {
                true
            }

            fn write(&mut self, _buffer: &[f32]) -> usize {
                if self.priming.replace(false) {
                    self.fill_frames.set(8);
                    return 8;
                }
                let written = self.writes[self.next];
                self.next += 1;
                written
            }

            fn current_format(&self) -> Result<(u32, u32, u32), String> {
                Ok((48_000, 16, 8))
            }

            fn config_changed(&self) -> bool {
                false
            }

            fn clear_config_changed(&self) {}

            fn set_engine_ready(&self, _ready: bool) {}

            fn reconnect(&mut self) -> Result<(), String> {
                Ok(())
            }

            fn reload_cipher(&mut self) -> Result<(), String> {
                Ok(())
            }

            fn encryption_key_ready(&self) -> bool {
                true
            }

            fn available_read_frames(&self) -> usize {
                self.fill_frames.get()
            }

            fn flush_audio(&self) {
                self.fill_frames.set(0);
                self.priming.set(true);
            }
        }

        let mut plugin = make_test_plugin();
        plugin.channels = 16;
        plugin.sample_rate = 0;
        plugin.writer = Some(Box::new(DrainNoAllocWriter {
            writes: [0, 0, 8],
            next: 0,
            priming: std::cell::Cell::new(false),
            fill_frames: std::cell::Cell::new(0),
        }));
        plugin.initialize(48_000.0).unwrap();
        let context = ProcessContext::new(48_000, 0);
        let mut empty = PluginDrainResult {
            frames: 0,
            complete: false,
        };
        assert_no_heap_activity("HAL output initially empty drain", || {
            empty = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        });
        assert!(empty.complete);

        let input = [0.25; 8 * 16];
        plugin
            .process(&input, &mut [], &ProcessContext::new(48_000, 8))
            .unwrap();
        let pending_capacity = plugin.pending.capacity();
        let staging_capacity = plugin.drain_staging.capacity();

        let mut blocked = PluginDrainResult::COMPLETE;
        assert_no_heap_activity("HAL output blocked drain", || {
            blocked = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        });
        assert!(!blocked.complete);

        let mut retried = PluginDrainResult::COMPLETE;
        assert_no_heap_activity("HAL output retry drain", || {
            retried = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        });
        assert!(retried.complete);

        let mut terminal = PluginDrainResult {
            frames: 0,
            complete: false,
        };
        assert_no_heap_activity("HAL output terminal drain", || {
            terminal = Plugin::drain(&mut plugin, &mut [], &context).unwrap();
        });
        assert!(terminal.complete);
        assert_eq!(plugin.pending.capacity(), pending_capacity);
        assert_eq!(plugin.drain_staging.capacity(), staging_capacity);
    }

    #[test]
    fn initialize_rejects_transport_rate_and_channel_mismatch() {
        let (mut wrong_rate, _) = make_plugin_with_writer(2, (44_100, 2, 256), vec![]);
        assert!(wrong_rate.initialize(48_000.0).is_err());

        let (mut wrong_channels, _) = make_plugin_with_writer(2, (48_000, 6, 256), vec![]);
        assert!(wrong_channels.initialize(48_000.0).is_err());
    }

    #[test]
    fn process_validates_initialization_context_overflow_and_sink_output() {
        let (mut plugin, _) = make_plugin_with_writer(2, (48_000, 2, 256), vec![]);
        let mut output = Vec::new();
        assert!(
            plugin
                .process(&[0.0; 8], &mut output, &ProcessContext::new(48_000, 4))
                .is_err()
        );

        plugin.initialize(48_000.0).unwrap();
        assert!(
            plugin
                .process(&[0.0; 8], &mut output, &ProcessContext::new(44_100, 4))
                .is_err()
        );
        assert!(
            plugin
                .process(&[], &mut output, &ProcessContext::new(48_000, usize::MAX))
                .is_err()
        );
        assert!(
            plugin
                .process(&[0.0; 8], &mut [0.0], &ProcessContext::new(48_000, 4))
                .is_err()
        );
    }

    #[test]
    fn ring_capacity_is_not_reported_as_latency() {
        let (plugin, _) = make_plugin_with_writer(2, (48_000, 2, 512), vec![]);
        assert_eq!(plugin.latency_samples(), 0);
    }
}
