# AUD135 actual engine EOS review requirements

Use Astra medium once Luna freezes a passing host/engine checkpoint. This
handoff defines acceptance questions; it contains no passing result claim.
Earlier accepted native/isolated component checkpoints remain separate.

## Actual engine evidence

Review the real ProcessingThread fixture in manager-thread `apply.rs`, its
executed feature-enabled test, loaded bundle and freshly rebuilt isolated
worker. Earlier attempts failed at compile time, missing VST3 features, a stale
worker/control timeout, then actual finite-tail preflight. Preserve those logs.

- Late refusal occurs after outer typed validation, preserves complete live
  engine/playback/worker metadata and populated audio versus a synchronized
  twin, with a cold control demonstrating retained history.
- Valid retry commits order seven through the existing playback acknowledgment,
  changing 64 inputs to 16 outputs with the full 8192-frame worker latency.
- Program is nonzero and channel-distinct, with irregular block sizes and a
  separately constructed in-process native reference. Ordinary silence is
  tested independently of a true EOS marker.
- The final maximum-size worker block must contain **nonzero program** and be
  followed immediately by EOS, without a test-only sleep hiding pending work.
  Require nonzero drained samples, exact full counts, finite complete vectors,
  maximum full-reference residual <=2e-5 and exactly one EOS marker.
- The current draft ended with 8192 silence frames: those flush preceding
  single-band matrix program before EOS, leaving an all-zero residual. Root
  requested the nonzero final-block correction before acceptance. A green
  count alone or equality against zero residual is insufficient.

## Host and worker contracts

Check bounded pending-worker completion before metadata preflight, with no
wait in ordinary callbacks. Destination-capacity refusal occurs before metadata
preparation and consumes no output. EOS context has zero source frames;
declared output capacity drives isolated drain progress. Active multi-call
drain and completed prefix stages remain idempotent, while downstream stages
refresh after accepting upstream tail input.

Require a gated worker fixture that deterministically holds the final request
pending, plus a nonzero finite native-tail case whose total work exceeds one
transport block. Verify full zero-padded reference audio, pipeline and native
tail frame counts, bounded completion, reset/fresh equivalence, unknown/infinite
preflight refusals and sticky post-mutation error behavior. Preflight refusals
without drain mutation remain retryable.

Tail metadata must belong to the latest completed accepted work, not a stale
Describe response. Metadata publication precedes worker-ready publication.
CLAP and VST3 sentinel semantics differ; rerun corrected VST3 UINT32_MAX cases
instead of citing the earlier signed-threshold expectation. Preserve the IPC
encoding's separately documented conservative representational limit.

VST3's UI-only native tail query must not occur in an in-process audio callback.
Review cached/control-side query ownership, construction and state-load refresh,
invalidation after accepted controls and native tail-change notifications, and
failed-input preservation. Serialized worker control/process ownership is a
separate path; do not imply a headless worker test proves application UI dispatch.

## Scope and provenance

Record actual command/feature set, terminal results, selected pre/post source
hashes, test binary, loaded bundle and worker identities. Qualify partial source
closures rather than inventing a complete build binding. The direct native
reference establishes transport and route behavior; independent Ambisonics DSP
accuracy remains supported by its separate accepted core evidence.

Do not claim wide isolated CLAP, mounted reactivation, AU/macOS hardware, IIR
unknown-tail completion or whole-workspace acceptance from this VST3 fixture.
General manager protocol/unequal-branch queue rewrites and MIDI/IAMF are excluded.
Return actionable findings to Luna and re-review its corrections until the
bounded checkpoint works; the full audit remains active afterward.
