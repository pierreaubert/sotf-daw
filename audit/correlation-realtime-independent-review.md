# AUD107 — independent conditional cache publication review

2026-09-28. Read-only inspection of `/tmp/sotf-correlation-realtime-proposal.md`,
`sotf-host/tests/correlation_realtime.rs`, current RealTimeCache, CorrelationData,
and standalone correlation plugin source. No edits to Rust or new builds/runs.
TokenSave was used; its index rebuild was in progress, so current filesystem
reads supplied the relevant source bodies.

## Conclusion

No blocker in the revised design. A mutable readiness predicate using nested
`Arc::get_mut` is authoritative, unlike separate strong/weak count queries.
While the selected outer payload remains exclusively borrowed, a successful
nested ownership check proves that no external strong or Weak handle remains
from which sharing could arise before the writer. This proof assumes readiness
neither exports a clone/Weak handle nor changes rejected data, as the proposal
explicitly requires. The writer should still use `Arc::get_mut` for actual
in-place access, and prepared length validation avoids resize allocation.

The existing ownership-preserving publication swap is suitable: replacing the
shared Arc returns the previous generation, which keeps it alive while the
producer-local current is replaced. Taking a spare leaves its slot empty;
restoring the old generation into that slot does not destroy a last Arc on the
callback. The third prepared matrix is necessary for fallback with retained
outer or nested generations. It cannot promise fresh telemetry when every
candidate is retained; the documented best-effort contract is accurate.

Rejected candidates must never be swapped into publication. Both producer and
shared snapshot identity should remain unchanged on rejection or a busy lock;
reset still clears the monitor immediately even when publishing its cleared
snapshot is postponed. The subsequent successful publication must copy current
monitor count and matrix together. The public test for this ordering is strong.

The additive API is conditionally realtime: the callback and writer must honor
nonallocating/in-place obligations. An arbitrary user closure can allocate or
mutate on rejection; the type system does not enforce those documented limits.
Do not describe the primitive as independently guaranteeing zero heap activity
for unrestricted closures. Existing unconditional update semantics remain out
of scope and unchanged.

## Tests reviewed

The new integration file covers:

- all 77 initial partial-frame completion offsets at channel widths 2/7/32/40;
  first callback operations run on a fresh thread with allocations and frees
  counted separately, plus exact aligned-versus-fragmented matrices;
- cold process, cold reset, repeated reset/reuse, producer cache reads;
- held outer, nested, both, Weak outer and Weak nested reader cases;
- all retained generations causing atomic publication skip, unchanged snapshot
  identity/content, reset while blocked, and latest post-reset history after
  releasing a reader;
- independent positive/negative perfect-correlation signs and exact sample
  counts, so a perpetually stale matrix cannot satisfy the lifecycle tests.

Allocator setup and returned-data assertions occur outside measured callback
regions; the default GlobalAlloc realloc implementation routes through the
implemented allocation/deallocation methods. Thread-local counters use const
Cell state and do not allocate. No callback warmup is hidden in the fixture.

## Remaining deterministic API evidence

The current file predates the new generic cache method, so it cannot yet prove
its closure-call contract. Owner confirms the following will be permanent unit
or integration tests with implementation:

1. rejected primary candidate followed by accepted fallback: readiness order
   and at most two invocations; exactly one writer invocation;
2. both rejected: writer never called and both publication identities retained;
3. busy publication lock: neither readiness nor writer called, one contention
   event for one update attempt;
4. unchanged legacy unconditional update behavior;
5. Barrier-controlled Weak upgrade/drop interleaving showing authoritative
   `Arc::get_mut` rejection with 0 allocations/0 frees, followed by successful
   reuse once that retained strong owner is released.

Those additions close the reviewed test gaps. This is approval of the proposed
ownership argument, not verification of not-yet-applied production code. The
embedded LoudnessData nested matrix limitation remains explicitly separate.

## Applied source follow-up

Later on 2026-09-28, reviewed the applied `update_if`, prepared correlation
triplet, in-place reset, authoritative readiness helper, carry reservation,
four generic unit tests and permanent Barrier-controlled Weak interleaving.
No blocker found. `update` body remains unchanged. The new bounded loop tries
at most the two prepared slots, invokes its FnOnce writer only on acceptance,
and preserves generation owners during publication. The public documentation
now states the callback obligations. Generic rejection/fallback/busy-lock
checks and Weak evidence address the previously recorded gaps. No builds or
Rust edits were performed by this reviewer.
