//! Latest-pose publication with bounded, allocation-free audio-thread writes.

use crate::{state::HrtfState, types::BinauralState};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst};
use std::sync::mpsc::Receiver;

#[derive(Clone, Copy, Debug)]
pub(super) struct HeadPose {
    pub generation: u64,
    pub epoch: u64,
    pub angles: (f32, f32, f32),
}

pub(super) struct HeadRequests {
    generation: AtomicU64,
    epoch: AtomicU64,
    angles: [AtomicU32; 3],
    valid: AtomicBool,
    stopped: AtomicBool,
}

impl HeadRequests {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            generation: AtomicU64::new(0),
            epoch: AtomicU64::new(1),
            angles: std::array::from_fn(|_| AtomicU32::new(0)),
            valid: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        })
    }

    fn finish_write(&self) -> u64 {
        let odd = self.generation.load(SeqCst);
        // Keep the counter odd until the new epoch is installed on wrap.
        // Zero remains reserved for non-head-tracking publications.
        let generation = if odd == u64::MAX {
            self.epoch.fetch_add(1, SeqCst);
            2
        } else {
            odd + 1
        };
        self.generation.store(generation, SeqCst);
        generation
    }

    /// Commits a pose from the sole plugin/control owner, without waiting.
    pub(super) fn publish(&self, angles: (f32, f32, f32)) -> u64 {
        // Odd/even sequencing protects the three-word pose. Sequentially
        // consistent atomic payloads make coherent snapshots independent of
        // CPU/compiler reorderings; only the worker retries, never the writer.
        self.generation.fetch_add(1, SeqCst);
        for (target, value) in self.angles.iter().zip([angles.0, angles.1, angles.2]) {
            target.store(value.to_bits(), SeqCst);
        }
        self.valid.store(true, SeqCst);
        self.finish_write()
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, SeqCst);
        self.valid.store(false, SeqCst);
        self.epoch.fetch_add(1, SeqCst);
        self.finish_write();
    }

    pub(super) fn start(&self) {
        self.invalidate();
        self.stopped.store(false, SeqCst);
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, SeqCst);
        self.invalidate();
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped.load(SeqCst)
    }

    pub(super) fn accepts_epoch(&self, epoch: u64) -> bool {
        !self.stopped() && self.epoch.load(SeqCst) == epoch
    }

    /// Reads a coherent pose on the non-realtime worker.
    pub(super) fn snapshot(&self) -> Option<HeadPose> {
        loop {
            let generation = self.generation.load(SeqCst);
            if !generation.is_multiple_of(2) {
                std::thread::yield_now();
                continue;
            }
            let valid = self.valid.load(SeqCst);
            let epoch = self.epoch.load(SeqCst);
            let angles = self
                .angles
                .each_ref()
                .map(|x| f32::from_bits(x.load(SeqCst)));
            if self.generation.load(SeqCst) == generation {
                return valid.then_some(HeadPose {
                    generation,
                    epoch,
                    angles: (angles[0], angles[1], angles[2]),
                });
            }
        }
    }
}

pub(super) fn run_head_worker(
    receiver: Receiver<(f32, f32, f32)>,
    requests: Arc<HeadRequests>,
    state: Arc<HrtfState>,
    mut compute: impl FnMut(HeadPose) -> Result<Arc<BinauralState>, String>,
) {
    let mut completed = None;
    while receiver.recv().is_ok() {
        if requests.stopped() {
            break;
        }
        // Queue entries are wakeups. The atomic mailbox carries the newest
        // pose even when another wakeup was already pending in the bounded queue.
        // Consume at most one extra wakeup, so a continuously producing
        // callback cannot keep the worker in an unbounded queue-drain loop.
        let _ = receiver.try_recv();
        let Some(pose) = requests.snapshot() else {
            continue;
        };
        if completed == Some((pose.epoch, pose.generation)) {
            continue;
        }
        // A newer desired pose does not cancel every in-flight result: that
        // would starve tracking during continuous motion. Publish completed
        // generations monotonically; reset/resource epochs still cancel them.
        match compute(pose) {
            Ok(prepared) => {
                state.store_head(prepared, &requests, pose);
            }
            Err(error) => {
                log::warn!("[BinauralDecoder] Background HRTF recompute failed: {error}");
            }
        }
        completed = Some((pose.epoch, pose.generation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_wrap_changes_epoch_without_exposing_a_control_publication() {
        let requests = HeadRequests::new();
        requests.publish((1.0, 2.0, 3.0));
        let old = requests.snapshot().unwrap();
        requests.generation.store(u64::MAX - 1, SeqCst);
        assert_eq!(requests.publish((4.0, 5.0, 6.0)), 2);
        let new = requests.snapshot().unwrap();
        assert_eq!(new.generation, 2);
        assert_eq!(new.angles, (4.0, 5.0, 6.0));
        assert_ne!(old.epoch, new.epoch);
        assert!(!requests.accepts_epoch(old.epoch));
    }
}
