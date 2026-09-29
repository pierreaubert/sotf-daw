# AUD-093: XTC synchronous filter generation invalidation

2026-09-28. Separate from [XTC finite EOS](xtc-finite-stream.md). This investigation changed no repository Rust sources or tests. The reproduction and candidate patch were executed only in `/tmp/sotf-xtc-generation-race/`, using existing workspace dependency artifacts and copied XTC source. No Cargo rebuild or aggregate source mutation was needed.

## Executed deterministic red

The harness compiles the current XTC source, with exactly two instrumentation calls added to the persistent worker in `src/lib/xtc_plugin.rs`:

1. Immediately after `compute_filter_update` and the worker's generation equality check, before it locks the publication exchange, send the completed update to the harness and wait for a release message.
2. After publication and dropping any displaced owner, signal publication and park outside all production locks until the callback has been observed.

These are ordering barriers, not substitute production logic or timing sleeps. They do not change the generation check, synchronous installation, pending exchange or callback adoption. A ten-second receive timeout detects a failed fixture barrier; elapsed time never establishes the race order.

Sequence:

1. Construct and initialize synthetic N=128 stereo XTC at48k, AutoGain off.
2. Public `set_parameter(kappa_target,23)` starts the actual worker. Wait until its actual48k update has passed its generation check.
3. Public `initialize(96_000)` returns successfully and installs a new96k filter snapshot. The pending exchange is empty.
4. Release the worker, wait for its actual late publication, and call actual `process` with a valid96k one-frame stereo block.
5. Compare active snapshot Arc identity against both the synchronous96k snapshot and the completed48k worker result. Also compare coefficients to establish that the snapshots are observably different.

`red.log`, exit101:

```text
old_request_rate=48000 initialized_rate=96000
old_generation=1 new_generation=1
max_filter_ll_difference=0.558677018
adopted_old=true retained_initialized=false
```

The regression assertion is that the callback must retain the initialized96k snapshot. It fails. This demonstrates adoption of wrong-rate coefficients, not just an unnecessary pointer replacement. The filter data itself does not carry a rate field; the known request rate and barrier identify its provenance.

## Smallest proposed production change

Reviewable unapplied patch: `/tmp/sotf-xtc-generation-race/generation-invalidation.patch`.

### Invalidate at successful synchronous installation

In `update_filters(true)`, after the newly prepared filters pass the existing output-width check and immediately before replacing `cached_current_filters`:

- Increment `filter_update_generation` with `fetch_add(1, AcqRel)` and obtain the resulting generation with wrapping arithmetic.
- Stamp the new `active_filter_update` bundle with that returned generation rather than loading the unchanged counter.
- Leave the existing publication exchange and callback validation unchanged.

Current call-site inventory, verified with TokenSave callers and direct search:

- `initialize` is the only caller passing `true`.
- Scalar/string setter paths pass `false`.
- `new` builds the initial snapshot and generation0 before any worker exists; it needs no invalidation.
- `reset` installs no new filters and must not invalidate desired pending configuration.

Put the increment inside the successful synchronous installation block rather than only in `initialize`, so every future valid synchronous installation uses the same rule. A source-load failure or output-width mismatch that returns before installation must not consume a generation. Existing early auxiliary-cache mutations on those failure paths are a separate limitation, described below.

### Preserve known initialization preflight failures

`initialize(0)` already rejects before mutation. Keep it first.

When `params.auto_gain_enabled && output_channels()==2`, also preflight the **existing meter constructor's** rate domain16..=2_822_400 before changing sample rate, filter generation, buffers or active ownership. This is not a new DSP rate limit: XTC's stereo AutoGain constructor already rejects these rates through its two loudness monitors. The current `initialize` reaches that failure only after replacing filters and mutating the clock.

Exact current failure chain:

| Configuration / rate | Existing result |
| --- | --- |
| Any initialized layout, rate0 | XTC's own early error. |
| Stereo with AutoGain selected, rate1..9 | `LoudnessMonitor::new_inner` rejects below10. |
| Stereo with AutoGain selected, rate10..15 or >2_822_400 | Vendored `EbuR128::new` rejects outside16..=2_822_400. |
| Stereo with AutoGain selected, rate16 or2_822_400 | These constructor rate checks accept; they are boundary cases, not proof of acoustic usefulness. |
| AutoGain off, or a fixed output width other than2 | No AutoGain meter is constructed; preserve existing acceptance of nonzero rates rather than extending the meter limit to these paths. |

Pointers: `sotf-host/src/auto_gain.rs:97,131`; `sotf-host/src/analyzer_loudness_monitor.rs:487`; `../math-audio/crates/math-dsp/src/ebur128/ebu_r128.rs:51`. The meter's fixed channel count is2 and no explicit layout is supplied, so its other zero-channel/explicit-layout failure branches do not apply here.

The narrow patch does **not** claim all existing initialization failures are transactional. Synchronous HRTF/recommended-matrix loading currently uses `let Ok(...) else { return; }` inside a void helper, and output-width changes similarly return early. Auxiliary room/HRTF caches can already have changed; `initialize` then continues and may return success. Missing files, parse errors, missing HRTF sample rate/measurement, source-rate mismatch and changed recommended output width are existing silently handled source failures, not new errors introduced by this fix. Converting that helper to a staged fallible install is separately reviewable work, not hidden in the generation patch. Allocation failure/panic is likewise outside a recoverable initialization guarantee.

## Ownership and race proof

- Worker still computing: it sees the new generation at its existing check and destroys its stale result on the worker.
- Worker already passed its check: it may publish after synchronous installation, as the executed fixture forces. The existing callback generation comparison rejects that publication. It never becomes current or starts a fade.
- Publication already waiting before initialize: the existing synchronous branch clears it on the control thread. No callback destruction is introduced.
- Stale publication after initialize with a free retired-update slot: callback transfers ownership to that slot; worker reclaims it later outside the mutex.
- Full retirement slots or callback mutex contention: callback leaves the stale pending Arc owned in the exchange and retains the current snapshot. No last-Arc drop, blocking wait or replacement is necessary.
- Old request still in latest-only request mailbox: it may compute and fail the generation check. Do not clear it from callback/reset or launch a replacement worker. A later accepted control update gets the normal newer generation and remains eligible.
- Same-rate synchronous rebuilds also invalidate older work. This matters when a configured external source changed on disk even though rate and output width stayed equal.
- Generation wrap retains the existing u64 counter model. The patch uses wrapping arithmetic to match atomic `fetch_add` behavior; it does not claim uniqueness over2^64 accepted replacements.

Control-side synchronous rebuilding may continue to destroy replaced snapshots/resources as today. The audio callback's retirement mechanism and capacity remain unchanged. No worker joins, cancellation messages or new synchronization primitives are proposed.

## Isolated candidate result

Applying only the proposed two-hunk patch to the instrumented `/tmp` source yields `candidate-green.log`, exit0:

```text
old_generation=2 new_generation=3
max_filter_ll_difference=0.558677018
adopted_old=false retained_initialized=true
stale_payload_retained_in_bounded_retirement=true
failed_rate_zero_preserved_generation=true
```

The harness checks the retired payload while the worker remains parked; releasing it first would allow legitimate reclamation to race the ownership assertion. The test also performs invalid `initialize(0)` before valid reinitialization and checks unchanged rate, generation and active Arc. Invalid AutoGain-rate preservation and full-retirement contention still need repository regressions after approval; the isolated candidate does not claim they have executed.

## Proposed repository regression scope after approval

One new private XTC test module and a per-instance test-only worker rendezvous (no production statics). It must be inert when absent and avoid synchronization while holding the publication mutex. Cover the actual-worker post-check race above, same-rate synchronous install, failed initialization0/1/9/10/15/2_822_401 with AutoGain selected, and control-rate endpoints16/2_822_400. Retain the no-AutoGain acceptance behavior.

Add private deterministic publication variants for pending-before-install, retirement slots full, exchange contention and valid later-generation adoption. Use the existing cold allocator harness for callback rejection/transfer, counting frees as well as allocations; source copying for this planning probe is not a substitute for that native regression. Verify failed preflight keeps active audio history and pending publication/generation intact. Keep source-load silent-failure limitations explicit.

Artifacts include `instrumentation.patch`, original source SHA256, copied source, original and candidate compiler command lines, `red.log`, `candidate-green.log`, and the unapplied production patch under `/tmp/sotf-xtc-generation-race/`. All executable artifacts use prebuilt dependencies resolved from the current XTC Cargo fingerprint. No repository source or test changes were made by this proof.
