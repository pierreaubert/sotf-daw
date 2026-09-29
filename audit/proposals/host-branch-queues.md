# Branch frame retention proposal — UNAPPLIED, UNCOMPILED

Artifact: `/tmp/sotf-branch-queues-proposed.patch`.
Rebased on the host checkpoint with 489 passing host unit tests and clean all-target clippy.
The proposal itself was only parsed/formatted with rustfmt and checked with `git apply --check`.
No repository source was changed by generating or checking this artifact. Automatic approval review rejected integrating this broader queue rewrite; explicit approval is still required before implementation/integration.

## Concrete proposed behavior

- Allocate separate f32 and f64 queues at graph build for every edge entering a merge and each terminal of a graph with multiple terminal outputs. Fan-out consumers have independent queues.
- Append each predecessor's newly emitted frames once per callback. At a merge consume only the common available prefix; retain every unmatched suffix for the next callback. Existing per-node accepted-input cursors advance by the consumed count, including zero when a branch has emitted nothing.
- Apply channel routing and latency compensation only to consumed frames. Terminal output delay lines advance only across emitted frames, leaving retained suffixes untouched.
- Return the actual emitted frame count for graph processing. Existing linear-chain same-rate zero-padding stays unchanged. This matters because zeros inserted into a queued graph's logical stream would shift later real samples.
- A single terminal whose emitted output does not fit the caller's buffer returns an error; it is not silently truncated.
- The current draft allocates two maximum graph blocks per queue. It never grows a queue when appending; overflow returns an explicit error. Queue compaction uses bounded copy_within, not allocation.
- Reset clears all queues. Nonlinear drain continues to return an explicit unsupported-graph error; this draft does not claim complete DAG tail support.
- Regression draft now covers three callbacks: 4 input frames with delayed branch emitting 2, then 6, then 4. Expected merged output is `[0,2]`, `[4,6,8,10,12,14]`, `[16,18,20,22]`. The original fixture incorrectly replayed its held pair on every later call; corrected. A drain assertion prevents silent COMPLETE.

## Required work before this could be merged

1. **Capacity/processing budget:** two graph blocks is an explicit policy, not a proven backlog bound. Plugins currently do not expose a general maximum retained-output contract. Derive and validate queue capacity from supported buffering guarantees or add an explicit configured backlog limit. Cap consumption at each node's preallocated maximum input block. Otherwise draining a queue backlog can enlarge downstream processing scratch on the audio thread or exceed the work budget. Output-frame bound reporting must include permitted backlog release.
2. **Overflow recovery:** a merge can append earlier queues before a later append fails, and upstream plugins have already processed input. Add preflight capacity checks and a clear latched-error/reset contract; a retry must not append the same block twice. Add deterministic unequal-average-rate/non-converging branch tests.
3. **Lifecycle and precision:** rebuild currently replaces queue storage, and f32/f64 queues are distinct while plugin state is shared. Define a control-thread reset/rebuild policy and reject a precision switch with pending queues (or preserve their exact contents). Transport discontinuities must clear queued audio along with the DSP reset contract. Unchanged callback positions must not clear queues.
4. **DAG drain:** current explicit error is honest but insufficient for offline export requiring complete tails. A later approved drain design must drain plugins causally, deliver retained edge/terminal prefixes, flush compensation delays, bound progress, and either zero-extend exhausted branches under a documented mixing policy or reject unequal stream lengths. Never infer COMPLETE merely because no new frames arrived in one call.
5. **Latency semantics:** retention compensates production scheduling, whereas delay lines compensate signal delay. The resampler currently reports filter delay plus buffering availability in one value. Validate whether queued retention and compensation would double-count that buffering term. Integer edge compensation still has less-than-one-frame fractional alignment residual; this proposal adds no fractional delay filters.
6. **Tests/integration:** compile the full changed source only after authorization. Run f32/native-f64/fallback paths, variable callback partitions, root fan-out, separate terminals, sidechain/channel maps, latency compensation, burst/zero-output patterns, reset/seek/rebuild/precision-switch, saturation of capacity, undersized caller output, and many callbacks with an independent complete-stream oracle. Test cold and sustained callbacks with the installed counting allocator and assert no allocation on successful bounded processing. Verify serial/parallel parity and downstream engine handling of shorter actual frame returns.

## Decision requested

Approve completing and integrating bounded graph branch retention with the above required contracts and validation, while retaining the explicit unsupported-DAG-drain error until complete draining is implemented. This is approval for the broader implementation work, not a claim that this uncompiled draft is ready to merge.
