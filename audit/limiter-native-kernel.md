# AUD010 phase 1: native private-kernel checkpoint

Date: 2026-09-28. Source frozen after verification. No 2x/4x path, new parameter,
host change, external wiring change, MIDI/IAMF edit, commit or publication.

## Scope

- New `sotf-plugin-limiter/src/lib/native_kernel.rs` owns the 27 existing DSP
  history/derived fields, detectors, sliding maxima and unchanged processing,
  coefficient update, initialization and reset arithmetic.
- `src/lib/limiter_plugin.rs` keeps all canonical parameter values, IDs/schema,
  public telemetry-cache ownership, validation, lifecycle, native finite EOS,
  latency and metadata. It passes Copy scalar controls and a mutable borrowed
  cache to the kernel. There is still exactly one public parameter owner/cache.
- `src/lib.rs` registers the private module. Existing private tests only follow
  moved `.kernel` field paths; the existing detector/ISP tests moved beside the
  same helpers without changing assertions or tolerances.
- `tests/native_baseline.rs` and its 96-line fixture add a permanent same-platform
  before-extraction regression. No dependency/manifest/schema changes.
- Approved integration plan copied to
  `audit/proposals/limiter-oversampling-integration.md`; its new telemetry section
  is **phase-2 proposal only**, not an alteration of native telemetry.

## Baseline captured before production extraction

`/tmp/sotf-limiter-native-before.log` records the unmodified production path:
4 rates (44.1/48/96/192 kHz) × 1/2/6 channels × 8 modes = 96 configurations,
each with two stream/reset epochs. Modes cover hard/soft, true peak, ISP,
dual release, independent/partial/full linking, wet/dry mixtures and lookahead
0/0.125/0.25/5/20 ms. Input lengths exceed one native telemetry interval even at
192 kHz. Inputs include deterministic dense audio, quiet passages, first/final
markers and NaN/infinity sanitation.

Each epoch changes controls at 17/255/256/257 frames and just after the telemetry
interval, using irregular 1/127/257/8192-frame processing and 1/17/256/1024-frame
drain capacities. Every output sample contributes its exact f32 bits to a
deterministic FNV-1a hash; separate hashes cover every observed public telemetry
field and every process/drain frame count, completion bit and remaining-call
bound. The test additionally asserts exact native latency tail length, trailing
sentinels, scalar getter values and stable completion.

All 96 recorded lines match after extraction with **no numerical tolerance**.
The fixture explicitly targets x86_64 Linux because libm/platform differences
should not be disguised as a portable bit-equality guarantee. Existing portable
numerical/streaming oracles remain unchanged. This is legacy behavior parity,
not a new independent proof of the legacy limiter's mathematical accuracy.

Original production source retained at
`target/audit-limiter-native-extraction/limiter_plugin_before.rs`, SHA-256:
`e1fa0cb2eebd0f2a3bf26f118226a7f34a7adb8f79687c28d8f64e91048248d2`.
Recorded fixture SHA-256:
`f9e9b372b1ed4ae8084b1e4651a9d399aeccd06f0042cffae4edfeb2a0373629`.

## Verification

- `cargo test -p sotf-plugin-limiter --all-features`: **125 passed**, zero failures
  or ignored tests. Log `/tmp/sotf-limiter-kernel-full.log`.
- `cargo clippy -p sotf-plugin-limiter --all-features --all-targets -- -D warnings`:
  passed. Log `/tmp/sotf-limiter-kernel-clippy.log`.
- Existing `cold_process_drain_queries_controls_and_reset_neither_allocate_nor_free`
  passes explicit `(0 allocations, 0 frees)` across its 12 rate/layout/settings
  configurations. Existing scalar automation and processing allocation tests
  also pass; no new callback allocation or owner was introduced.
- Mechanical source-body comparisons pass for processing arithmetic, DSP reset,
  coefficient arithmetic, and public setter/drain/bound/tail/process/latency/
  compile-metadata bodies after the explicit field/argument substitutions.
  No limiter equation, operation order, delay length or native gain smoothing
  changed. A mutable cache-borrow signature was corrected during compilation;
  no numerical mismatch was encountered.

## Phase 2 telemetry decision for review

The plan proposes measuring the composed path's actual per-channel output level
reduction against the full-latency-aligned dry reference, publishing the maximum
over channels at native cadence. This includes final protection and mix rather
than publishing an arbitrary private-stage envelope or adding unrelated maxima.
FFT passband loss and wet/dry cancellation would also be included and must be
documented. Final emitted ISP peaks would use the native finite reconstruction
oracle, while input peak remains on the original input clock. Phase 1 preserves
the old native telemetry exactly. The detailed convention/limits are in the
plan's final section and should be resolved before phase-2 implementation.

Root separately owns AUD102 engine conversion; it is not part of this delta.

## Independent review

Root reviewed the public-owner delegation, native lifecycle and exact baseline
fixture. A separate reviewer mechanically compared the retained pre-extraction
source with processing, initialization, reset, setters, drain, tail and compiled
entry points. No introduced blocker was found. Phase 2 remains separate work;
its proposed metering convention is still under review.
