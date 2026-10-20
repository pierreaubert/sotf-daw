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

- `try_commit_prepared_update_with_budget` retains a partially primed candidate
  in its caller-owned slot and replays at most the supplied input-frame budget.
  Pending attempts do not accept controls or start a blend. Replay must outpace
  arriving input to catch up. The old synchronous method remains compatible.
- Partly primed state is tied to a stable per-plugin token, reset epoch and
  input-clock/ring cursor. Refuse stale configuration or lost/foreign history
  before mutation; never install stale primed state after a reset or ring loss.
  Reprepare/drop stale candidates off audio; no new realtime allocation/free.
- `dynamic_host.rs` uses the budgeted API by default: two target history frames
  per incoming sample. Keep queue/cancel/reclaim routing once per callback and
  at most one acceptance; do not make catch-up acceptance block-size dependent.
  Publish pending replay/remaining counts without advancing accepted controls.
- Reset/rate changes invalidate unaccepted partly primed candidates; EOF must
  emit the old-filter tail without accepting them. Retain payloads until
  off-audio cancellation/reprepare. Accepted-blend reset semantics stay intact.
- Factory, facade and embedded-engine tests derive their changed control
  acceptance clock from support-2 incoming frames. Preserve exact-zero blend,
  partition invariance, 1e-5 error and zero alloc/free bounds. Full load/native
  deadline compliance is still open; a frame budget is not a time guarantee.

- Live control snapshots include an opaque owner/generation `FilterRevision`.
  Validate it before any target priming in both commit APIs. Accepting any
  update, including a no-op, and every real rate change advances the revision.
  Reset alone preserves it. Keep detached snapshot revision and published
  accepted generation aligned with the DSP; pending work never advances either.
- Preserve the 63-bit revision ceiling and typed `RevisionExhausted` refusal;
  never wrap the seqlock generation or mutate rate/state on exhaustion. Tokens
  come from live snapshots, not settings, serde or hand-assembled literals.
- Accepted-revision fencing does not implement newest-intent request ordering:
  same-base jobs follow the existing queue policy. Keep that distinction explicit.

- `qa-dynamic-linear-phase-eq` is an explicit 108-row maximum-tap load diagnostic,
  not part of the existing Justfile's static diagnostic list. It uses raw
  physical/reported deadlines (margin 1.0), retains every row and exits nonzero
  on any missed deadline or invariant. Do not trim cases or inflate the quantum.
- Static `qa-linear-phase-eq` preserves its old 4x local jitter gate but labels
  it as such and reports raw horizon misses separately. Never infer queued-work
  or native callback compliance from that relaxed diagnostic.
- Full-load evidence currently fails; numerical/heap correctness does not close
  capacity. The ring-span experiment was discarded and its shared runtime is
  restored. Numerical experiment tests and candidate code are artifact patches,
  not extra production arithmetic-order requirements.
