# AUD077: bounded serial EOS work without the 4096-call truncation

Read-only proposal, 2026-09-28. No repository implementation or test edits. MIDI/IAMF excluded.

## Verified behavior and independent capacity reproduction

- Engine `crates/sotf-engine/src/engine/processing_thread/processing_state.rs:1000-1140` counts every host drain call against a fixed 4096 limit, including calls producing audio. It polls a command before each call and uses interruptible delivery while an output frame is pending. Preserve both responsiveness mechanisms.
- Host `crates/sotf-plugins/crates/sotf-host/src/host/daw_host.rs:1950-2092` supplies each native plugin its declared maximum drain capacity. A host call advances at most one producing/pending stage, plus stages returning zero/complete. Output from that stage passes through all downstream processors. That output may be buffered downstream, so zero host frames do not imply zero DSP progress.
- The host currently starts traversal at stage zero on every call. A completed-prefix cursor is required if budgeting is per active stage: otherwise complete/pending oscillations can reset the active stage's allowance, and completed no-op calls are revisited unnecessarily.
- Convolution `src/lib/convolution_plugin.rs:1177-1278` freezes its active prepared kernel at the first native drain. Pending worker completion makes native `tail_length()` Unknown but does not make that frozen render's EOS work unknown. Before its own EOS begins it may accept a completion while processing an upstream tail.
- With 5,760,000 taps (30 seconds at 192 kHz), UPC delay 1024, no longer retained transition, and at least one source frame: remaining output is `1024 + 5,760,000 - 1 = 5,761,023` frames. Native maximum 1024 gives **5626 calls**, final block 1023 frames. After 4096 calls, **1,566,719 tail frames** remain. NUPC delay 32 also needs 5626 calls; zero-delay direct head needs 5625. These counts derive from the production decrement rule, not from a long convolution execution in this investigation.

### Separate confirmed validation/state-loss defect (ledger candidate)

Public `DawHost` with an injected four-frame finite-tail plugin advertises maximum four frames. Calling host drain with three samples returns `Host drain output too small: need 4 samples, got 3`. The caller buffer is untouched, but the native plugin has already emitted and consumed all four frames. A full-capacity retry returns zero/complete; an untouched twin returns `[1, 2, 3, 4]` and four/complete.

Executed artifacts: `/tmp/sotf-host-drain-capacity-probe.rs`, `/tmp/sotf-host-drain-capacity-probe.log`; binary `target/audit-host-drain-capacity-probe`. Compiled with rustc edition 2024 against current `target/debug/deps/libsotf_host-72ddaffb2be1c0c5.rlib`; exit 0. This is a current-source public host probe, using a deterministic test plugin, not a Gate DSP claim.

Existing host drain docs do not promise transactional failure, and a caller providing less than the conservative declared maximum has not supplied a universally sufficient buffer. Nevertheless, the current API checks and returns an ordinary size error only *after* irrevocable DSP work; retry loses audio. Proposed narrow correction: document the conservative capacity precondition and preflight it before any native drain/downstream processing. This changes acceptance of undersized buffers that happened to fit a short final result; do not disguise that strengthening. Completed/empty hosts can return COMPLETE without requiring storage. Graph queue/build effects are separate from DSP transactionality.

## Proposed additive plugin contract

Add `drain_call_bound(&self) -> Option<NonZeroU64>` to `Plugin`, `ParametricInPlacePlugin`, and `ParametricPlugin`; default None. Forward transparently through both parameter adapters. Audit wrapper forwarding/composition (generic and dynamic oversampling, external wrappers) rather than inheriting an inaccurate child bound.

Definition: a conservative maximum number of **successful native drain invocations through and including the first COMPLETE result**, starting from this observed state, with valid context and a destination of at least `drain_output_frames_max() * output_channels()`, no additional nonempty process input, and no accepted control/reset/replacement. Calls returning zero frames/noncomplete count. The bound must include any internal scheduling, padding, wrapper resampling, and terminal zero-output call. Query must be allocation-free, nonblocking, and must not accept pending worker results or mutate the DSP.

- Some(1) is suitable for already-complete/empty state; NonZero prevents an ambiguous zero-call declaration.
- None retains a documented host fallback of 4096 calls *for that stage*, not the entire chain. This remains a guard for undeclared or defective native drains, not a promise that every unknown external tail is fully rendered.
- TailLength remains an audio-tail/public-native metadata contract. Do not convert Unknown into infinite work or infer call count from `ceil(tail_length / maximum_output_frames)`: maximum output is no minimum progress guarantee.
- Counter arithmetic uses u64 checked conversions/ceil arithmetic. No elapsed-time timeout: scheduler load and output backpressure do not measure DSP convergence.
- The host queries the bound **when each stage actually begins its own drain**, after all upstream continuation has been processed. This is essential for pending Convolution IR adoption and downstream buffering.
- For Convolution, use its actual frozen/current finite budget, including retained transition, divided by its proven fixed native advance, with a minimum of one. A pending completion is excluded once that stage starts EOS; a completion adopted during upstream processing is included when that stage starts.
- Existing finite-drain implementers need explicit bound review. Simple cached/fixed zero-continuation drains can derive ceil(remaining/max) only where their actual loop guarantees that progress. Resampler and oversampling wrappers need internal work-step derivation, including zero-output calls and remaining-input staging. Binaural/Upmixer recursive render caps can exceed 4096 at high rates and likewise need declared bounds. Do not claim universal coverage merely from adding Convolution's override.

## Host algorithm and accounting

Use scalar serial state: completed-prefix cursor, current node identity, optional remaining successful-call quota. This is local linear-drain state, not a DAG queue or manager-generation rewrite.

1. Apply already-supported graph mutations/build; reject unsupported graph shape. Validate destination against the current conservative maximum using checked sample multiplication, before native DSP. Validate the stage's declared channels/rate/capacity before consuming budget. Capacity queried before queued graph mutation may be stale; the host preflight is authoritative after applying mutations.
2. Walk/skips through the completed prefix while preserving its sample-rate conversion for the next stage. Bypassed stages preserve the current rate according to existing semantics. Do not accidentally reset all downstream contexts to the host rate.
3. At the first unfinished stage, snapshot its declared bound or fallback exactly once. If zero remaining, return convergence error before invoking it again.
4. Invoke native drain with its full declared destination. Native Err does not consume the successful-call budget and does not advance the cursor; this is not a promise that arbitrary third-party Err is DSP-transactional. Validate returned frame count before slicing.
5. On successful native return, decrement the stage quota once, including zero/noncomplete results. If it returns audio, process all downstream stages and copy the final output to the preflighted caller buffer. Advance the completed prefix for a native complete result only after that downstream delivery succeeds. A successful native call followed by downstream Err has consumed native state/quota; do not pretend a retry reconstructs it. Such an error remains fatal to engine playback under existing semantics.
6. Zero/complete advances the cursor and may skip subsequent complete nodes in the same call. Zero/noncomplete returns to the engine promptly. Native positive/complete may yield host zero/noncomplete when downstream buffers it; still advance the native cursor once delivery succeeded, then start the downstream EOS on a later call.
7. Once the cursor reaches the end, return stable COMPLETE without invoking already-completed plugins. With finite stage quotas and monotone cursor, total successful native invocations in a stable epoch are bounded by their sum; successful host calls are bounded by that sum plus one terminal/empty call. The engine therefore removes its global 4096 loop limit and relies on host enforcement.

Do not add a universal 4096 *host-zero-output* cap: downstream buffering may swallow more than 4096 legitimate upstream outputs. Every native zero-output/noncomplete call already consumes the corresponding declared/fallback quota. Check command input before every host call exactly as today; keep interruptible frame/EOS delivery. This bounds stable DSP work, not wall time under backpressure or infinitely repeated external modifications.

## Necessary mutation and lifecycle hooks

| Actual source entry | Required treatment |
| --- | --- |
| DawHost construction, `reset()` at 3761 | Clear cursor/quota; reset is explicit permission for fresh input/continuation. |
| `process` at 1926 / f64 counterpart | Rearm serial EOS only when nonempty processing is actually accepted; empty/invalid input must not refresh a stalled drain allowance. Plugins' own reset-required lifecycle remains authoritative. |
| `apply_parameter_event` at 1728; immediate engine setter uses it | Rearm the active node's quota only after an accepted setter on that active node. Changes to a downstream not-yet-drained node need no refresh: snapshot later. Reads, validation, queue insertion, rejected setter, and panic must not rearm. |
| Successful controls to a completed stage | Do not automatically rewind the completed prefix. Generic Plugin has no promise that a completed stage can produce new audio after a scalar control, or that downstream EOS-latched stages can receive new process input. Reactivation needs explicit reset/replacement or a future resume contract. This proposal preserves the existing reset-required native lifecycle. |
| `set_bypass_state` at 1885 | Detect an actual state change; same-value bypass/unbypass must not refresh quota. Topology-affecting state changes invalidate cursor/index assumptions. Preserve plugin lifecycle restrictions; bypass does not reset DSP implicitly. |
| add/remove node/plugin/edge at 428/463/944/981; queued `apply_graph_mutation` at 1844 | Invalidate on a successful actual topology change, not enqueue attempt/failure or inspection. New traversal can only use the resulting valid linear graph. Existing errors/resume restrictions still apply. |
| `build()` at 525 | Do not refresh allowance just because a rebuild/read path runs; mutation/reset is the cause. Recompute cursor identity only after a changed graph, or initialize it for a fresh host. |
| engine `commit_host_update` near 330 | Replacement host owns fresh guard state; old host counter must not transfer. Same-format replacement can continue current drain loop; output-format change retains existing discard-unsent/end-drain policy. |
| engine Stop/Shutdown/global bypass | Preserve current control flow/reset behavior. Stop resets; shutdown exits; bypass aborts current render and existing EOS cleanup resets frozen state. |
| asynchronous plugin preparation | Snapshot only at that node's EOS start. No resnapshot every loop. Convolution's first drain freezes active kernel; worker completion after that cannot silently extend this EOS. A plugin that permits asynchronous extension needs its own stable conservative bound/policy. |

Setter no-op nuance: current `set_parameter` returns only Result<(), String>, and command-like controls may act despite unchanged getter readback. The generic host cannot reliably distinguish a successful no-op from an accepted action by comparing getters. Economical compatible policy is to treat successful active-node writes as authorized work mutations; an infinite stream of accepted writes can extend the epoch indefinitely. Reads/rejections do not. If strict boundedness under repeated successful identical writes is required, it needs an additional plugin work-generation/change indication; do not invent that guarantee or silently drop commands. Ordinary gain writes after native EOS are already rejected by many implemented drains.

Graph edits during an already partially drained graph may cause current plugins' reset-required process errors even with a refreshed guard. Fixing arbitrary live graph reactivation is outside this work-bound contract; do not promise seamless traversal restarts where existing plugin lifecycle disallows them. No manager protocol/queue rewrite is proposed.

## Alternatives considered

- Larger global constant: postpones the valid-tail cutoff and provides no per-plugin guarantee.
- TailLength-based engine allowance: cannot represent pending-IR Unknown plus finite active EOS; ignores zero-output work and downstream acceptance.
- Sum all plugin bounds at decoder EOS: downstream state and prepared IR can still change while upstream tails are processed. Premature snapshots can be too small.
- Recompute/refresh an allowance every loop: a broken plugin can return the same positive bound forever. No progress guard remains.
- Global accepted-control generation: still requires per-stage state knowledge, refreshes unrelated stalled stages, and adds unnecessary engine/protocol coupling. Host-local active stage is more precise.
- Wall-clock timeout: false failures under backpressure/slow hardware, and not a proof of finite DSP work.

## Required deterministic regressions before implementation is accepted

1. Single actual 30-second/192-kHz Convolution (sparse last tap), UPC and pending-prepared-IR state: all 5,761,023 continuation frames and last marker arrive; 5626 native calls; Unknown native tail does not force fallback. Keep expensive convolution reference separate from cheap synthetic work-count tests.
2. Two finite stages requiring >4096 calls in aggregate, each below fallback; full ordered output and EOS. Add pass-through stage after the producer to cover final terminal skip.
3. Unknown always-zero/noncomplete stage: exactly 4096 successful calls, then error before call 4097. Also positive-output forever stage; audio is not proof of convergence.
4. Declared M stage with M-1 zero/noncomplete steps and final complete at M: accepted. Declared M stage still pending after M: next attempt errors without another plugin call. Zero/complete prefixes are called once per epoch.
5. Long upstream producer whose output is buffered downstream so the host emits zero for >4096 calls: accepted according to independent stage quotas; final markers intact.
6. Downstream bound queried after upstream processing, including a deterministic prepared-IR handoff adopted before downstream EOS; a completion arriving after that node's EOS remains deferred.
7. Active-node accepted control near quota exhaustion, including same numerical bound after work restarts; refreshed allowance. Read/rejected control/unrelated downstream change does not refresh the active stalled node. Document accepted same-value writes as above.
8. Host replacement near old quota exhaustion, same and changed output format. Preserve existing unsent-frame semantics; fresh host allowance, no stale node indices. Stop/Shutdown/bypass both between steps and while output is backpressured remain responsive.
9. Preflight short destination with a direct four-frame native tail and with a downstream expander: error before native/DSP position changes; full-size retry exactly matches untouched twin. Misaligned/overflow dimensions rejected before DSP. Already-complete empty call stable.
10. Mixed sample-rate chain: completed-prefix skip advances clocks correctly; no sample-rate unit conversion is used to manufacture call counts. Assert each native context explicitly.
11. Malicious complete/pending oscillation: once completed, a stage is not revisited without an explicit relevant lifecycle mutation, so it cannot refresh quota by alternation.
12. Native Err consumes no successful-call quota; invalid caller capacity consumes none. Downstream Err remains explicitly fatal/nontransactional after successful upstream work, never silently duplicates output. Exact-bound successful completion is accepted.
13. Cold successful drain and guard/cursor/reset/control bookkeeping have measured zero allocations and zero frees after setup. Existing graph rebuild/allocation behavior is a separate known control-path concern; this change must not add per-step heap work.

Implementation remains pending parent review and aggregate checkpoint. This proposal makes no claim to solve unsupported recursive-tail policies, arbitrary DAG draining, crossfade/manager-transition protocols, or reversible downstream plugin failures.
