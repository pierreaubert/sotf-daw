# Preparing reviewed Hiss profiles through hosted readers

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The retained IntervalReviewReader now exposes an explicit allocating control-time
`prepare_reviewed_profile` method. A person must select the current candidate and
the caller must retain its exact original dry PCM. The method verifies token,
source geometry and SHA256, performs ordinary native profile measurement, then
checks the source token again before returning. It rejects reset, expired,
superseded, unavailable or changed observations. Contention is a conservative
failure that callers can retry later.

ReviewedIntervalProfile retains the native profile together with its original
context and source/model observation. Keep the provenance with the review; only
the native profile belongs in the existing captured_profile preset field. The
reader cannot mutate a running processor. Preparation does not consume the token,
arm capture or change the profile-use flag. Native explicit in-place application
and hosted preparation now share their measurement implementation.

Expanded native regression checks compare hosted preparation and native explicit
capture in both Hiss backends. They confirm unchanged stored state, original
source/token rejection and refusal after token retirement. The generic factory
test feeds source with actual transport positions, obtains its final asynchronous
proposal, prepares a profile and proves the original processor stays profileless.
It explicitly serializes/reconstructs a preset containing the selected profile
and chosen use flag. The rebuilt processor exports the identical profile and
renders bitwise like a fresh native processor, despite193-frame versus48000-frame
callbacks. Preparation after the original worker shuts down is refused.

Validation:149 Hiss tests pass, zero ignored; the two targeted factory review
tests pass. Strict all-target Hiss/facade Clippy and feature-disabled release
compilation pass. The focused14 native review/unit tests also pass. Commands,
exit codes, durations and full output are adjacent. Focused formatting/diff checks
pass. Source copies and hashes identify the checked implementation.

The model, learned/classical decision policy and audio processing algorithm are
unchanged. No recordings or pretrained weights were added. The existing
synthetic example remains opt-in, with the previously established semantic
ambiguity and lack of demonstrated classical-baseline advantage. This is local
functional factory/preset evidence, not classifier quality, DAW resource/deadline
or cross-platform acceptance. A user review UI and live graph application commands
remain open. The Hiss proposal and full ML audit remain in progress. Nothing is
automatically installed into a live processor by a classifier result.
