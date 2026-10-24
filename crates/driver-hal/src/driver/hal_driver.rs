use super::consts::DAEMON_CONFIG_ACK_POLL_INTERVAL;
use super::consts::DAEMON_CONFIG_ACK_TIMEOUT;
use super::consts::DAEMON_HEARTBEAT_INTERVAL;
use super::misc::check_hal_driver_installed;
use super::misc::spawn_engine_heartbeat;
use crate::shared_memory::{
    DEFAULT_HAL_CHANNEL_COUNT, HalInputReader, MAX_HAL_CHANNEL_COUNT, SharedAudioBuffer,
};
use driver_common::{
    AudioDriver, ConfigResult, DriverConfig, DriverConfigRequest, DriverError, DriverStatus,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Instant;

pub(super) fn reset_new_daemon_mapping(buffer: &mut SharedAudioBuffer) -> Result<(), DriverError> {
    if buffer.reset_daemon_runtime_state() {
        Ok(())
    } else {
        Err(DriverError::io(
            "failed to quiesce and reset stale HAL shared-memory runtime state",
        ))
    }
}

/// Geometry a new daemon should adopt from an existing shared-memory header.
///
/// A daemon restart must not force the transport back to the compiled-in
/// 48 kHz/512-frame/stereo defaults: doing so made a live 96 kHz interface
/// appear to lose synchronization until the user switched devices. Missing or
/// invalid headers retain the historical creation defaults.
pub(super) fn initial_shared_memory_geometry() -> (u32, u32, u32) {
    const DEFAULT_GEOMETRY: (u32, u32, u32) = (48_000, 512, DEFAULT_HAL_CHANNEL_COUNT);
    let Ok(buffer) = SharedAudioBuffer::open_default() else {
        return DEFAULT_GEOMETRY;
    };
    let geometry = (
        buffer.sample_rate(),
        buffer.buffer_frames(),
        buffer.channel_count(),
    );
    if geometry.0 == 0 || geometry.1 == 0 || geometry.2 == 0 {
        DEFAULT_GEOMETRY
    } else {
        geometry
    }
}

/// macOS CoreAudio HAL driver.
///
/// Reads audio from the shared memory region created by the Swift HAL virtual device.
/// The Swift driver captures system audio and writes it to `/tmp/sotf-{uid}/audio.shm`;
/// this driver reads from that ring buffer.
pub struct HalDriver {
    pub(super) reader: Option<HalInputReader>,
    /// Separate buffer handle for config negotiation and status queries.
    /// We open this lazily and separately from the reader because the reader
    /// owns its own `SharedAudioBuffer` internally.
    pub(super) config_buffer: Option<SharedAudioBuffer>,
    pub(super) last_polled_request_id: u64,
    // Retained until publication succeeds or the request is superseded.
    pending_config_ack: Option<(u64, DriverConfig, u32, u32)>,
    pub(super) driver_installed: bool,
    pub(super) heartbeat_stop: Option<Arc<AtomicBool>>,
    pub(super) heartbeat_thread: Option<JoinHandle<()>>,
}

impl HalDriver {
    pub fn new() -> Self {
        Self {
            reader: None,
            config_buffer: None,
            last_polled_request_id: 0,
            pending_config_ack: None,
            driver_installed: false,
            heartbeat_stop: None,
            heartbeat_thread: None,
        }
    }

    /// Try to open (or reopen) the config buffer for status/config operations.
    pub(super) fn ensure_config_buffer(&mut self) {
        if self
            .config_buffer
            .as_ref()
            .is_some_and(|buffer| !buffer.backing_file_is_current())
        {
            self.stop_engine_heartbeat();
            self.config_buffer = None;
            self.last_polled_request_id = 0;
            self.pending_config_ack = None;
        }
        if self.config_buffer.is_none()
            && let Ok(buf) = SharedAudioBuffer::open_default()
        {
            self.config_buffer = Some(buf);
        }
    }

    fn flush_pending_config_ack(&mut self) {
        let Some((id, actual, status, error)) = self.pending_config_ack else {
            return;
        };
        let Some(buffer) = self.config_buffer.as_ref() else {
            return;
        };
        if buffer.acknowledge_config_request(id, actual, status, error)
            || buffer
                .configuration_snapshot()
                .is_some_and(|snapshot| snapshot.request.request_id != id)
        {
            self.pending_config_ack = None;
        }
    }

    pub(super) fn wait_for_daemon_config_ack(
        buf: &SharedAudioBuffer,
        request_id: u64,
    ) -> ConfigResult {
        let deadline = Instant::now() + DAEMON_CONFIG_ACK_TIMEOUT;
        loop {
            if let Some(snapshot) = buf.configuration_snapshot() {
                if snapshot.request.request_id != request_id {
                    return ConfigResult::error(DriverError::invalid_config(
                        "hal_config",
                        "Configuration request superseded",
                    ));
                }
                match snapshot.status {
                    1 => return ConfigResult::Accepted,
                    2 => {
                        return ConfigResult::negotiated(
                            snapshot.actual.sample_rate,
                            snapshot.actual.buffer_frames,
                            snapshot.actual.channel_count,
                        );
                    }
                    3 => {
                        return ConfigResult::error(DriverError::invalid_config(
                            "hal_config",
                            format!(
                                "HAL rejected config request (error code {})",
                                snapshot.error_code
                            ),
                        ));
                    }
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return ConfigResult::error(DriverError::timeout(
                    "Timed out waiting for HAL to apply config request",
                ));
            }
            std::thread::sleep(DAEMON_CONFIG_ACK_POLL_INTERVAL);
        }
    }

    pub(super) fn start_engine_heartbeat(&mut self) {
        if self.heartbeat_thread.is_some() {
            return;
        }

        self.ensure_config_buffer();
        let Some(path) = self
            .config_buffer
            .as_ref()
            .map(|buf| buf.path().to_path_buf())
        else {
            return;
        };

        match spawn_engine_heartbeat(path, DAEMON_HEARTBEAT_INTERVAL) {
            Ok((stop, handle)) => {
                self.heartbeat_stop = Some(stop);
                self.heartbeat_thread = Some(handle);
            }
            Err(e) => {
                log::warn!("[HalDriver] Failed to start daemon heartbeat thread: {}", e);
            }
        }
    }

    pub(super) fn stop_engine_heartbeat(&mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            stop.store(true, Ordering::Release);
        }

        if let Some(handle) = self.heartbeat_thread.take() {
            handle.thread().unpark();
            if handle.join().is_err() {
                log::warn!("[HalDriver] Daemon heartbeat thread panicked");
            }
        }
    }
}

impl Default for HalDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioDriver for HalDriver {
    fn initialize(&mut self) -> Result<(), DriverError> {
        log::info!("[HalDriver] Initializing macOS HAL driver");

        // Check if driver bundle is installed
        self.driver_installed = check_hal_driver_installed();
        if !self.driver_installed {
            log::warn!("[HalDriver] HAL driver not installed at /Library/Audio/Plug-Ins/HAL/");
            log::warn!("[HalDriver] HAL capture will not be available until driver is installed");
            return Ok(());
        }

        log::info!("[HalDriver] HAL driver is installed");

        // The daemon owns shared-memory creation. The HAL plugin runs inside
        // coreaudiod, so it should only have to open an already-sized file from
        // its realtime paths. Adopt any live header geometry instead of
        // resetting the transport to creation defaults on every daemon start.
        let (sample_rate, buffer_frames, channel_count) = initial_shared_memory_geometry();
        let mut buffer = SharedAudioBuffer::create_replacement_default(
            sample_rate,
            buffer_frames,
            channel_count,
        )
        .map_err(|error| {
            DriverError::io(format!("failed to initialize HAL shared memory: {error}"))
        })?;
        reset_new_daemon_mapping(&mut buffer)?;
        log::info!(
            "[HalDriver] Prepared shared memory: {}Hz, {}ch, {} frames",
            buffer.sample_rate(),
            buffer.channel_count(),
            buffer.buffer_frames()
        );
        self.config_buffer = Some(buffer);

        // Initialize the reader
        self.reader = HalInputReader::new();
        if self.reader.is_none() {
            return Err(DriverError::io(
                "failed to open HAL shared memory for audio input",
            ));
        }
        log::info!("[HalDriver] HAL input reader initialized");

        Ok(())
    }

    fn shutdown(&mut self) {
        self.stop_engine_heartbeat();

        // Clear engine_ready flag
        if let Some(ref buf) = self.config_buffer {
            buf.set_engine_ready(false);
            log::info!("[HalDriver] Cleared engine_ready flag");
        }

        self.reader = None;
        self.config_buffer = None;
        log::info!("[HalDriver] Shutdown complete");
    }

    fn status(&self) -> DriverStatus {
        let (
            sample_rate,
            channel_count,
            buffer_frames,
            capture_active,
            driver_ready,
            encryption_overflow_count,
        ) = if let Some(ref buf) = self.config_buffer {
            let backing_file_is_current = buf.backing_file_is_current();
            (
                buf.sample_rate(),
                buf.channel_count(),
                buf.buffer_frames(),
                backing_file_is_current && buf.is_active(),
                backing_file_is_current && buf.driver_ready(),
                buf.encryption_overflow_count(),
            )
        } else {
            (0, 0, 0, false, false, 0)
        };

        DriverStatus::new(
            true,
            self.driver_installed,
            capture_active,
            sample_rate,
            channel_count,
            buffer_frames,
            "macOS CoreAudio HAL",
            driver_ready,
            encryption_overflow_count,
        )
    }

    fn read_audio(&mut self, buffer: &mut [f32]) -> usize {
        if let Some(ref mut reader) = self.reader {
            reader.read(buffer)
        } else {
            0
        }
    }

    fn available_frames(&self) -> usize {
        if let Some(ref reader) = self.reader {
            reader.available_read_frames()
        } else {
            0
        }
    }

    fn sample_rate(&self) -> u32 {
        if let Some(ref reader) = self.reader {
            reader.sample_rate()
        } else if let Some(ref buf) = self.config_buffer {
            buf.sample_rate()
        } else {
            0
        }
    }

    fn channel_count(&self) -> u32 {
        if let Some(ref reader) = self.reader {
            reader.channel_count()
        } else if let Some(ref buf) = self.config_buffer {
            buf.channel_count()
        } else {
            0
        }
    }

    fn request_config(&mut self, config: DriverConfig) -> ConfigResult {
        self.ensure_config_buffer();
        if let Some(ref mut buf) = self.config_buffer {
            // Use 0 as sentinel for "keep current" — don't write zero to shared memory
            let current_rate = buf.sample_rate();
            let current_frames = buf.buffer_frames();
            let current_channels = buf.channel_count();
            let requested_rate = config.sample_rate_or(current_rate);
            let requested_frames = config.buffer_frames_or(current_frames);
            let requested_channels = config.channel_count_or(current_channels);

            if requested_channels == 0 || requested_channels > MAX_HAL_CHANNEL_COUNT {
                return ConfigResult::error(DriverError::invalid_config(
                    "channel_count",
                    format!(
                        "HAL channel count must be between 1 and {}, got {}",
                        MAX_HAL_CHANNEL_COUNT, requested_channels
                    ),
                ));
            }
            if requested_channels > buf.max_channel_count() {
                return ConfigResult::error(DriverError::invalid_config(
                    "channel_count",
                    format!(
                        "HAL shared memory was sized for at most {} channels, cannot request {}",
                        buf.max_channel_count(),
                        requested_channels
                    ),
                ));
            }

            let Some(request_id) =
                buf.request_config_change(requested_rate, requested_frames, requested_channels, 2)
            else {
                return ConfigResult::error(DriverError::not_available(
                    "Configuration mailbox is busy",
                ));
            };

            log::info!(
                "[HalDriver] Config request: {}Hz, {} frames, {} channels",
                requested_rate,
                requested_frames,
                requested_channels
            );
            Self::wait_for_daemon_config_ack(buf, request_id)
        } else {
            ConfigResult::error(DriverError::not_available("Shared memory not available"))
        }
    }

    fn poll_config_change(&mut self) -> Option<DriverConfig> {
        self.poll_config_request().map(|request| request.config)
    }

    fn poll_config_request(&mut self) -> Option<DriverConfigRequest> {
        self.ensure_config_buffer();
        self.flush_pending_config_ack();
        let snapshot = self.config_buffer.as_ref()?.configuration_snapshot()?;
        if snapshot.source != 1
            || !snapshot.pending
            || snapshot.request.request_id == self.last_polled_request_id
        {
            return None;
        }
        self.last_polled_request_id = snapshot.request.request_id;
        Some(snapshot.request)
    }

    fn config_request_is_current(&mut self, request_id: u64) -> bool {
        if request_id == 0 {
            return true;
        }
        self.ensure_config_buffer();
        // Advisory identity recheck only; the eventual ACK is protected by the
        // control gate. Contention cannot discard an already-polled intent.
        self.config_buffer.as_ref().is_some_and(|buffer| {
            buffer.header().config_request_id.load(Ordering::Acquire) == request_id
        })
    }

    fn acknowledge_config_change(&mut self, actual: DriverConfig, result: ConfigResult) {
        self.acknowledge_config_request(self.last_polled_request_id, actual, result);
    }

    fn acknowledge_config_request(
        &mut self,
        request_id: u64,
        actual: DriverConfig,
        result: ConfigResult,
    ) -> bool {
        if request_id == 0 {
            return true;
        }
        let Some(buf) = self.config_buffer.as_ref() else {
            return false;
        };
        let (status, error_code) = match result {
            ConfigResult::Accepted => (1, 0),
            ConfigResult::Negotiated { .. } => (2, 0),
            _ => (3, 1),
        };
        let published = buf.acknowledge_config_request(request_id, actual, status, error_code);
        if published {
            self.pending_config_ack = None;
        } else if buf
            .configuration_snapshot()
            .is_none_or(|snapshot| snapshot.request.request_id == request_id)
        {
            self.pending_config_ack = Some((request_id, actual, status, error_code));
        }
        published
    }

    fn patch_output_volume(
        &mut self,
        volume: Option<f32>,
        muted: Option<bool>,
    ) -> Option<(f32, bool, u32)> {
        self.ensure_config_buffer();
        Some(
            self.config_buffer
                .as_ref()?
                .patch_output_volume(volume, muted),
        )
    }

    fn output_volume(&mut self) -> Option<(f32, bool, u32)> {
        self.ensure_config_buffer();
        let buffer = self.config_buffer.as_ref()?;
        let (volume, muted, generation) = buffer.output_volume();
        if generation == 0 {
            return None;
        }
        Some((volume, muted, generation))
    }

    fn set_output_volume(&mut self, volume: f32, muted: bool) -> Option<u32> {
        self.ensure_config_buffer();
        let buffer = self.config_buffer.as_ref()?;
        Some(buffer.set_output_volume(volume, muted))
    }

    fn set_engine_ready(&mut self, ready: bool) {
        if let Err(error) = self.set_engine_ready_checked(ready) {
            log::warn!("[HalDriver] Cannot publish readiness: {error}");
        }
    }

    fn set_engine_ready_checked(&mut self, ready: bool) -> Result<(), DriverError> {
        self.ensure_config_buffer();
        let buffer = self.config_buffer.as_ref().ok_or_else(|| {
            DriverError::not_available("HAL mapping unavailable for readiness publication")
        })?;
        if ready
            && buffer
                .configuration_snapshot()
                .is_none_or(|snapshot| snapshot.pending)
        {
            return Err(DriverError::not_available(
                "Configuration acknowledgement is pending",
            ));
        }
        buffer.set_engine_ready(ready);
        if ready {
            self.start_engine_heartbeat();
        } else {
            self.stop_engine_heartbeat();
        }
        if !self.engine_ready_matches(ready) {
            if let Some(buffer) = self.config_buffer.as_ref() {
                buffer.set_engine_ready(false);
            }
            return Err(DriverError::not_available(
                "HAL readiness publication or heartbeat failed",
            ));
        }
        Ok(())
    }

    fn engine_ready_matches(&self, ready: bool) -> bool {
        self.config_buffer.as_ref().is_some_and(|buffer| {
            buffer.backing_file_is_current()
                && buffer.engine_ready() == ready
                && (!ready
                    || self
                        .heartbeat_thread
                        .as_ref()
                        .is_some_and(|thread| !thread.is_finished()))
        })
    }
}

impl Drop for HalDriver {
    fn drop(&mut self) {
        self.stop_engine_heartbeat();
    }
}
