# AUD120 — private AutoGain measurement subset

Approved implementation scope, 2026-09-28. This follows the frozen AUD112 causal
clock correction; no measurement frames may be skipped and no public API or
generic analyzer policy changes.

## Source evidence

EQ already ingests maximal callback/reference prefixes bounded by its 10 Hz
interval (`auto_gain_clock.rs:115-124`). There is no per-frame ingestion API
loop to replace. Generic LoudnessMonitor nevertheless performs four 12-tap
true-peak FIR phases per channel/frame at44.1/48k, stereo covariance, and
integrated history/query work. AutoGain exposes only selected momentary or
short-term LUFS and query-interval sample peaks. At192k the true-peak path is
unsupported and returns immediately; the historical cost increase drops from
about172ns/frame at48k to38ns/frame at192k. This is source-ranked attribution,
not a sampled profiler percentage.

Buffering a complete100ms interval could preserve published semantics but would
concentrate all measurement work into one callback. A canonical small batch
could amortize single-frame API calls but adds copying and does not remove the
per-sample FIR/covariance work. Neither changes the existing1024-frame benchmark's
principal cost. Preserve current EQ/raw boundaries and bounded existing spans.

## Narrow implementation

Replace only AutoGain's private input/output LoudnessMonitor and LoudnessData
fields with a private gain-meter wrapper around the same pinned EbuR128:
`Mode::M | Mode::S | Mode::SAMPLE_PEAK`. Query selected M/S using the original
`unwrap_or(NEG_INFINITY)` policy. On every refresh, query/reset sample peaks for
**all** channels, folding their maximum with zero just as the previous monitor.
No true-peak, correlation or integrated-LUFS state is needed or exposed.

The pinned backend processes every sample in the same channel/frame order and
completes the same floor(sample_rate/10) subblocks. M/S ring means are read-only;
omitting I only removes the separate unused gating deque branch. Neither the
K-filter nor gain recurrence changes. Keep the constructor/rate accepted range,
malformed-frame preflight and error text/policy, nonfinite propagation, empty
input, reset, disabled queries and current setter behavior. No parameter,
telemetry-field, default, latency or source/output scheduling change.

Own `host/src/auto_gain.rs`, a private module, tests and this report only.
`analyzer_loudness_monitor.rs` and `analyzer.rs` belong to concurrent AUD117 LRA.
The generic analyzer remains unchanged. Root separately owns the Ebu integrated
history preallocation fix AUD119; this optimization must not depend on that fix.

## Evidence before and after

1. Before production, capture direct public AutoGain rendered samples and all
   public scalar telemetry as float bits using immutable matching baseline host
   artifacts. Include variable input/output programmes, ordinary and absolute
   targets, disabled/enable, M/S switches, partial measurement refreshes,
   reset/rate reinitialization, and multi-channel widths. Keep the existing
   frozen EQ audio/telemetry/compiled/EOS tests and raw baseline unchanged.
2. Compare the private meter directly with the unchanged public generic
   LoudnessMonitor for widths1/2/6/24, rates44.1/48/96/192k and valid edge rates,
   irregular ingestion and refresh boundaries, first/late peak markers, silence,
   finite extremes, NaN/Inf followed by reset, and M↔S switching. Require exact
   f64 bits, with explicit NaN classification where applicable, and query-reset
   peaks from every channel.
3. Count allocations **and frees** for cold ingestion/refresh/reset and enough
   zero-input subblocks to cross the former unused-I capacity. All new private
   state has fixed constructor storage; do not weaken caller-ID ownership scope.
4. Run host and affected AutoGain caller suites/strict Clippy after focused
   tests pass. Preserve exact gain arithmetic; investigate every waveform or
   telemetry delta, rather than relaxing tolerances.
5. Recompile the identical retained two-channel/two-band EQ cost harness against
   before/after matching optimized artifacts. Run seven alternating trials and
   report medians versus both corrected AUD112 baseline and original skipped
   input baseline, retaining immutable rlibs/manifests. No portable speed claim.

If direct subset queries differ from the existing public meter's selected data,
stop and explain the precise difference before proceeding. No shared analyzer,
queue, manager, MIDI or IAMF edits are part of this change.
