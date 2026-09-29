//! Prepared HRTF publication with nonblocking access on the audio thread.

// Rust guideline compliant 2026-02-21
use super::types::BinauralState;
use std::sync::{Arc, Mutex, MutexGuard, TryLockResult};

pub(super) struct HrtfState {
    value: Mutex<Arc<BinauralState>>,
}

impl HrtfState {
    pub(super) fn new(value: Arc<BinauralState>) -> Arc<Self> {
        let shared = Arc::new(Self {
            value: Mutex::new(value),
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
            std::mem::replace(&mut *current, value)
        };
        drop(old);
    }

    /// Borrow a publication without waiting or acquiring thread-local resources.
    pub(super) fn try_lock(&self) -> TryLockResult<MutexGuard<'_, Arc<BinauralState>>> {
        self.value.try_lock()
    }
}
