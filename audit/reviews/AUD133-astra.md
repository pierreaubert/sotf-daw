# AUD133 Astra design checkpoint

Status: **pending design refinement; proposal only, implementation not accepted**.
Review paused at the user's requested commit/sync checkpoint. No Rust changes or
Cargo gates were made by this review. AUD131/AUD132 acceptance is unaffected.

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

Keep this review and `audit/proposals/ambisonics-orders-4-through-7.md` explicitly
pending. Neither document is evidence that AUD133 functionality is implemented.
No further edits, implementation requests, or gates are requested while paused.
