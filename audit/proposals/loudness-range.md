# Proposed Loudness Range addition

Status: approved AUD117 implementation plan, 2026-09-28. The design review
was read-only; implementation was authorized after the wave15 checkpoint. MIDI/IAMF, host queues and engine
protocols are excluded. The main index was rebuilding; current source was read
directly. The stale sibling math graph was used only to locate symbols, then
the actual `../math-audio/crates/math-dsp` implementation was inspected.

## Standard and explicitly unperformed work

Use programme-loudness LRA in **LU**, per
[EBU Tech 3342, November 2023](https://tech.ebu.ch/docs/tech/tech3342.pdf), not SPL,
phon, sone, crest factor or loudness-compensation gain. The normative algorithm
uses overlapping three-second measurements at least 10 Hz, inclusive −70 LUFS
absolute gating, then a threshold 20 LU below their energy mean. Its reference
uses zero-based percentile positions `round((n−1)*0.10)` and
`round((n−1)*0.95)`; their level difference is LRA. This is more precise than
the ambiguous phrase “nearest rank.” A sole retained observation gives zero.

The published synthetic cases expect 10, 5, 20 and 15 LU, each within 1 LU.
They should also pass when repeated. Its reference recommends at least 1.5 s
of trailing silence for final file measurements. Feed that silence explicitly;
neither a query nor this feature silently extends a stream. Passing these
synthetics is not full EBU compliance. The official archive returned HTTP 403:
no authentic corpus was downloaded or tested. Those remain unperformed checks.

## Current source hooks

| Current location | Relevant behavior |
| --- | --- |
| `sotf-host/src/analyzer_loudness_monitor.rs:637` (`add_frames`) | Validates whole frames, subdivides at the backend's existing sub-block boundaries and counts completions independently of callbacks/publication. |
| Same file:741 (`query_shortterm`) | Aggregates the same channel-role-aware three-second measurement already exposed publicly. |
| Same file:175–285 (`ExplicitLoudnessMeters`) | Wide layouts aggregate mono-meter energies using semantic role weights and exclude LFE; narrow explicit layouts follow the existing remapping path. |
| Same file:36–163 | Prepared whole-program and rolling integrated stores show existing capacity/status/reset patterns. Their −10 LU gate and strict comparisons must **not** be reused for LRA. |
| Same file:752 (`update_loudness_data`) | Common scalar publication point, including internal AutoGain queries. |
| Same file:895–1065, 1235–1380 | Analyzer construction/builders and complete prepared snapshot/reset writers, including AUD111 nested-Arc readiness. |
| `sotf-host/src/analyzer.rs:370–505` | Public serde data, defaults and complete scalar copying. |
| `../math-audio/crates/math-dsp/src/ebur128/ebu_r128.rs:156–215` | The actual dependency completes fixed sub-blocks and retains 30 short-term energies. Querying short-term does not consume peak history. |

Use `query_shortterm()` at that existing boundary. Do not enable a dependency
LRA flag or average already rounded UI publications: neither establishes the
host's layout and timeline semantics.

## Public API and defaults

Recommended additive types in the host analyzer module:

- `LoudnessRangeMode { Rolling, WholeProgram }`, copyable and serde-compatible.
  A separate name avoids repurposing `IntegratedLoudnessMode`'s documented
  I-measurement meaning or adding variants to its existing enum.
- `LoudnessRangeConfig { mode, capacity_windows: usize }`, with a checked
  positive capacity no greater than 36,000. A default constructor selects
  rolling/36,000. Capacity counts short-term observations, not gated survivors.
- `LoudnessRangeStatus { WarmingUp, BelowGate, Valid, CapacityExceeded,
  MeasurementError }`.
- Copyable scalar `LoudnessRangeData`: `range_lu: Option<f64>`, `status`, `mode`,
  `retained_windows`, `observed_windows`, `capacity_windows`, and
  `timebase_is_exact`. Publish only finite `Some` values; zero is a valid range,
  while absent data is not represented as a plausible zero.

Add exactly one field to `LoudnessData`:

```
#[serde(default, skip_serializing_if = "Option::is_none")]
pub loudness_range: Option<LoudnessRangeData>
```

`None` means the optional feature is unprepared/disabled. A prepared cold
feature is `Some` with `WarmingUp` and no value. Every constructor, complete
writer/reset and `update_from` must copy/set this field. Do not extend nested
Arc payloads: LRA publishes scalars only.

Keep every existing low-level `LoudnessMonitor` constructor **LRA off**. This
preserves internal AutoGain memory and query work, including monitors created
through layout and integrated-mode constructors. Add a setup-only consuming
builder `with_loudness_range(Option<LoudnessRangeConfig>) -> Result<Self,String>`
and a scalar configuration getter. Changing the config after accepted input
must reject before mutating state; a same-value call can be a no-op. Enabling
mid-programme without resetting would otherwise include pre-feature samples
in the first short-term windows. Initialization before input remains supported.

The actual `LoudnessMonitorPlugin` should prepare LRA by default in **rolling
36,000-window mode** and expose the same optional builder for disabling or
selecting whole-program/custom capacity. Rolling is appropriate for an
indefinitely running display and must be labeled explicitly; it is not a claim
to retain an arbitrarily long programme. Whole-program is an explicit builder
choice. Keep LRA policy independent of integrated-loudness policy; configuring
one should not silently rewrite the other.

All monitor reconstruction sites (`initialize`, `with_integrated_mode`, layout
construction) must preserve and re-prepare the LRA config alongside spatial
mode. Builders compose in either order. Stage new storage before replacing the
old monitor/cache, and keep this preparation on the control thread. No new
realtime parameter ID, UI slider, bridge preset field or generic host API is
needed for the first patch.

### Compatibility

Old serialized snapshots deserialize with `loudness_range=None`; ordinary
Serde consumers ignoring unknown fields can read new snapshots. No existing
numeric field changes. Adding a public struct field is a source compatibility
change for exhaustive external Rust literals, though current workspace uses
the constructor rather than such literals. Document this explicitly and inspect
known sibling consumers read-only before implementation completion.

Preserve the current M/S/I/peak `measurement_valid`, `query_error`, and
`query_error_generation` behavior. Clarify that their validity covers those
existing measurements; optional LRA has its own explicit status. In particular,
silence below LRA's gate must not make formerly valid basic metering invalid,
and an LRA capacity failure must not erase usable momentary/peak data.

## Observation timeline and layouts

At each completed sub-block in `add_frames`, after all role meters have accepted
that same span, sample `query_shortterm()` **only if LRA is prepared** and at
least 30 sub-blocks have completed. This uses the backend's complete-window
geometry, not `frames_seen` before it is updated at callback end. Push exactly
one observation regardless of callback length, cache availability or user query
frequency. Empty calls do not sample; repeated publications do not duplicate
observations. All malformed dimension preflight remains ahead of accumulation.

Use precisely the same role-weighted aggregate as the existing short-term
display. Do not average LUFS across channels or independently gate each channel.
LFE-only content remains below gate. Layout permutation with the same semantic
roles must produce the same LRA; count-only multichannel construction retains
its existing `channel_layout_is_compliant=false` qualification.

The existing dependency uses `h=floor(sample_rate/10)` and a 30h-frame window.
For sample rates divisible by ten, that is the required exact three seconds
and 10 Hz. Other previously accepted rates must remain accepted: expose the
computed result with `timebase_is_exact=false` and document its inherited
30h-frame approximation. Do not silently label those clocks fully compliant,
change the shared backend clock, or reject previously supported rates solely
because the analyzer now prepares LRA.

Negative infinity is valid silence and occupies an observation slot. NaN,
positive infinity or an actual query error latches `MeasurementError` until
reset; do not silently omit an unknown window or convert it to quiet audio.
This is a conservative optional-statistic failure policy, not a repair to
preexisting nonfinite-input behavior elsewhere in the meter.

## Exact bounded histories

Use one private `LoudnessRangeHistory` module holding prepared arrays and scalar
indices. Store nonnegative linear energies rather than repeatedly quantizing
levels. The exact configured capacity is separate from allocator-reported Vec
capacity. Two initialized f64 arrays of C slots suffice: retained history and
query scratch. The default 36,000 costs 576,000 bytes (562.5 KiB) per **enabled
meter**, and no additional snapshot arrays.

- **Rolling:** retain the last C completed observations using a circular index.
  Silence still replaces old values, so wall-clock age is not extended by the
  gate. At normal rates C=36,000 means approximately the last hour of window
  endpoints; each observation itself summarizes the preceding three seconds.
- **Whole-program:** retain every observation without eviction. The first
  attempted C+1 observation latches `CapacityExceeded`, clears the range value
  and stops admitting further history until reset. Keep observed-count/status
  metadata honest; no rolling or histogram fallback. Capacity describes C
  observations, so the maximum programme duration also includes initial window
  warmup rather than being asserted as exactly C/10 seconds.
- **Reset:** reuse both allocations, clear logical history/counts/status/cache,
  and restart the shared sub-block phase with existing meter reset. No pending
  asynchronous worker or ownership retirement is introduced.

Capacity validation uses checked byte multiplication and `try_reserve_exact`
before resizing initialized storage; reject zero, overflow and over-limit
configuration at setup. The proposed hard limit bounds work and memory; it is
not an assertion that a particular machine can select percentiles from 36,000 values within every
possible audio deadline.

## Query algorithm and work bound

Maintain a dirty flag changed only by a new observation/reset/error. Querying
unchanged history returns a cached scalar result. In `update_loudness_data`,
refresh at most once for the history accumulated by that call, and only after
the existing cache readiness predicate succeeds. Multiple boundaries crossed
by one offline callback do not trigger intermediate selections. A held snapshot
can delay publication without changing which observations are retained.

For a dirty valid history:

1. Find the absolute-gated population using inclusive comparison. Compute its
   f64 energy mean; use a finite nonnegative scaled sum (divide energies by the
   maximum, sum, divide by count, then restore the scale) so summation itself
   cannot overflow. The scale multiplication occurs only after averaging.
2. Apply both absolute and relative gates inclusively, filling only the used
   prefix of prepared scratch. This distinction matters when relative threshold
   falls below the absolute threshold.
3. Use allocation-free `select_nth_unstable_by(total_cmp)` for the upper
   reference percentile, then select the lower rank in its lower partition
   (or reuse the upper value when ranks coincide). Subtract their logarithmic
   levels. The independent test oracle fully sorts its own values. Compute log differences, not a potentially overflowing energy ratio.
4. No survivors: `BelowGate`/`None`; one survivor: `Valid`/zero. Nonfinite or
   negative candidate results are explicit `MeasurementError`, not clamped to
   a plausible statistic.

Worst-case query work is bounded by O(C) with fixed C<=36,000 and two
prepared arrays. At conventional rates, dirty transitions occur no faster than
10 Hz; callback frequency alone cannot trigger additional selections. Queries and
publications do not alter audio, observations or peak intervals except through
their already existing basic-meter behavior.

This is intentionally the simplest exact bounded design. It is **not a claim
of a hard realtime deadline** at the maximum history size: measure dirty/full
and cached queries at realistic small audio callbacks. If that cost is not
acceptable, return to design review for a lower default capacity or an explicit
bounded incremental statistic; do not silently introduce percentile binning,
unbounded work, a callback allocation, or an unchecked background worker.

## AUD111 publication and lifecycle preservation

All three cache candidates start with correctly configured LRA scalar state.
The existing predicate checks writability of every nested peak/correlation Arc
before the entire snapshot writer. New LRA fields join that complete write;
never update a scalar LRA status before the predicate or replace an Arc to
publish it. Reset/disable/reenable use complete writers and the existing pending
clear bit. A retained reader may see the prior complete snapshot until a slot
is available, but must never see old LRA with newly cleared epoch fields.

Holding only a nested strong or Weak reader must continue to skip publication
without allocating. History sampling continues while the analyzer is enabled;
disabling the analyzer follows its existing reset/no-ingestion policy. LRA
configuration persists through reset and rate reinitialization; prepared
history and cached results restart. A reset to empty followed by a fresh
programme must match a fresh same-config monitor exactly.

## Independent permanent verification

1. **Pure algorithm oracle:** separately implement a small direct f64 LUFS-vector
   reference in tests, sorting its own vectors and using the published
   percentile equation. Include empty/silence, single/repeated values, inclusive
   threshold equality, relative-gate equality, nonuniform counts that distinguish
   rounding conventions, amplitude shifts, isolated outliers, errors, and
   wide finite energies. Do not call production gate/index helpers.
2. **Published synthetics:** generate the four tone programmes independently,
   append the documented trailing silence, reset each run, and verify their
   published limits. Also repeat each complete programme. These replace no
   missing authentic-corpus claim.
3. **Public timeline:** render identical stepped signals with callbacks
   1/17/137/8193 and boundaries h−1/h/h+1, query at different frequencies and
   after held-cache gaps, and require identical observed counts and final LRA.
   Test before/at first complete 30h window, zero frames and reinitialization.
4. **Layouts:** mono/stereo, explicit 5.1 and wide layouts with channel-role
   permutations, LFE-only and mixed LFE negatives. Compare with a separately
   accumulated energy reference rather than a fixed dependency channel map.
5. **History semantics:** small configured capacities make exhaustion/eviction
   tests cheap. Whole-program C/C+1 invalidation is stable on repeated queries;
   rolling silence ages earlier material out; reset recovers. Validate setup
   errors before mutation and builder-order preservation of spatial/integrated
   config. Verify LRA-off internal AutoGain paths create no new history.
6. **Snapshot/serialization:** old JSON without the new field, finite new result
   roundtrip, empty/invalid status roundtrip, `update_from`, complete reset,
   disable/reenable under held outer and nested strong/Weak readers. No mixed
   epoch payloads; released readers eventually permit a complete publication.
7. **Cold allocation/free:** construct/prepare on a control thread; first audio
   thread callback, first 3 s observation, first dirty percentile query, rolling
   wrap, small-cap whole-program failure, cached query, reset and cache recovery
   must add zero allocations/frees. Final owners drop outside the counter.
8. **Compatibility and cost:** existing audio passthrough/M/S/I/peak/correlation
   fixtures unchanged; compare prepared LRA off/on with identical inputs.
   Record memory, add_frames cost, first/full dirty query, repeated cached query,
   and retained-publication cost. Run host full tests/strict Clippy and relevant
   analyzer facade/serialization checks after source freezes.

## Separate inherited realtime limitation

The actual vendored backend currently prepares integrated gating history for
6,000 entries (`ebu_r128.rs:92`) but admits up to 36,000 (`:174–180`). Its VecDeque
can therefore grow after roughly ten minutes even when **LRA is off**. The new
feature must not be credited with whole-hour zero-heap behavior based only on
short cold tests. Capture or repair that inherited issue separately under a
reviewed scope; this plan does not edit vendor allocation policy. Small-capacity
LRA wrap/exhaustion tests can isolate the new statistic without crossing that
unrelated threshold.

## Minimal proposed file scope and decisions

Production: `sotf-host/src/analyzer.rs`,
`sotf-host/src/analyzer_loudness_monitor.rs`, and one private range-history
module. Reexport additive public types consistently if needed. New focused
integration/algorithm tests; update only required complete writers and docs.
No plugin AutoGain source, shared smoothing math, EBU backend, UI, native
wrapper, host queue or engine protocol changes are required.

Parent decisions before coding: accept analyzer default rolling LRA on / lower
level default off; accept the explicit 36,000-window memory/query ceiling and
source-literal compatibility note; accept independent nested status rather than
changing old validity/error semantics; accept the documented nonstandard-rate
qualification and setup-only configuration policy. If full-history query CPU
fails the measured budget, revisit that tradeoff rather than broaden silently.

## Approved selection amendment

Rust [slice documentation](https://doc.rust-lang.org/std/primitive.slice.html#method.select_nth_unstable_by) specifies in-place allocation-free O(n) selection. Use exact reference ranks, without histograms. Report full dirty and cached query p50/p99/max at small callback sizes and prepared memory; these are local measurements, not deadline guarantees. Inherited backend 6000-to-36000 integrated-history growth is tracked separately (AUD119), with no vendor edits in AUD117.
