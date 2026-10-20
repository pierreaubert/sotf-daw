# sotf-plugin-linear-phase-eq

FIR EQ — parametric EQ with selectable linear or minimum-phase FIR convolution.

## Architecture

- `lib.rs` — Main `LinearPhaseEqPlugin`, implements `ParametricParametricInPlacePlugin` trait
- `params.rs` — Parameter definitions
- `lib/ordered.rs` — Ordered per-band cascade route (pair validation, bank
  construction, cascade processing, channel-aware response math)
- `lib/types.rs` — Params/band config, placement enum, update snapshot types

## Key Public API

- `LinearPhaseEqPlugin` implementing `ParametricParametricInPlacePlugin`

## Testing

```bash
cargo test -p sotf-plugin-linear-phase-eq
```

## Important Notes

- Linear phase preserves phase coherence and reports `fir_length / 2 + 32` samples, including partition latency.
- Minimum phase reports the 32-sample partition latency and avoids pre-ringing.
- FIR convolution remains more CPU-intensive than the standard IIR EQ.
- Any Left/Right/Mid/Side band selects the ordered cascade route: one stage
  per band slot in band order, latency `stages * (fir_length / 2 + 32)`
  (linear) or `stages * 32` (minimum). Legacy configs stay single-FIR.
- Band filter shapes update dynamically via `snapshot_config` /
  `prepare_band_update` / `try_commit_prepared_update` (caller-owned `Option`,
  allocation-free success and refusal with typed `CommitRefusal`) with a fixed
  crossfade; `commit_prepared_update` is the control-thread wrapper.
  Topology, counts, placements, FIR length, phase and auto gain stay
  structural (rebuild the plugin). `dynamic_host.rs` owns the detached mailbox,
  accepted snapshot, host render and reclamation path. Ordinary parameter
  setters remain structural; do not infer loaded native/FFI coverage.
- `channel_complex_response` answers single-channel excitation (diagonal
  transfer); during a blend it reports the committed target while audio morphs.

- Both host sample formats must reject a wrong prepared clock before mailbox
  consumption, accepted-state publication, output writes or f64 scratch growth.
- Dynamic accuracy is covered by `tests/audit_dynamic_response.rs`: independent
  f64 full convolution, all lengths/phases, maximum ordered support, EOF during
  the blend, and streamed complex response after reset. Taps come from the DSP;
  the separate prototype probes test designer magnitude only in resolved bands.
- Preserve the explicitly deferred FIR-M1 diagnostic and its 0.05 dB bound.
  Callback deadline misses and synchronous history priming remain limitations;
  bounded allocation-free work is not a portable realtime deadline guarantee.
