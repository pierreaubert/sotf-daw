# sotf-plugin-analog-common

Shared building blocks for the analog plugin family (`analog-eq`,
`analog-compressor`, `analog-limiter`). Not a plugin itself.

## What It Holds

- `AnalogColorStage`: one `math-analog` model (Harmonics, Static,
  Hammerstein, Tape, Transformer, Console Preamp) behind a uniform
  drive/color/character/trim surface, processing interleaved audio in place
- Shared parameter specs so all three plugins expose identical analog blocks
- 0 VU = −18 dBFS drive calibration

## Processing Contract

Prepare on the control thread for a fixed sample rate and channel count. Model
selection is structural; valid live drive, color, character and trim updates,
processing and reset require no heap activity. Oversized callbacks are chunked
at the prepared maximum. Only the requested interleaved frames are modified.
Rejected preparation preserves the previous layout, including when a positive
`f64` rate underflows to zero in the model's `f32` representation. Unknown model
defaults in parameter schemas fall back to Harmonics; unknown selections fail.

Zero color takes effect immediately, including after processing colored audio.
It preserves nominal dry samples while the model continues updating its internal
state. Nonzero color changes use a 10 ms smoother. Output trim scales the wet
model before the color blend; it does not scale the dry contribution. Console
drive maps to input gain after its input highpass, and character maps to asymmetry.
The −18 dBFS label is a proposed reference level, not a fitted hardware calibration.
The models sanitize nonfinite inputs to zero, clamp input to ±16 and bound output
to ±128, flushing subnormal output. These safety rails qualify sample identity.

## Conformance Evidence

`tests/audit_conformance.rs` covers all six models at 44.1/48/96/192 kHz:
zero color, independent dB gain/trim relations, physical-time nonzero color
transitions, callback partitioning, reset, rejected controls/layouts and finite
recovery. It measures heap activity and exercises 32 channels with an 8193-frame
callback. The drive equivalence uses binary amplitude gains to avoid reordering
rounding in Console's single-precision recursive input filter.

The decay test compares excited state against an unexcited baseline. Across the
24 model/rate cases, the final 100 ms residual of a two-second zero-input capture
is below 1e-5 peak. This does not establish finite support for recursive models.

`tests/audit_alias.rs` independently checks coherent spectral power against the
closed-form folded third harmonic of `sin³`. For a 9996.09375 Hz tone at 48 kHz,
+24 dB drive and full color, native nonharmonic in-band energy ranges from −23.11
to −15.64 dBc. Analytic input at 2x/4x rates followed by ideal spectral band
selection reduces this proxy in all six models. This is an offline model-rate
comparison; it does not measure a shipping resampler or establish hardware fidelity.
The 16x/768 kHz diagnostic is nonmonotonic for Console, so it is excluded as an
alias reference. Console's recursive `f32` path needs separate numerical
validation at that extrapolated rate; the cause has not been isolated here.

Per-model spectral and decay values, dependency revision and the exact test
command are recorded in
`audit/artifacts/plugin-audit-20261018/analog-common-conformance.json` at the
workspace root. Physical-device fitting and alternative model architectures
require an identified target; optional ML work is tracked separately.
