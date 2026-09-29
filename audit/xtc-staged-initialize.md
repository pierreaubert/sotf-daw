# AUD096 XTC staged synchronous initialization — verified checkpoint

2026-09-28. Scope: XTC crate and approved proposal only. No host, engine, dependency, MIDI/IAMF or asynchronous protocol changes. Proposal: `audit/proposals/xtc-staged-initialize.md`. This report supersedes its historical read-only status.

## Corrected behavior

Initialization now prepares a private candidate using the requested sample rate: room data, HRTF or recommended matrix, same-width filter bundle, and fresh stereo AutoGain resources. All recoverable source/rate/layout errors propagate before changing live state. Active file artifacts are loaded once by preparation, and their in-memory results are installed directly.

A successful commit advances the generation, installs filters/auxiliary owners/clock/AutoGain, clears superseded ready publication, then resets the audio/EOS epoch. The existing AUD093 callback check still rejects workers that passed their own generation check before the synchronous installation. Initializer failure leaves clock, initialized state, filter/room owners, generation, ready publication, audio buffers, partial drain cache and diagnostic cadence intact. A naturally running worker can still finish its existing desired request; failure does not invalidate or discard that work.

Synchronous room spectra are recomputed for the requested rate. The old parameter-only room hash could reuse old-rate spectra. Active room IR errors now propagate through the fallible loader instead of being conflated with disabled reflections.

Successful initialization alone also resets the diagnostic callback counter. The positive fresh-instance test observed old counter2 versus fresh0 before this correction. Ordinary reset behavior and normal callback-dependent measurement cadence are unchanged. A fresh candidate AutoGain starts the new initialized epoch with fresh meters; no shared AutoGain implementation was changed.

## Source scope

- `src/lib/xtc_plugin.rs`: private PreparedInitialization and fallible preparation; all-or-nothing synchronous commit; five existing setters now call the async-only private update method. Removed the superseded synchronous branch and its AutoGain helper. The worker's meaningful statements are unchanged apart from wrapping/formatting, checked directly against the saved pre-change source. No `process_audio`, drain, callback adoption or retirement arithmetic changed.
- `src/lib.rs`: register one private test module.
- New `src/lib/initialize_tests.rs`: real matrix/room artifacts and initialization transaction regressions.
- `src/lib/generation_tests.rs`: one actual-worker failure-preservation regression using the existing per-instance rendezvous.
- README/CHANGELOG document error propagation, retained state and successful reset semantics.
- Isolated production delta: `/tmp/sotf-xtc-staged-initialize-production.patch`.

## Evidence

### Before correction

The public temporary probe executed four cases: missing matrix, invalid JSON,48k artifact with96k initialization, and a valid matrix changing output width2→4. All returned Ok and lost the retained marker suffix (max absolute difference0.125 versus an untouched twin); the rate mismatch also stopped accepting the original48k clock. `/tmp/sotf-xtc-init-probe.log` and `.rs` retain exact fixture/build provenance in the proposal.

The permanent failure matrix was installed before production changes and failed immediately on its first missing-file/uninitialized case because initialize returned Ok: `/tmp/sotf-xtc-initialize-red.log`. The positive fresh-instance regression later independently failed on counter2 versus0: `/tmp/sotf-xtc-initialize-expanded.log`. These are the captured permanent reds; other matrix cases were also executed in the earlier public probe, not misreported as separate pre-fix permanent test runs.

### Current tests

Five new tests,40 configuration/scenario cases:

-24 failure cases: two/four outputs × missing/JSON/rate/width failures × uninitialized/live/partial-EOS epochs. Descriptive Err; unchanged clock/layout, initialization flag, generation, active/ready owners, input/ring/cache contents, coefficients and callback counter; exact continuation against an untouched twin. Uninitialized remains unusable; failed reinitialize during partial drain remains frozen and returns the identical retained suffix.
-8 successful reload/rate cases: two/four outputs × AutoGain off/on ×44.1/96k target rate. Restore a removed artifact at the requested rate, commit exactly one new generation, then compare24 ordinary callbacks plus drain bit-for-bit with a newly constructed target-rate instance.
-4 synthetic room cases: AutoGain off/on ×44.1/96k target rate. Room spectra and filter coefficients equal a fresh instance exactly, and process/drain audio agrees exactly.
-2 real PCM16 room-IR cases: missing path and mismatched sample rate reject without dropping the prior cache/active owner or clock/generation/audio; restoring the file at the requested rate permits retry.
-2 actual-worker cases: failure while a desired result is paused after checking its generation, or already published and paused before continuing. Failure preserves its generation/active audio; the same desired result remains pending and adopts at the next valid old-rate callback. Fresh-thread adoption measures0 allocations and0 deallocations.

## Verification

- `cargo test -p sotf-plugin-xtc --all-features --offline`: **191 passed**, zero failed, one preexisting ignored doctest. Full ordinary numerical, finite-stream, AUD093 race, layout, and cold ownership suite included. `/tmp/sotf-xtc-initialize-full.log`.
- `cargo clippy -p sotf-plugin-xtc --all-features --all-targets --offline -- -D warnings`: passed. `/tmp/sotf-xtc-initialize-clippy.log`.
- After adding explicit failed-init callback-counter snapshot assertions, the4 initialization tests reran and passed: `/tmp/sotf-xtc-initialize-final-focused.log`.
- Scoped rustfmt and diff whitespace checks passed. Initializer may allocate/read files on the control thread; callback zero-heap evidence does not claim initialization is allocation-free.

## Limits

Root independently reviewed the isolated production delta, preparation/commit
ordering, preserved worker body, and permanent failure-state matrix. No
remaining blocker was found in the initialization scope.

No new valid SOFA fixture was added or executed for HRTF-specific failure parsing. The staged HRTF path uses the existing fallible loader and cannot mutate live state before returning, but the new executed artifact matrix covers recommended matrices and real room IRs. Existing broader HRTF tests remain part of the full suite. Asynchronous setter error reporting remains unchanged. Multiple external artifacts do not form a filesystem-wide snapshot, although preparation installs exactly the objects it loaded and never validates then reloads one artifact for commit. Allocation failure/panic recovery is outside the existing Result contract. Hard-disabled latency and normal AutoGain callback dependence remain separate audits.

## Suggested ledger row

AUD096 — Fixed XTC synchronous initialization returning success after source/rate/width rejection and resetting live audio. Stage all fallible target-rate resources, commit one generation only on success, preserve failed live/EOS/worker state, recompute rate-dependent room spectra.191 tests + strict all-target Clippy;40 new scenarios including real artifacts and actual worker barriers. HRTF-specific new fixture coverage remains explicitly unclaimed.
