# AUD-133 proposal: Ambisonics orders 4 through 7

Status: source-grounded proposal. Astra's initial design review identified
acceptance refinements; work is paused before implementation. See
`audit/reviews/AUD133-astra.md`. This is a bounded extension of the existing
Ambisonics decoder, not a claim of complete spatial-feature parity.

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

The source survey found an unrelated accepted AUD051 tail-support change
currently staged in this crate. Preserve that work and its behavior; AUD133
must not rewrite native tail or drain semantics.

## Proposed scope

- Extend the existing ACN/SN3D implementation to orders 4–7 while keeping
  orders 1–3, channel ordering, angular signs, normalization, defaults, and
  output layout choices unchanged.
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
  grid counts 64/96/128 for orders 1/2/3. Initial candidates for orders 4–7
  are 256/384/512/512 virtual directions (at least eight samples per harmonic
  channel); confirm or adjust them only from rank, conditioning and independent
  matrix evidence. Do not relax the existing coefficient safety bound to make
  a layout pass.
- Allow all existing named target layouts to be constructed at orders 4–7 if
  they satisfy the existing decoder safety policy. Lower speaker counts may
  produce rank-deficient matrices; report their measured rank and do not claim
  that a 6–16 speaker layout reproduces 25–64 independent spatial modes.
- Keep AllRAD/mode-matching matrix construction outside `process()`. Report
  setup memory/time and worst supported callback cost; verify that order 7
  single- and dual-band processing remains allocation-free. Preserve the
  current latency, bypass, EOS, dual-band crossover, reset, tail and drain
  contracts.
- Do not change IAMF wire behavior, IAMF-specific code, MIDI, Upmixer, custom
  layout configuration, or broad host/engine transition protocols.

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

1. Capture the current source and deterministic decoded output for every
   existing order 1–3, target layout, algorithm and max-rE setting before
   production edits. Afterward, compare outputs against that baseline exactly
   or report any numerical difference with an independent explanation.
2. Independently verify ACN indices 0–63, `(N+1)^2` channel counts, and selected
   order 4/7 SH values from test-owned formulas. Add a dense direction/quadrature
   check for finite values, normalization and basis orthogonality; production
   `spherical_harmonics_vector` alone is not its own oracle.
3. Check max-rE degree weights for every new order against independent
   `P_l(r_E)` values. Preserve the established order 1–3 values.
4. Build both decoder algorithms across every new order and all named layouts.
   Assert finite matrices and diagnostics, expected matrix dimensions, virtual
   grid dimensions, coefficient safety bound, and explicit rank deficiency
   where the output layout has fewer independent speakers than input channels.
   A construction failure must remain a clear error; do not weaken safety
   thresholds.
5. Test config and factory integration for input widths 25/36/49/64, parameter
   metadata and invalid orders 0/8. Confirm a legacy order 1–3 serialized
   config still constructs unchanged.
6. Exercise actual plugin processing at 64 input channels: dense finite and
   non-finite blocks, single/dual-band, irregular frame partitions, reset,
   exact output frame counts, and retained tail/drain semantics. Add a measured
   callback allocation/deallocation guard for both algorithms and modes.
7. Run focused Ambisonics and factory/engine integration tests, strict Clippy
   and formatting. After source is frozen, run the offline workspace gate with
   MIDI and IAMF excluded. Capture exact source/lock manifests at gate start and
   end; report setup and callback costs without implying quality from timing.

## Limits

Orders 4–7 on existing named speaker layouts extend input compatibility and
decoder computation, but the built-in outputs have at most 16 channels and may
be rank-limited. Custom layouts are needed to use denser loudspeaker arrays and
remain open work. The acceptance claim will be bounded to tested matrices and
the stated audio metrics; it will not assert equivalent localization, source
width, or energy distribution to IEM without comparative measurements. No
official EBU corpus or external decoder matrix is required for this batch.
