//! Coherent control-plane publication for the version 8 HAL protocol.

// Rust guideline compliant 2026-02-21

use super::SharedAudioHeader;
use super::shared_audio_buffer::SharedAudioBuffer;
use driver_common::{DriverConfig, DriverConfigRequest};
use std::sync::atomic::{AtomicU32, Ordering};

/// One coherent request and its matching acknowledgement.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationSnapshot {
    /// Identity and requested geometry.
    pub request: DriverConfigRequest,
    /// Initiator: 1 is HAL, 2 is daemon.
    pub source: u32,
    /// Negotiated or restored geometry, including channels.
    pub actual: DriverConfig,
    /// Zero while pending; 1 accepted, 2 negotiated, 3 rejected.
    pub status: u32,
    /// Platform error code, meaningful only for a matching acknowledgement.
    pub error_code: u32,
    /// Whether this request still requires acknowledgement.
    pub pending: bool,
}

struct ControlGuard<'a>(&'a AtomicU32);

impl Drop for ControlGuard<'_> {
    fn drop(&mut self) {
        self.0.store(0, Ordering::Release);
    }
}

fn lock_control(header: &SharedAudioHeader) -> Option<ControlGuard<'_>> {
    // Only control-plane operations use this gate. A stopped/crashed peer
    // cannot make a callback wait, nor make a control caller spin forever.
    for _ in 0..128 {
        if header
            .control_lock
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            return Some(ControlGuard(&header.control_lock));
        }
        std::hint::spin_loop();
    }
    None
}

impl SharedAudioBuffer {
    /// Invalidate callbacks retained across a daemon restart.
    ///
    /// Returns false on control-gate contention; callers must not claim readiness.
    pub fn invalidate_config_request(&self) -> bool {
        let h = self.header();
        let Some(_guard) = lock_control(h) else {
            return false;
        };
        let Some(id) = h.config_request_id.load(Ordering::Relaxed).checked_add(1) else {
            return false;
        };
        let id = id.max(1);
        h.config_request_id.store(id, Ordering::Relaxed);
        h.config_ack_id.store(id, Ordering::Relaxed);
        h.config_changed.store(0, Ordering::Relaxed);
        h.config_status.store(0, Ordering::Relaxed);
        h.config_error_code.store(0, Ordering::Relaxed);
        h.config_source.store(0, Ordering::Relaxed);
        true
    }

    /// Snapshot request and acknowledgement without mixing publications.
    ///
    /// Returns `None` on bounded control-gate contention. Callers retry later.
    pub fn configuration_snapshot(&self) -> Option<ConfigurationSnapshot> {
        let h = self.header();
        let _guard = lock_control(h)?;
        let request_id = h.config_request_id.load(Ordering::Relaxed);
        let acknowledged = h.config_ack_id.load(Ordering::Relaxed) == request_id;
        Some(ConfigurationSnapshot {
            request: DriverConfigRequest {
                request_id,
                config: DriverConfig::new(
                    h.requested_sample_rate.load(Ordering::Relaxed),
                    h.requested_buffer_frames.load(Ordering::Relaxed),
                    h.requested_channel_count.load(Ordering::Relaxed),
                ),
            },
            source: h.config_source.load(Ordering::Relaxed),
            actual: DriverConfig::new(
                h.actual_sample_rate.load(Ordering::Relaxed),
                h.actual_buffer_frames.load(Ordering::Relaxed),
                h.actual_channel_count.load(Ordering::Relaxed),
            ),
            status: if acknowledged {
                h.config_status.load(Ordering::Relaxed)
            } else {
                0
            },
            error_code: if acknowledged {
                h.config_error_code.load(Ordering::Relaxed)
            } else {
                0
            },
            pending: h.config_changed.load(Ordering::Relaxed) != 0 && !acknowledged,
        })
    }

    /// Publish one identified request or return `None` on gate contention.
    pub fn publish_config_request(&self, config: DriverConfig, source: u32) -> Option<u64> {
        let h = self.header();
        let _guard = lock_control(h)?;
        let request_id = h
            .config_request_id
            .load(Ordering::Relaxed)
            .checked_add(1)?
            .max(1);
        h.requested_sample_rate
            .store(config.sample_rate, Ordering::Relaxed);
        h.requested_buffer_frames
            .store(config.buffer_frames, Ordering::Relaxed);
        h.requested_channel_count
            .store(config.channel_count, Ordering::Relaxed);
        h.config_source.store(source, Ordering::Relaxed);
        h.config_status.store(0, Ordering::Relaxed);
        h.config_error_code.store(0, Ordering::Relaxed);
        h.config_changed.store(1, Ordering::Relaxed);
        h.config_request_id.store(request_id, Ordering::Relaxed);
        Some(request_id)
    }

    /// Acknowledge exactly one request, rejecting superseded identities.
    ///
    /// Returns false for contention, an untracked identity, or a newer request.
    /// A rejected acknowledgement cannot overwrite status or clear notification.
    pub fn acknowledge_config_request(
        &self,
        request_id: u64,
        actual: DriverConfig,
        status: u32,
        error_code: u32,
    ) -> bool {
        let h = self.header();
        let Some(_guard) = lock_control(h) else {
            return false;
        };
        if request_id == 0 || h.config_request_id.load(Ordering::Relaxed) != request_id {
            return false;
        }
        h.actual_sample_rate
            .store(actual.sample_rate, Ordering::Relaxed);
        h.actual_buffer_frames
            .store(actual.buffer_frames, Ordering::Relaxed);
        h.actual_channel_count
            .store(actual.channel_count, Ordering::Relaxed);
        h.config_status.store(status, Ordering::Relaxed);
        h.config_error_code.store(error_code, Ordering::Relaxed);
        h.config_ack_id.store(request_id, Ordering::Relaxed);
        h.config_changed.store(0, Ordering::Relaxed);
        true
    }

    /// Atomically patch user volume or mute, preserving the other writer's field.
    ///
    /// The complete tuple occupies one 64-bit word: scalar bits 0..31,
    /// mute bit 32, and generation bits 33..63. Zero generation is unpublished.
    /// This control-thread operation retries CAS contention without taking a lock.
    pub fn patch_output_volume(
        &self,
        volume: Option<f32>,
        muted: Option<bool>,
    ) -> (f32, bool, u32) {
        let state = &self.header().volume_state;
        let mut old = state.load(Ordering::Acquire);
        loop {
            let previous_generation = (old >> 33) as u32;
            let scalar = volume.unwrap_or_else(|| {
                if previous_generation == 0 {
                    1.0
                } else {
                    f32::from_bits(old as u32)
                }
            });
            let scalar = if scalar.is_finite() {
                scalar.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let mute = muted.unwrap_or(old & (1 << 32) != 0);
            let generation = (previous_generation.wrapping_add(1) & 0x7fff_ffff).max(1);
            let new = u64::from(scalar.to_bits())
                | (u64::from(mute) << 32)
                | (u64::from(generation) << 33);
            match state.compare_exchange_weak(old, new, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return (scalar, mute, generation),
                Err(current) => old = current,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn stale_ack_cannot_clear_a_new_request_or_replace_actual_channels() {
        let file = NamedTempFile::new().unwrap();
        let buffer = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        let first = buffer
            .publish_config_request(DriverConfig::new(96_000, 128, 8), 1)
            .unwrap();
        let latest_config = DriverConfig::new(48_000, 64, 2);
        let latest = buffer.publish_config_request(latest_config, 1).unwrap();
        assert!(!buffer.acknowledge_config_request(first, DriverConfig::new(96_000, 128, 8), 1, 0));
        let snapshot = buffer.configuration_snapshot().unwrap();
        assert_eq!(snapshot.request.request_id, latest);
        assert_eq!(snapshot.request.config, latest_config);
        assert!(snapshot.pending);
        assert_eq!(snapshot.status, 0);
        let restored = DriverConfig::new(44_100, 256, 4);
        assert!(buffer.acknowledge_config_request(latest, restored, 2, 0));
        let snapshot = buffer.configuration_snapshot().unwrap();
        assert_eq!(snapshot.actual, restored);
        assert_eq!(snapshot.status, 2);
        assert!(!snapshot.pending);
        assert!(!buffer.acknowledge_config_request(first, latest_config, 3, 99));
        assert_eq!(buffer.configuration_snapshot().unwrap().actual, restored);
    }

    #[test]
    fn held_control_gate_never_blocks_audio_copy_or_cursor_publication() {
        let file = NamedTempFile::new().unwrap();
        let mut buffer = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        buffer.header().control_lock.store(1, Ordering::Release);
        assert!(buffer.configuration_snapshot().is_none());
        assert!(
            buffer
                .publish_config_request(DriverConfig::new(48_000, 64, 2), 1)
                .is_none()
        );
        assert!(!buffer.acknowledge_config_request(1, DriverConfig::new(48_000, 64, 2), 1, 0));
        let samples = [0.25, -0.5, 0.5, -0.25];
        assert_eq!(buffer.write_audio(&samples), 2);
        let mut readback = [0.0; 4];
        assert_eq!(buffer.read_audio(&mut readback), 2);
        assert_eq!(readback, samples);
        buffer.header().control_lock.store(0, Ordering::Release);
    }

    #[test]
    fn simultaneous_partial_writers_keep_mute_and_count_every_publication() {
        let file = NamedTempFile::new().unwrap();
        let buffer = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        let start = buffer.set_output_volume(0.75, true);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..10_000 {
                    buffer.patch_output_volume(Some(0.25), None);
                    assert!(buffer.output_volume().1);
                }
            });
            scope.spawn(|| {
                for _ in 0..10_000 {
                    buffer.patch_output_volume(None, Some(true));
                    assert!(buffer.output_volume().1);
                }
            });
        });
        assert_eq!(buffer.output_volume(), (0.25, true, start + 20_000));
    }
    #[test]
    fn incompatible_peer_readiness_and_old_restart_acks_are_not_adopted() {
        let file = NamedTempFile::new().unwrap();
        let buffer = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        let old = buffer
            .publish_config_request(DriverConfig::new(96_000, 128, 8), 1)
            .unwrap();
        assert!(buffer.invalidate_config_request());
        assert!(!buffer.acknowledge_config_request(old, DriverConfig::new(96_000, 128, 8), 1, 0));
        buffer.header().driver_ready.store(1, Ordering::Release);
        buffer.header().version.store(7, Ordering::Release);
        drop(buffer);
        let upgraded = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        assert!(!upgraded.driver_ready());
    }

    #[test]
    fn request_identity_exhaustion_requires_replacing_the_transport() {
        let file = NamedTempFile::new().unwrap();
        let buffer = SharedAudioBuffer::create_or_open(file.path(), 48_000, 64, 2).unwrap();
        buffer
            .header()
            .config_request_id
            .store(u64::MAX, Ordering::Release);
        assert!(
            buffer
                .publish_config_request(DriverConfig::new(96_000, 128, 8), 1)
                .is_none()
        );
        assert!(!buffer.invalidate_config_request());
        assert_eq!(
            buffer.header().config_request_id.load(Ordering::Acquire),
            u64::MAX
        );
    }

    #[test]
    fn daemon_replacement_retires_poisoned_gates_without_touching_old_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.shm");
        let mut old = SharedAudioBuffer::create_or_open(&path, 48_000, 64, 2).unwrap();
        old.set_output_volume(0.375, true);
        old.header().control_lock.store(1, Ordering::Release);
        old.header().configuring.store(2, Ordering::Release);
        old.header().driver_ready.store(1, Ordering::Release);
        let old_len = old.mmap.len();
        let mut current =
            SharedAudioBuffer::create_replacement_with_max_geometry(&path, 96_000, 128, 8, 128, 8)
                .unwrap();
        assert_eq!(old.mmap.len(), old_len);
        assert_eq!(old.sample_rate(), 48_000);
        assert_eq!(old.channel_count(), 2);
        assert_eq!(old.header().control_lock.load(Ordering::Acquire), 1);
        assert_eq!(old.header().configuring.load(Ordering::Acquire), 2);
        assert!(!old.backing_file_is_current());
        assert!(current.backing_file_is_current());
        assert!(!current.driver_ready());
        assert_eq!(current.output_volume(), old.output_volume());
        assert!(current.configuration_snapshot().is_some());
        assert_eq!(current.write_audio(&[0.25; 8]), 1);
        let mut samples = [0.0; 8];
        assert_eq!(current.read_audio(&mut samples), 1);
        assert_eq!(samples, [0.25; 8]);
        // The retired mapping remains writable but cannot alter its successor.
        old.header().configuring.store(0, Ordering::Release);
        assert_eq!(old.write_audio(&[0.5; 2]), 1);
        assert_eq!(current.available_read_frames(), 0);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert!(
            SharedAudioBuffer::create_replacement_with_max_geometry(&path, 96_000, 128, 0, 128, 8,)
                .is_err()
        );
        assert!(current.backing_file_is_current());
        assert!(current.configuration_snapshot().is_some());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
