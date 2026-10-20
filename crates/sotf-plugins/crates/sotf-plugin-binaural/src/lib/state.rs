//! Prepared HRTF publication with nonblocking access on the audio thread.

// Rust guideline compliant 2026-02-21
use super::head_tracking::{HeadPose, HeadRequests};
use super::types::BinauralState;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockResult};

pub(super) struct HrtfState {
    value: Mutex<Arc<BinauralState>>,
    head_generation: AtomicU64,
    head_epoch: AtomicU64,
}

impl HrtfState {
    pub(super) fn new(value: Arc<BinauralState>) -> Arc<Self> {
        let shared = Arc::new(Self {
            value: Mutex::new(value),
            head_generation: AtomicU64::new(0),
            head_epoch: AtomicU64::new(0),
        });
        // Prepare lazy pthread mutex resources (including macOS) after the
        // mutex reaches its stable heap address, on the construction thread.
        drop(shared.value.lock().expect("new HRTF mutex is unpoisoned"));
        shared
    }

    /// Clone a snapshot on a control or filter worker thread.
    pub(super) fn load(&self) -> Arc<BinauralState> {
        self.load_full()
    }

    pub(super) fn load_full(&self) -> Arc<BinauralState> {
        Arc::clone(&self.value.lock().unwrap_or_else(|error| error.into_inner()))
    }

    /// Replace a publication and reclaim its old reference off the audio thread.
    pub(super) fn store(&self, value: Arc<BinauralState>) {
        let old = {
            let mut current = self.value.lock().unwrap_or_else(|error| error.into_inner());
            self.head_generation.store(0, Ordering::Relaxed);
            self.head_epoch.store(0, Ordering::Relaxed);
            std::mem::replace(&mut *current, value)
        };
        drop(old);
    }

    /// Publishes current worker results and reclaims stale results off audio.
    pub(super) fn store_head(
        &self,
        value: Arc<BinauralState>,
        requests: &HeadRequests,
        pose: HeadPose,
    ) -> bool {
        let old = {
            let mut current = self.value.lock().unwrap_or_else(|error| error.into_inner());
            if !requests.accepts_epoch(pose.epoch) {
                return false;
            }
            if self.head_epoch.load(Ordering::Relaxed) == pose.epoch
                && self.head_generation.load(Ordering::Relaxed) >= pose.generation
            {
                return false;
            }
            self.head_generation
                .store(pose.generation, Ordering::Relaxed);
            self.head_epoch.store(pose.epoch, Ordering::Relaxed);
            std::mem::replace(&mut *current, value)
        };
        drop(old);
        true
    }

    /// Reads publication metadata while its value lock is held.
    pub(super) fn head_publication(&self) -> (u64, u64) {
        (
            self.head_epoch.load(Ordering::Relaxed),
            self.head_generation.load(Ordering::Relaxed),
        )
    }

    /// Borrow a publication without waiting or acquiring thread-local resources.
    pub(super) fn try_lock(&self) -> TryLockResult<MutexGuard<'_, Arc<BinauralState>>> {
        self.value.try_lock()
    }
}
