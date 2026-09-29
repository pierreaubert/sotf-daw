# Read-only engine pending-frame / generation audit

Date: 2026-09-28. Production and repository tests were not changed in this pass. Root owns the local AUD-062 Stop/Shutdown fix and tests. The blocked host DAG branch-queue rewrite is entirely outside this proposal; nothing here applies that rewrite.

## Existing identity and ordering contracts

- `types/state.rs:12-21`: AudioFrame has data, frame count, channel count and sample rate. It has no stream epoch, accepted host revision, channel semantic layout, or source ID.
- `engine/types.rs:299-316`: DecoderMessage/ProcessingMessage carry Frame, bare EndOfStream, and bare Flush. Flush is an ordered FIFO marker on the data path.
- `engine/processing_thread.rs:214-218`: host_generation increments when a replacement request is submitted. `processing_state.rs:306-316` compares it to the prepared request generation. This prevents stale prepared request commits; it is not a tag on emitted audio or an active output-format revision.
- PreparedHostUpdate records expected channel count/latency and candidate output channels/rate/latency. The processing thread acknowledges the host swap before manager `apply.rs:212-225` requests playback reconfiguration.
- `DecoderThread` command replies are correlated by its handle's queued request bookkeeping. Processing replies have request IDs and tickets. Processing Stop is currently excluded from expects_response (`processing_thread.rs:182-190`) and its handler emits no acknowledgement.
- Manager commands execute synchronously through a mutable ManagerContext. A manager-controlled decoder Stop acknowledgement establishes quiescence of that decoder producer before the next serialized Play command, provided the command succeeded. Do not infer quiescence from a timeout or from Stop merely entering the command queue.

## 1. Confirmed pending output-frame rate mismatch on host commit

`processing_state.rs` normal-frame pending-send loop (~940-968 after AUD-062) remembers only `old_channels`. A host commit retaining channel count but changing output sample rate leaves the already-rendered pending frame intact, even after the replacement commit is acknowledged. Example: input48k/output96k one-channel host, old pending frame [.5,.5]@96k, then commit empty one-channel host output48k. The old frame remains eligible for send.

Root's AUD-062 fix correctly drops pending Stop/Shutdown frames and breaks the outer loop on Shutdown. It does not address this rate-only commit case.

The proposed additive fixture `audit_same_channel_rate_commit_cannot_emit_old_clock_pending_frame` is saved in `/tmp/sotf-engine-pending-frame-repros.rs`. It reuses the real Worker, output rendezvous and decoder-recycle barrier from root's eos.rs. It is NOT applied/compiled/executed in this read-only pass. Expected current failure is old sample_rate96k after a48k commit acknowledgement.

### Already-queued frames and playback

- `playback_thread/runtime.rs:1239-1302` processes frames without comparing frame.sample_rate to active config.sample_rate.
- `playback_thread/frame_writer.rs:28-94` writes or converts channel count and ignores sample rate. A valid frame can therefore be interpreted at the wrong hardware clock.
- `playback_thread/runtime.rs:506-519` drains queued messages around a10ms sleep during reconfiguration, but processing remains active. These opportunistic drains are not a producer-quiescence barrier. New-format frames can play before playback handles its reconfigure command; an old pending frame can arrive after a drain and play on the new device configuration.
- Channel count changes discard the one pending processing frame, but already-queued old-channel frames can still be channel-converted by the playback writer during a format transition. This conversion is intentional for stable hardware-channel adaptation, so rejecting every unequal channel count would break an existing feature.
- Same-count semantic layout changes cannot be identified from AudioFrame at all. Present contracts carry counts, not speaker-role maps. This audit does not claim an independent semantic-layout bug without a specific routing case.

### Minimum scoped proposal

1. Local guard: capture pending frame's actual rate and channel count and invalidate it when the accepted output contract changes. Apply consistently to normal and drained frames. This is useful, but is insufficient alone for frames already queued to playback.
2. Add a manager-coordinated output transition barrier for rate/layout changes: playback enters hold/drop and acknowledges; processing freezes output publication or reaches a bounded block-boundary barrier; discard old pending/queued output; install the accepted host and playback configuration; release processing/playback only after both sides agree. Preserve normal same-format crossfade behavior.
3. Keep the barrier cancellable with existing tickets and explicit completion, retain owned prepared host state off callback, and fail stopped if host committed but hardware reconfiguration fails. Do not use sleeps or draining a still-live producer as proof of completion.
4. A dedicated output-format revision on data envelopes is an alternative, but must represent accepted configuration rather than the request-generation counter. Playback would need to hold or reject future-format frames until hardware is ready, and reject old-format frames after install. Bare numeric channel mismatch checks alone are not adequate because hardware downmix/upmix is supported.

## 2. Stop: FIFO Flush exists, but desktop Resume can overtake it

Correction to the initial search hypothesis: manager Stop does not directly send ProcessingStop, but decoder Stop DOES enqueue DecoderMessage::Flush before its acknowledgement (`decoder_thread/types.rs:161-168`, repeated interrupted-command paths). Processing Flush resets DSP and forwards ProcessingMessage::Flush (`processing_state.rs:1131+`). Thus omitting ProcessingStop by itself is not evidence of a stale tail escaping normal ordered flush completion.

The real problematic ordering is across separate control/data channels:

1. Old decoder/processed frames are queued; decoder Stop stops its source and enqueues Flush behind them, then acknowledges.
2. Manager posts Playback Stop, setting `DroppingUntilFlush` and requesting hardware-ring flush (`playback_thread/runtime.rs:432-440`).
3. A subsequent serialized Play enqueues another decoder Flush, opens the new source, and acknowledges. Manager posts Playback Resume (`manager_thread/commands/play.rs:22-29`).
4. Desktop Resume changes `flush_mode` directly to Normal or WaitingForDrain based only on the callback ring state (`runtime.rs:416-423`). It does not wait for the data-path Flush marker.
5. Once the hardware ring is empty, old processing messages queued ahead of the Stop/Play Flush markers can be accepted as normal audio (`handle_frame:1240+`). No epoch distinguishes them. The existing device-free playback fuzzer has no Resume operation (`playback_runtime_harness.rs:302-318`), so it cannot expose this ordering.

This is a source-proven schedule, not a newly executed hardware test. A deterministic playback regression should enqueue control Stop and Resume while processing-message queue contains old sentinel frame, Flush, then new sentinel frame; drive an empty/drained mock ring. The old sentinel must never reach the ring. Test both orderings of callback completion relative to Resume and Flush.

### Queued decoder frames after direct ProcessingStop

The current local Stop reset only cancels the current interrupted output frame. The next already-queued DecoderMessage::Frame reactivates processing (`processing_state.rs:834-835`) and is emitted. `/tmp/sotf-engine-pending-frame-repros.rs` includes an unapplied real-worker fixture with a queued .6 frame before Stop and a fresh .75 frame after the command barrier. It specifies the proposed quiesced-producer Stop semantics and currently should see .6 first. Root's new .5-pending/.75-new regression covers the current frame, not this additional queue.

### Minimum ordered Stop protocol without pervasive epochs

The manager's serialized command path permits a smaller architecture than adding stream tags everywhere:

1. Tell playback to enter drop/hold early, before waiting on decoder. Decoder Stop currently uses a blocking Flush send; holding a full pipeline without a dropping consumer can prevent its acknowledgement.
2. Stop decoder and wait for successful acknowledgement. At this point the sole managed producer is quiescent and its pre-stop Flush has been enqueued. New Play must not run until the whole Stop transaction finishes.
3. Processing Stop barrier cancels its unsent normal/drain output, recycles all queued decoder frames and consumes stale EOS/Flush, resets DSP, and acknowledges only once it can no longer emit old stream output. This needs a real acknowledged barrier, not the current fire-and-forget Stop handler.
4. Playback Stop barrier discards its pending frame and drains queued processed frames/markers while processing is quiescent, requests callback ring flush, and acknowledges only after the flush completes outside an active callback. It stays stopped/held until explicit new-start release.
5. Only then publish manager Stop completion/allow Play. Timeout/failure must not be treated as quiescence or permit silent resume. Device shutdown may need a bounded error path if callbacks have stopped.

This contract is specific to a quiesced managed producer. Arbitrary concurrent producers or independent public low-level callers would require explicit stream identity or stronger API restrictions. It should preserve FIFO gapless playback that intentionally avoids Flush between compatible tracks.

## 3. Bypassing a resampling chain permanently changes the stream rate

Separate concrete correctness gap, requiring coordinated design:

- `processing_state.rs:143-157` returns input frame count/rate whenever chain bypass is enabled, even when the active chain previously emitted another rate.
- `manager_thread/commands/bypass_processing.rs:12-34` waits for a generic processing Ok, updates only `processing_bypassed`, and never reconfigures playback.
- Therefore a48→96k chain switched to bypass produces48k frames while hardware can remain96k indefinitely. This is not only a pending-frame race. The inverse unbypass is also affected.

Minimum choices: preserve the active output-rate contract during bypass using a prepared conversion/routing path, or treat bypass/unbypass as a coordinated output-format transaction using the same barrier above. No heuristic sample relabeling. Add an actual processing/output contract test with unequal input/output rates and a hardware-free playback observer; assert exact durations, not only finite samples.

## Desktop versus iOS

- Desktop combines stop and pause in one FlushMode and Resume overwrites it. This enables the Stop/Resume overtaking schedule above.
- iOS `playback_thread_stub/audio_unit_handle.rs:235-246,305-326` retains separate `flush_dropping` and `pause_dropping`; Resume clears the pause state after callback flush but leaves `flush_dropping` until ProcessingMessage::Flush (`384+`). Do not claim the same Resume bug there without a separate reproduction.
- iOS still has no rate validation on received frames (`335+`). It rejects runtime reconfiguration to a different rate/count, unlike desktop; any coordinated bypass/host change must fail safely before incompatible audio reaches RemoteIO, or require engine rebuild.
- iOS does not wait for callback flush completion after clearing `flush_dropping` on a stream Flush before accepting following frames; fresh frames could be discarded by an outstanding callback flush. This is a distinct candidate requiring a callback-interleaving regression, not proven by the desktop fixture.

## Compressor and Denoiser review

Skimmed the root getter/setter diffs without new tests or edits. No concrete new compatibility regression found:

- Compressor borrowed validation uses the same cached schema and then existing inherent setter; special range/hold path remains, dynamic band getter remains gated by declared IDs.
- Denoiser moves the existing bulk body to a borrowed iterator, retains structural prechecks and param_bridge clamp/coercion behavior, then refreshes29 primitive cached values in place. Profile triggers can affect other values, so full primitive refresh preserves the prior schema-rebuild value effect.
- Native changed-value matrix from the preceding completed checkpoint already exercised both compressor exports and Denoiser across all exported realtime primitive controls. This skim does not claim additional waveform or lifecycle coverage.

## Verification and scope status

Read-only source evidence only during this task; no production files or repository tests changed, no Cargo tests or broad runs started, and no host DAG queues touched. Two concrete additive real-worker fixture drafts are saved but deliberately uncompiled/unapplied pending protocol review. Root owns existing AUD-062 implementation/gate and main AUDIT.md. A temporary disk-space issue prevented several read-only shell launches; no files were deleted by this agent, and report writing resumed after root recovered space.
