# XTC finite-stream continuation proposal

2026-09-28. Read-only design against the corrected AUD080 scheduler. This task changes only this document; no Rust source, tests, dependencies, host queues, or engine protocols. TokenSave status/context/search/read and current source slices were inspected. No new executable evidence is claimed below.

## Scope and current hooks

The historical startup/input-loss results in [spatial-finite-stream.md](spatial-finite-stream.md) describe the pre-AUD080 implementation. The current `sotf-plugin-xtc` scheduler already preserves every accepted input frame and emits the declared enabled-path delay. Retain that implementation and its existing `tests/stream_clock.rs` assertions.

All following source pointers are under `crates/sotf-plugins/crates/sotf-plugin-xtc/` unless stated otherwise:

| Current hook | Relevant behavior / proposed use |
| --- | --- |
| `src/lib/xtc_plugin.rs:152`, `XtcFilterExchange` | Pending publication plus two retired-update and two retired-snapshot slots. Preserve the existing ownership scheme. |
| `:162`, `XtcFilterState` | Callback-owned current/previous filter snapshots, active update bundle, generation and persistent worker. No new publication mechanism is needed. |
| `:205`, `XtcOutputBuffers` | Prepared OLA ring, output clock, startup silence and negative-synthesis prefix. Add separate drain cache/state without repurposing these counters. |
| `:343`, `new` | Validates power-of-two N in 128..16384; H=N/4. Prepare H stereo zero frames and H frames at the fixed output width. |
| `:713`, `update_filters`; `:841`, worker | Asynchronous work publishes prepared matrices and reclaims retired owners outside the callback. Drain must not call either control-side update path. |
| `:924`, `adopt_pending_filters` | Called once after normal process validation. Suppress this call throughout drain; preserve ordinary adoption on validated nonempty process after reset. |
| `:977`, `retire_completed_filter_snapshot` | Callback uses only `try_lock`; full slots retain the old owner. Continue using this during the existing active fade. |
| `:1016`, `process_stft_frame`; `:1293`, `shift_input_buffer` | Existing finite WOLA kernel. Retain its sample arithmetic, fade progression and startup-prefix clearing. |
| `:1342`, `set_parameter` | Add an EOS guard and borrowed same-value recognition before metadata rebuild, resource creation or worker scheduling. |
| `:1555`, `initialize`; `:1579`, `reset` | Clear new audio-epoch/drain state. Preserve callback-safe reset and existing retirement backpressure. |
| `:1603`, `process` | Keep complete preflight before state mutation/adoption; factor its validated audio body for use by canonical drain refills. |
| `:1640`, AutoGain measurement cadence | Every tenth ordinary callback measures input/output. Continue that counter at EOF, advancing it once per canonical refill rather than once per caller destination. |
| `:1658`, hard-disabled branch | Immediate direct routing, no STFT-clock progress. Preserve ordinary routing/toggle behavior; EOS freezes the selected branch. |
| `:1791`, `latency_samples` | Existing N report remains unchanged. The disabled direct branch's reported N versus actual zero delay is a separate existing issue, not fixed by finite tail metadata. |

The existing public `Plugin` API already supplies `drain`, `drain_output_frames_max`, `tail_length`, and the new `drain_call_bound`. No trait or host changes are required. Keep `begin_drain` as its default no-op: no wrapper-style partial-block preparation is necessary and the scalar bound can be queried without adopting pending state.

## Support and bounded work

Let N be transform length, H=N/4, and D=N. Count S as **enabled accepted input frames** since reset; hard-disabled calls do not advance this clock. Store only `S mod H` and whether any enabled input was accepted, avoiding an unbounded absolute counter.

For S>0, the final potentially input-containing analysis window starts at `floor((S−1)/H)H` and synthesizes N samples. Its exclusive endpoint on the enabled clock is `D + floor((S−1)/H)H + N`. After S emitted enabled frames, the required conservative continuation is:

```text
padding = (H − S mod H) mod H
R       = 2N − H + padding
R_max   = 2N − 1
```

This includes startup silence and complete final-window support once. It may include trailing zeros; do not threshold-trim. For a clean enabled epoch, concatenated process+drain length is exactly S+R. The same R applies to retained STFT state after arbitrary hard-disable intervals, using only its enabled clock; absolute source delay across those intervals is intentionally not redefined.

Every current filter path multiplies a finite spectrum and synthesizes one N-sample window. Room/HRTF response data does not introduce a running recursive audio convolution state here. Old/new matrices in a crossfade have the same window support. AutoGain and the limiter multiply current audio; their persistent envelope/measurement state cannot create audio from exact zero. No extra silent tail is needed merely to complete a long fade or decay an envelope.

Use checked support and storage products, even though validated N is bounded. Advertise `drain_output_frames_max() = H`, i.e. 32..4096 frames, rather than a new global callback limit. One drain call either serves existing cache or generates one canonical refill; it never loops to fill an arbitrarily large destination. Thus it performs at most one hop's normal streaming work (at most one new STFT frame) and emits at most H frames.

Prepare an H-frame stereo zero source and H-frame output cache at the initialized output width. The extra storage is `H * (2 + output_channels)` f32 samples plus scalar state. No allocation, resizing, async polling, blocking lock or file I/O occurs in successful process/drain/reset/query paths added by this patch.

### Canonical refill and query arithmetic

Track `remaining` as total not-yet-returned continuation frames, including unread cache. Track unread cache C separately (`cache_frames − cache_position`).

1. If C>0, copy `min(C, destination_frames)` frames and return without refilling.
2. If C=0 and remaining>0, process `min(H, remaining)` prepared zero frames through the same audio body as ordinary processing, with publication adoption disabled. This deterministic final partial refill depends only on R, not destination size.
3. Copy the available prefix, decrement remaining by frames actually returned, and retain all unread cache.
4. Mark complete on the call returning the final frame; repeated drain returns COMPLETE with zero frames. Leave destination suffix samples untouched.

Starting from identical pre-EOF state, all destination schedules therefore yield identical samples and AutoGain/diagnostic/fade/limiter progression. Keep the existing diagnostics counter at EOF rather than resetting it. Normal callback partition dependence of AutoGain remains an existing behavior and is not claimed corrected.

For full-native-capacity successful calls, the current-state work bound is:

```text
B = max(1, (C > 0 ? 1 : 0) + ceil((remaining − C) / H))
```

Before first drain, compute R from the enabled clock without mutating state. Empty, completed, or selected disabled state needs one successful terminal call. Return `None` before successful initialization or if a checked invariant fails. Fresh enabled state requires at most eight full-capacity calls. This formula includes an existing partial cache as one call and does not infer progress merely from advertised capacity.

`tail_length()` is scalar-only: Unknown before initialization; `Finite(2N−1)` while the selected branch is enabled; `Finite(0)` while hard-disabled. The bound assumes controls remain fixed, as does the tail contract. Pending publications do not enlarge finite window support, and a drain never adopts them.

## Lifecycle and hard-disabled behavior

Use a separate `received_program_input` flag, an `enabled_input_seen` flag, enabled phase, and drain state. Zero-frame normal calls never mark input or adopt state.

| State at valid EOF | Result and lifecycle |
| --- | --- |
| No accepted program frames | COMPLETE/0; do not freeze. New input and parameter changes remain permitted. |
| Currently enabled with enabled input history | Freeze input/control changes and publication adoption; drain R using the retained active STFT state. |
| Currently enabled, but all accepted input was hard-disabled | COMPLETE/0 and freeze. There is no wet audio history to release. |
| Currently hard-disabled, including earlier enabled history | COMPLETE/0 and freeze. This preserves the selected direct route's ordinary zero-input response; retained wet history is deliberately unobservable until reset clears it. |
| Already complete | COMPLETE/0; preserve the frozen epoch until reset/reinitialize. |

Ordinary live `enabled` toggles remain permitted before EOF. A disabled call pauses STFT input, startup and OLA counters; re-enabling continues those same counters today. Therefore no new Unknown classification or reset-required restriction is necessary solely because a stream toggled. At EOF, frozen controls close the only path by which latent wet history could become observable again. Test this directly against an ordinary-zero continuation twin with arbitrary toggle history and the same final branch.

After a nonempty accepted EOS, reject new nonempty `process` input and all changed parameters before mutating any state. Exact recognized same-value snapshots remain successful no-ops, including structural strings; compare borrowed fields and primitive schema values so this does not allocate, rebuild cached metadata, queue filters, reset the limiter or construct/drop AutoGain. Unknown IDs and invalid values retain error semantics. A zero-frame normal process remains a validated no-op.

Drain preflight must validate initialized state, sample rate, whole output-frame alignment and checked dimensions before latching EOF, advancing DSP, changing ownership or touching output. Reject zero destination capacity when a positive tail remains; empty/completed/direct-terminal paths may return COMPLETE/0 without storage. Errors must preserve both output canaries and later waveform relative to an untouched twin. Do not reinterpret `context.num_frames` as new input; the output slice supplies drain capacity. XTC currently does not consume transport/sample-position metadata in its kernel, so no invented clock is needed when refilling.

Successful initialize/reset clear EOS, cache, enabled phase and input flags and restore the corrected N-delay startup. Reset may retire a completed old snapshot through the existing bounded try-lock path, but must never clear/drop an owned Arc merely to make room. Preserve the existing normal diagnostic cadence policy unless a separate behavior change is reviewed. Failed initialize validation must not accidentally unfreeze or clear an existing epoch; retain existing rate-zero transactionality and test it explicitly.

## Filter publication, fade and resource ownership

Freeze **adoption**, not worker execution, on the first valid nonempty EOS boundary. Do not call `adopt_pending_filters` immediately before freezing: a result that happens to be ready at EOF must not change the tail's active filter pair. The current and previous snapshots remain callback-owned through the response.

Continue the already active old/new fade using existing per-hop progression. When it completes, `retire_completed_filter_snapshot` can move its owner into a free retirement slot. On contention/full slots it retains the owner. If fade duration exceeds finite support, do not extend output or forcibly release the previous snapshot. Reset or eventual control-side destruction handles retained owners under the current policy.

The worker may still replace `exchange.pending` or collect retired owners. Those mutations happen under its mutex and resource destruction remains worker-side. None changes the active audio state while drain suppresses adoption. Do not clear pending mailboxes, replace active bundles, clone external resources, or join/wake a worker from drain solely for completion.

### Reset/new epoch policy

Pending requests represent desired configuration, not old audio. Preserve them across callback-safe reset, including their existing generation and latest-only ordering. Reset clears all old audio history but does not adopt or destroy pending publications. The first subsequent **validated nonempty** ordinary process may adopt a valid ready publication through the existing retirement-capacity checks; if it arrives later, normal later-callback adoption remains possible. Failed/zero-frame calls do not adopt. Stale generations/widths continue through the existing rejection and retirement path.

This is explicit asynchronous ordering, not a promise that reset waits for a requested configuration. A deterministic new render requiring the latest requested configuration must use a successful control-thread preparation/initialization barrier. Do not blindly increment generation in callback reset: without reconstructing/requeueing the desired request, that would silently lose accepted settings.

### Separate finding: reinitialize can admit an old-rate publication

Current `initialize` changes `fft.sample_rate` then calls `update_filters(true)`. The synchronous branch installs new filters using the current generation and clears `exchange.pending`, but does not advance generation. An already computing old-rate request can finish afterward with that same generation; `adopt_pending_filters` checks generation and width, not request sample rate, so it can accept old-rate coefficients into the new-rate stream. This is a source-proven race shape, not newly executed red evidence in this planning task.

A separately approved narrow fix would advance/invalidate the async generation on the control thread before synchronous replacement and stamp the new active bundle with that generation. Stale work can finish and be retired under existing ownership; a worker that passed its check before invalidation may still publish, but callback generation validation then rejects it. No callback reset cancellation, new worker or publication API is required. Reproduce with a deterministic compute/publication barrier before changing source. Keep this finding distinct from the finite-drain implementation scope and from broader synchronous source-load error reporting.

## Minimal production diff

1. `src/lib/xtc_plugin.rs`: add prepared drain storage/scalars; factor the validated audio body without altering STFT/filter math; update normal accepted-clock accounting only on enabled nonempty input; add scalar metadata/bounds and drain hooks; add EOF/no-op setter guards; clear audio-epoch fields in reset/initialize.
2. `src/lib.rs`: register one private EOF/ownership test module if access to current filter/counter state is needed. No public configuration or parameter-index changes.
3. New `tests/finite_stream.rs` for public waveform/lifecycle tests and `src/lib/drain_tests.rs` for canonical continuation/ownership tests. Extend the existing private allocator harness or add a separate integration executable so allocator definitions do not conflict.
4. README/CHANGELOG: document finite enabled support, selected disabled EOF behavior, reset requirement after nonempty EOS, canonical drain cadence and explicit async reset ordering. Keep the ordinary disabled latency mismatch and normal AutoGain partition limitation visible.

No host/engine/NIH/bridge changes, transform redesign, new dependency, sample cutoff, blocking worker barrier or maximum ordinary callback restriction is proposed.

## Red-to-green and independent evidence plan

Start with one final-sample marker and a dense neutral block through `bypass_xtc_filters=true`, AutoGain off. Current default drain returns COMPLETE/0 despite retained output; preserve that red log before implementation.

| Proof | Required cases / oracle |
| --- | --- |
| Independent neutral support | Compare complete process+drain to N exact startup zeros followed by original dyadic stereo program and structural zero suffix, with measured FFT reconstruction error and existing tolerances preserved. Enumerate every H phase for small N; boundary phases for every supported N through16384; S=1,H−1,H,H+1,N−1,N+1 and ring-wrap histories. |
| Positive tail semantics | Check exact R, exact final completion and untouched destination suffixes; capacities1/17/H−1/H/H+1 and irregular schedules. Input callbacks1/17/137/H/4096/16385 include the prior lost-interval regression. |
| Nonidentity finite operator | For small N, use a scalar f64 DFT/WOLA reference over a fixed known spectral matrix, finite windows and explicit D scheduling. Separately compare stable synthetic/room/HRTF or prepared multi-output matrices against ordinary zero continuation at the canonical refill schedule. The latter is parity evidence, not an independent acoustic model oracle. |
| AutoGain/limiter | Identical pre-EOF callback histories, default AutoGain enabled, measurement counter just before its tenth-call boundary, nonzero compensation/envelope, and a final partial refill. Require exact outputs across destination schedules and equality to ordinary-zero H-block continuation with no pending adoption. Do not compare differently partitioned ordinary AutoGain histories as if they must agree. |
| Toggle history | Enabled→disabled→enabled, disabled→enabled, enabled→disabled, repeated toggles, and toggles without intervening input. Final enabled state matches same-state ordinary zeros using enabled-frame phase; final disabled state emits no tail. After either EOS, attempting to reveal old wet history fails until reset; reset replay matches a fresh audio epoch. |
| Publication freeze | Ready pending result before EOF, result published between drain calls, active fade shorter/longer than R, interrupted fade, stale result, output-width mismatch, full retirement slots and held exchange lock. Tail samples/current active owners remain unchanged by deferred publication. |
| Reset adoption | Deferred valid publication survives reset and is adopted only on validated nonempty normal process, subject to retirement space. Empty/invalid calls preserve owners. Stale results stay rejected. Repeated EOS/reset/adoption cannot exhaust ownership slots by silently dropping a live owner. |
| Transactional lifecycle | Before initialize, rate mismatch, malformed output width, zero positive-tail capacity, invalid normal input/output shapes, rate-zero reinitialize, same-value primitive/string snapshots, changed controls during partial cache and after completion. Compare subsequent waveform and private ownership/scalar snapshots to untouched twins. |
| Cold realtime | Fresh callback thread; first enabled drain, first AutoGain measurement during drain, first fade retirement, existing cache delivery, completion, reset and post-reset adoption. Count both allocations and deallocations; force lock contention/full retirement and retain drop probes to prove no final resource destruction on the callback. Preserve existing mutex preinitialization after final Arc placement. |
| Query correctness | Repeated metadata/bound reads do not adopt, lock, allocate or mutate counters. Count full-H successful calls against the declared bound at initial state and after one-frame partial reads; include cached final frames with no remaining ungenerated frames. |

Run the complete XTC crate tests and strict all-target Clippy after focused red/green fixtures, then hand a frozen checkpoint to the parent for aggregate verification. Record actual matrix size, maximum absolute error/minimum SNR, cold allocation/free counts and exact log paths. This plan makes no claim that those new tests have already run.
