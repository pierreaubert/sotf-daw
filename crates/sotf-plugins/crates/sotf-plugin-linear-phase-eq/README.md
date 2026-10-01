# sotf-plugin-linear-phase-eq

Parametric FIR EQ with selectable linear or minimum phase.

## What It Does

A parametric equalizer that uses FIR (Finite Impulse Response) filters instead of traditional IIR biquads. This eliminates phase distortion entirely — all frequencies are delayed by the same amount. The tradeoff is higher latency compared to a minimum-phase EQ.

## Features

- **Zero phase distortion**: All frequencies delayed equally
- **Minimum-phase mode**: Low-latency, causal response without linear-phase pre-ringing
- **FIR convolution**: Non-uniform partitioned convolution with a 32-sample head
- **Parametric bands**: Standard frequency, Q, and gain controls
- **High precision**: Ideal for mastering and critical listening
- **Auto Gain**: Normalizes the FIR's DC gain to unity. It is a predictable
  reference-point correction, not a perceptual loudness match.

## When to Use

- Crossover alignment where phase coherence matters
- Mastering chains where phase transparency is critical
- Situations where latency is acceptable (not live monitoring)

For the lowest CPU cost, prefer `sotf-plugin-eq` (minimum-phase IIR).

## Architecture

```
src/
├── lib.rs, params.rs       # Public facade and canonical parameter schema
└── lib/                    # FIR design, streaming convolution, and tests

The implementation preallocates steady-state partitioned-convolution state.
Band count/settings, FIR length, phase and Auto Gain are structural: rebuild
the plugin to change them. FIR design and planning therefore stay outside the
realtime callback and old/new filter tails cannot be spliced. Mix remains
realtime-automatable and is smoothed per sample. Linear-phase latency is
`N/2 + 32` samples; minimum phase reports the 32-sample partition latency.
```

## Per-band channel routing

Each band accepts an optional `placement` (`stereo`, `left`, `right`, `mid`,
`side`; absent keeps the legacy stereo-linked route). Any `left`/`right`/
`mid`/`side` band selects the ordered cascade route: every band slot occupies
one FIR stage in band order (identity when inactive, so latency never depends
on which bands are enabled), Mid/Side bands process pairs via 0.5·(L±R)
encode/filter/decode, and bypassed domains run through identity-FIR engines
to stay delay-aligned. Latency is `stages * (fir_length / 2 + 32)` samples in
linear phase and `stages * 32` in minimum phase. Pair placements need
explicit disjoint `stereo_pairs` except on stereo input, which defaults to
`[[0, 1]]`. `auto_gain` stays stereo-linked only.
`channel_complex_response()` / `channel_group_delay_samples()` report the
per-channel cascade response for charts, using single-channel excitation
(diagonal transfer: Mid/Side spreading included, not correlated input).

## Dynamic band updates

Band filter shapes (`filter_type`, `frequency`, `q`, `gain_db`, `active`)
update without a rebuild: capture `snapshot_config()`, design
`prepare_band_update()` off the audio thread, then
`try_commit_prepared_update()` with a caller-owned `Option` slot (bounded,
allocation-free on success and refusal; primes the full cascade support from
recorded history; retains the prepared update on typed `CommitRefusal` for
correction or retry) to start a fixed 513-frame exact-0/1 output crossfade.
`commit_prepared_update()` is the control-thread compatibility wrapper (it
allocates its `String` error and frees the prepared update on refusal).
Phase mode and latency never change across an update. Reclaim the previous
banks with `take_retired_route()` off the audio thread before the next
commit. At commit, controls and the chart-facing response APIs report the
target design immediately while audio morphs old-to-new over the blend.

This is currently a DSP-level API: no engine/FFI/NIH automation path adopts
it yet, so host band controls remain structural-rebuild with truthful refusal
(see the R3 shared-patch notes in the audit directory).

## Testing

```bash
cargo test -p sotf-plugin-linear-phase-eq
```

## License

Part of the SOTF (Sound of the Future) project.
