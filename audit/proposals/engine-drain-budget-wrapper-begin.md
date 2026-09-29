# AUD077 addendum: prepare the oversampled child before snapshotting its work bound

2026-09-28. Design only; no API or source edits. This addresses the two current wrappers, not arbitrary graph/manager changes.

## Why another boundary is necessary

`Oversampler::drain_with` (`oversampling/oversampler.rs:340-468`) first sends residual input and the up-filter overlap through `inner.process`, then enters `InnerTail` and calls `inner.drain`. An inner bound queried before those process calls is not a bound from the eventual child EOS state. A currently empty child can acquire a long tail; Convolution can adopt a longer queued IR during either process call. Returning None preserves the old 4096 fallback and still truncates such known finite wrapped work.

Folding the setup into the first wrapper drain does not fix the snapshot ordering: the host must select a quota *before* that call under the accepted design. Refreshing every returned zero/positive call would let a stalled implementation rearm forever. A separate, bounded setup hook lets the host validate destination, prepare once, then query a truthful current-state bound.

Proposed extra hook: `begin_drain(&mut self, context: &ProcessContext) -> PluginResult<()>`, default no-op on the four DSP traits with adapter forwarding. The host calls it once for an active native stage, after capacity/context preflight and before snapshotting `drain_call_bound`. This method performs bounded prepared work and emits no caller output; it must retain anything it produces. Repeated successful begin in the same epoch is a no-op. It is not a plugin initialize/reset operation.

## Exact setup work from current scheduler

Let C=256 (`oversampling/misc.rs:3`), f=2 or4, U=C*f child frames, and r be the wrapper's pending source frames. Successful ordinary process leaves `0 <= r < C` (`oversampler.rs:286-304`).

| State entering begin | Full resampling/setup chunks | Child process submissions | State after successful begin |
| --- | ---: | ---: | --- |
| No accepted source | 0 | 0 | Preserve empty-stream no-op behavior |
| Idle, r=0 | 1 UpTail | 1 × U | InnerTail |
| Idle, 1<=r<C | 1 padded Input +1 UpTail | 2 × U | InnerTail |
| Existing Input/UpTail (legacy direct drain already began) | Remaining 1 or2 steps | Remaining U submissions | InnerTail |
| InnerTail/DownPartial/DownTail/Complete | 0 | 0 | Unchanged |
| Failed | 0 | 0 | Return reset-required error |

The two setup steps are exactly the existing `process_chunk` operations (`:371-389`), including the normal downsampler. No extra transform, padding chunk, or child process call is introduced. The first completes r real frames with C-r zeros. The second all-zero input chunk flushes Rubato's finite upsampling overlap. Each passes U frames to the child and produces C base-rate frames into the residual output queue (`:534-646`). Child drain has not yet been invoked.

The hook must run these steps even when the wrapper already has queued output; it must not use the old drain loop's `residual_out_frames == 0` gate to postpone setup. After setup, call the child's own `begin_drain` with its saved child-clock context, allowing nested wrappers to prepare themselves before the outer bound is composed. Ordinary native plugins use the default no-op. Nested wrapper depth is a configured finite call graph; each layer adds at most two prepared chunks and its usual child processing work.

## Retained output capacity and exact delivery

The constructor seeds C output zeros (`oversampler.rs:126-130`). At every successful ordinary process boundary, with T accepted source frames and no previous EOF, it has produced `C + floor(T/C)*C` total queue frames and delivered T. Thus ready output is `Q=C-(T mod C)`, between1 andC. Input buffering holds `r=T mod C`; this proof uses the current pinned integer-ratio FFT stages that each emit exactly C base-rate frames per complete chunk.

Begin appends C frames when r=0, or2C when r>0. Therefore prepared queued output is at most **3C=768 frames/channel** (the sharper r>0 maximum is767). Existing minimum `residual_out` allocation is `(C+latency)*4` frames/channel (`:121-125`) and latency includes C (`:104-111`), so it is at least8C, already more than sufficient. `reserve_for_max_frames` never shrinks it (`:149-164`). A nonzero read cursor is handled by existing in-place compaction (`:503-531`) before appending. The proof concerns capacity after compaction, not the raw cursor+length.

For a hard no-heap success guarantee, preflight required queue length and scratch capacities before starting setup and compact once if needed. Do not rely on `ensure_residual_out_capacity` silently resizing if an invariant is violated. Setup storage is already prepared for max(U, declared child drain frames) by constructors/initialize (`auto_oversampled_plugin.rs:63-75,108-121`; generic counterpart `:67-80,125-145`).

All queued frames remain in `residual_out`; begin never copies into the host destination or increments its read cursor. Subsequent ordinary wrapper drain first serves that queue in <=C-frame pieces, then enters InnerTail and uses the unchanged tail/downsample scheduler. This preserves ordering and emits every setup sample exactly once. Repeated begin must not append the same chunks twice.

## Context and EOS adoption ordering

Use `next_os_context` already saved after ordinary processing (`auto_oversampled_plugin.rs:203-205`, generic `:211-213`). Its position is the next child input sample, including the wrapper's buffered-source offset; `misc::oversampled_context` converts sample/loop clocks and keeps the PPQ origin. Each successful U-frame setup submission advances it by U exactly once via existing `advance_context` (`misc.rs:33-41`). Child native drain does not advance accepted-input position. Do not replace it with host EOF sample position multiplied by f: that skips r buffered frames.

Convolution accepts queued prepared IR completion in `process_in_place` before normal processing (`convolution_plugin.rs:1217-1221`). Consequently completion can be adopted in either setup submission. Only **after both submissions**, and after child begin if any, may the wrapper snapshot the child's bound. Once in InnerTail, it never calls child process again; Convolution's native drain freezes the active IR and does not accept subsequent worker completion (`:1266-1271`). A successful setter accepted between outer calls is handled by the host's active-stage quota invalidation; it does not replay setup. A later unknown external asynchronous extension remains subject to that plugin's honest unknown bound policy.

This changes command interleaving granularity at the first EOS step from one setup chunk per host call to at most two. Host command polling still occurs immediately before and after this bounded call, and output backpressure occurs only when subsequently delivering queued frames. This bounded two-chunk setup tradeoff needs explicit acceptance; it is not sample-accurate external automation inside the two chunks.

## Invalid, repeated and failed begin semantics

1. Host applies already-supported queued graph changes/build, validates conservative caller destination (AUD090), stage channels/rate and prepared output/scratch geometry, then calls begin. A wrong-capacity host call never begins EOS.
2. The hook itself validates initialized state and rate, residual range/queue capacity and child scratch capacities before moving cursors, appending output or processing. Invalid preflight returns with all audio/control state unchanged. Explicitly store/validate initialized rate where the current wrapper only has a default context; do not mistake constructor's default48000 for completed initialization.
3. Empty accepted stream stays a no-op and does not freeze later input, matching current wrappers. Successful begin on a nonempty stream latches InnerTail setup completion and ordinary nonempty process thereafter keeps the existing reset-required error.
4. If processing or child begin fails after setup has started, set existing `DrainStage::Failed`; repeat begin/drain returns reset-required without retrying or duplicating work. A child process may already have changed state, so this is a **terminal partial failure**, not a retry-safe validation error. Host should not burn the successful drain-call quota, but also must not treat the node as successfully prepared. Reset restores the normal initial state. The engine already terminates playback for such an error.
5. Child begin must be called at most once after successful wrapper setup. A private boolean/explicit preparation stage is needed to distinguish 'setup is done but child begin not yet accepted' from an already prepared active InnerTail. Existing stages alone do not encode recursive child begin completion. This flag has no allocation and resets with the wrapper.
6. Direct native callers that never use the new hook must still be able to call wrapper drain: its first validated nonempty drain calls the same idempotent begin internally. This changes only the first-call bounded work limit to the declared preparation maximum; subsequent drain retains at most one native child drain or one downsampling step.

## Conservative post-begin composition

After begin, let Q be queued base-rate frames, B the child's declared successful-call bound, K its full native maximum output frames, and U=C*f. Native child calls and output transfer/downsampling are distinct wrapper calls (`oversampler.rs:392-411` explicitly returns zero after each child drain).

An economical checked upper bound is:

`ceil(Q/C) + B * (2 + ceil(K/U)) + 2`.

Reason: each child result costs one native call, plus at most `ceil(K/U)+1` transfer steps to combine it with a partial downsample chunk; the extra step also covers a zero-frame/complete child result's transition. Each full transfer produces at most C output frames and the host supplies C, so that output is delivered in the same transfer call. The final partial downsample chunk and down-filter overlap cost at most2 calls. This deliberately overestimates some paths; it does not infer minimum progress from K. B already counts zero-frame/noncomplete native calls. Return a minimum bound of1 for completed/empty wrappers; unknown child B yields None, with the explicitly documented unknown fallback limitation.

For a query after partial wrapper drain, include existing cached child frames and partial downsample state separately, or retain the already snapshotted child quota/initial conservative bound until next accepted control. Do not mistakenly count only future child calls while forgetting an already fetched child block. A safe first implementation may use a larger state-independent overhead for one cached K-frame block, but must document and test that term.

## Focused proof tests before acceptance

- For f2/f4, channels1/2/6, each residual r0..255, irregular ordinary callbacks: compare all subsequent output and exact context positions with the current stepwise setup reference; begin appends 1 or2 chunks once, source/output markers unchanged.
- Explicit long configured child with no complete pre-EOF inner chunk, and prepared Convolution replacement accepted in Input or UpTail: bound queried only after setup, succeeds beyond4096 steps.
- Repeated begin before any output and after partial queue delivery does no DSP/context work. Direct drain without prior begin is equivalent. Nested wrappers retain all samples and query the deepest child after its own setup.
- Queue read cursor near allocation end forces compaction; measured0alloc/0free on a cold thread with maximum retained Q and minimum constructor max_frames.
- Capacity/rate/scratch preflight failure is replayable against untouched twin. Inject first/second setup process failure and child-begin failure: terminal Failed, no repeated side effects, reset restores fresh behavior.
- Bound exhausted/malformed child and zero-frame child progress tests; query with an already fetched partial child block; final partial downsample plus overlap cannot outlive composed quota.

This addendum proposes a bounded preparation hook; no code is authorized by this document alone.
