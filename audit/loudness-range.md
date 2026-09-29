# AUD117 — prepared Loudness Range

The host now computes optional Loudness Range in LU using
[EBU Tech 3342](https://tech.ebu.ch/docs/tech/tech3342.pdf). The reviewed design
is in [the proposal](proposals/loudness-range.md).

## Behavior and scope

Actual `LoudnessMonitorPlugin` analyzers default to rolling history of 36,000
three-second observations. Low-level `LoudnessMonitor` constructors, including
those used internally by AutoGain, remain off by default. Setup builders can
disable LRA, select whole-program history, or choose a smaller capacity.
Changes after accepted audio are rejected; the same configuration is a no-op.

Observation timing follows accepted frames and semantic channel roles.
Silence occupies a rolling slot. Whole-program exhaustion and invalid
observations latch explicit unavailable states until reset. The optional
scalar snapshot has its own validity, counts and policy; it does not invalidate
existing M/S/I/peak measurements. At rates not divisible by ten, the inherited
backend clock is explicitly labeled approximate.

Two prepared f64 arrays consume 16 bytes per configured observation: 576,000
bytes at the default capacity. Dirty queries use exact in-place selection of
the two reference percentile ranks, bounded by O(C); repeated queries reuse
the result. The entire snapshot writer still requires every nested array to be
writable. Held readers can defer publication without delaying observation or
allocating replacement buffers. Reset and enable transitions publish a complete
epoch, including LRA. Rate and integrated-policy preparation preserve LRA policy.

The additive `LoudnessData.loudness_range` field defaults to absent when reading
older serialized snapshots. Exhaustive external Rust struct literals require
the new field. The three known sibling application/daemon literals use struct
update syntax and remain source-compatible by inspection; their applications
were not rebuilt. Existing UI and manually assembled external JSON endpoints
are not extended by this core feature.

## Verification

- Three private algorithm tests compare exact selection with an independent
  full-sort level-domain reference, including gates, rank rounding, silence,
  capacity/wrap/reset, nonfinite errors and finite energies whose direct sum
  would overflow.
- Five public tests cover the four published generated tone programmes and
  complete repetition, seven callback sizes, query-frequency independence,
  first-window boundaries, setup/capacity behavior, serialization and builders.
- Explicit six- and twelve-channel role permutations match independently
  derived stationary energy levels; large LFE-only input stays below gate.
- Three public history heap tests cover dependency/host storage and full LRA
  histories. The nonzero full-capacity case uses 8 kHz audio; old-growth storage
  checks additionally use accelerated, accepted 20 Hz silence clocks.
- All six nested-snapshot regressions pass, including a new warm LRA test with
  strong and Weak readers retaining all three generations through reset and
  disable/enable. Process, query and reset counters remain zero for allocations
  and frees in these fixtures.
- The complete host suite passed 693 tests with eight existing ignored doctests
  before the final additional retained-generation regression. The subsequent
  six-test retained suite and final strict all-target Clippy pass. The workspace
  checkpoint records the aggregate including that final test.

Logs: `/tmp/sotf-lra-algorithm.log`, `/tmp/sotf-lra-public-final.log`,
`/tmp/sotf-lra-heap-final.log`, `/tmp/sotf-lra-retained.log`,
`/tmp/sotf-lra-host-full.log`, `/tmp/sotf-lra-clippy-complete.log`.

The first nonzero full-history heap fixture incorrectly used the accelerated
20 Hz clock: the inherited weighting filter became nonfinite after 260
observations. It was replaced with a normal 8 kHz signal. Silence-only storage
tests retain the accelerated clock. No production numerical policy was changed
to make that fixture pass.

The official authentic-programme archive returned HTTP 403 and was not tested.
Generated requirements tests do not establish complete EBU compliance. File
finalization silence is supplied explicitly by callers. This patch does not
add automatic stream extension or arbitrary unbounded programme history.

## Local cost

Seven alternating stereo analyzer-tap trials, four seconds of warmup then two
seconds measured, report median ns/frame with the same compiled host artifact:

| Rate | Callback frames | LRA off | LRA on | Ratio |
| --- | ---: | ---: | ---: | ---: |
| 48 kHz | 64 | 103.103 | 103.439 | 1.00326 |
| 48 kHz | 512 | 96.667 | 97.134 | 1.00483 |
| 192 kHz | 512 | 21.898 | 21.929 | 1.00142 |

Those short programmes do not represent full-history query cost. Five isolated
trials of the actual history implementation, each with 2,000 dirty queries,
give the following median trial statistics:

| Capacity | Dirty median | Dirty p99 | Worst observed across trials |
| ---: | ---: | ---: | ---: |
| 2 | 0.080 µs | 0.101 µs | 6.683 µs |
| 600 | 2.815 µs | 4.238 µs | 9.078 µs |
| 6,000 | 25.448 µs | 30.588 µs | 63.530 µs |
| 36,000 | 255.473 µs | 277.114 µs | 335.004 µs |

Cached queries average about 7.7–7.8 ns locally. A full dirty query adds a
measurable callback spike even though new observations arrive only at 10 Hz.
These are throughput measurements, not a portable realtime deadline guarantee.
Code, logs and SHA manifests are `/tmp/sotf-lra-{query,process}-cost*`; executables
are under `target/audit-tmp`. The unrelated backend's long-run reserve issue is
fixed separately by [AUD119](loudness-history-capacity.md).
