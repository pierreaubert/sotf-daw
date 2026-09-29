# Correlation callback heap activity and conditional telemetry publication

**Status:** Implemented as AUD107. Historical design/reproduction notes follow; see [verified implementation](../correlation-realtime.md) for final source scope and test results.

Date: 2026-09-28. Prospective AUD107. Proposal pending parent review; no production edits by this agent. The temporary public probe executed unchanged after root supplied the active user goal and full probe for automatic review. The initial rejection incorrectly treated this assignment as outside the preceding SOFA task; no workaround was attempted. Root obtained approval with scope evidence and ran the original command. Measured results are in `/tmp/sotf-correlation-realtime-probe.log`.

## Scope and ownership

Root owns AUD106 ring removal and process validation in `analyzer_channel_correlation/channel_correlation_plugin.rs`. This proposal concerns the monitor carry buffer, standalone telemetry storage/reset, and an additive conditional-publication cache primitive. Coordinate the overlapping plugin file after AUD106 stabilizes. No MIDI/IAMF, manager, host routing or queue changes.

Applicable instructions read: workspace/host AGENTS, ms-rust universal/performance/safety guidance, sotf-plugin-host-dsp and host DSP checklist. TokenSave-first inspection followed by direct current source reads. No repository source/test edits made for this investigation.

## Concrete source paths

1. `sotf-host/src/analyzer_channel_correlation/channel_correlation_monitor.rs:57` allocates `partial_frame` capacity `channels−1`; `:101–103` appends the completing prefix until length is `channels`. The first split-frame completion must grow that vector for every positive split of a multichannel frame. A normal System-backed realloc can allocate and free, even though the accumulator arithmetic and final matrix remain correct. Reserve the actual maximum **channels** during construction; no DSP arithmetic or alignment change is needed.
2. `channel_correlation_plugin.rs:40` calls `RealTimeCache::new(CorrelationData::new(channels))`. `analyzer.rs:88–90` clones that data to initialize outer slots, sharing its nested `Arc<Vec<f32>>`. First publication therefore takes `CorrelationData::update_matrix_with`'s allocation branch (`analyzer.rs:279–295`), despite unique ownership of the outer spare.
3. `channel_correlation_plugin.rs:118–120` resets by replacing a payload with `CorrelationData::new`. This constructs a Vec and Arc inside reset and can free the previous payload's matrix when no reader retains it. Reset must reuse prepared matrix storage.
4. Independently owned `new_triplet` fixes initial sharing but is insufficient for arbitrary safe UI readers. A consumer can clone `.matrix`, release the outer `Arc<CorrelationData>`, and retain only the nested Arc. The cache sees an exclusively owned outer spare while the matrix writer still copy-on-writes. Weak references matter too: `Arc::get_mut` requires no weak references as well as a unique strong owner.
5. `RealTimeCache::update` (`analyzer.rs:133–165`) unconditionally swaps the chosen spare after the closure returns. Adding `if matrix_is_shared { return; }` inside a writer will publish old candidate data. A sequence of current count32, spare count0 and retained spare matrix can publish count0 again. This changes matrix/count identity and freshness while ostensibly skipping publication; it is not an acceptable fix.

The public API already supports retaining outer snapshots and cloning nested matrix Arcs. Realtime claims must include those legal reader behaviors; merely dropping UI snapshots quickly is not the contract.

## Minimal proposed implementation

### Prepared storage

- Reserve `channels` for the monitor carry buffer.
- Construct standalone cache with `RealTimeCache::new_triplet(CorrelationData::new(n), CorrelationData::new(n), CorrelationData::new(n))`. Three independently owned matrices and three outer allocations are prepared on the control thread. The third slot preserves the existing reset intention under readers retaining the two normally alternating generations.
- Reset monitor moments in place. For the selected publishable payload, zero the existing matrix, write diagonal1, set channels and samples_seen0. Do not replace the Vec or Arc.

### Additive cache operation

Proposed surface (name negotiable):

```text
update_if(can_update: FnMut(&mut T)->bool, writer: FnOnce(&mut T)) -> bool
```

- Increment update_count once per attempt and use the existing nonblocking publication lock.
- Inspect at most the two existing spare slots. A candidate must have exclusive outer ownership (`Arc::get_mut`, which also handles Weak references) and satisfy the `can_update(&mut T)` predicate. The predicate is allowed a mutable borrow only to perform authoritative nested `Arc::get_mut` checks, and must leave rejected data unchanged. Try fallback when the ordinary spare has a retained nested payload, even when its outer Arc is uniquely owned.
- Execute writer exactly once on the chosen eligible candidate, then use the current ownership-preserving publication swap. Return true only after publication.
- If neither candidate is eligible or the publication lock is busy, leave both producer current and shared publication unchanged, increment contention_count once, return false. No stale candidate is swapped, no matrix/count mutation occurs, no ownership allocation/free is introduced.
- Leave the existing `update` implementation and its caller behavior unchanged. The new operation is additive and used only by the standalone correlation publication/reset paths in this scope. Do not refactor general cache behavior or change other analyzers in the same patch.

For CorrelationData the predicate performs **`Arc::get_mut(&mut data.matrix)`** and verifies the prepared matrix length through that authoritative mutable reference. Do not use separate `strong_count==1` and `weak_count==0` reads: a safe Weak observer can upgrade between the first read and the second, then drop its Weak while retaining the resulting strong reference. The two reads can report1/0 even though another strong owner exists. This was a flaw in the first proposal and is explicitly withdrawn.

A successful `Arc::get_mut` proves that no other strong or Weak owner exists at one atomic ownership decision. Once it succeeds, no external observer remains from which sharing can be created before the writer; the cache holds the candidate's exclusive outer borrow throughout selection and publication. The writer performs its actual in-place access via `Arc::get_mut` as well. Readiness must not clone the matrix or create a Weak handle. There is no allocation fallback on a failed readiness check.

A `try_update(&mut T)->bool` closure could combine validation/write and try the fallback on false, but the chosen mutable readiness predicate plus exactly one writer limits the callback's side effects: rejected candidates remain untouched, and monitor scalar counts are only copied for a successful matrix update.

### Snapshot contract

- Correlation accumulation continues while publication is skipped. The next eligible publication contains the current whole matrix and its matching current samples_seen.
- Retained snapshots/matrices remain immutable, including while process/reset runs.
- If all three generations are retained, reset may postpone telemetry publication. It still resets DSP history immediately. No finite preallocated scheme can guarantee an immediate cleared snapshot under unbounded reader retention; document best-effort publication honestly.
- A successfully published reset legitimately changes samples_seen to zero. A skipped update must preserve the currently published value/Arc identity, and must never publish a previous spare's older counter.
- The callback may clone/drop non-last Arc references but must never destroy a matrix allocation or outer payload. Reader-owned final drops belong to the reader; producer slots keep their normal owners during publication.

## Public probe and permanent regression plan

Prepared public harness: `/tmp/sotf-correlation-realtime-probe.rs`. Intended binary: workspace `target/audit-tmp/sotf-correlation-realtime-probe`. Uses the already-built host rlib and a thread-local System-forwarding allocator counting **both alloc and dealloc**, with setup outside measured regions. Each cold operation group starts on a fresh OS thread; there is no callback warmup.

Prepared matrix:

- Monitor: channels2/7/32/40, every split offset1..channels−1 =77 cases. Count initial partial ingestion separately from first completion; compare bit-exact final matrix/count against an independently aligned call partition. Measured completion growth: **1 allocation/1 free in all77 cases**; initial partial ingestion0/0; all final matrices bit-identical to the aligned oracle.
- Standalone plugin: same four widths; cold first process, reset after first process, first process after reset, held outer snapshot then held nested matrix after releasing its outer snapshot. Output must remain exact pass-through. Measured at every width: first publication **2 allocations/0 frees**; reset after first process **2/2**; next process **0/0**; held outer **0/0**; retaining its inner matrix after releasing outer **2/0**. A completely cold reset separately measured **2/0**.
- Independent generic `new_triplet<CorrelationData>`: hold initial inner matrix only, publish into independent spareB, then cycle back to initial spareA. Measured first publication **0/0**, second publication **2/0**, at all four widths, despite independently prepared initial slots. This isolates the nested-reader defect from constructor sharing and the standalone ring.
- Direct current-cache no-op guard: publish count32, retain previous candidate's matrix, return without mutation from the next writer. Measured current API republishing **32→0** with **0 allocations/0 frees**. This demonstrates why allocation checks alone are insufficient.

Permanent suite after approval should assert0/0 and add:

1. Every split offset with repeated incomplete fragments, reset before/after partial completion, and exact matrix/sample-count partition oracle.
2. Cold first process and cold reset for all four widths, then ordinary repeated publication and reset/reuse.
3. Hold only outer, only inner, both, Weak-only inner, two generations and all three generations. Preserve retained contents exactly; verify fallback selection and skip when all candidates are unavailable.
4. On skip, assert published Arc identity, counter and matrix unchanged; after reader release, assert latest cumulative count and known correlation signs. A never-changing stale matrix is not success.
5. Reset under contention: DSP history clears even when publication cannot; once eligible, the next output includes only post-reset input. Test empty process separately if it is a documented publication trigger.
6. First callback and reset measured from fresh threads, without invoking reader-side `SharedCache::load` inside the counted callback. Include producer `cache.load` and diagnostics if those are part of the promised callback API.
7. Independent cache tests with plain scalar payload confirm unchanged unconditional-update semantics, accurate single-attempt counters, and no publication on predicate rejection.

## Adjacent limitation to keep separate

The embedded LoudnessMonitor uses an independently owned CorrelationData scratch buffer (`analyzer_loudness_monitor.rs:838,851`) and already creates independent cache triplets (`:1262`). Its scratch matrix is private and copied, so the standalone constructor-sharing defect does not directly apply. However, public `LoudnessData.correlation_matrix` is another nested Arc, and `LoudnessData::update_correlation_matrix` (`analyzer.rs:445–452`) also allocates when only that nested matrix is retained. The additive cache primitive can support a later narrowly scoped fix, but this proposal does not claim that changing standalone correlation repairs all existing payload types or all nested-buffer analyzer paths.

## Revised deterministic Weak evidence

The temporary probe now additionally contains `weak_count_interleaving` and `held_weak_triplet`; these additions are prepared and **not yet run**. The previously logged baseline remains valid.

- `weak_count_interleaving` uses a two-thread Barrier schedule: producer reads strong_count1; a reader upgrades an existing Weak and drops that Weak while retaining the strong reference; producer reads weak_count0. It asserts that the misleading pair is1/0 while authoritative `Arc::get_mut` rejects access without heap activity. After the reader releases its strong reference, the same authoritative check succeeds. Barriers/thread construction are outside the allocation measurement.
- `held_weak_triplet` repeats the independent prepared-triplet cycle at2/7/32/40channels with only a Weak to the initial matrix retained. Current allocation fallback is expected to allocate a replacement Vec/Arc and free the old Vec, because Weak does not preserve the Vec's value. Counts remain to be measured for this extension.
- The permanent corrected-cache tests must use that authoritative readiness helper in Weak-only, upgraded-Weak-to-held-inner, and held-inner cases; select fallback without mutating the held payload. If both candidates are unavailable, preserve current published Arc identity/count/matrix and return false. On reader release, publish the latest monitor state rather than a stale prior spare.
