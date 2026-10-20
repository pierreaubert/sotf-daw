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
- Successful initialize clears prior envelope/history and settles retained
  controls; rejected initialize preserves populated state.
- Recorded-sound attenuation, calibrated LF/HF analysis and reinitialize
  regressions are in `tests/audit_*.rs`. Fixture licenses and source hashes are
  under `tests/fixtures/recorded`; those audio licenses differ from Rust code.
- The detector is not speech-conditioned: report unwanted music/plosive/vowel
  attenuation explicitly. A clean native release stress-matrix pass is not
  established by focused supported-clock routing tests.
