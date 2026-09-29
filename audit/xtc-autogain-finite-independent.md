# AUD104 independent finite-stream verification and matched CPU comparison

2026-09-28. Owned repository change: only new
`crates/sotf-plugins/crates/sotf-plugin-xtc/tests/autogain_finite_stream.rs`.
No production, private-test, host, metadata, GSC or documentation edits.

## Finite-stream matrix

One integration test exercises 12 configurations: rates 44.1/48/96/192 kHz ×
FFT sizes 128/2048/16384. Processing patterns `[1]`, `[17]`, `[137]` and
`[8193,137,1]` are distributed across the matrix. An independent twin receives
`[257,509]` source callbacks and ordinary zero continuation in
`[137,1,8193,17]` callbacks. Native drain uses repeating capacities
`[1,17,137,8193]`; its first one-frame call leaves a partial hop cache.

Each input is six seconds plus 17 frames of stepped three-tone stereo audio
with deterministic dense noise. Both real XTC filters and AutoGain are active.
A disabled interval from 2 seconds to 3 seconds plus 137 frames tests warm wet,
meter and limiter state. EOF routes cycle between enabled, settled dry and a
one-frame-old transition toward dry. Diagnostic snapshots require finite input
and output loudness, compensation magnitude >.01 dB, and limiter envelope <.99,
so the fixture is not an inactive-meter or identity-filter test.

Checks:

- Complete source prefix and all emitted tail frames match the ordinary-zero
  twin; **maximum waveform difference is exactly 0 across all 12 cases**.
- Input/output loudness and compensation snapshots agree within 1e−9.
- Startup silence is exactly N frames. Exact tail count is N for settled dry;
  otherwise `2N−H + ((H−S mod H) mod H)`, where H=N/4 and S is accepted frames.
- Caller canaries around process output and after each emitted drain prefix
  remain untouched. Per-call drain output never exceeds native H or capacity.
- Tail metadata remains latched at N or 2N−1 throughout partial drain and
  completion. Final call bound is 1; repeated completion is idempotent.
- Empty drain does not freeze an empty stream. Nonempty post-EOF input is
  rejected without touching caller output. Identical enabled snapshots remain
  valid; changed snapshots are rejected until reset. Reset removes all audio.

The current test passes in approximately 3 seconds. Strict single-target Clippy,
Rustfmt and scoped diff checks pass. Files:

- `target/audit-xtc-autogain-finite-stream.log`
- `target/audit-xtc-autogain-finite-clippy.log`

## Red baseline without live source swapping

The test was also compiled and run against an isolated reconstruction using
`/tmp/sotf-xtc-pre-autogain-clock.rs`, which already includes AUD099's aligned,
continuously warm bypass. All sibling modules and dependencies were identical
to the corrected snapshot. The original failed at rate44.1k/N128/sample67926:
actual −.008730844 versus expected −.008729146, error **1.697801e−6**.

Log: `target/audit-xtc-autogain-finite-before.log`. Both libraries and source
snapshots are under `target/audit-tmp/xtc-autogain-comparison/`; the two recorded
`build-*.json` files contain exact rustc commands. Construction source was never
swapped into the repository.

## Why the finite support remains valid

The new meter clock changes when gain targets update, not how long audio is
stored. Both source and zero-continuation paths execute the same accepted-frame
clock. The STFT and delayed dry ring retain their existing finite support;
AutoGain and limiter multiply current samples and cannot synthesize audio once
these histories are zero. The canonical-hop drain cache may advance processing
ahead of a short destination, but subsequent capacity changes only deliver its
already-computed samples. EOF support and controls stay latched throughout that
cache lifetime. The independent ordinary-zero comparison exercises this directly
without assuming any particular AutoGain callback cadence.

## Matched CPU workload and ordinary-path parity

Both snapshots were compiled with `rustc -O`, the same target CPU and identical
existing dependency artifacts. This is a local matched comparison, not a release
deadline guarantee. Each run processes the same six-second stepped tonal/dense
input with a timed disabled→enabled history. Initialization and source generation
are outside the timer. Seven trials alternate old/new order; the first is
discarded and the median of six is reported. Both versions include aligned warm
bypass, avoiding the older pre-AUD099 shortcut as a confounder.

| Rate | N | Callback | Old ms | Corrected ms | Change | Corrected / audio duration |
|---:|---:|---:|---:|---:|---:|---:|
| 48k | 128 | 512 | 18.684057 | 70.453003 | +277.076% | 1.1742% |
| 48k | 128 | 8193 | 17.636333 | 69.984446 | +296.820% | 1.1664% |
| 48k | 2048 | 512 | 18.745914 | 70.519845 | +276.188% | 1.1753% |
| 48k | 2048 | 8193 | 17.611556 | 69.638415 | +295.413% | 1.1606% |
| 96k | 128 | 512 | 33.243490 | 101.176159 | +204.349% | 1.6863% |
| 96k | 128 | 8193 | 32.791409 | 100.993859 | +207.989% | 1.6832% |
| 96k | 2048 | 512 | 33.119350 | 100.791074 | +204.327% | 1.6799% |
| 96k | 2048 | 8193 | 33.094839 | 100.565267 | +203.870% | 1.6761% |

The additional work is material: continuous loudness ingestion replaces the old
discontinuous one-in-ten-callback measurement. These percentages report the
measured cost, not an equivalence claim for AutoGain-enabled audio. AutoGain-off
complete source output is **bit-identical in all eight matched configurations**.

Artifacts:

- Build script `/tmp/sotf-xtc-aud104-build.py`
- CPU source `/tmp/sotf-xtc-aud104-cpu.rs`
- Build log `target/audit-xtc-autogain-comparison-build.log`
- CPU results `target/audit-xtc-autogain-cpu.log`
- Libraries/binaries/snapshots `target/audit-tmp/xtc-autogain-comparison/`

No broader suite was rerun by this reviewer; root owns the current complete XTC
suite, private causality/cold tests, release QA and aggregate gate.
