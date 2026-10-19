# sotf-plugin-transient-shaper

Transient Shaper — SPL Transient Designer approach for attack/sustain control.

## What It Does

Controls the attack (transient) and sustain portions of audio with linked fast/slow envelope detection. Sensitivity moves a smoothly automated low-level gate; above that gate, the detector responds to signal shape. Attack and sustain can be boosted or cut independently without shifting the stereo image.

## Features

- **Shape-based with sensitivity gate**: Envelope ratios drive shaping above a smooth low-level gate
- **Linked multichannel detection**: One gain trajectory preserves channel ratios
- **Bounded headroom**: Shaping gain is bounded and a linked soft ceiling protects peaks
- **Attack control**: Boost for punch, cut for smoothness
- **Sustain control**: Boost for fullness, cut for tighter sound
- **SPL Transient Designer approach**: Proven algorithm for natural-sounding results

## Architecture

```
src/
├── lib.rs     # TransientShaperPlugin implementation
└── params.rs  # Parameter definitions
```

## Lifecycle and diagnostics

Callbacks must use the prepared sample rate. `initialize()` updates time constants
and starts a new diagnostic interval while preserving the audio envelopes; call
`reset()` when a new source or transport restart should begin with fresh history.
Reset clears envelopes, control ramps and published levels without allocating.
Bulk parameter updates validate every value before changing any target.

`get_data()` exposes immutable `TransientShaperData` snapshots at approximately
30 Hz: interval peak transient/sustain levels and the strongest shaping gain.
Neutral/reset gain is 1.0. Retaining snapshots never causes copy-on-write work on
the audio thread; publication can wait when both cache slots are held.

## Testing

```bash
cargo test -p sotf-plugin-transient-shaper
```

## License

Part of the SOTF (Sound of the Future) project.
