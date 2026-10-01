# sotf-plugin-dynamic-eq

Dynamic EQ — frequency-selective dynamics processing.

## What It Does

A parametric EQ where each filter band only activates when the signal in that frequency range crosses a threshold. Unlike a static EQ that applies constant gain, a dynamic EQ adapts in real-time — useful for taming resonances that only appear at certain levels or for frequency-dependent compression.

## Features

- **Per-band dynamics**: Each EQ band has its own threshold, ratio, attack, and release
- **Parametric EQ base**: Standard frequency, Q, and gain per band
- **Adaptive processing**: Gain changes dynamically based on signal level
- **Band shapes**: Peak, low shelf, high shelf, and pivot tilt
- **Per-band routing**: Stereo, Left, Right, Mid, or Side inside explicit
  stereo pairs (two-channel input defaults to `[[0, 1]]`)

Band count, channel linking, filter frequency/Q/gain/shape/slope, placement
and active/solo routing are structural: rebuild the plugin to change them.
Threshold, ratio, attack, release, knee, per-band dynamics overrides and mix
remain realtime controls. Zero-target-gain bands and a settled fully dry mix
use exact transparent fast paths; wet re-entry begins from deterministic
reset detector/filter state.

## Detector, response, and cut/boost laws

- **Peak**: Q-derived bandpass detector; peaking EQ response.
- **Low shelf / high shelf**: single lowpass/highpass detector at the cutoff;
  RBJ/W3C shelf response with slope `S` in 0.1..=1.0.
- **Tilt**: full-band peak detector (no sidechain filter); first-order pivot
  response with exactly `+gain` dB at DC, `-gain` dB at Nyquist, and 0 dB at
  the pivot frequency. Q and shelf slope are stored but ignored.
- **Cut/boost**: the smoothed above-threshold gain reduction `g` (dB) maps to
  an amplitude blend proportion `p = (A(g') - 1) / (A(G) - 1)` where
  `g' = clamp(g, 0, |G|) * sign(G)` and `G` is the target gain; negative `G`
  cuts, positive `G` boosts. Partial activation blends dry with the held
  full-target response; it never redesigns coefficients at the instantaneous
  gain.
- **Routing**: bands run in ascending index order; pairs run in
  `stereo_pairs` order. Mid/Side use `M = (L+R)/2`, `S = (L-R)/2` with the
  pair's left-channel filter state, matching `sotf-plugin-eq`.

## Supported configurations

- Sample rates: 100 Hz and up. Band frequencies must stay below 47.5%
  of the rate (20 kHz cap); strict construction and `initialize` reject
  anything outside that range without mutating state.
- Channels: one or more (the QA matrix covers 1/2/8/16/32).
  Left/Right/Mid/Side bands need at least two channels plus disjoint
  explicit `stereo_pairs`; two-channel input defaults to `[[0, 1]]`.
- Routed bands without pairs are rejected by the strict constructors
  (`try_from_params`, `try_from_params_at_sample_rate`). The infallible
  `from_params` legacy path instead keeps them as a documented silent
  bypass: emitted audio is bit-identical to the input and the band
  reports 0 dB of gain reduction. Factories and state-restore paths
  must use the strict constructors for routed configurations.
- Precision: `f32` audio buffers with `f64` detector/filter internals;
  zero-latency in-place processing (no lookahead, no latency samples).

## Architecture

```
src/
├── lib.rs     # DynamicEqPlugin implementation
└── params.rs  # Parameter definitions
```

## Testing

```bash
cargo test -p sotf-plugin-dynamic-eq
```

## License

Part of the SOTF (Sound of the Future) project.
