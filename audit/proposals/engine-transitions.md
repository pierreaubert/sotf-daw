# Engine transition review

## Status

**Unapplied, unvalidated design draft.** The incomplete production integration was removed after automatic approval review rejected the broader manager write. The verified local processing command outcomes / format guards (AUD062) and desktop Stop/Pause/Resume stream-boundary fix (AUD064) remain in the checkout.

`protocol-draft.patch` is the complete captured desktop/processing/manager prototype plus the proposed host commit integration, relative to the restored checkout. It is not an integration-ready patch: the outstanding items below must be completed and tested after explicit authorization. Root-owned portable feeder / iOS proposal artifacts are separate in this directory and must be reconciled with the shared vocabulary before applying the draft.

The automatic reviewer rejected the proposed manager host-update integration with this reason:

> This writes broad, unvalidated production manager/engine transition logic with concurrency and playback side effects, while the trusted authorization only covered tests and isolated fixes, not production integration.

No retry or alternate production integration was performed.

## Artifacts

- `PLAN.md`: ordered protocol and failure semantics, including desktop/iOS differences.
- `protocol-draft.patch`: reviewed scope as a concrete source delta; **do not apply without the requested review**.
- `partial/`: exact source snapshots before restoring the checkpoint. The original three processing-worker repros remain byte-for-byte here.
- `proposed/`: draft source with the known mpsc error destructuring corrected, Resume ordered before release, and two repro expectations changed to the proposed acknowledged APIs.
- `manager_apply.proposed.rs`: proposed pre-commit hold and post-commit configure/release for both linear and graph host updates.
- `restore_plugin_chain_protocol.py`: record of narrowly reversing only this agent's incomplete additions. It preserved root's `PreparedTransitionDelay::reset`, `CommandOutcome`, pending-frame format guards, and the six desktop DrainState tests. Do not rerun it.

## Verified evidence

- `target/audit-engine-protocol-red.log`: three real processing-worker regressions failed before the local fixes. An exact 2x rate probe emitted old 96 kHz data after a 48 kHz host commit; a bare Stop left an old queued decoder frame; bypass acknowledged generic Ok while changing output from 96 to 48 kHz.
- The rate-only pending-frame problem is independently fixed by root and covered in the live EOS suite. The broader Stop/barrier and bypass reproductions are saved here and removed from the live suite; no ignore was added.
- `target/audit-playback-stop-resume-red.log`: two tests using extracted production DrainState and actual ring writing exposed old audio after Stop -> Resume before the FIFO Flush marker.
- `target/audit-playback-stop-resume-green.log`: 45 desktop playback tests passed, including all six new Stop/Pause/Resume/callback-order regressions.
- Restored processing verification was attempted after all transition source was removed. The first attempt encountered still-being-restored feeder tests. The second encountered another agent's in-progress missing Gate module. Neither is claimed as a passing restored gate; a final rerun is pending.

## Affected files and order

1. Shared `engine/types.rs` adds correlated transition vocabulary; `engine/output_transition.rs` supplies scalar source-format acceptance and matching-ID state.
2. `processing_thread.rs` tracks manager transition state. `processing_state.rs` suspends at the call site that owns pending output, preserving EOS/Flush for `RetainInput`; only `DiscardQuiescedStream` retires input after decoder Stop ACK. A raw low-level Stop remains distinct from this barrier.
3. Desktop `playback_thread.rs` and `runtime.rs` hold acceptance, drain quiescent output, flush callback-visible samples, and configure the source/hardware contract before release. Queue EOS is preserved unless the manager explicitly discards the stopped stream.
4. Manager Stop orders playback Hold ACK -> decoder Stop ACK -> processing Discard ACK -> playback Configure/flush ACK. A successful stop remains held with an empty decoder queue.
5. Host format changes order playback Hold -> processing Retain -> host commit -> playback Configure -> playback Release ACK -> processing Release. Same-format host crossfades retain the existing path. Compatible gapless QueueNext is unchanged.
6. Bypass uses actual output metadata; it never relabels samples. Play following successful Stop enqueues its initial Flush into the proven-empty decoder queue, orders Resume while playback is held, then releases playback before processing.
7. Root-owned iOS/portable feeder must implement the same messages or reject unsupported rate/channel changes while held. Native iOS behavior has not been tested on this Linux host.

## Required completion before validation

- Add the bounded initial-playback-hold failure path: attempt decoder quiescence/teardown, publish an explicit error and forbid new starts. Do not claim hardware silence if its hold ACK is missing. The draft's current Stop error is descriptive but does not yet perform this attempt.
- Ensure every failed host-commit/configuration path publishes a stopped/error state. A held transition is marked incomplete and new Play/Resume must require rebuild; this draft does not provide automatic rollback.
- Coordinate existing parameter-triggered format changes (`commands/set_plugin_parameter.rs`) with the new barrier, including source-width changes that leave limited hardware width unchanged. The draft host-commit and bypass callsites alone do not complete that path.
- Integrate root's portable/iOS feeder proposal and remove duplicate type/helper implementations if any.
- Finalize all real-worker fixtures against the explicit APIs; original bare-Stop and old-clock bypass expectations are historical red evidence, not the new contract.
- No claims about zero-allocation control transactions, native hardware transitions, or full workspace readiness follow from the draft.

## Required tests

- Saturated decoder and output queues, delayed callback flush, already-stopped Play with initial Flush capacity, and interruption during normal audio, tail audio, zero-output drain, pending EOS, and pending Flush.
- RetainInput EOS/Flush exact-once preservation with deterministic evidence that the marker was already consumed before the hold. Stop's discard path must clear old terminal state.
- Canceled unclaimed hold/configure/release, stale/mismatched IDs, late replies, repeated failed configuration after earlier success, and Shutdown in every held state.
- 48 -> 96 -> 48 kHz and asymmetric channel sentinels; assert frame duration and playback acceptance, not only metadata. Include A -> B -> A to prove queue ownership beyond rate checks.
- Bypass and unbypass metadata negotiation before publication; ordinary same-format edits retain crossfade and compatible gapless source transitions.
- Actual shared acceptance + playback feeder/ring tests, bounded callback flush failure, source versus hardware channel adaptation, and cold callback allocation/deallocation checks.
- Focused engine Clippy and complete workspace verification only after the above pass.

MIDI, IAMF, and the separately blocked host DAG branch-queue rewrite are excluded.
