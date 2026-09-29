//! Optional configuration tail publication, independent of processing activity.

// Rust guideline compliant 2026-02-21
use crate::plugin::ProcessStatus;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub(crate) struct TailState {
    explicit: bool,
    samples: AtomicU32,
    changed: AtomicBool,
}

impl TailState {
    pub(crate) fn new(value: Option<u32>) -> Self {
        Self {
            explicit: value.is_some(),
            samples: AtomicU32::new(Self::normalize(value.unwrap_or(0))),
            changed: AtomicBool::new(false),
        }
    }

    pub(crate) fn is_explicit(&self) -> bool {
        self.explicit
    }

    pub(crate) fn invalidate(&self) {
        self.refresh(Some(u32::MAX));
    }

    pub(crate) fn refresh(&self, value: Option<u32>) {
        if self.explicit {
            // Opt-in cannot disappear after construction. Treat a violated
            // contract conservatively instead of publishing a false zero tail.
            let value = Self::normalize(value.unwrap_or(u32::MAX));
            if self.samples.swap(value, Ordering::AcqRel) != value {
                self.changed.store(true, Ordering::Release);
            }
        }
    }

    pub(crate) fn get(&self, legacy_status: ProcessStatus) -> u32 {
        if self.explicit {
            self.samples.load(Ordering::Acquire)
        } else {
            match legacy_status {
                ProcessStatus::Tail(samples) => samples,
                ProcessStatus::KeepAlive => u32::MAX,
                _ => 0,
            }
        }
    }

    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    fn normalize(value: u32) -> u32 {
        if value >= i32::MAX as u32 {
            u32::MAX
        } else {
            value
        }
    }
}
