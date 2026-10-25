# Hiss factory configuration and retained review diagnostics

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The optional structural `interval_review` Hiss field selects a local model by
explicit path, lowercase SHA256, recording setup identity and maximum endpoint
age. Rate, channel count and cutoff must match the native processor. Omission
remains disabled and does not add a field to serialized legacy/default params.
The facade forwards `hiss-interval-review` to the Hiss inference feature.

Model preparation opens the requested file once, reads at most1MiB plus a
sentinel byte and verifies/parses the same bytes. Invalid configuration settings
are rejected transactionally. Missing, changed, oversized, unsupported or
context-mismatched models and feature-disabled builds keep classical audio and
publish an explicit unavailable diagnostic. No replacement model is selected.
Requested configuration persists for reconstruction. Reinitialization reuses
verified in-memory model state and does not reopen a changed path. Replacing
a valid selection with an unavailable model retires the previous worker.

`get_data()` retains its original stable `Arc<ProfileSnapshot>` and atomic
profile payload/generation semantics. A separate control-time slot exposes
Disabled, Unavailable or a retained read-only IntervalReviewReader. Only control
configuration uses its RwLock; audio profile publication never accesses it.
Unavailable diagnostic cloning is control/UI work and may allocate.

The reader combines a bounded coherent atomic source snapshot with a retained
shared-inference result reader. The new Copy-result-only inference reader checks
reset generation again before returning, reports worker termination and cannot
submit/reset work or prolong worker lifetime. Existing AsyncInference read
semantics remain unchanged. Readers can receive the final asynchronously
completed interval without another process/get_data callback. Reset, source
epoch changes, expiry, future endpoints and shutdown invalidate observations.

Evidence:

- 149 Hiss and19 shared-inference tests pass, zero ignored (168 combined).
- Four targeted facade factory tests pass, including actual final-window inference
 through a generic factory processor and unavailable-model audio preservation.
- Four final configuration tests pass, covering invalid identity/rates, single-read
 model reuse, changed/oversized file refusal, final-callback reader delivery,
 stable profile snapshot identity, reset and replacement with an unavailable model.
- Two native feature-disabled configuration tests and one feature-disabled factory
 test pass. Valid model configuration retains classical audio without inference.
- Strict all-target Clippy passes for Hiss, shared inference and the facade,
 including examples. Focused formatting/diff checks pass.

The full native/factory runs preceded an additional invalid-rate validation guard
and the example literal fix. Focused configuration tests, strict all-target lint
and feature-disabled tests then checked the final implementation; accepted finite
native configurations are unchanged by the guard. Commands, durations and full
output are adjacent. `source/` and `source-sha256.json` retain final inputs.

Retained failures: the first shared-inference unit build needed an `alive` field
in an existing manual test fixture (`initial-build.txt`). The first facade
all-target lint caught an exhaustive example parameter literal missing the new
optional field (`clippy.*`); fixed to explicit None before `clippy-r2.*` passed.

No model was retrained, recordings added or ML quality policy changed. The same
synthetic example remains opt-in, with its previous semantic ambiguity and lack
of demonstrated improvement over the simple classical baseline. The new reader
cannot apply a profile. Native explicit original-PCM application remains separate;
a user review UI, generic live host application commands and higher-level app
settings integration remain open. Real-source qualification, calibrated
abstention, actual DAW resource/deadline and cross-platform acceptance remain
required. This checkpoint does not complete Hiss or the full ML audit.
