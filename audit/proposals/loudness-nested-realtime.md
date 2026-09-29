# AUD111 — coherent LoudnessMonitor nested-payload publication

Status: approved and implemented, 2026-09-28. Permanent regressions captured five
failures before the correction and now pass five of five. Full host verification
is recorded in `../loudness-nested-realtime.md`. The proposal below preserves the
reviewed design and original evidence.

## Executed public evidence

Source: `/tmp/sotf-loudness-nested-realtime-probe.rs`; complete output:
`/tmp/sotf-loudness-nested-realtime-probe.log`. Root reviewed the full source,
submitted the compilation with the active user-goal context, and ran it
successfully against the current host library. The probe uses only public
`LoudnessMonitorPlugin`, `Plugin`, `LoudnessData`, and `ProcessContext` APIs.
It counts both allocation and deallocation on fresh callback threads; all
fixtures, readers, output buffers and threads are prepared outside measurement.

Widths are 2, 7, 32 and 40, at 48 kHz. Each callback accepts 32 deterministic
sine frames and is also checked for exact audio passthrough. Both spatial states,
all three nested fields, and strong/Weak inner-only readers are covered. The
outer snapshot is released immediately after retaining its nested field.

| Executed group | Configurations | Observed result |
|---|---:|---|
| First/second/third callbacks with one retained inner field | 48 | First and second are 0 allocations/0 frees. Third revisits the retained candidate and allocates in every configuration. |
| Held empty matrix, spatial off | 8 of 48 | Third callback: 1 allocation/0 frees (new Arc header, no vector payload). |
| Held nonempty field, strong reader | 20 of 48 | Third callback: 2 allocations/0 frees. |
| Held nonempty field, Weak reader | 20 of 48 | Third callback: 2 allocations/1 free. The old Weak no longer upgrades after replacement. |
| Warm/reset with retained inner field | 48 | Reset is 0 allocations/0 frees, but 40 final snapshots contain an uncleared nonempty field alongside invalid flags, scalar peak 0 and correlation frame count 0. The other eight held matrices were already empty. |
| Explicit spatial toggle and restore | 8 | Cold reset, first process and each post-toggle callback are 0/0. Matrix geometry is correct; retained old payload values remain unchanged. Structural setter work is outside callback measurement. |
| `with_spatial()` after initialization | 4 | First callback: 2 allocations/1 free, because the builder changes the monitor without preparing cache matrix geometry. |

The 48 reset cases also measure one subsequent callback at 0/0; that callback
uses another slot and does not establish that the retained slot is safe later.
The reset failure is a snapshot-coherence failure: compliant readers can ignore
invalid values, but the published payload itself mixes cleared metadata and old
array contents. It is not evidence that the actual meter DSP failed to reset.

## Current source and cause

Paths below are under `crates/sotf-plugins/crates/sotf-host/src/`.

- `analyzer.rs:507`, `:518`, `:529`: the three `LoudnessData` writers allocate a
  replacement Arc/vector whenever authoritative `Arc::get_mut` fails or the
  prepared length is wrong. A unique outer `LoudnessData` is insufficient.
- `analyzer_loudness_monitor.rs:1224`: normal processing uses unconditional
  `cache.update` and reaches those writers after scalar fields have changed.
- `analyzer_loudness_monitor.rs:1274`: reset changes scalar fields first, then
  conditionally clears individual nested buffers; a held nested buffer remains
  old while that mixed candidate is published.
- `:961`, `:1073`, `:1128`: disable/reset visit up to three slots; reenable only
  changes `measurement_enabled`. Reusing an incompletely cleared spare can
  therefore publish pre-reset data again.
- `:988` prepares spatial geometry correctly on the control thread; `:1001`
  bypasses that preparation in the builder. `new_loudness_cache` already uses
  three independent payloads and does not need a new cache design.
- Cache reconstruction currently starts from `measurement_enabled=true` even
  if an existing plugin is disabled. The explicit spatial setter and integrated
  builder should preserve the owning enable state in freshly prepared data.

## Narrow implementation proposal

Own only `analyzer_loudness_monitor.rs`, a new public realtime integration test,
and this report. Reuse the accepted AUD107 `RealTimeCache::update_if` API.
Do not change `RealTimeCache::update`, the generic `LoudnessData` writers, shared
cache ownership, loudness/correlation arithmetic, UI schemas, or audio geometry.

### 1. Authoritative readiness before any writes

Add one private readiness helper taking `&mut LoudnessData`, width and spatial
mode. Require all three nested arrays to have both authoritative mutable access
and the prepared lengths:

- `Arc::get_mut(&mut channel_peaks)`, length `channels`;
- `Arc::get_mut(&mut true_peaks_dbtp)`, length `channels`;
- `Arc::get_mut(&mut correlation_matrix)`, length `channels * channels` when
  spatial is enabled, otherwise zero (checked multiplication).

Reject the whole candidate before any scalar/array change if any check fails.
Do not use separate strong/weak count observations. The helper creates/exports
no Arc or Weak owners; after successful `get_mut`, no outside owner can appear
before the writer while the unique outer borrow remains held. Repeating
`get_mut` in the existing writers is then safe and nonallocating.

Normal publication uses `update_if(readiness, full_monitor_writer)`. Measurement
ingestion remains before publication, so an unavailable cache does not stop the
meter. A rejected candidate is unchanged; rejecting both leaves the currently
published outer Arc identity and every field unchanged. The fallback slot can
still publish when the preferred slot has a held nested owner.

### 2. Coherent reset/disable/reenable and deferred publication

Retain immediate meter reset on reset/disable. Every successful reset-related
publication must run the complete `reset_loudness_data` writer through the same
all-three readiness helper. Never publish scalar-only enable updates.

Add a private boolean `clear_publication_pending`:

| Transition | DSP/control action | Publication action |
|---|---|---|
| Reset | Existing meter reset, preserve enabled state | Mark pending; attempt a complete cleared snapshot with current enabled bit. |
| Disable | Existing meter reset, set enabled false | Mark pending; attempt a complete cleared disabled snapshot. |
| Reenable | Set enabled true; meter was cleared on disable | Mark pending; attempt a complete cleared enabled snapshot. Do not reset the meter a second time. |
| Enabled callback | Existing input accumulation | Attempt a complete current snapshot; clear pending only after successful publication. |
| Disabled callback, pending | No meter ingestion | Retry one complete cleared snapshot; clear pending only on success. |
| Disabled callback, not pending | No meter ingestion | Leave the coherent snapshot unchanged. |

The existing reset/control loops may remain bounded to three attempts to avoid
an unrelated contention-statistics policy change. They no longer need to clear
every spare: all subsequent writers fully populate an eligible candidate. Any
successful attempt is sufficient to clear the pending flag. Never clear pending
because a candidate was merely inspected or rejected.

With all candidates retained, old telemetry deliberately stays intact until a
candidate becomes available; the scalar control getter still reports the new
enabled state immediately. After release, the next valid disabled callback can
publish the cleared state, or the next enabled callback publishes measurements
from the current meter epoch. No promise of asynchronous publication without a
callback/control operation is added. No obsolete intermediate reset snapshot is
required if enabled input has already advanced the current epoch.

`update_loudness_data` currently consumes the true-peak interval accumulator on
successful query. Conditional publication retains that existing behavior: while
publication is blocked, the true-peak interval extends until publication, as it
already does under outer-cache contention. Do not claim a new common peak-window
definition or redesign these meter queries in this patch.

### 3. Preserve preparation and structural semantics

Pass the owning `enabled` bit into the private cache/data preparation helpers so
all three fresh payloads have coherent state after initialization, spatial
changes, and integrated-policy builder use. This removes the need for an
initialization-time scalar-only rotation. Newly prepared cache state clears the
pending flag. Constructor defaults remain enabled.

Implement `with_spatial()` through the existing control-thread structural setter
so it prepares all arrays both before and after initialization. Preserve the
setter's current monitor-history behavior and matrix enable semantics. Geometry
changes/cache replacement are still control-thread operations, not allocation-
free callback operations. Explicitly document the builder's same requirement.

Do not change fallible initializer transactionality, detector numerics, meter
history policies or generic public snapshot mutation methods in this scope.

## Permanent red/green acceptance cases

1. Port the 48 retained-inner process cases and 48 reset cases as public tests;
   require 0 allocation/0 deallocation on every valid measured process/reset,
   and all-or-nothing coherent snapshots. Prepare IDs/readers outside counters.
2. Retain all three nested buffers together and separately across successive
   generations (strong and Weak), plus held outer snapshots. Exhaust both spare
   candidates; assert skipped publication preserves exact outer identity and
   fields and old readers' contents. Release selected owners, then assert the
   next successful snapshot reflects the current input count/data rather than a
   previously skipped generation.
3. Exercise warm -> reset, warm -> disable, disabled -> reenable, and repeated
   reset with all slots retained. Release while disabled and verify the next
   disabled passthrough callback publishes a fully cleared disabled snapshot,
   without advancing meter history. Release after reenable and compare the next
   current snapshot with a fresh twin fed exactly the post-reset programme.
4. Cover all three fields' Weak ownership. Existing AUD107 generic Barrier test
   already proves the separate-count race; no new cache algorithm is required.
   In plugin tests, no nested Arc may be replaced merely because a Weak exists.
5. Test cold/reset first processing, spatial off/on/off, builder before/after
   initialize, and disabled spatial/integrated-policy preparation. Structural
   preparation occurs outside callback counters; the first callback and valid
   enabled setters are measured 0/0. Verify geometry, enabled bit, and preserved
   ordinary passthrough. Include both analyzer-only and ordinary process routes.
6. Keep existing meter numerical/validity tests and full host tests, followed by
   strict all-target host Clippy. Do not lower numerical tolerances.

## Cost and limits

No additional nested arrays or reference DSP branch. Add one boolean per plugin,
at most six authoritative nested readiness checks per publication attempt, and
at most one extra attempted publication per disabled callback while a clear is
pending. Existing clear-array writes are performed only on eligible candidates.

Permanent reader retention can delay telemetry indefinitely; bounded cache
storage cannot provide both unbounded retained snapshots and guaranteed fresh
publication without allocations. Audio passthrough and meter state are not
delayed. Old snapshots remain valid, immutable snapshots of their original
generation. Generic `LoudnessData` mutation APIs outside this prepared plugin
remain allowed to allocate. This proposal addresses the measured nested cache
paths; it makes no new blanket claim about every loudness backend/control path.
