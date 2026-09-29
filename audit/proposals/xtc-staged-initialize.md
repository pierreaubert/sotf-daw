# XTC synchronous initialization: reproduced failure and narrow staging proposal

**Status:** Assigned AUD096 and implemented on 2026-09-28. The investigation and
proposal below are the historical pre-implementation record. Verified scope,
tests and limits are recorded in `/tmp/sotf-xtc-staged-initialize-verified.md`.

2026-09-28. Read-only repository investigation. No AUD number assigned, no repository production/test edits, and no Cargo run. Existing rlibs were used for a public-API temporary Rust probe.

## Deterministic reproduction

N128, stereo input, a real loaded RoomEQ matrix with two outputs, diagonal gain0.5, AutoGain disabled. Construct and initialize two identical instances at48kHz; feed17 frames containing first/final markers0.25/-0.125. Change only the candidate's subsequent initialization conditions. After that call, compare512 zero-input frames against the untouched instance.

| Rejected source condition | Current initialize result | Original48k clock still accepted? | Maximum audio difference from untouched twin |
|---|---|---|---:|
| Matrix file removed | `Ok(())` | yes | 0.125 |
| Matrix replaced with invalid JSON | `Ok(())` | yes | 0.125 |
|48k matrix, requested96k initialization | `Ok(())` | no | 0.125 |
| Matrix replaced with valid four-output artifact | `Ok(())` | yes | 0.125 |

The reference retained-output peak is0.125 in every case. The candidate silently resets and loses that history. The rate-mismatch case additionally accepts96k processing and rejects48k processing despite retaining the old48k filters. Width remains2, so this is silent rejection plus state mutation, not a demonstrated buffer overrun.

Artifacts: `/tmp/sotf-xtc-init-probe.rs`, `/tmp/sotf-xtc-init-probe.log`, `/tmp/sotf-xtc-init-probe-build.log`; prepared files under `/tmp/sotf-xtc-init-probe-3`. Compiled against `libsotf_plugin_xtc-390effc6ac46b5ac.rlib` and `libsotf_host-4b0f15e066611283.rlib` with rustc, no Cargo. This diagnostic records observed failures rather than claiming a permanent red test has been installed.

## Exact source cause

- `xtc_plugin.rs:1791-1820`: initialize preflights basic/meter rates, then mutates `fft.sample_rate` and limiter coefficients, calls the void `update_filters(true)`, modifies AutoGain/staging, sets initialized and resets regardless of source failure.
- `xtc_plugin.rs:743-805`: synchronous update mutates fade rate, room cache/hash and HRTF cache before every fallible operation has completed. Missing paths, HRTF load errors, matrix load errors and changed output width return from a void helper.
- `load.rs:94-153` already returns specific file/JSON/rate/geometry/route errors. `load_hrtf_for_xtc` similarly returns missing/invalid SOFA, rate or measurement errors. The synchronous caller discards them.
- `compute.rs:13-38` hashes room parameters without sample rate. The synchronous cache reuse at750-761 can therefore retain room spectra computed for an old rate on an otherwise successful rate change. This is source-proven, not numerically reproduced here. A staged target-rate preparation must not reuse that cache across clocks.
- `compute_room_reflection_data` returns `Option`, conflating disabled reflections with a failed active IR load via `.ok()?` (`compute.rs:45-67`). A synchronous error-propagation helper must call the existing fallible IR builder directly for that case; wrapping this Option in Ok would hide the failure again.

## Minimal control-side design

1. **Separate synchronous preparation from the asynchronous request path.** Only initialize calls `update_filters(true)`; the five setter sites use false. Replace the synchronous branch with a private fallible `prepare_initialization(&self, target_rate) -> Result<PreparedInitialization, String>`. Keep the existing async mailbox, worker launch/wakeup, generation check, publication exchange, retirement and callback adoption mechanism unchanged. Prefer a private prepared struct over constructing an entire second public plugin or worker.
2. **Perform all recoverable work using the candidate rate, with no live writes.** Retain current rate/meter preflight. Stage room data, HRTF, filters and owned update bundle locally. Validate the source and output width and return the original descriptive errors. Recompute synchronous room data at the requested rate; for active file IR call the fallible builder and retain its result, avoiding separate validation/reload races. The existing synthetic geometry and filter arithmetic remain unchanged. Preserve the structural output width; a changed artifact requires rebuilding the graph and returns an error.
3. **Stage the remaining fallible resources.** For stereo AutoGain, create a fresh candidate AutoGain from the existing XTC settings and target rate. Successful initialize already intends a reset of its meter/audio history; a fresh instance prevents `set_sample_rate` from mutating the live meter before its own failure. No arbitrary external AutoGain target is exposed by XTC. Explicitly verify successful reinitialize equivalence with a fresh instance; do not preserve accidental stale cached targets. Non-stereo/disabled AutoGain remains None. Prepare any required temporary storage before commit; fixed FFT, width and structural cache capacities remain unchanged. Compute limiter/fade coefficients locally or only after preparation succeeds.
4. **One commit with no Result-returning operations remaining.** Only after preparation succeeds: advance the generation and stamp the prepared bundle; install clock, filter/auxiliary owners, room hash, AutoGain and coefficients; clear superseded ready publication using the existing control-side lock; mark initialized and reset audio/EOS. Retain AUD093's generation invalidation so a worker paused after its own check still cannot replace the new synchronous installation. Control-side destruction of displaced resources is allowed. Do not add callback allocation, lock behavior or new queues.
5. **Failure leaves the original epoch intact.** Do not call reset, update fade rate, resize live storage, modify initialized/rate/filter/auxiliary owners, clear pending publication/mailbox, increment generation or modify desired settings until commit. In a concurrent worker scenario this means initialize itself does not touch pending state; a worker may naturally finish existing work. Tests should use the existing rendezvous to distinguish those independent events. Failed initialize after partial drain must preserve its EOS latch and unread cached tail as well.

## Required regression evidence

- Convert the four executed public cases into permanent red→green tests: assert Err, descriptive cause, unchanged width and original-rate processing, exact output/drain replay versus untouched twin. Include failure before first initialize (remains uninitialized) and after partial EOS cache service (remains frozen and resumes the identical tail).
- Unit-level ownership checks with ready and paused actual-worker publications: failure retains generation, active/previous filter owners, auxiliary cache/hash, crossfade coefficients/progress, pending request/publication, sample clock, meter state and audio/EOS storage. Existing AUD093 successful-install race remains green.
- Positive real source cases: same-rate reload after restoring the file, valid rate-matched new artifact at a new rate, synthetic room reflections across a rate change compared with a fresh target-rate instance, two/four-output constructor layouts, and rejection of changing an established width. Validate successful history reset deliberately.
- HRTF missing/rejected sample-rate tests may use an existing small valid SOFA fixture; if none is available, matrix probes cover the common staging transaction, but do not claim executed HRTF parsing coverage. Active room IR failure should be separately exercised if that loading branch is included.
- Existing finite drain/publication/cold ownership suite and strict crate Clippy. New initialization preparation is control-side and may allocate; only subsequent callback guarantees must remain unchanged.

## Scope and limits

This fixes initialization transactionality, not general asynchronous setter failure reporting. It does not change structural source controls, ordinary reset semantics, drain support, auto-gain cadence, disabled latency metadata, filter math or worker protocol. Multiple external files can still change independently during preparation; using each successfully loaded in-memory result once prevents a single-source validation/reload race but does not provide a filesystem-wide snapshot. Allocation failure/panic handling is outside the existing Result contract.
