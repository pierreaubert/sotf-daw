# AUD107: allocation-free correlation publication and reset

Implemented and verified 2026-09-28. The preceding [proposal](proposals/correlation-realtime.md) records the rejected count-based readiness idea and the authoritative ownership correction. [Independent source/test review](correlation-realtime-independent-review.md) found no blocker. MIDI/IAMF, dependency revisions, embedded LoudnessMonitor publication and host routing are unchanged by this issue.

## Defects reproduced before the correction

A public standalone Rust probe linked to the existing host artifact measured actual allocations **and frees**, with construction outside each measured callback. The baseline included:

| Operation | Configurations | Allocations / frees |
|---|---:|---:|
| First completion of a split interleaved frame | All 77 offsets for 2/7/32/40 channels | 1 / 1 each |
| Standalone first publication | Four channel widths | 2 / 0 |
| Reset after first process | Four channel widths | 2 / 2 |
| Completely cold reset | Four channel widths | 2 / 0 |
| Hold nested matrix after releasing outer snapshot | Four channel widths | 2 / 0 |
| Independently prepared triplet, held nested matrix, second publication | Four channel widths | 2 / 0 |
| Independently prepared triplet, held nested Weak, second publication | Four channel widths | 2 / 1 |

Returning early from the old cache writer to avoid nested allocation was independently shown to republish a stale spare: `samples_seen` regressed from 32 to 0 with no heap activity. Therefore a zero-allocation check alone would miss an incorrect publication fix.

The first readiness proposal used separate strong/weak count reads. Root review found the race: a Weak observer can upgrade and drop its Weak between those reads. The extended probe deterministically synchronized that interleaving, observed counts 1/0, and showed authoritative `Arc::get_mut` correctly rejecting the still-shared matrix. The implementation uses that authoritative operation.

Probe artifacts/logs:

- `/tmp/sotf-correlation-realtime-probe.rs`
- `/tmp/sotf-correlation-realtime-probe.log`
- `/tmp/sotf-correlation-realtime-weak-probe.log`
- `/tmp/sotf-correlation-realtime-red.log` — all four initial permanent tests failed for the reproduced heap activity.

The automatic approval reviewer initially lacked the root user's all-workspace goal in this subagent context. No rejected execution was retried indirectly. Root reviewed the exact source/commands, supplied the active goal evidence, obtained approval and executed the probes/gates. This affected execution provenance, not test expectations.

## Implementation

### Shared cache

`RealTimeCache::update_if` is an **additive** API. The original `update` implementation and all its caller behavior remain unchanged.

The method tries at most the ordinary spare and the fallback spare, requires authoritative exclusive outer `Arc::get_mut` access, then calls mutable readiness. It executes the writer once on an eligible candidate and publishes with the existing ownership-preserving swap order. If no candidate is ready or the publication lock is busy, it keeps producer/shared snapshot identity unchanged. Each call counts one update attempt; a failed call counts one contention.

For nested matrices, readiness uses `Arc::get_mut(&mut data.matrix)` and checks the exact prepared length. A successful check establishes that no external strong or Weak reference remains from which sharing could be created before the writer. The writer accesses the same prepared storage. Reference-count snapshots are not used for nested eligibility.

Both closures remain caller-supplied code: for realtime use they must avoid allocations, frees and blocking. Readiness must leave rejected payloads unchanged and must not create or export new shared references to an accepted nested buffer. The API does not make arbitrary allocating closures realtime-safe.

### Standalone correlation

- The monitor reserves one complete frame, including the transient final sample appended during partial-frame completion. Accumulator arithmetic and channel ordering are unchanged.
- The standalone plugin prepares three independent matrix payloads at construction. It no longer clones one nested Arc across initial cache slots.
- Publication checks nested ownership before computing a candidate matrix. Reset fills an eligible prepared matrix in place with identity and count 0.
- Root's AUD106 direct ingestion, exact frame/width/rate/nonfinite preflight and constructor validation remain intact. This issue changes only publication/storage behavior in that wrapper.

The control thread now prepares three matrices instead of initially sharing one. Relative to the old cold constructor this adds two `channels²` f32 matrix buffers, their Arc headers and one outer cache slot. The additional matrix elements occupy `8*channels²` bytes (12,800 bytes at 40 channels), before headers. Carry capacity increases by one f32 for nonzero channel counts. No FFT, detector law, delay, or parameter/schema changes were made.

## Snapshot and reset semantics

Accumulation continues even when readers retain every reusable payload. Publication is best effort: if all candidates are held, current telemetry can remain stale until a reader releases a generation. Reset clears monitor history immediately, but a cleared telemetry snapshot can also be postponed under full retention. The next successful publication reflects only current/post-reset accumulated history.

A skipped publication never rotates an older spare into public view. Published matrix and counter remain one coherent snapshot. Retained outer snapshots, inner matrices, and Weak-accessible generations are not overwritten. The ownership swap retains both generations before replacing producer current, so callback publication does not destroy a last payload owner.

## Permanent evidence

New `tests/correlation_realtime.rs`: **5/5 passing**, no ignores, with a thread-local allocator that counts both `alloc` and `dealloc`. Callback setup is outside the measured region; there is no callback warmup.

- 77 split-offset configurations on fresh callback threads, including reset after an incomplete frame, repeated one-sample fragments, and bit-exact comparison with aligned ingestion.
- Eight cold histories across 2/7/32/40 channels: direct first process or cold reset first, then repeated reset/reuse.
- Twenty retained-reader configurations: outer, inner-only, Weak-inner-only, Weak-outer-only, and outer+inner. Verify fallback publication advances every block, retained data stays unchanged, and reset/reuse remains free of allocations and frees.
- Four all-generations-retained sequences: skipped publication preserves Arc identity/counter/matrix; reset clears DSP history while publication remains unavailable; releasing a reader publishes the latest post-reset correlation and count. Known in-phase/anti-phase signals provide independent expected matrix signs.
- Deterministic two-thread Weak upgrade/drop interleaving permanently demonstrates why separate count reads are insufficient and verifies the authoritative check without heap activity.

The monitor/plugin cold matrices run on 109 fresh callback threads. Across the plugin sequences, 224 valid process calls and 52 resets are measured for **0 allocations and 0 frees**. Reader creation/release and immutable-content checks are outside those measured callback regions. These counts cover valid processing/reset/publication, not constructor/initialize or error-message construction.

Four new private cache tests verify first-rejected/fallback-accepted order, writer exactly once, both-rejected writer zero, busy-lock predicate/writer zero, producer/shared Arc identity, single-attempt diagnostics, nested Weak rejection and resume. Existing ordinary-cache tests remain unchanged.

## Verification

Focused commands executed by root with `TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-tmp`:

```text
cargo test -p sotf-host --test correlation_realtime -- --nocapture
cargo test -p sotf-host --lib conditional_cache
```

- Five public realtime tests passed; `/tmp/sotf-correlation-realtime-green.log`.
- Four conditional-cache unit tests passed; `/tmp/sotf-correlation-cache-unit.log`.
- Full host gate passed **658 tests across 21 suites**, with 8 preexisting ignored documentation examples; `/tmp/sotf-correlation-host-full.log`. This checkpoint includes AUD106 and the contemporary AUD105 helper, before AUD110 precision changes.
- Strict all-target host Clippy passed with warnings denied; `/tmp/sotf-correlation-host-clippy.log`.
- Scoped diff check and Rust formatting passed.

## Explicit remaining limitation

The embedded LoudnessMonitor uses a private CorrelationData scratch buffer and independent `RealTimeCache<LoudnessData>` triplets. That prevents this standalone constructor-sharing problem. However, the public `LoudnessData.correlation_matrix` remains a nested Arc, and `LoudnessData::update_correlation_matrix` still replaces it when only that inner matrix is retained by a reader. Its publication path has not adopted conditional nested readiness in AUD107. This is a source-backed separate limitation; this report does not claim zero heap activity for every existing analyzer payload or held-inner LoudnessData reader.
