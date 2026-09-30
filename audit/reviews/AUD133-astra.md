# AUD133 Astra design checkpoint

Status: **ACCEPTED for the bounded named-layout orders 4–7, 64-input engine
admission and graph-model scope**. Native bridge/FFI/NIH and mounted higher-order UI
routes remain separately open as listed in the feature-route inventory.

### Final acceptance evidence

Verified the coordinated terminal result: **6100 passed, 19 skipped across
362 binaries**, 272.300 seconds. Log `/tmp/sotf-aud133-134-workspace-rerun.log`
SHA-256 `4c417bab556373409374329b1de38b30abab67cdbafc36908037eb23bb4dd449`;
2676-file start/end manifests match exactly at
`85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`.
All seven earlier workspace failures are closed in this run.

Two subsequent fixture-only lint changes replace `chunks_exact(64)` with
`as_chunks::<64>()` and assign through the existing Box. Their affected AIFF
tests pass 2/2 in `/tmp/sotf-aud133-postlint-fix-aiff.log`; the **actual changed**
`processing_scratch_prepares_order_seven_input_extent` passes 1/1 in
`/tmp/sotf-aud133-postlint-fix-scratch.log`. The earlier capacity log selects a
different supplemental test and is not substituted for this result. Selected
engine/bridge all-target strict Clippy passes in
`/tmp/sotf-aud133-postlint-fix-clippy.log`. Two-fixture-plus-lock manifests match
at `3d80364901e30d358d6b5190896966add772f4e76f32f816951e0a02f8ed7074`.
These changes do not require repeating the production workspace gate.

Final report reviewed with numerical, performance, format, capacity and route
limitations intact. No remaining finding in this bounded batch. MIDI/IAMF
exclusions and corpus-access hold remain; neither AUD134's UI nor the complete
audit is accepted by this disposition.

## Current findings

No current blocking source finding in the named-layout/core/engine/model scope.
The final report `audit/ambisonics-orders-4-through-7.md` correctly distinguishes
64-channel AIFF and service PCM support from unsupported standard wide WAV,
model reconciliation from mounted UI, and prepared-vector reuse from whole-
callback allocation freedom. The literal service PCM 64-positive/65-negative
fixture checks actual decoded spec and samples; its focused log passes 1/1.
Five prior Ambisonics workspace failures have focused correction evidence; the
historical 6093-pass/7-fail gate is not represented as green.

Timing correction: Criterion's automatic +8.14%/+2.36% comparisons used the
preceding candidate run, not the preserved pre-edit run. Direct log-center
comparison is 263.30/248.82 minus one = **+5.82%** for order-1 AllRAD matrix
build without max-rE and 692.81/677.47 minus one = **+2.26%** for order 3.
The report's corrected values and noisy point-estimate qualification are
verified. No further timing run is required. Historical notes below retain
their context; this paragraph supersedes their pre-edit attribution.

## Final scoped evidence review before coordinated gate

No blocking source defect found; cleared for the coordinated workspace gate.
Verified `/tmp/sotf-aud133-ambi-package-final.log`: 59 library, 21 integration,
2 tail and 1 pre-edit fingerprint test pass (capture ignored). Strict Clippy
passes in `/tmp/sotf-aud133-ambi-clippy-final.log`. The final workspace gate
will cover the subsequent literal-precision/lint cleanup. Actual component
test `aud133_order7_graph::order_edit_reconciles_64_input_ports_through_canvas_roundtrip`
passes 1/1 in `/tmp/sotf-aud133-graph-test2.log`; the earlier excluded-unit
fixture is not used as execution evidence. Sibling tests used temporary lock
475d5890…; original 2c87468c… was restored, without claiming that restored lock
was tested.

Paired Criterion evidence is qualified: lower-order AllRAD matrix setup has
measured regressions, including +8.14% order 1 without max-rE and +2.36% order 3
without max-rE. It does not establish universally unchanged CPU cost. Candidate
order-7 512-frame processing is approximately 396.5 microseconds single-band
and 1.044 milliseconds dual-band; AllRAD construct-plus-initialize is about
26.5 milliseconds. These are benchmark estimates, not callback worst cases.
Owner is consolidating the final report, memory bounds and exact provenance;
overall acceptance remains pending that report and the coordinated gate.

The user resumed after commit/sync at clean `9302797`. The revised proposal
resolves the virtual/physical diagnostic distinction, independent quadrature
criteria, all-channel coverage, actual baseline capture and performance scope.
No Rust changes or Cargo gates were made by this reviewer. AUD131/AUD132
acceptance is unaffected.

## Incremental production core review

Inspected the stable spherical harmonics, decoder matrix, parameter metadata,
scratch constant, plugin processing and factory catalog changes while the owner
continues tests/integration. No new production defect found in that scoped pass.
Order-7 normalization requires at most 14!, within the existing u64 table;
the recurrence and ACN 0–63 mapping remain valid. Added max-rE coefficients
match the independent SciPy design probe, with lower-order table values and
virtual grids preserved. Scratch derives 64 channels, crossover initialization
uses the actual input width, and both processing paths iterate that width.
Existing SVD truncation/regularization and diagnostic semantics are unchanged.

This is source review only. The test-owned oracle file is still being assembled
and was not treated as reviewed evidence. All-channel numerical/tail/heap tests,
baseline comparisons, and full engine/app/decoder integration remain pending.

## Incremental numerical evidence review

The new matrix references use an independently implemented Jacobi solve for
physical designs and normal equations for the well-conditioned virtual grid.
Virtual and final physical diagnostics are distinguished correctly. Public
all-64 basis processing compares each output with its matrix column, allowing
real layout nulls; the independent matrix test supplies the algorithm check.
Dual-band channel 63 is exercised with reset and partition replay. Lower-order
fingerprints cover 192 order/layout/algorithm/weight/band combinations with
stored pre-edit arrays, including continuation after a late input sample.

Two evidence refinements remain open: the test harmonic evaluator duplicates
the production associated-Legendre recurrence and sqrt ACN mapping, so add
independently derived polynomial values or provenance-backed SciPy vectors,
including axes/poles/parity and integer degree/m ACN enumeration. The synthetic
near-cutoff Jacobi matrix is diagonal and exercises no rotation; add known
orthogonal mixing and rectangular null modes, retaining current thresholds.

The owner reported 59 lib, 21 integration, 2 tail and baseline passes but did
not save those initial command outputs to logs. These are provisional reported
results, not an exact-source final gate. Logged scoped manifests/results will
be captured with the final stable implementation; no reviewer rerun requested.

Those two numerical-oracle findings are now **closed at source review** for
`decode_matrix_aud133_tests.rs` SHA-256
`a3c1b242122721b585e27bd32e3368634094254e61d9386dde087eba7dacfec6`.
The SH reference differentiates Rodrigues' explicit polynomial, independently
of the production recurrence, and enumerates degree/m ACN pairs. Generic
directions, axes and both poles are compared. The SVD fixture now mixes the
near-cutoff modes by known orthogonal rotations and includes a rectangular
rank-two/null-space case, preserving convergence and error bounds. There is
no separate antipodal-parity assertion in this snapshot; evidence descriptions
should not claim one. Final logged source-qualified results remain pending.

## Resumed numerical refinement

The independent SciPy probe uses a separate associated-Legendre implementation,
correct SN3D solid-angle norms, and Gauss-Legendre/Fourier quadrature. Candidate
grid criteria and explicit physical f32 numerical-rank threshold are coherent.
Before implementing the physical reference, distinguish truncated SVD from a
regularized row-Gram inverse: rank-deficient speaker layouts can make the latter
numerically unstable, and tiny singular modes are not treated identically.
Retain the documented cutoff explicitly, or restrict normal equations to the
well-conditioned virtual grid and use an independent rank-revealing physical
reference. Do not relax the coefficient tolerance to accommodate oracle error.
Baseline capture may proceed; it must finish before production changes.

The consuming application order limits/settings must also be inspected. Final
acceptance requires an order-7 configuration through actual engine conversion
and a 64-input processing chain, not factory width metadata alone. Coordinate
shared engine/factory files with the AUD134 owner.

### Bounded integration audit findings

- `EngineConfig::MAX_CHANNELS` in `types/config/engine_config.rs:125` is 16
  and validates both input and output. Decoder and processing prepared sample
  capacities derive from it. A bounded 64-input admission/capacity extension
  needs measured prepared-memory consequences and intentional device/output
  limits, rather than an unexplained global replacement.
- Sibling `app-gpui/components/plugins/ui_graph/consts.rs:16` caps workflow ports
  at 32 and clips actual node ports at lines 75–76. `ui_graph/plugin.rs:72`
  gives Ambisonics a four-input maximum. These obstruct higher-order graph
  editing; defaults may remain FOA but actual selected order must expose all
  its channels.
- Engine `required_input_channels` already derives `(order+1)^2`, and spatial
  conversion already passes the selected order unchanged. No additional cap
  was found in these two paths.
- Locked Symphonia 0.6.1 core supports `Discrete(u16)` and `Ambisonic(u8)`;
  the engine interleaver sizes from the decoded channel count. However its
  RIFF parser's `map_wave_channel_count` only accepts 1–26 channels (the
  dependency's own test rejects 27 and above), and extensible PCM uses a
  positioned 32-bit mask. A standard 64-channel WAV is not a proven source.
  Require an actual supported, user-accessible file/source through decoder
  and host with 64 distinct channels, and state format limitations. This
  finding does not authorize an unreviewed decoder/vendor rewrite.

Root authorized the bounded engine input/capacity and graph integration work;
the separate broad manager/concurrency approval restriction remains unchanged.

### Capacity implementation source pass

The stable change separates `MAX_INPUT_CHANNELS=64` from
`MAX_OUTPUT_CHANNELS=16`, retaining `MAX_CHANNELS` as the output compatibility
alias. Decoder and processing scratch now reserve the full 64-input extent
globally, including previous-host and recycle fallback vectors. This avoids
relying on a new preparation protocol when an existing narrow engine widens.
No new production defect found in the changed capacity lines.

Derived reservation consequence: the listed non-HAL decoder buffers reserve
36 MiB instead of 9 MiB; the two processing scratch plus four fallback vectors
reserve 24 MiB instead of 6 MiB. Combined increase is 45 MiB per engine for
these vector payload capacities. HAL adds a separate 2 MiB instead of 0.5 MiB.
These figures are not total engine heap/RSS, and require measured corroboration
in the final report.

Requested evidence refinements: assert literal 64/65 input and 16/17 output
boundaries rather than deriving both input and expectation from the constant;
start processing state narrow and verify full-width prepared reuse, including
previous/recycle buffers, at the maximum block under heap guards. Confirm actual
decoded 65-channel input is rejected at the intended admission boundary, not
only a manually populated EngineConfig.

Inspected SHA-256 values:

- config: `91adf7e67aa8f64e04d6fec467d50ae33c1eab8044919685147edbf617dc9ebc`
- decoder core: `ace327e47ad4bde6d9929c8d5a4bd4d9535d1c56b032281b45f68bbb608ba183`
- decoder constants: `4921695418b9c36447c6416fad00d4aa15f10013f2b80f5eb6c1be30495ec656`
- decoder tests: `643bd60c14e0526116b3f9846fc60e0682f9c4cb913191f28544d1e7c520ac09`
- processing state: `0a95a3496cdd34e268061f599efe78c5f6e774e894d4287d89f8ec7c1d0fb28b`
- processing tests: `c39e018e1e942fd6b60351b3bd9d0324b09d9a61797377be57ed6f14343fbc5f`
- config tests: `23b569663829f3098a5db8172606d4a2afae433cfea2b7d8b1bde35096915ddd`

No Cargo run by the reviewer; actual AIFF route and final gates remain pending.

The revised proposal now restricts normal equations to the well-conditioned
virtual grid and uses a separate one-sided Jacobi SVD physical reference with
explicit relative truncation and retained-mode regularization. This resolves
the design finding. The implementation review must check oracle convergence,
singular-triplet reconstruction/orthogonality and null-mode assertions as well
as decoder agreement. Actual baseline capture precedes production changes.

## Scope assessment

Orders 4–7 for existing named layouts are a bounded extension of the current
ACN/SN3D implementation. The 64-channel scratch and factory widths, setup-time
matrix preparation, preserved lower-order constants, and explicit exclusion of
custom-layout/export work are coherent. Existing speaker counts cannot establish
full higher-order spatial reproduction; the proposal correctly limits that claim.

## Findings to resolve when work resumes

1. **Separate virtual-grid and physical-output rank diagnostics.** In
   `decode_matrix.rs`, AllRAD obtains `quality` from `virtual_y` and subsequently
   replaces only `peak_coefficient` after composing the physical matrix. Its
   reported rank/condition therefore describe the virtual SH solve, not the
   final speaker matrix. Acceptance item 4 cannot use that rank to demonstrate
   physical rank deficiency. Specify separately measured physical matrix rank
   (excluding zero LFE rows) and virtual full-rank/conditioning checks. Preserve
   existing public diagnostic semantics unless an explicit API change is reviewed.
2. **Make numerical acceptance quantitative before choosing grids.** Eight
   samples per harmonic is a candidate grid size, not an accuracy guarantee.
   Specify independent quadrature and virtual solve residual/conditioning
   criteria, plus a denser-grid comparison or equivalent independent matrix
   reference. SN3D sphere integrals must use the correct degree-dependent norm
   `4*pi/(2*l+1)` and solid-angle weights; uniformly sampled elevation is not
   uniform solid angle. Include signs, poles, parity, and highest ACN 63.
3. **Preserve the actual current lower-order implementation.** Capture source
   and audio before edits, including the staged accepted AUD051 tail behavior,
   stateful dual-band processing and complete EOS. Add per-input basis impulses
   through all 64 channels, including channel 63, rather than relying solely on
   a dense input that could conceal truncation. Verify factory-created high-order
   processing and structural rejection/rebuild behavior without expanding into
   the blocked manager protocol.
4. **Qualify performance evidence.** Report measured callback distributions or
   maxima for stated fixtures, not a proven worst-case execution bound. Capture
   a pre-edit timing baseline if preservation/regression comparisons are claimed.

The source ACN mapping comment claiming floating square root fails at ACN >=48
also needs reconciliation: the existing floor-of-square-root mapping is valid
for the proposed 0–63 range. This is documentation/test coverage, not a reason
to introduce an unrelated algorithm rewrite.

## Checkpoint handling

### Engine and AIFF integration source pass

The input-64/output-16 split and globally prepared current/previous/recycle
scratch are coherent. The new AIFF fixture independently checks PCM decoding,
then compares all 16 engine output channels over the complete 512-frame input
against all 64 matrix columns. Matrix numerics are independently checked in the
separate algorithm suite. The 65-channel fixture proves engine-constructor
rejection, not rejection by the decoder. Focused route log
`/tmp/sotf-aud133-aiff-route-focused.log` records 2/2 passes; processing-capacity
log records 1/1. No reviewer Cargo invocation.

Inspected source SHA-256 values:

- AIFF route: `28fe02f459141314c45b2ee41b321c9174ddba1428b364f6e1248b5d9707b76d`
- Processing tests: `145ac3857201a46467c4cefb3f20979eeea6db200934343919a55b551a04e714`
- Sibling graph conversion: `9d72bb29e7d98b761a235b393115b3d41b2d6e88d474a8c86ecb2265a78dc275`
- Sibling node channel mapping: `eb7cc483e430843440249d05bac448047610e22cc5c8e3958d128dcdc612bfa3`

Pointer/capacity assertions establish reuse of the selected vectors, not absence
of every callback allocation. The graph fixture currently calls an extracted
connection-rebuild helper, bypassing full reconciliation and subsequent channel
propagation. Require a full reconciliation or mounted order-edit/persistence
route before claiming accessible end-to-end graph behavior. Sibling execution
and measured prepared-memory reporting remain pending; its dependency compile
blocker is separate from this source pass.

Follow-up graph source `1f25d2822ca0e26d61ba5d9f0b1b96c0e82ac1123f76d2f4c25c354b809a0dba`
now tests the full production model reconciliation, including channel propagation
after indexed order editing and canvas serialization. This closes the helper-only
test-design gap, pending execution. It is a model integration test with explicit
node/input width refresh, not a mounted click test. Remove the unused local
`GraphNodeId` import left in the AppState wrapper after extraction.

Keep this review and `audit/proposals/ambisonics-orders-4-through-7.md` explicitly
pending. Neither document is evidence that AUD133 functionality is implemented.
The preceding checkpoint was superseded by the user's explicit resume. The
proposal remains implementation-pending until source and executed evidence pass
independent review.
