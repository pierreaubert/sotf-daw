//! Sample observation for the simulated playback backend.
//!
//! Enabled only by `test-util`. This observes every sample after the real playback
//! volume/clamp kernel. It never records native hardware callbacks or claims
//! realtime timing: its mutex is used only by the simulated output worker.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const MAX_CHANNELS: usize = 16;

/// Named simulated output with observation available before the engine starts.
///
/// Clones share one observer. Independently constructed outputs have separate
/// identities even when their display names match. This never opens a device.
#[derive(Clone, Debug)]
pub struct TestOutput {
    name: String,
    probe: Arc<LabPcmProbe>,
}

impl PartialEq for TestOutput {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && Arc::ptr_eq(&self.probe, &other.probe)
    }
}

impl Eq for TestOutput {}

impl TestOutput {
    /// Create a simulated output whose observer starts disabled.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            probe: Arc::new(LabPcmProbe::default()),
        }
    }

    /// Return the identity reported by the simulated playback stream.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Share observation, allowing a test to arm it before engine construction.
    pub fn probe(&self) -> Arc<LabPcmProbe> {
        Arc::clone(&self.probe)
    }
}

/// Aggregate of every post-clamp sample in an explicitly armed lab window.
#[derive(Clone, Copy, Debug)]
pub struct LabPcmSnapshot {
    /// Number of interleaved output channels.
    pub channels: usize,
    /// Number of complete output frames, including underrun silence.
    pub frames: u64,
    /// Number of observed output blocks.
    pub blocks: u64,
    /// Per-channel minimum, populated through `channels`.
    pub minimum: [f32; MAX_CHANNELS],
    /// Per-channel maximum, populated through `channels`.
    pub maximum: [f32; MAX_CHANNELS],
    /// Number of non-finite samples or malformed blocks.
    pub invalid: u64,
    /// Per-channel minimum among nonzero samples; infinity until a signal arrives.
    pub nonzero_minimum: [f32; MAX_CHANNELS],
    /// Number of nonzero samples per channel.
    pub nonzero_samples: [u64; MAX_CHANNELS],
    /// Entirely silent frames before any signal, including startup prefill.
    pub leading_silent_frames: u64,
    /// Entirely silent frames after the first signal frame.
    pub silent_after_signal_frames: u64,
    /// Number of frames containing any nonzero sample.
    pub signal_frames: u64,
    /// Generation observed in the first block.
    pub first_generation: u64,
    /// Generation observed in the latest block.
    pub last_generation: u64,
}

impl Default for LabPcmSnapshot {
    fn default() -> Self {
        Self {
            channels: 0,
            frames: 0,
            blocks: 0,
            minimum: [f32::INFINITY; MAX_CHANNELS],
            maximum: [f32::NEG_INFINITY; MAX_CHANNELS],
            invalid: 0,
            nonzero_minimum: [f32::INFINITY; MAX_CHANNELS],
            nonzero_samples: [0; MAX_CHANNELS],
            leading_silent_frames: 0,
            silent_after_signal_frames: 0,
            signal_frames: 0,
            first_generation: 0,
            last_generation: 0,
        }
    }
}

/// Complete post-clamp blocks from the simulated output worker.
///
/// Enabled only by `test-util`. This callback never observes native device IO.
/// Implementations must return promptly; no aggregate-window lock is held.
pub trait LabPcmObserver: std::fmt::Debug + Send + Sync {
    /// Observe one complete output block at its post-clamp arrival time.
    fn observe(&self, samples: &[f32], channels: usize, generation: u64, at: Instant);
}

/// Per-engine lab observation, disabled until a test explicitly arms it.
#[derive(Debug, Default)]
pub struct LabPcmProbe {
    window: Mutex<Option<LabPcmSnapshot>>,
    mute_requests: AtomicU64,
    observer: OnceLock<Arc<dyn LabPcmObserver>>,
}

impl LabPcmProbe {
    /// Install one observer, normally before starting the simulated output.
    ///
    /// The probe retains its observer for its lifetime. Observation is independent
    /// of the aggregate window and includes startup and underrun silence.
    ///
    /// # Errors
    /// Returns the supplied observer if an observer was already installed.
    pub fn set_observer(
        &self,
        observer: Arc<dyn LabPcmObserver>,
    ) -> Result<(), Arc<dyn LabPcmObserver>> {
        self.observer.set(observer)
    }

    /// Start a fresh window, synchronizing with any observation already in progress.
    ///
    /// # Panics
    /// Panics if another test panicked while holding the window lock.
    pub fn arm(&self) {
        *self.window.lock().unwrap() = Some(LabPcmSnapshot::default());
    }

    /// Copy the entire current window under the same lock used by its producer.
    ///
    /// # Panics
    /// Panics if another test panicked while holding the window lock.
    pub fn snapshot(&self) -> Option<LabPcmSnapshot> {
        *self.window.lock().unwrap()
    }

    /// Stop observation and return the final coherent window.
    ///
    /// # Panics
    /// Panics if another test panicked while holding the window lock.
    pub fn disarm(&self) -> Option<LabPcmSnapshot> {
        self.window.lock().unwrap().take()
    }

    /// Count all attempted mute commands, including redundant mute/unmute pairs.
    pub fn mute_requests(&self) -> u64 {
        self.mute_requests.load(Ordering::Relaxed)
    }

    pub(crate) fn record_mute_request(&self) {
        self.mute_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn observe(&self, samples: &[f32], channels: usize, generation: u64) {
        if let Some(observer) = self.observer.get() {
            observer.observe(samples, channels, generation, Instant::now());
        }
        let mut guard = self.window.lock().unwrap();
        let Some(window) = guard.as_mut() else { return };
        if channels == 0 || channels > MAX_CHANNELS || !samples.len().is_multiple_of(channels) {
            window.invalid += 1;
            return;
        }
        if window.blocks == 0 {
            window.channels = channels;
            window.first_generation = generation;
        } else if channels != window.channels {
            window.invalid += 1;
            return;
        }
        window.last_generation = generation;
        window.blocks += 1;
        window.frames += (samples.len() / channels) as u64;
        for frame in samples.chunks_exact(channels) {
            if frame.iter().any(|&sample| sample != 0.0) {
                window.signal_frames += 1;
            } else if window.signal_frames == 0 {
                window.leading_silent_frames += 1;
            } else {
                window.silent_after_signal_frames += 1;
            }
            for (channel, &sample) in frame.iter().enumerate() {
                if sample.is_finite() {
                    window.minimum[channel] = window.minimum[channel].min(sample);
                    window.maximum[channel] = window.maximum[channel].max(sample);
                    if sample != 0.0 {
                        window.nonzero_minimum[channel] =
                            window.nonzero_minimum[channel].min(sample);
                        window.nonzero_samples[channel] += 1;
                    }
                } else {
                    window.invalid += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_receives_unarmed_blocks_without_holding_the_window_lock() {
        #[derive(Debug)]
        struct Observer {
            probe: std::sync::Weak<LabPcmProbe>,
            blocks: Mutex<Vec<(Vec<f32>, usize, u64, Instant)>>,
        }
        impl LabPcmObserver for Observer {
            fn observe(&self, samples: &[f32], channels: usize, generation: u64, at: Instant) {
                // Reentering the aggregate API must not deadlock this observer.
                let _ = self.probe.upgrade().unwrap().snapshot();
                self.blocks
                    .lock()
                    .unwrap()
                    .push((samples.to_vec(), channels, generation, at));
            }
        }
        let probe = Arc::new(LabPcmProbe::default());
        let observer = Arc::new(Observer {
            probe: Arc::downgrade(&probe),
            blocks: Mutex::new(Vec::new()),
        });
        probe.set_observer(observer.clone()).unwrap();
        assert!(probe.set_observer(observer.clone()).is_err());
        let before = Instant::now();
        probe.observe(&[0.0, 0.0, 0.25, -0.5], 2, 7);
        let after = Instant::now();
        let blocks = observer.blocks.lock().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, [0.0, 0.0, 0.25, -0.5]);
        assert_eq!((blocks[0].1, blocks[0].2), (2, 7));
        assert!(before <= blocks[0].3 && blocks[0].3 <= after);
        assert!(probe.snapshot().is_none());
    }

    #[test]
    fn window_retains_a_single_sample_dip_and_separates_channels() {
        let probe = LabPcmProbe::default();
        probe.observe(&[0.0, 0.0], 2, 1);
        assert!(probe.snapshot().is_none());
        probe.arm();
        probe.observe(&[0.25, 0.5, 0.25, 0.5], 2, 7);
        probe.observe(&[0.01, 0.5, 0.25, 0.5], 2, 7);
        probe.observe(&[0.25, 0.5], 2, 7);
        let window = probe.disarm().unwrap();
        assert_eq!(window.frames, 5);
        assert_eq!(window.blocks, 3);
        assert_eq!(&window.minimum[..2], &[0.01, 0.5]);
        assert_eq!(&window.maximum[..2], &[0.25, 0.5]);
        assert_eq!(window.first_generation, 7);
        assert_eq!(window.last_generation, 7);
        assert_eq!(window.invalid, 0);
        assert!(probe.snapshot().is_none());
    }

    #[test]
    fn startup_observation_distinguishes_prefill_gaps_and_missing_channels() {
        let output = TestOutput::new("Output A");
        assert_eq!(output, output.clone());
        assert_ne!(output, TestOutput::new("Output A"));
        let probe = output.probe();
        probe.arm();
        probe.observe(&[0.0, 0.0, 0.0, 0.0], 2, 1);
        probe.observe(&[0.25, 0.5, 0.25, 0.5], 2, 1);
        probe.observe(&[0.0, 0.5, 0.0, 0.0, 0.25, 0.5], 2, 1);
        let window = probe.disarm().unwrap();
        assert_eq!(window.frames, 7);
        assert_eq!(window.leading_silent_frames, 2);
        assert_eq!(window.signal_frames, 4);
        assert_eq!(window.silent_after_signal_frames, 1);
        assert_eq!(&window.nonzero_samples[..2], &[3, 4]);
        assert_eq!(&window.nonzero_minimum[..2], &[0.25, 0.5]);
        assert_eq!(window.invalid, 0);
    }

    #[test]
    fn reset_preserves_mute_history_and_reports_invalid_samples() {
        let probe = LabPcmProbe::default();
        probe.record_mute_request();
        probe.record_mute_request();
        probe.arm();
        probe.observe(&[f32::NAN, 0.5], 2, 1);
        probe.observe(&[0.5], 2, 1);
        assert_eq!(probe.snapshot().unwrap().invalid, 2);
        probe.arm();
        probe.observe(&[0.25, 0.5], 2, 2);
        let window = probe.snapshot().unwrap();
        assert_eq!(window.frames, 1);
        assert_eq!(window.invalid, 0);
        assert_eq!(window.first_generation, 2);
        assert_eq!(probe.mute_requests(), 2);
    }
}
