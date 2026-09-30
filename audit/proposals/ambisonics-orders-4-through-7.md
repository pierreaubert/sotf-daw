# AUD-133 proposal: Ambisonics orders 4 through 7

Status: bounded design accepted by Astra on 2026-09-29; the named-layout
orders 4–7 implementation, independent numerical checks, lower-order
preservation, engine/AIFF/service-PCM routes, graph-model reconciliation and
paired Criterion measurements are complete. Focused gates pass. Final scoped
acceptance awaits a coordinated workspace rerun after the separate AUD-134
convolution fixes. Evidence is consolidated in
`audit/ambisonics-orders-4-through-7.md`; review history is in
`audit/reviews/AUD133-astra.md`. This is one bounded extension of the existing
decoder, not a claim of complete spatial-feature parity.

## Confirmed gap

The decoder documents and accepts only ACN/SN3D orders 1–3. The limit is
enforced by `spherical_harmonics::MAX_ORDER`, the `order` parameter range,
constructor and setter checks, fixed per-frame scratch, max-rE weights, the
AllRAD virtual-speaker table, and the factory input-width catalog. The current
factory advertises widths 4, 9 and 16 only. Orders 4–7 need 25, 36, 49 and 64
ACN/SN3D input channels.

The public `DecodeMatrix::build` API already accepts a `SpeakerConfig`, but the
plugin constructor resolves `target_layout` through a fixed named-layout list.
IEM's AllRADecoder guide documents orders through 7 and arbitrary physical
speaker coordinates. This proposal addresses higher input orders with SOTF's
existing named layouts only. User-authored layouts, imaginary-speaker editing,
decoder export/import, and their application UI remain a separate gap.

Relevant implementation paths:

- `crates/sotf-plugins/crates/sotf-plugin-ambisonics/src/spherical_harmonics.rs`
  (`MAX_ORDER`, ACN mapping, SN3D basis)
- `crates/sotf-plugins/crates/sotf-plugin-ambisonics/src/decode_matrix.rs`
  (order validation, AllRAD virtual grid, max-rE weights and rank diagnostics)
- `crates/sotf-plugins/crates/sotf-plugin-ambisonics/src/params.rs` and
  `src/lib/ambisonics_decoder_plugin.rs` (serialized config, parameter limits,
  channel scratch and plugin construction)
- `crates/sotf-plugins/src/factory/catalog.rs` (advertised input widths)
- `crates/sotf-plugins/src/factory/tests.rs` and
  `crates/sotf-engine/src/plugins/chain/plugin_chain.rs` (factory and output
  channel integration)

AUD051 tail support is committed. Preserve its behavior and test it against a
pre-edit baseline; AUD133 must not rewrite native tail or drain semantics.

## Proposed scope

- Extend the existing ACN/SN3D implementation to orders 4–7 while keeping
  orders 1–3, channel ordering, angular signs, normalization, defaults, and
  output layout choices unchanged. Correct the stale ACN square-root comment:
  floor(sqrt(acn)) gives the valid degree for every ACN index 0–63; do not alter
  the mapping algorithm without separate evidence.
- Size fixed input scratch for 64 channels. Expand plugin config and parameter
  metadata to orders 1–7 and advertise exact supported input widths
  `[4, 9, 16, 25, 36, 49, 64]` through the factory. Keep the serialized
  `order` field and parameter identifier unchanged so old order 1–3 configs
  remain valid. Continue requiring a host rebuild for structural changes.
- Extend max-rE weighting using the existing definition
  `w_l = P_l(r_E)`, where `r_E` is the largest root of `P_(N+1)`. Validate new
  orders against independent roots and Legendre evaluations; preserve existing
  order 1–3 weight values exactly.
- Add deterministic AllRAD virtual grids for new orders. Preserve existing
  grid counts 64/96/128 for orders 1/2/3. Candidate counts for orders 4–7 are
  256/384/512/512 virtual directions. The independent numerical probe below
  supports these candidates; the Rust test-owned oracle must reproduce its
  criteria before they are accepted. Do not relax the existing coefficient
  safety bound to make a layout pass.
- Allow all existing named target layouts to be constructed at orders 4–7 if
  they satisfy the existing decoder safety policy. Keep `DecodeQuality.rank`
  and condition semantics unchanged: mode matching reports the physical
  unweighted speaker-harmonic solve, while AllRAD reports its virtual-grid
  solve. They do not report rank of the final composed AllRAD loudspeaker
  matrix. Tests must separately calculate the active (non-LFE) physical output
  matrix rank and condition in f64 using a test-owned SVD. Use the standard
  f32-storage threshold `tau = max(rows, columns) * f32::EPSILON * sigma_max`;
  count singular values above `tau` as numerical rank. Report condition as
  `sigma_max` divided by the smallest retained singular value. Also report rank
  relative to input channel count, since a matrix can have full row rank and
  still discard input modes when it has fewer speaker rows than harmonic
  columns. Physical rank below input channel count is expected for sparse
  layouts; make no claim that 6–16 speakers reproduce 25–64 independent spatial
  modes.
- Keep AllRAD/mode-matching matrix construction outside `process()`. Measure
  setup resources and observed callback timing distributions/maxima; make no
  worst-case execution claim. Verify that order 7 single- and dual-band
  processing remains allocation-free. Preserve current latency, bypass, EOS,
  dual-band crossover, reset, tail and drain contracts.
- Do not change IAMF wire behavior, IAMF-specific code, MIDI, Upmixer, custom
  layout configuration, or broad host/engine transition protocols.

## Quantitative numerical acceptance

Evaluate the AllRAD grid as a quadrature design, not by direction count alone.
For `M` equal-area grid points, compute

`G[i,j] = (4*pi/M) * sum_k Y_i(direction_k) * Y_j(direction_k)`.

For degree `l`, the continuous-sphere SN3D norm is `4*pi/(2*l+1)`; different
ACN basis functions have zero cross integral. Normalize each Gram entry by the
square root of the corresponding expected norms. Require full column rank,
2-norm condition number `<= 4.0`, and maximum absolute entrywise deviation
from the identity `<= 0.01`. Independently form the virtual solve with the
documented relative singular cutoff `1e-7 * sigma_max` and regularization
`1e-6 * sigma_max`; require
`||Y^T D - I||_F / sqrt(channel_count) <= 1e-8`.

An exploratory source-independent SciPy/NumPy probe (SciPy 1.11.1) yields the
candidate values below. The committed Rust oracle must independently reproduce
the acceptance criteria; these measurements justify the proposed counts but
are not implementation evidence.

| Order | Channels | Grid | Rank | `cond2(Y)` | Max normalized diagonal error | Max normalized cross-term | Solve residual |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 | 4 | 64 | 4 | 1.7341 | 0.001141 | 0.002698 | 2.65e-12 |
| 2 | 9 | 96 | 9 | 2.2381 | 0.000607 | 0.005246 | 4.12e-12 |
| 3 | 16 | 128 | 16 | 2.6608 | 0.003268 | 0.009420 | 5.57e-12 |
| 4 | 25 | 256 | 25 | 3.0062 | 0.000458 | 0.004764 | 7.00e-12 |
| 5 | 36 | 384 | 36 | 3.3190 | 0.000454 | 0.003855 | 8.43e-12 |
| 6 | 49 | 512 | 49 | 3.6093 | 0.000816 | 0.003838 | 9.85e-12 |
| 7 | 64 | 512 | 64 | 3.8799 | 0.001267 | 0.005254 | 1.13e-11 |

The probe also evaluates max-rE roots and degree weights with independent
SciPy routines. Its continuous norm check uses an independent `N+1`-node
Gauss-Legendre by `(2N+1)`-point azimuth quadrature. Its maximum
normalized Gram error is below `4e-15` through order 7. Reproduce with
`python3 audit/proposals/ambisonics_grid_probe.py`; output is in
`/tmp/sotf-aud133-grid-probe.log`. Probe SHA-256 is
`8c19ef3483ff9afcd9e10b45ac5e9508a690625a280502b977e19ce9d8e7bb0d` and log
SHA-256 is `41b1758044db7a507b834a01afd5fda4ee53b4ff5c5fb9828c98ecacc1a457ee`.

Add same-grid independent matrix references using test-owned ACN/SN3D values.
For the oversampled AllRAD virtual grid, use a separately implemented
regularized column normal-equation solve: the accepted grid criteria bound its
condition number to `<= 4`, so squaring that condition remains well behaved.
Compose the reference with the unchanged physical VBAP remap and compare the
complete AllRAD coefficient matrix to production with maximum absolute error
`<= 2e-6` after f32 storage. This validates the virtual solve and its
composition without claiming independent VBAP algorithm parity.

Do not use a normal-equation inverse as the physical mode-matching oracle.
Sparse and underdetermined layouts have true null modes, and a tiny regularizer
does not implement the production solver's `1e-7 * sigma_max` truncated-SVD
cutoff reliably after forming a Gram matrix. Instead, implement a small,
test-owned one-sided Jacobi SVD directly on the physical speaker-by-harmonic
matrix. Build the reference decoder from singular triplets retained strictly
above that same relative cutoff, with the same `lambda = 1e-6 * sigma_max`
Tikhonov gain on retained modes. Assert expected numerical rank and null-mode
discard for layouts with fewer active speakers than harmonic channels, as well
as a full-column-rank physical layout where available. Compare the complete
mode-matching matrix against production at `<= 2e-6` maximum absolute error
after f32 storage. Independently measure the final physical matrix rank and
condition from non-LFE rows for every layout/order/weighting; report those
results separately from the existing `DecodeQuality` rank/condition, and never
use AllRAD's virtual-grid rank as a physical output-rank claim.

The Jacobi reference must report convergence and fail if its maximum normalized
off-diagonal column correlation remains above `1e-12` after the documented
iteration cap. Check retained singular-triplet reconstruction against the
original physical matrix and retained left/right vector orthogonality before
using its decoder as an oracle. Include exact null modes and modes on each side
of the `1e-7 * sigma_max` cutoff in small synthetic matrix tests, so rank
truncation is verified independently of production layouts.

The official AmbiX format describes the standard channel layout as ACN with
SN3D normalization. ETSI TS 103 491 uses ACN indexing and SN3D for Ambisonic
signals of order `N`. IEM's AllRADecoder guide supports orders through 7 and
user-defined loudspeaker directions. These sources establish the format and
feature comparison, not output-quality equivalence:

- [IEM AllRADecoder guide](https://plugins.iem.at/docs/allradecoder/)
- [IEM configuration files](https://plugins.iem.at/docs/configurationfiles/)
- [IEM AmbiX format reference](https://iem-projects.github.io/ambix/apiref/format.html)
- [ETSI TS 103 491 V1.2.1](https://www.etsi.org/deliver/etsi_ts/103400_103499/103491/01.02.01_60/ts_103491v010201p.pdf)

## Acceptance evidence

1. Before any production edit, hash the clean `9302797` Ambisonics/factory/engine
   source and capture deterministic decoded outputs for every existing order
   1–3, named layout, algorithm, max-rE state and dual-band state. Include dense
   multichannel signals and the committed AUD051 single/dual-band paths with
   irregular partitions, late input, reset, zero continuation and exact
   EOS/drain behavior. Preserve source copy and output arrays under the ignored audit
   artifact directory with hashes. After implementation, require bit-exact
   equality for all captured lower-order output arrays.
2. Independently verify integer ACN pairs 0–63, `(N+1)^2` channel counts, and
   selected order 4/7 SH values. Check all 64 order-7 basis impulses through
   the actual plugin for both algorithms, including ACN channel 63. Use a
   test-owned harmonic formula plus the quantitative quadrature, normalization,
   rank, condition and virtual residual criteria above; production
   `spherical_harmonics_vector` alone is not its own oracle.
3. Check max-rE weights for every order against independently solved largest
   roots of `P_(N+1)` and a test-owned `P_l` recurrence. Preserve established
   order 1–3 f32 weight bits exactly.
4. Build both algorithms across orders 1–7, every named layout, max-rE
   on/off and dual-band on/off. Assert dimensions, finite matrices/diagnostics,
   candidate grid counts, coefficient bound, and a same-grid independent
   AllRAD matrix reference. Report the existing `DecodeQuality` rank and
   condition with their actual semantics: physical unweighted design for mode
   matching, virtual design for AllRAD. Separately calculate and report the
   final physical non-LFE matrix rank and condition using a test-owned f64 SVD
   and documented threshold. Do not claim sparse outputs reproduce the full
   input basis or relax clear construction errors/safety policy.
5. Test factory/config paths for widths 4/9/16/25/36/49/64, parameter
   metadata, invalid orders 0/8, structural rebuild behavior, and unchanged
   construction of legacy serialized order 1–3 configs. Confirm engine output
   channel negotiation follows the target speaker layout.
6. Exercise order-7 processing at 64 inputs: all single-band basis impulses,
   dense finite/non-finite blocks, dual-band channel 63, irregular partitions,
   reset, exact frame counts, and committed AUD051 tail/EOS behavior. Guard both
   algorithms/bands for process/drain allocations and deallocations; constructor
   setup is outside the callback guard.
7. Capture pre-edit Criterion estimates for lower-order matrix construction,
   plugin construction/initialization, and callback processing on fixed fixtures
   and layouts. Repeat paired cases after the change, alternating order where
   practical. Separately measure order-7 setup and callback cost, including
   dual-band processing at the largest output layout. Report Criterion
   estimates and observed callback distributions/maxima; do not claim worst-case
   execution time. Run focused Ambisonics/factory/engine tests, strict Clippy
   and formatting, then the offline workspace gate excluding MIDI/IAMF. Capture
   exact source/lock manifests at gate start and end.

## Limits

Orders 4–7 on existing named speaker layouts extend input compatibility and
decoder computation, but the built-in outputs have at most 16 channels and may
be rank-limited. Custom layouts are needed to use denser loudspeaker arrays and
remain open work. The acceptance claim will be bounded to tested matrices and
the stated audio metrics; it will not assert equivalent localization, source
width, or energy distribution to IEM without comparative measurements. No
official EBU corpus or external decoder matrix is required for this batch. The
performance evidence is bounded to measured fixtures and host; no hard-realtime
worst-case execution-time claim is made.
