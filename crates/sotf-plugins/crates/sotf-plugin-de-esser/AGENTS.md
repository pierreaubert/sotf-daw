# sotf-plugin-de-esser

De-esser — sibilance reduction.

## Architecture

- `lib.rs` — Main `DeEsserPlugin`, implements `ParametricInPlacePlugin` trait
- `params.rs` — Parameter definitions (`DeEsserPluginParams`)

## Key Public API

- `DeEsserPlugin` implementing `ParametricInPlacePlugin`

## Testing

```bash
cargo test -p sotf-plugin-de-esser
```

## Important Notes

- ParametricInPlacePlugin — same channel count in/out
- Targets sibilant frequencies (typically 4-10 kHz)
- Wideband reduces the full signal; Split-Band reduces only the LR4 high output.
- Split-Band Mix is reduction depth on a phase-matched low+high reference, not
  a raw-input/wet crossfade.
- Q controls symmetric octave edge spacing; detector filter poles remain
  Butterworth (`1/sqrt(2)`) so bandwidth is not encoded twice.
- Frequency, Q, and Mode are structural. Do not rebuild filters/crossovers or
  switch topology in the audio callback.
- Threshold, Ratio, Attack, Release, Mix, Range, and Stereo Link setters must remain allocation-free.
- Range bounds the reduction envelope; Stereo Link interpolates in dB toward
  the strongest channel's smoothed reduction. Multichannel linking spans every
  channel. Both controls use sample-based 5 ms smoothing.
- Append parameters to preserve host indices; Range and Stereo Link use 8 and 9.
- Meter cadence is sample-count based at about 30 Hz and reset is deterministic.
