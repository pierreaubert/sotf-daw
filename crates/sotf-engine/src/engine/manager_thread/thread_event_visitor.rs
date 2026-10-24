//! Visitor dispatch for [`ThreadEvent`]s arriving on the manager thread.
//!
//! This replaces the repetitive `let mut new_state = (**state.load()).clone(); ... state.store(...)`
//! blocks in `handle_thread_event` with focused per-event methods.

use crate::decoder::AudioSource;
use crate::engine::{
    AudioEngineState, DecodeAttempt, DecoderAsyncError, PlaybackState, ThreadEvent,
};
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Snapshot of playback-thread statistics delivered by [`ThreadEvent::PlaybackStats`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaybackStatsSnapshot {
    pub output_generation: u64,
    pub callback_count: u64,
    pub buffer_fill_percent: u64,
    pub stream_error_count: u64,
    pub frames_received: u64,
    pub frames_written: u64,
    pub frames_dropped: u64,
    pub effective_sample_rate: u64,
    /// Playback-side stream generation; applied only on epoch match.
    pub epoch: u64,
}

/// Visitor interface for [`ThreadEvent`] variants.
///
/// Default implementations are no-ops so implementors only override the events
/// they care about.
pub trait ThreadEventVisitor {
    fn decoder_end_of_stream(&mut self, _state: &mut AudioEngineState) {}
    fn decoder_gapless_transition(&mut self, _state: &mut AudioEngineState, _source: AudioSource) {}
    fn decoder_error(&mut self, _state: &mut AudioEngineState, _err: String) {}
    fn stream_metadata_changed(
        &mut self,
        _state: &mut AudioEngineState,
        _metadata: Option<crate::engine::StreamMetadata>,
    ) {
    }
    fn playback_channels_changed(&mut self, _state: &mut AudioEngineState, _channels: usize) {}
    fn playback_output_device_changed(&mut self, _state: &mut AudioEngineState, _device: String) {}
    fn playback_output_device_cleared(&mut self, _state: &mut AudioEngineState) {}
    fn playback_output_access_changed(
        &mut self,
        _state: &mut AudioEngineState,
        _status: crate::OutputAccessStatus,
    ) {
    }
    fn playback_stats(&mut self, _state: &mut AudioEngineState, _stats: &PlaybackStatsSnapshot) {}
    fn playback_output_meter(
        &mut self,
        _state: &mut AudioEngineState,
        _peak_linear: f32,
        _clipping_detected: bool,
        _epoch: u64,
    ) {
    }
    fn playback_drained(
        &mut self,
        _state: &mut AudioEngineState,
        _epoch: u64,
        _epoch_peak_max: f32,
        _flush_gen: u64,
    ) {
    }
    fn playback_underrun(&mut self, _state: &mut AudioEngineState, _underruns: u64) {}
    fn processing_error(&mut self, _state: &mut AudioEngineState, _err: String) {}
    fn processing_warning(&mut self, _state: &mut AudioEngineState, _warning: String) {}
    fn thread_panic(&mut self, _state: &mut AudioEngineState, _thread_name: String) {}
    fn position_update(&mut self, _state: &mut AudioEngineState, _position: f64) {}
    fn seek_complete(&mut self, _state: &mut AudioEngineState) {}
    fn plugin_latency_update(&mut self, _state: &mut AudioEngineState, _latency_samples: usize) {}
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn isolated_external_plugin_worker_statuses(
        &mut self,
        _state: &mut AudioEngineState,
        _statuses: Vec<crate::engine::IsolatedExternalPluginWorkerStatus>,
    ) {
    }
}

/// Dispatch an event to a visitor, mutating the provided state in place.
pub fn visit<V: ThreadEventVisitor>(
    event: ThreadEvent,
    state: &mut AudioEngineState,
    visitor: &mut V,
) {
    match event {
        ThreadEvent::DecoderTagged { attempt, event } => {
            if attempt == state.decoder_attempt {
                visit(*event, state, visitor);
            }
        }
        ThreadEvent::DecoderEndOfStream => visitor.decoder_end_of_stream(state),
        ThreadEvent::DecoderGaplessTransition(source) => {
            visitor.decoder_gapless_transition(state, source)
        }
        ThreadEvent::DecoderError(err) => visitor.decoder_error(state, err),
        ThreadEvent::StreamMetadataChanged(metadata) => {
            visitor.stream_metadata_changed(state, metadata)
        }
        ThreadEvent::PlaybackOutputChanged {
            incident,
            generation,
            channels,
            device,
            access,
        } => {
            if adopt_output_generation(state, generation) {
                adopt_output_incident(state, incident, generation);
                visitor.playback_channels_changed(state, channels);
                // A complete snapshot also clears an obsolete device name when absent.
                if let Some(device) = device {
                    visitor.playback_output_device_changed(state, device);
                } else {
                    visitor.playback_output_device_cleared(state);
                }
                visitor.playback_output_access_changed(state, access);
            }
        }
        ThreadEvent::PlaybackStats {
            output_generation,
            callback_count,
            buffer_fill_percent,
            stream_error_count,
            frames_received,
            frames_written,
            frames_dropped,
            effective_sample_rate,
            epoch,
        } => visitor.playback_stats(
            state,
            &PlaybackStatsSnapshot {
                output_generation,
                callback_count,
                buffer_fill_percent,
                stream_error_count,
                frames_received,
                frames_written,
                frames_dropped,
                effective_sample_rate,
                epoch,
            },
        ),
        ThreadEvent::PlaybackOutputMeter {
            peak_linear,
            clipping_detected,
            epoch,
        } => visitor.playback_output_meter(state, peak_linear, clipping_detected, epoch),
        ThreadEvent::PlaybackDrained {
            epoch,
            epoch_peak_max,
            flush_gen,
        } => visitor.playback_drained(state, epoch, epoch_peak_max, flush_gen),
        ThreadEvent::PlaybackUnderrun(underruns) => visitor.playback_underrun(state, underruns),
        ThreadEvent::ProcessingError(err) => visitor.processing_error(state, err),
        ThreadEvent::ProcessingWarning(warning) => visitor.processing_warning(state, warning),
        ThreadEvent::ThreadPanic(thread_name) => visitor.thread_panic(state, thread_name),
        ThreadEvent::PositionUpdate(position) => visitor.position_update(state, position),
        ThreadEvent::SeekComplete => visitor.seek_complete(state),
        ThreadEvent::PluginLatencyUpdate(latency_samples) => {
            visitor.plugin_latency_update(state, latency_samples)
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        ThreadEvent::IsolatedExternalPluginWorkerStatuses(statuses) => {
            visitor.isolated_external_plugin_worker_statuses(state, statuses)
        }
    }
}

/// Merge the authoritative incident record carried by an output snapshot.
/// Missing records are not resolution; older receipts cannot clear newer faults.
fn adopt_output_incident(
    state: &mut AudioEngineState,
    incident: Option<crate::PlaybackIncident>,
    generation: u64,
) {
    let Some(incident) = incident else { return };
    if incident.id == 0
        || incident.stream_generation == 0
        || incident.stream_generation > generation
    {
        return;
    }
    if let Some(resolution) = incident.resolution
        && (resolution.output_generation > generation
            || resolution.output_generation < incident.stream_generation
            || resolution.callback_count == 0)
    {
        return;
    }
    if let Some(current) = state.playback_incident.as_ref() {
        if current.id > incident.id {
            return;
        }
        if current.id == incident.id
            && (current.stream_generation != incident.stream_generation
                || (current.resolution.is_some() && incident.resolution.is_none()))
        {
            return;
        }
    }
    // Resolution carries immediate callback proof, avoiding a wait for the
    // periodic statistics cadence. A receipt from a previous stream remains
    // useful incident history but never seeds activity for a replacement.
    if let Some(resolution) = incident.resolution
        && resolution.output_generation == generation
    {
        state.playback_callback_count =
            state.playback_callback_count.max(resolution.callback_count);
    }
    state.playback_incident = Some(incident);
}

/// Fence queued events as soon as a synchronous output commit is acknowledged.
/// Historical callback activity is not evidence of flow from the replacement.
pub(super) fn adopt_output_generation(state: &mut AudioEngineState, generation: u64) -> bool {
    if generation == 0 || generation < state.playback_output_generation {
        return false;
    }
    if generation != state.playback_output_generation {
        state.playback_callback_count = 0;
        state.playback_buffer_fill_percent = 0;
        state.playback_stream_error_count = 0;
        state.playback_effective_sample_rate = 0;
        state.playback_output_generation = generation;
    }
    true
}

/// Visitor that applies event mutations to the shared [`AudioEngineState`].
pub struct AudioEngineStateUpdater;

pub(crate) fn reset_output_meter(state: &mut AudioEngineState) {
    state.output_peak_linear = 0.0;
    state.output_clipping_detected = false;
    // playback_peak_max_linear is intentionally preserved here: it holds the
    // epoch maximum until the next Play/PlayAt starts a new epoch.
}

/// Record a decoder failure into transport state.
///
/// Shared by the DecoderError event arm and the PlayAt command's
/// ack-failure path, so a known decoder failure is always visible in
/// state without depending on the lossy event channel.
pub(crate) fn record_decoder_error(state: &mut AudioEngineState, err: String) {
    log::debug!("[Manager Thread] Decoder error: {}", err);
    state.playback_state = PlaybackState::Stopped;
    reset_output_meter(state);
    state.last_error = Some(err);
}

/// Select the drained async failures belonging to the current attempt.
///
/// Pure over the drained vec (the channel drain itself is thin
/// handle wiring): tag-equal messages yield their text in drain
/// order; older tags belong to superseded sessions and are dropped
/// with a warn log (moot — a newer attempt owns the session), never
/// applied. A tag AHEAD of current is impossible by construction
/// (adoption causally follows assignment via the command itself)
/// and is dropped with the same loud log: unlike a future drain
/// generation — where rejection would wedge transport — dropping an
/// impossible error tag loses nothing, because the session it names
/// cannot exist manager-side. Shared by the manager tick and the
/// sync cause-chain paths — single rule, two call sites.
pub(crate) fn select_current_decoder_errors(
    errors: Vec<DecoderAsyncError>,
    current: DecodeAttempt,
) -> Vec<String> {
    let mut selected = Vec::new();
    for error in errors {
        if error.attempt == current {
            selected.push(error.message);
        } else {
            log::warn!(
                "[Manager Thread] Dropping async decoder error for attempt {} (current {}): {}",
                error.attempt,
                current,
                error.message
            );
        }
    }
    selected
}

/// Apply selected async failures to transport state.
///
/// Each message records in drain order (last wins in `last_error`;
/// every message is warn-logged at selection, so none vanishes
/// silently). Applies into any transport sub-state — including an
/// already-Stopped one, where the record is informative, not wrong —
/// EXCEPT a poisoned one: the tag stays current forever (no new Play
/// while poisoned), so every queued pre-death symptom would apply.
/// On poison each message is warn-logged and skipped, preserving the
/// death record. Returns whether anything applied (the caller skips
/// its store when nothing did).
pub(crate) fn apply_decoder_async_errors(
    state: &mut AudioEngineState,
    messages: &[String],
) -> bool {
    if state.worker_death_poisoned {
        for message in messages {
            log::warn!(
                "[Manager Thread] Dropping async decoder error on poisoned transport: {message}"
            );
        }
        return false;
    }
    let mut applied = false;
    for message in messages {
        record_decoder_error(state, message.clone());
        applied = true;
    }
    applied
}

/// Chain drained async causes behind a sync decoder failure.
///
/// Identity on empty causes (sync-only failure, unchanged text);
/// otherwise `"{sync} (decoder stopped: {cause[; cause...]})"` so
/// the sync symptom and the async root cause stay visible together.
/// Root-cause preservation for failed-seek-after-error cascades.
pub(crate) fn chain_decoder_causes(message: String, causes: &[String]) -> String {
    if causes.is_empty() {
        message
    } else {
        format!("{message} (decoder stopped: {})", causes.join("; "))
    }
}

impl ThreadEventVisitor for AudioEngineStateUpdater {
    fn decoder_end_of_stream(&mut self, _state: &mut AudioEngineState) {
        log::debug!("[Manager Thread] Decoder end of stream (waiting for playback drain)");
    }

    fn decoder_gapless_transition(&mut self, state: &mut AudioEngineState, source: AudioSource) {
        log::info!(
            "[Manager Thread] Gapless transition to: {}",
            source.display_name()
        );
        state.current_file = source.as_path().map(|p| p.to_path_buf());
        state.current_source = Some(source);
        state.position = 0.0;
    }

    fn decoder_error(&mut self, state: &mut AudioEngineState, err: String) {
        record_decoder_error(state, err);
    }

    fn stream_metadata_changed(
        &mut self,
        state: &mut AudioEngineState,
        metadata: Option<crate::engine::StreamMetadata>,
    ) {
        state.stream_metadata = metadata;
    }

    fn playback_channels_changed(&mut self, state: &mut AudioEngineState, channels: usize) {
        state.playback_channels = channels;
    }

    fn playback_output_device_changed(&mut self, state: &mut AudioEngineState, device: String) {
        state.playback_output_device = Some(device);
    }

    fn playback_output_device_cleared(&mut self, state: &mut AudioEngineState) {
        state.playback_output_device = None;
    }

    fn playback_output_access_changed(
        &mut self,
        state: &mut AudioEngineState,
        status: crate::OutputAccessStatus,
    ) {
        state.output_access_status = status;
    }

    fn playback_stats(&mut self, state: &mut AudioEngineState, stats: &PlaybackStatsSnapshot) {
        if stats.epoch != state.playback_epoch
            || stats.output_generation != state.playback_output_generation
        {
            log::debug!(
                "[Manager Thread] Dropping stale stats for epoch {} / output {} (current {} / {})",
                stats.epoch,
                stats.output_generation,
                state.playback_epoch,
                state.playback_output_generation
            );
            return;
        }
        state.playback_callback_count = stats.callback_count;
        state.playback_buffer_fill_percent = stats.buffer_fill_percent;
        state.playback_stream_error_count = stats.stream_error_count;
        state.playback_frames_received = stats.frames_received;
        state.playback_frames_written = stats.frames_written;
        state.playback_frames_dropped = stats.frames_dropped;
        state.playback_effective_sample_rate = stats.effective_sample_rate;
    }

    fn playback_output_meter(
        &mut self,
        state: &mut AudioEngineState,
        peak_linear: f32,
        clipping_detected: bool,
        epoch: u64,
    ) {
        if state.playback_state == PlaybackState::Stopped {
            return;
        }
        if epoch != state.playback_epoch {
            log::debug!(
                "[Manager Thread] Dropping stale meter for epoch {epoch} (current {})",
                state.playback_epoch
            );
            return;
        }
        state.output_peak_linear = peak_linear;
        state.output_clipping_detected = clipping_detected;
        state.playback_peak_max_linear = state.playback_peak_max_linear.max(peak_linear);
    }

    fn playback_drained(
        &mut self,
        state: &mut AudioEngineState,
        epoch: u64,
        epoch_peak_max: f32,
        flush_gen: u64,
    ) {
        if epoch != state.playback_epoch {
            log::debug!(
                "[Manager Thread] Dropping stale drained receipt for epoch {epoch} (current {})",
                state.playback_epoch
            );
            return;
        }
        // Peak recovery is unconditional on epoch match: a receipt born
        // before a seek Flush still carries real per-epoch audio, and
        // max-folding composes with Stop acks in any arrival order.
        state.playback_peak_max_linear = state.playback_peak_max_linear.max(epoch_peak_max);
        if flush_gen < state.flushes_sent {
            // Born before a newer Flush (pre-seek drain racing the
            // boundary): transport stays live, but the peak above is
            // already recovered. A later receipt completes the epoch.
            log::debug!(
                "[Manager Thread] Drained receipt predates Flush {} (born at {}); transport held",
                state.flushes_sent,
                flush_gen
            );
            return;
        }
        if flush_gen > state.flushes_sent {
            // Impossible by construction (playback cannot consume a
            // Flush the manager never sent); degrade to the epoch-only
            // gate rather than wedging transport on a counting bug.
            // Policy: availability over integrity — rejecting here
            // would brick every EOF on a backstop path, while the
            // peak above was already recovered, so the integrity loss
            // is confined to transport-state timing. The count makes
            // the degradation state-visible (suite-pinned zero) rather
            // than log-only.
            state.gen_ahead_events = state.gen_ahead_events.wrapping_add(1);
            log::warn!(
                "[Manager Thread] Drained receipt gen {flush_gen} exceeds sent {}; accepting",
                state.flushes_sent
            );
        }
        log::debug!("[Manager Thread] Playback drained - all audio played");
        state.playback_state = PlaybackState::Stopped;
        reset_output_meter(state);
        // A drain confirms transport completion, not recovery from a
        // processing failure. The next committed play clears last_error.
    }

    fn playback_underrun(&mut self, state: &mut AudioEngineState, underruns: u64) {
        state.underruns = underruns;
        if underruns == 1 || (underruns <= 1000 && underruns.is_multiple_of(100)) {
            log::warn!("[Manager Thread] Playback underrun count: {}", underruns);
        } else if underruns.is_multiple_of(10000) {
            log::debug!("[Manager Thread] Playback underrun count: {}", underruns);
        }
    }

    fn processing_error(&mut self, state: &mut AudioEngineState, err: String) {
        log::debug!("[Manager Thread] Processing error: {}", err);
        state.playback_state = PlaybackState::Stopped;
        reset_output_meter(state);
        state.last_error = Some(err);
    }

    fn processing_warning(&mut self, state: &mut AudioEngineState, warning: String) {
        log::warn!("[Manager Thread] Processing warning: {}", warning);
        // Death-text preservation: the warning text is preserved in the
        // log above; it must not overwrite a death record in state.
        if !state.worker_death_poisoned {
            state.last_error = Some(warning);
        }
    }

    fn thread_panic(&mut self, state: &mut AudioEngineState, thread_name: String) {
        log::debug!("[Manager Thread] Thread panicked: {}", thread_name);
        state.playback_state = PlaybackState::Stopped;
        reset_output_meter(state);
        state.last_error = Some(format!("Thread panicked: {}", thread_name));
    }

    fn position_update(&mut self, state: &mut AudioEngineState, position: f64) {
        if state.playback_state != PlaybackState::Stopped && !state.seeking {
            let latency_sec = if state.sample_rate > 0
                && state.latency_compensation_enabled
                && !state.processing_bypassed
            {
                state.plugin_latency_samples as f64 / state.sample_rate as f64
            } else {
                0.0
            };
            state.position = (position - latency_sec).max(0.0);
        }
    }

    fn seek_complete(&mut self, state: &mut AudioEngineState) {
        log::debug!("[Manager Thread] Seek complete");
        state.seeking = false;
        state.seeking_since = None;
    }

    fn plugin_latency_update(&mut self, state: &mut AudioEngineState, latency_samples: usize) {
        let old_latency = state.plugin_latency_samples;
        state.plugin_latency_samples = latency_samples;
        if state.sample_rate > 0
            && state.latency_compensation_enabled
            && old_latency != latency_samples
        {
            let delta_sec =
                (latency_samples as f64 - old_latency as f64) / state.sample_rate as f64;
            state.position = (state.position - delta_sec).max(0.0);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn isolated_external_plugin_worker_statuses(
        &mut self,
        state: &mut AudioEngineState,
        statuses: Vec<crate::engine::IsolatedExternalPluginWorkerStatus>,
    ) {
        state.isolated_external_plugin_worker_statuses = statuses;
    }
}

/// Convenience entry point used by `handle_thread_event`.
#[cfg(test)]
pub fn update_state_with_event(event: ThreadEvent, state: &Arc<ArcSwap<AudioEngineState>>) {
    update_state_with_events(std::iter::once(event), state);
}

/// Apply ordered events and publish one snapshot for the manager tick.
///
/// No events are discarded: lifecycle, error and telemetry mutations all run
/// in receive order against the same state. An empty batch leaves the current
/// Arc untouched. The caller bounds the batch to preserve command responsiveness.
/// Returns the number of events applied.
pub fn update_state_with_events(
    events: impl IntoIterator<Item = ThreadEvent>,
    state: &Arc<ArcSwap<AudioEngineState>>,
) -> usize {
    let mut events = events.into_iter();
    let Some(first) = events.next() else {
        return 0;
    };
    let mut new_state = (**state.load()).clone();
    let mut updater = AudioEngineStateUpdater;
    let mut count = 0;
    for event in std::iter::once(first).chain(events) {
        visit(event, &mut new_state, &mut updater);
        count += 1;
    }
    state.store(Arc::new(new_state));
    count
}

/// Clear a seeking indicator older than the display timeout.
///
/// Called once per manager tick. Drain safety never reads the flag;
/// this only bounds how long a dropped SeekComplete can freeze the
/// position display.
pub fn expire_seeking_display(state: &Arc<ArcSwap<AudioEngineState>>) {
    let current = state.load();
    let expired = match (current.seeking, current.seeking_since) {
        (true, Some(since)) => {
            since.elapsed()
                > std::time::Duration::from_millis(super::consts::SEEKING_DISPLAY_TIMEOUT_MS)
        }
        (true, None) => true,
        (false, _) => false,
    };
    if !expired {
        return;
    }
    let mut new_state = (**current).clone();
    new_state.seeking = false;
    new_state.seeking_since = None;
    state.store(Arc::new(new_state));
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    #[ignore = "release snapshot publication measurement; run explicitly with --nocapture"]
    fn audit_publication_timing() {
        use std::time::Instant;
        let initial = AudioEngineState {
            current_file: Some(std::path::PathBuf::from("synthetic-track.wav")),
            playback_state: PlaybackState::Playing,
            ..AudioEngineState::default()
        };
        for batch in [false, true] {
            let state = Arc::new(ArcSwap::from_pointee(initial.clone()));
            let mut times = Vec::with_capacity(1000);
            for tick in 0..1000 {
                let events =
                    (0..256).map(|event| ThreadEvent::PositionUpdate((tick * 256 + event) as f64));
                let start = Instant::now();
                if batch {
                    update_state_with_events(events, &state);
                } else {
                    for event in events {
                        update_state_with_event(event, &state);
                    }
                }
                times.push(start.elapsed().as_nanos() as u64);
            }
            assert_eq!(state.load().position, 255999.0);
            times.sort_unstable();
            let p = |n: usize| times[((times.len() - 1) * n).div_ceil(1000)];
            println!(
                "{}",
                serde_json::json!({"workload":"manager_publication", "batched":batch,
                "ticks":1000, "events_per_tick":256, "p50_ns":p(500), "p95_ns":p(950),
                "p99_ns":p(990), "p999_ns":p(999), "max_ns":times.last()})
            );
        }
    }

    #[test]
    fn batch_matches_sequential_order_and_empty_batch_keeps_snapshot() {
        let initial = AudioEngineState {
            playback_state: PlaybackState::Playing,
            sample_rate: 48_000,
            ..AudioEngineState::default()
        };
        let batch = Arc::new(ArcSwap::from_pointee(initial.clone()));
        let sequential = Arc::new(ArcSwap::from_pointee(initial));
        let events = || {
            [
                ThreadEvent::PositionUpdate(1.0),
                ThreadEvent::PluginLatencyUpdate(4800),
                ThreadEvent::PositionUpdate(4.0),
                ThreadEvent::PlaybackUnderrun(3),
                ThreadEvent::ProcessingError("failed".into()),
            ]
        };
        for event in events() {
            update_state_with_event(event, &sequential);
        }
        assert_eq!(update_state_with_events(events(), &batch), 5);
        let actual = batch.load_full();
        let expected = sequential.load_full();
        assert_eq!(actual.position, expected.position);
        assert_eq!(actual.position, 3.9);
        assert_eq!(actual.last_error, expected.last_error);
        assert_eq!(actual.playback_state, expected.playback_state);
        let before = batch.load_full();
        assert_eq!(update_state_with_events([], &batch), 0);
        assert!(Arc::ptr_eq(&before, &batch.load_full()));
    }
}

#[cfg(test)]
mod output_generation_tests {
    use super::*;

    fn output(
        generation: u64,
        channels: usize,
        device: Option<&str>,
        access: crate::OutputAccessStatus,
    ) -> ThreadEvent {
        ThreadEvent::PlaybackOutputChanged {
            incident: None,
            generation,
            channels,
            device: device.map(str::to_owned),
            access,
        }
    }

    #[test]
    fn committed_stream_fence_rejects_queued_retired_output_snapshots() {
        // The synchronous reconfiguration receipt has already committed stream 42,
        // but old notifications are still waiting in the asynchronous event queue.
        let shared = Arc::new(ArcSwap::from_pointee(AudioEngineState {
            playback_output_generation: 42,
            playback_channels: 8,
            playback_output_device: Some("Current interface".into()),
            output_access_status: crate::OutputAccessStatus::ExclusiveActive,
            ..AudioEngineState::default()
        }));
        update_state_with_events(
            [
                output(
                    41,
                    2,
                    Some("Retired speakers"),
                    crate::OutputAccessStatus::Shared,
                ),
                output(0, 1, None, crate::OutputAccessStatus::Shared),
            ],
            &shared,
        );
        let current = shared.load();
        assert_eq!(current.playback_output_generation, 42);
        assert_eq!(current.playback_channels, 8);
        assert_eq!(
            current.playback_output_device.as_deref(),
            Some("Current interface")
        );
        assert_eq!(
            current.output_access_status,
            crate::OutputAccessStatus::ExclusiveActive
        );
    }

    #[test]
    fn newer_output_snapshot_replaces_the_complete_tuple_once() {
        let shared = Arc::new(ArcSwap::from_pointee(AudioEngineState::default()));
        update_state_with_events(
            [
                output(
                    10,
                    8,
                    Some("Previous interface"),
                    crate::OutputAccessStatus::ExclusiveActive,
                ),
                output(11, 2, None, crate::OutputAccessStatus::Shared),
                output(
                    10,
                    8,
                    Some("Previous interface"),
                    crate::OutputAccessStatus::ExclusiveActive,
                ),
            ],
            &shared,
        );
        let current = shared.load();
        assert_eq!(current.playback_output_generation, 11);
        assert_eq!(current.playback_channels, 2);
        assert!(current.playback_output_device.is_none());
        assert_eq!(
            current.output_access_status,
            crate::OutputAccessStatus::Shared
        );
    }
}

#[cfg(test)]
mod incident_tests {
    use super::*;
    use crate::{PlaybackIncident, PlaybackIncidentKind, PlaybackIncidentResolution};

    fn incident(id: u64, resolved: bool) -> Option<PlaybackIncident> {
        Some(PlaybackIncident {
            id,
            stream_generation: 41,
            kind: PlaybackIncidentKind::StreamError,
            message: "text is not an error classifier".into(),
            resolution: resolved.then_some(PlaybackIncidentResolution {
                output_generation: 42,
                callback_count: 9,
            }),
        })
    }

    #[test]
    fn stale_resolution_cannot_clear_newer_incident_or_unrelated_fatal_error() {
        let mut state = AudioEngineState {
            playback_output_generation: 42,
            last_error: Some("decoder failure".into()),
            ..AudioEngineState::default()
        };
        adopt_output_incident(&mut state, incident(100, false), 42);
        adopt_output_incident(&mut state, incident(101, false), 42);
        adopt_output_incident(&mut state, incident(100, true), 42);
        assert_eq!(state.playback_incident.as_ref().unwrap().id, 101);
        assert!(state.playback_incident.as_ref().unwrap().is_active());
        adopt_output_incident(&mut state, None, 42);
        assert!(state.playback_incident.as_ref().unwrap().is_active());
        adopt_output_incident(&mut state, incident(101, true), 42);
        assert!(!state.playback_incident.as_ref().unwrap().is_active());
        adopt_output_incident(&mut state, incident(101, false), 42);
        assert!(!state.playback_incident.as_ref().unwrap().is_active());
        assert_eq!(state.last_error.as_deref(), Some("decoder failure"));
    }

    #[test]
    fn invalid_resolution_receipts_and_retired_stats_do_not_prove_health() {
        let mut state = AudioEngineState {
            playback_output_generation: 42,
            playback_callback_count: 5,
            ..AudioEngineState::default()
        };
        adopt_output_incident(&mut state, incident(100, false), 42);
        let mut invalid = incident(100, true).unwrap();
        invalid.resolution.as_mut().unwrap().output_generation = 40;
        adopt_output_incident(&mut state, Some(invalid), 42);
        assert!(state.playback_incident.as_ref().unwrap().is_active());
        let mut updater = AudioEngineStateUpdater;
        updater.playback_stats(
            &mut state,
            &PlaybackStatsSnapshot {
                output_generation: 41,
                callback_count: 1000,
                ..Default::default()
            },
        );
        assert_eq!(state.playback_callback_count, 5);
        assert!(adopt_output_generation(&mut state, 43));
        assert_eq!(state.playback_callback_count, 0);
        adopt_output_incident(&mut state, incident(100, true), 43);
        assert!(!state.playback_incident.as_ref().unwrap().is_active());
        assert_eq!(state.playback_callback_count, 0);
    }
}
