# Proposed engine control barriers and output acceptance

Status: protocol proposal only. No changes described here are applied, except separately approved local fixes identified below. Engine data channels and host DAG queues remain unchanged.

## Current reproduction evidence

`target/audit-engine-protocol-red.log` runs three real processing-worker tests in `processing_thread/tests/protocol.rs` (independent2x rate probe, output rendezvous, decoder-buffer recycle barrier):

- Rate-only replacement acknowledges48k but emits its older pending96k frame.
- A successful bare ProcessingStop followed by a command barrier leaves already-queued decoder [.6,.6] eligible before fresh [.75,.75]. Test explicitly quiesces the decoder producer before Stop; this is the desired stronger manager barrier, not a claim that arbitrary concurrent queue draining is valid.
- Generic bypass Ok emits48k while the previously negotiated playback contract remains96k.

All three failed deterministically in0.02s before subsequent fixes. Root now owns/fixes pending format invalidation and accepted-command outcomes; the queued-stream and bypass transaction tests remain broader red specifications.

`target/audit-playback-stop-resume-red.log` runs the extracted production DrainState transitions and actual ring writer without CPAL construction. Both callback-completion orderings allowed old.5 into the ring after Stop→Resume before the FIFO Flush. The extraction patch was reviewed and applied; a narrow independent stream-pending/pause state fix and positive transition tests are under focused verification (AUD064). This does not establish a full manager stop barrier by itself.

## Minimum protocol vocabulary

Use request IDs/tickets already present. Add internal manager transaction ID separate from host_generation: that atomic identifies submitted host requests, not accepted audio stream state. No AudioFrame epoch field is required for the serialized managed path below.

1. Playback `HoldOutput { transition_id, reply }`: records a hold that FIFO Flush and ordinary Resume cannot clear, cancels/recycles the currently blocked playback frame, and acknowledges that acceptance is disabled. This acknowledgement does NOT promise its input queue/device ring is empty yet. While held it may recycle old queued frames but must remain responsive to commands.
2. Processing `HoldOutput { transition_id, discard_decoder_queue }`: acknowledged only after canceling pending normal/tail/EOS/Flush sends and reaching a block boundary from which no further output is published until release. Retain incoming decoder messages for format changes. For Stop, discard/recycle them only after manager has a successful decoder Stop acknowledgement and therefore owns a quiesced producer. Reset DSP after discarded-stream completion. A held worker receives commands with a bounded wait instead of consuming decoder audio or spinning.
3. Processing `ReleaseOutput { transition_id }`: release only the currently held matching transaction. A stale or canceled release cannot resume a later transaction. Existing host commits/parameter queries remain command-responsive while held.
4. Playback `ConfigureHeld { transition_id, source_format, requested_hardware, reply }`: validate matching hold, drain the now-quiescent processing queue, flush or rebuild the callback ring/device, and publish accepted source format only after the operation completes. Same hardware configuration still needs the queue/ring barrier; the existing early no-op reconfigure path is insufficient for a Stop boundary.
5. Playback `ReleaseOutput { transition_id, reply }`: release only the matching hold and only with an accepted format. Preserve independent pause/stream-Flush conditions. Acknowledge before processing publication is released.

These are internal protocol operations, not new user controls. Names are proposed; they can share existing request/ticket infrastructure rather than adding unrelated channels. Holds must be processed in every interrupted send branch, including zero-output drain progress and pending Flush/EOS; root's explicit command-outcome work is the basis for distinguishing claimed operations from canceled requests.

## Stop transaction: quiescence before queue removal

Manager serializes:

1. Establish Playback HoldOutput and receive its acceptance-disabled acknowledgement. This happens before decoder Stop because decoder currently uses a blocking Flush send; the consumer must be able to drop queued frames to release backpressure.
2. Send DecoderStop and receive its successful acknowledgement. Current decoder sends its FIFO Flush before that acknowledgement and no further old-source frames afterward.
3. Send Processing HoldOutput(discard_decoder_queue=true), receive boundary acknowledgement, recycle all queued decoder frames, drop stale EOS/Flush, reset all DSP. No new Play is issued while this is pending.
4. Playback ConfigureHeld with unchanged output format performs queue drain + callback flush, and acknowledges only when callback-visible old audio is absent (callback not active). Keep both holds in force; return manager Stop success only now.
5. A later successful Play loads the source/enqueues a new FIFO Flush using the existing decoder protocol. Release playback hold first, then processing hold; ordinary Resume clears pause but not the pending stream Flush. The first new decoder Flush travels before its frames and opens the fresh stream normally.

RetainInput for a configuration transition is different from DiscardStream for Stop. Draining an actively producing decoder queue is explicitly prohibited. Gapless compatible-source transitions continue to avoid Flush and do not use this Stop transaction.

## Host output-format transition

Prepare candidate host and required storage off processing thread as today. Determine candidate source format and hardware channel limit before starting the transaction.

1. Playback hold acceptance; acknowledge.
2. Processing hold publication with decoder input retained; acknowledge. Decoder may fill its existing bounded queue and backpressure, preserving accepted input for the new graph. No new branch queues are introduced.
3. Commit the candidate host while held. Its response reports actual source channels/rate/latency. No new-format frame can escape while hardware still has the old configuration.
4. ConfigureHeld drains old processed messages and configures playback. The new device/ring is ready before it acknowledges. Validate actual hardware sample rate against source rate; no implicit relabeling is allowed.
5. Release playback (matching transaction), then processing. This gives a strict causal boundary across both workers.

This changes only output-format transitions; same-format host edits retain existing normal crossfades. Rate-changing transitions may have an explicit silent interval rather than playing samples at the wrong clock. Candidate rejection before host commit can restore the old held configuration. After host commit, failures remain held/stopped and report the committed configuration plus output failure, matching existing committed-but-output-failed reporting. Do not silently resume an incompatible old device stream.

## Bypass/unbypass

Use the same coordinated format transaction if effective bypass changes source rate or routing width:

- Processing computes the candidate bypass output contract while held, without publishing audio. For current direct bypass this is input rate and the declared destination channel width.
- Manager receives actual metadata, configures held playback, then releases in the same order. The current generic Ok response is not enough to update the hardware contract.
- Do not change frame.sample_rate without converting samples. A prepared transparent resampler/routing path that preserves active output format is an alternative future design, but it requires explicit latency/history/capacity evidence and is not proposed as a quick metadata fix.
- On iOS, unsupported rate/channel reconfiguration must be rejected while both paths remain held, or require engine rebuild. Never release incompatible frames into RemoteIO.

## Explicit frame acceptance

Playback must know both the accepted *source* format and the actual hardware format. Source channel count differs intentionally from hardware count when downmix/upmix or hardware limits apply.

Before writing a frame, require:

- no active configuration hold;
- `frame.sample_rate == accepted_source.sample_rate`;
- `frame.num_channels == accepted_source.channels`;
- dimensional validation as already present.

Then use the existing prepared channel converter to adapt accepted source channels to hardware channels. Reject/recycle incompatible frames with a bounded counter/event path. Do not reject every source/hardware channel mismatch, which would disable supported routing adaptation. Hardware sample-rate mismatch needs an actual converter; a channel converter cannot repair it.

Metadata checks defend against mistakes but do not replace the ordered barrier. Same-format stale frames and A→B→A configurations have identical metadata; the quiescence and queue flush prevents them from crossing boundaries. If independently concurrent producers must later be supported, introduce explicit stream identity on internal data envelopes and markers at that point, with an acceptance policy for future as well as stale revisions.

## Failure and cancellation semantics

- Request timeout/cancellation before claim means no acknowledged barrier. Do not drain queues based on it.
- Once a hold is established, later failure leaves it established until explicit recovery/rebuild; no implicit Resume in a generic error branch. Failed transitions cannot advance the manager into Playing state.
- Match every release/configure to its transaction ID. Late replies/releases from abandoned operations must not satisfy or reopen a later transition.
- Before candidate commit, an explicit rollback can restore known old host/output state under the holds and then release in order. After candidate commit, report output failure and remain held/stopped unless a separately prepared rollback succeeds.
- If the initial playback hold acknowledgement itself fails, do not claim that hardware is silent: stop/quiesce producers and initiate bounded teardown/recovery, return an error, and prohibit new starts until output state is known. Existing command-only handles cannot promise immediate callback silence after a dead worker without an additional shared emergency gate; this proposal does not invent that guarantee.
- Shutdown remains accepted from held states and terminates outer worker loops; all pending owned buffers use existing recycle/retirement paths.

## Required deterministic regression matrix before broad enablement

- Existing real-worker pending/frame-rate and quiesced Stop cases become green through the appropriate acknowledged APIs.
- Manager sequence records HoldPlayback→Ack→StopDecoder→Ack→HoldProcessing/Discard→Ack→FlushPlayback→Ack; no successful Stop/new Play before final ack.
- Saturated decoder and playback queues, delayed callback flush, canceled unclaimed commands, timeouts at each barrier, late releases and shutdown while held.
- Host48→96 and96→48 transitions: tagged asymmetric sentinel streams never play at wrong clock; actual duration oracle. Channel-count-changing candidate tests distinguish accepted source width from limited hardware width.
- Bypass/unbypass with unequal input/output rate: expected frame duration and hardware reconfiguration ordering, no sample relabeling.
- Desktop Stop→Resume, Stop→Pause→Resume, Pause→Resume, and stream Flush while paused; callback completion before/after Resume. iOS independent flags need their own transition tests and unsupported-config failure case.
- No render allocation/deallocation introduced by stable per-frame acceptance checks. Control transaction storage is prepared off the callback; compilation and full workspace gate after the protocol tests pass.

The broad protocol remains unapplied and requires review. Current implementation ownership: root processing_state.rs local guards/outcomes; plugin_chain desktop DrainState/local tests and this proposal. MIDI/IAMF and blocked host DAG queues are excluded.
