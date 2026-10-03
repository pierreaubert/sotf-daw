//! Worker-death detection, transport poison, and honest partial peaks.
//!
//! Three independent signals feed death detection, in priority order:
//!
//! 1. The per-worker [`WorkerExitStatus`] slot, written exactly once by the
//!    thread wrapper (`record_worker_exit`) as the worker exits.
//! 2. The structured exit event the wrapper sends on the error path
//!    (decoder/processing/playback error events, `ThreadEvent::ThreadPanic`).
//!    The event path is untouched by P3: the manager drains events after the
//!    death poll each tick, so a drained event refines the poll's generic
//!    record with its specific text. Inversion: an exit event can drain
//!    BEFORE its slot store is visible (the wrapper sends the event, then
//!    records the slot), applying specific text to an unpoisoned state;
//!    the next poll then stamps generic text over it. Final text is
//!    generic-but-true with a one-tick poison gap (fail-loud via
//!    `last_error = Some`; a Play in the gap fails at the dead worker
//!    send). The unconditional stamp stays — no string matching.
//! 3. The `JoinHandle::is_finished` fallback poll in the manager loop, which
//!    catches exits the wrapper never recorded ([`WorkerDeathKind::Unrecorded`]).
//!
//! [`worker_death_disposition`] merges the slot reading with the fallback poll
//! into one decision; the manager loop calls [`record_worker_death`] at most
//! once per worker (guarded by the pre-existing per-worker reported flags).
//! Recording a death poisons the transport: [`poison_refuses_command`] rejects
//! every state-changing command with [`POISON_REFUSAL`], so the only recovery
//! is a fresh engine. There is no dead-handle `rePlay` promise.
//!
//! Peak honesty on death: the playback wrapper retains its shared peak atomic
//! past thread exit, the manager folds the residual peak into
//! `playback_peak_max_linear`, and `peak_record_complete` is set to `false`
//! (structured partial). The cumulative value is an honest lower bound, never
//! a completed record. See the audio-peak-provenance contract in
//! `audit/requirements/analog-limiter.md` (APP article).

// Rust guideline compliant 2026-02-21

use super::manager_thread::thread_event_visitor::reset_output_meter;
use super::{AudioEngineState, ManagerCommand, PlaybackState};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Durable exit disposition for one worker thread.
///
/// Written at most once, by the thread wrapper, as the worker exits. The
/// manager loop reads the slot alongside `JoinHandle::is_finished` and merges
/// both through [`worker_death_disposition`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerExit {
    /// The worker has not recorded an exit.
    Running,
    /// The worker function returned `Ok(())`, which must not happen: worker
    /// loops only exit via shutdown commands that consume the wrapper first.
    UnexpectedReturn,
    /// The worker function returned `Err`; the wrapper also sent the
    /// worker-specific structured error event.
    Errored,
    /// The worker panicked; the wrapper also sent `ThreadEvent::ThreadPanic`.
    Panicked,
}

impl WorkerExit {
    fn as_u8(self) -> u8 {
        match self {
            WorkerExit::Running => 0,
            WorkerExit::UnexpectedReturn => 1,
            WorkerExit::Errored => 2,
            WorkerExit::Panicked => 3,
        }
    }

    /// Decode a slot value. `None` means corruption (unreachable: the slot has
    /// one writer and four values); fail-closed handling lives in
    /// [`worker_death_disposition`].
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(WorkerExit::Running),
            1 => Some(WorkerExit::UnexpectedReturn),
            2 => Some(WorkerExit::Errored),
            3 => Some(WorkerExit::Panicked),
            _ => None,
        }
    }
}

/// Shared exit-disposition slot for one worker thread.
///
/// The wrapper holds one clone and records the exit; the manager loop holds
/// the other clone (via the worker handle) and polls it. Release/Acquire is
/// the weakest sufficient pairing for this flag handoff: the manager polls
/// every tick until a death records, so any physically propagated store is
/// observed (the same eventual-visibility class as the pre-existing
/// `is_finished` poll, which likewise has no join edge). The wrapper sends
/// its legacy structured event before storing the slot; the manager drains
/// events after the death poll each tick, so the poll's generic record lands
/// first and a drained event refines it.
#[derive(Clone, Debug)]
pub(crate) struct WorkerExitStatus {
    raw: Arc<AtomicU8>,
}

impl WorkerExitStatus {
    /// Create a slot in the [`WorkerExit::Running`] state.
    pub(crate) fn new() -> Self {
        Self {
            raw: Arc::new(AtomicU8::new(WorkerExit::Running.as_u8())),
        }
    }

    /// Load the recorded disposition. `None` means slot corruption.
    pub(crate) fn load(&self) -> Option<WorkerExit> {
        WorkerExit::from_u8(self.raw.load(Ordering::Acquire))
    }

    /// Record an unexpected clean return.
    pub(crate) fn record_unexpected_return(&self) {
        self.raw
            .store(WorkerExit::UnexpectedReturn.as_u8(), Ordering::Release);
    }

    /// Record an errored exit.
    pub(crate) fn record_errored(&self) {
        self.raw
            .store(WorkerExit::Errored.as_u8(), Ordering::Release);
    }

    /// Record a panic exit.
    pub(crate) fn record_panicked(&self) {
        self.raw
            .store(WorkerExit::Panicked.as_u8(), Ordering::Release);
    }
}

impl Default for WorkerExitStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// Classified worker death, for the poison record and logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerDeathKind {
    /// The worker panicked.
    Panicked,
    /// The worker returned `Err`.
    Errored,
    /// The worker returned `Ok(())` without a shutdown consuming it first.
    UnexpectedReturn,
    /// The worker is finished but recorded no disposition (wrapper bypassed,
    /// or a corrupt slot read fail-closed).
    Unrecorded,
}

impl WorkerDeathKind {
    /// Human-readable disposition for the poison record and logs.
    pub(crate) fn label(self) -> &'static str {
        match self {
            WorkerDeathKind::Panicked => "panicked",
            WorkerDeathKind::Errored => "returned an error",
            WorkerDeathKind::UnexpectedReturn => "returned unexpectedly",
            WorkerDeathKind::Unrecorded => "exited without recording a status",
        }
    }
}

/// Merge one slot reading with the `is_finished` fallback poll.
///
/// `None` means the worker is alive; `Some` means dead. A recorded
/// disposition always wins over the poll; a finished worker with no recorded
/// disposition (or a corrupt slot) is an [`WorkerDeathKind::Unrecorded`]
/// death. A corrupt slot on a live worker is trusted-alive: the handle proves
/// the thread exists, so recording a death there would poison wrongfully.
pub(crate) fn worker_death_disposition(
    slot: Option<WorkerExit>,
    finished: bool,
) -> Option<WorkerDeathKind> {
    match (slot, finished) {
        (Some(WorkerExit::Panicked), _) => Some(WorkerDeathKind::Panicked),
        (Some(WorkerExit::Errored), _) => Some(WorkerDeathKind::Errored),
        (Some(WorkerExit::UnexpectedReturn), _) => Some(WorkerDeathKind::UnexpectedReturn),
        (_, true) => Some(WorkerDeathKind::Unrecorded),
        (_, false) => None,
    }
}

/// Record one worker exit into its slot.
///
/// Called by each thread wrapper after it logs and forwards its legacy
/// structured event. The event path is untouched: P3 adds the durable slot
/// beside it, and the manager merges the slot with its `is_finished`
/// fallback poll through [`worker_death_disposition`].
pub(crate) fn record_worker_exit(
    slot: &WorkerExitStatus,
    result: &std::thread::Result<Result<(), String>>,
) {
    match result {
        Ok(Ok(())) => slot.record_unexpected_return(),
        Ok(Err(_)) => slot.record_errored(),
        Err(_) => slot.record_panicked(),
    }
}

/// Refusal for transport commands issued to a poisoned engine.
///
/// Terminal by design: the only recovery is a fresh engine. Exact bytes are
/// part of the poison contract and pinned by unit test.
pub(crate) const POISON_REFUSAL: &str = "worker dead, recreate engine";

/// Decide whether the poison gate refuses `command`.
///
/// Unpoisoned engines refuse nothing. A poisoned engine refuses every
/// state-changing or audio-path command; only inert reads, sink-state
/// commands safe on a dead transport, and shutdown pass through:
///
/// * `Shutdown` (quiesce a dead engine),
/// * `GetState`, `GetPosition` (inert reads),
/// * `GetPluginData` (bounded 100 ms worker round-trip; no mutation,
///   fail-loud on timeout/disconnect),
/// * `Mute`, `SetVolume`, `BypassProcessing` (sink-side latches),
/// * `Pause`, `Stop`, `CancelNext`, `LinearPhaseEqCancel`,
///   `LinearPhaseEqStatus` (idempotent quiesce/reads),
/// * `MaintainIsolatedExternalPluginWorkers` (external-worker poll, no
///   transport touch; non-iOS only).
///
/// `ReloadConfig` is refused: it enqueues a plugin-chain rebuild —
/// indirectly what refused `UpdatePluginChain` does synchronously —
/// and no use case exists on a terminal engine (fresh engines read
/// config fresh).
///
/// The allow-set uses `matches!` so an unlisted future variant fails closed
/// (refused). The exhaustive census test below pins the gate on every
/// `ManagerCommand` variant at compile time; keep it and this list in sync.
pub(crate) fn poison_refuses_command(poisoned: bool, command: &ManagerCommand) -> bool {
    if !poisoned {
        return false;
    }
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    if matches!(
        command,
        ManagerCommand::MaintainIsolatedExternalPluginWorkers
    ) {
        return false;
    }
    !matches!(
        command,
        ManagerCommand::Shutdown
            | ManagerCommand::GetState
            | ManagerCommand::GetPosition
            | ManagerCommand::GetPluginData(_)
            | ManagerCommand::Mute(_)
            | ManagerCommand::SetVolume(_)
            | ManagerCommand::BypassProcessing(_)
            | ManagerCommand::Pause
            | ManagerCommand::Stop
            | ManagerCommand::CancelNext
            | ManagerCommand::LinearPhaseEqCancel { .. }
            | ManagerCommand::LinearPhaseEqStatus { .. }
    )
}

/// Record one worker death into shared transport state.
///
/// Sets `Stopped`, zeroes the live output meter (preserving the epoch latch
/// for the fold below), stamps the death into `last_error`, poisons the
/// transport, and marks the peak record structurally incomplete. Folds the
/// retained residual (when the playback wrapper still holds its shared peak
/// atomic) into `playback_peak_max_linear` as an honest lower bound.
/// `f32::max` ignores NaN on either side, so a non-finite residual can neither
/// clear nor poison the latch. Idempotent: recording twice keeps poison,
/// `false`, and the larger peak.
pub(crate) fn record_worker_death(
    state: &mut AudioEngineState,
    worker: &'static str,
    death: WorkerDeathKind,
    retained_peak: Option<f32>,
) {
    state.playback_state = PlaybackState::Stopped;
    reset_output_meter(state);
    state.last_error = Some(format!("worker dead ({worker}): {}", death.label()));
    state.worker_death_poisoned = true;
    state.peak_record_complete = false;
    if let Some(peak) = retained_peak {
        state.playback_peak_max_linear = state.playback_peak_max_linear.max(peak);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioSource, PluginGraphConfig};
    use sotf_plugins::plugin_linear_phase_eq::BandConfig;
    use std::path::PathBuf;

    fn test_band() -> BandConfig {
        BandConfig {
            filter_type: "peak".to_string(),
            frequency: 1000.0,
            q: 0.7,
            gain_db: 0.0,
            active: true,
            placement: None,
        }
    }

    fn test_source() -> AudioSource {
        AudioSource::File(PathBuf::from("worker-death-test.wav"))
    }

    #[test]
    fn exit_records_unexpected_return() {
        let slot = WorkerExitStatus::new();
        assert_eq!(slot.load(), Some(WorkerExit::Running));

        record_worker_exit(&slot, &Ok(Ok(())));

        assert_eq!(slot.load(), Some(WorkerExit::UnexpectedReturn));
    }

    #[test]
    fn exit_records_error() {
        let slot = WorkerExitStatus::new();

        record_worker_exit(&slot, &Ok(Err("boom".to_string())));

        assert_eq!(slot.load(), Some(WorkerExit::Errored));
    }

    #[test]
    fn exit_records_panic() {
        let slot = WorkerExitStatus::new();
        let result: std::thread::Result<Result<(), String>> =
            Err(Box::new("panic payload") as Box<dyn std::any::Any + Send>);

        record_worker_exit(&slot, &result);

        assert_eq!(slot.load(), Some(WorkerExit::Panicked));
    }

    #[test]
    fn disposition_table_pins_every_slot_and_poll_cell() {
        use WorkerDeathKind::{Errored, Panicked, UnexpectedReturn, Unrecorded};
        use WorkerExit::UnexpectedReturn as SlotReturned;
        use WorkerExit::{Errored as SlotErrored, Panicked as SlotPanicked, Running};
        let cases: [(Option<WorkerExit>, bool, Option<WorkerDeathKind>); 10] = [
            (Some(Running), false, None),
            (Some(Running), true, Some(Unrecorded)),
            (Some(SlotReturned), false, Some(UnexpectedReturn)),
            (Some(SlotReturned), true, Some(UnexpectedReturn)),
            (Some(SlotErrored), false, Some(Errored)),
            (Some(SlotErrored), true, Some(Errored)),
            (Some(SlotPanicked), false, Some(Panicked)),
            (Some(SlotPanicked), true, Some(Panicked)),
            (None, false, None),
            (None, true, Some(Unrecorded)),
        ];
        for (slot, finished, expected) in cases {
            assert_eq!(
                worker_death_disposition(slot, finished),
                expected,
                "slot={slot:?} finished={finished}"
            );
        }
    }

    #[test]
    fn corrupt_slot_loads_fail_closed() {
        let slot = WorkerExitStatus::new();
        slot.raw.store(99, Ordering::Release);
        assert_eq!(slot.load(), None);
    }

    #[test]
    fn barrier_join_rendezvous_feeds_proven_exit_to_disposition() {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let parked = Arc::clone(&barrier);
        let worker = std::thread::Builder::new()
            .name("worker-death-test".to_string())
            .spawn(move || {
                parked.wait();
            })
            .expect("test worker spawns");
        // Parked behind the barrier proves the thread alive: no sleep, the
        // barrier wait below is the only synchronization.
        assert!(!worker.is_finished());
        assert_eq!(
            worker_death_disposition(Some(WorkerExit::Running), worker.is_finished()),
            None
        );
        barrier.wait();
        worker.join().expect("released worker joins");
        // join() returning proves termination; the proven exit with an
        // unrecorded slot is an Unrecorded death.
        assert_eq!(
            worker_death_disposition(Some(WorkerExit::Running), true),
            Some(WorkerDeathKind::Unrecorded)
        );
    }

    #[test]
    fn is_finished_true_reading_is_observed_on_a_real_handle() {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let parked = Arc::clone(&barrier);
        let worker = std::thread::Builder::new()
            .name("worker-death-finish-poll".to_string())
            .spawn(move || {
                parked.wait();
            })
            .expect("test worker spawns");
        assert!(!worker.is_finished());
        barrier.wait();
        // Liveness bound only: the released worker exits in microseconds; the
        // bound trips (fail-loud) only if is_finished never flips.
        let start = std::time::Instant::now();
        while !worker.is_finished() {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "is_finished never flipped after the worker was released"
            );
            std::hint::spin_loop();
        }
        assert_eq!(
            worker_death_disposition(Some(WorkerExit::Running), worker.is_finished()),
            Some(WorkerDeathKind::Unrecorded)
        );
        worker.join().expect("finished worker joins");
    }

    #[test]
    fn poison_census_exhaustive_match_pins_every_variant() {
        assert_eq!(POISON_REFUSAL, "worker dead, recreate engine");
        // Exhaustive over ManagerCommand with NO wildcard: a new variant
        // breaks compilation here until this test and the predicate's
        // allow-set extend together. Arms assert (unit-typed) rather than
        // returning bool, so no matches!-form applies.
        fn assert_census(command: &ManagerCommand) {
            match command {
                ManagerCommand::Play(_) => {
                    assert!(poison_refuses_command(true, command), "Play must refuse")
                }
                ManagerCommand::PlayAt(..) => {
                    assert!(poison_refuses_command(true, command), "PlayAt must refuse")
                }
                ManagerCommand::Pause => {
                    assert!(!poison_refuses_command(true, command), "Pause must pass")
                }
                ManagerCommand::Resume => {
                    assert!(poison_refuses_command(true, command), "Resume must refuse")
                }
                ManagerCommand::Stop => {
                    assert!(!poison_refuses_command(true, command), "Stop must pass")
                }
                ManagerCommand::Seek(_) => {
                    assert!(poison_refuses_command(true, command), "Seek must refuse")
                }
                ManagerCommand::QueueNext(_) => {
                    assert!(
                        poison_refuses_command(true, command),
                        "QueueNext must refuse"
                    )
                }
                ManagerCommand::CancelNext => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "CancelNext must pass"
                    )
                }
                ManagerCommand::SetVolume(_) => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "SetVolume must pass"
                    )
                }
                ManagerCommand::Mute(_) => {
                    assert!(!poison_refuses_command(true, command), "Mute must pass")
                }
                ManagerCommand::UpdatePluginChain(_) => {
                    assert!(
                        poison_refuses_command(true, command),
                        "UpdatePluginChain must refuse"
                    )
                }
                ManagerCommand::UpdatePluginGraph(_) => {
                    assert!(
                        poison_refuses_command(true, command),
                        "UpdatePluginGraph must refuse"
                    )
                }
                ManagerCommand::SetPluginParameter { .. } => {
                    assert!(
                        poison_refuses_command(true, command),
                        "SetPluginParameter must refuse"
                    )
                }
                ManagerCommand::BypassProcessing(_) => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "BypassProcessing must pass"
                    )
                }
                ManagerCommand::LinearPhaseEqRequest { .. } => {
                    assert!(
                        poison_refuses_command(true, command),
                        "LinearPhaseEqRequest must refuse"
                    )
                }
                ManagerCommand::LinearPhaseEqCancel { .. } => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "LinearPhaseEqCancel must pass"
                    )
                }
                ManagerCommand::LinearPhaseEqStatus { .. } => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "LinearPhaseEqStatus must pass"
                    )
                }
                #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                ManagerCommand::MaintainIsolatedExternalPluginWorkers => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "MaintainIsolatedExternalPluginWorkers must pass"
                    )
                }
                ManagerCommand::GetState => {
                    assert!(!poison_refuses_command(true, command), "GetState must pass")
                }
                ManagerCommand::GetPosition => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "GetPosition must pass"
                    )
                }
                ManagerCommand::GetPluginData(_) => {
                    assert!(
                        !poison_refuses_command(true, command),
                        "GetPluginData must pass"
                    )
                }
                ManagerCommand::ReloadConfig => {
                    assert!(
                        poison_refuses_command(true, command),
                        "ReloadConfig must refuse"
                    )
                }
                ManagerCommand::Shutdown => {
                    assert!(!poison_refuses_command(true, command), "Shutdown must pass")
                }
            }
        }
        assert_census(&ManagerCommand::Play(test_source()));
        assert_census(&ManagerCommand::PlayAt(test_source(), 1.0));
        assert_census(&ManagerCommand::Pause);
        assert_census(&ManagerCommand::Resume);
        assert_census(&ManagerCommand::Stop);
        assert_census(&ManagerCommand::Seek(1.0));
        assert_census(&ManagerCommand::QueueNext(test_source()));
        assert_census(&ManagerCommand::CancelNext);
        assert_census(&ManagerCommand::SetVolume(0.5));
        assert_census(&ManagerCommand::Mute(true));
        assert_census(&ManagerCommand::UpdatePluginChain(Vec::new()));
        assert_census(&ManagerCommand::UpdatePluginGraph(PluginGraphConfig {
            nodes: Vec::new(),
            edges: Vec::new(),
        }));
        assert_census(&ManagerCommand::SetPluginParameter {
            plugin_index: 0,
            param_id: String::new(),
            value: String::new(),
        });
        assert_census(&ManagerCommand::BypassProcessing(true));
        assert_census(&ManagerCommand::LinearPhaseEqRequest {
            plugin_index: 0,
            band_index: 0,
            new_band: test_band(),
        });
        assert_census(&ManagerCommand::LinearPhaseEqCancel { plugin_index: 0 });
        assert_census(&ManagerCommand::LinearPhaseEqStatus { plugin_index: 0 });
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        assert_census(&ManagerCommand::MaintainIsolatedExternalPluginWorkers);
        assert_census(&ManagerCommand::GetState);
        assert_census(&ManagerCommand::GetPosition);
        assert_census(&ManagerCommand::GetPluginData(0));
        assert_census(&ManagerCommand::ReloadConfig);
        assert_census(&ManagerCommand::Shutdown);
        // Unpoisoned engines refuse nothing (representative pair; the
        // predicate short-circuits on `poisoned` before matching).
        assert!(!poison_refuses_command(
            false,
            &ManagerCommand::Play(test_source())
        ));
        assert!(!poison_refuses_command(false, &ManagerCommand::Stop));
    }

    #[test]
    fn death_record_poisons_folds_residual_and_stays_honest() {
        let mut state = AudioEngineState {
            playback_state: PlaybackState::Playing,
            output_peak_linear: 0.4,
            output_clipping_detected: true,
            playback_peak_max_linear: 0.5,
            peak_record_complete: true,
            ..Default::default()
        };
        assert!(!state.worker_death_poisoned);

        record_worker_death(&mut state, "decoder", WorkerDeathKind::Panicked, Some(0.7));

        assert_eq!(state.playback_state, PlaybackState::Stopped);
        assert_eq!(state.output_peak_linear, 0.0);
        assert!(!state.output_clipping_detected);
        assert_eq!(
            state.last_error.as_deref(),
            Some("worker dead (decoder): panicked")
        );
        assert!(state.worker_death_poisoned);
        assert!(!state.peak_record_complete);
        assert_eq!(state.playback_peak_max_linear, 0.7);
    }

    #[test]
    fn death_record_without_residual_preserves_latch_and_stays_partial() {
        let mut state = AudioEngineState {
            playback_peak_max_linear: 0.5,
            peak_record_complete: true,
            ..Default::default()
        };

        record_worker_death(&mut state, "playback", WorkerDeathKind::Unrecorded, None);

        assert_eq!(state.playback_peak_max_linear, 0.5);
        assert!(!state.peak_record_complete);
        assert!(state.worker_death_poisoned);
        assert_eq!(
            state.last_error.as_deref(),
            Some("worker dead (playback): exited without recording a status")
        );
    }

    #[test]
    fn death_record_ignores_non_finite_residual() {
        let mut state = AudioEngineState {
            playback_peak_max_linear: 0.5,
            ..Default::default()
        };

        record_worker_death(
            &mut state,
            "processing",
            WorkerDeathKind::Errored,
            Some(f32::NAN),
        );

        assert_eq!(state.playback_peak_max_linear, 0.5);
        assert!(!state.peak_record_complete);
    }

    #[test]
    fn death_record_is_idempotent() {
        let mut state = AudioEngineState {
            playback_peak_max_linear: 0.5,
            peak_record_complete: true,
            ..Default::default()
        };

        record_worker_death(&mut state, "decoder", WorkerDeathKind::Panicked, Some(0.7));
        record_worker_death(&mut state, "decoder", WorkerDeathKind::Panicked, Some(0.6));

        assert!(state.worker_death_poisoned);
        assert!(!state.peak_record_complete);
        assert_eq!(state.playback_peak_max_linear, 0.7);
    }
}
