# Native Stereo Imager reference for offline learned settings

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The audit proposes offline learned scene classification to suggest settings while
retaining predictable realtime width control. This checkpoint supplies the native
renderer and independent qualification path required before fitting that model.
No learned classifier, default model or automatic parameter decision is added;
the proposal remains in progress.

`qa-stereo-imager-render` accepts complete eight-field native parameters, actual
stereo float32 little-endian PCM,44.1..192kHz and1..4096-frame callbacks. Requests
are capped at64KiB, programme input at4million samples and explicit observation
zeros at65536 frames. Unknown/partial controls, nonfinite/out-of-range input and
invalid geometry are refused. Construction, initialization and reset are fresh;
process/reset paths assert no allocations or frees. Existing output files are
never overwritten. The renderer does no gain matching, normalization or limiting.

The Python wrapper checks geometry, effective controls, exact byte counts, finite
output, input/output hashes and native zero latency. Additional processed zeros
retain observable IIR state, without claiming a finite mathematical tail or adding
unreported alignment. Native static parameters and audio processing are unchanged.

Five Python integration tests pass. They independently reconstruct complementary
one-pole M/S filtering with f64 bilinear sections across four rates and both bass
settings; check exact neutral/dry/mono routes; compare complex impulse response;
verify JSON preset reopen at17/193/4096-frame callbacks; and reject invalid
requests. All60 existing native tests pass, zero ignored, and strict all-target
Clippy passes.

The retained96-case evaluation uses16 procedural stereo inputs reused over six
settings: correlated channels, dual mono, antiphase and one silent channel at four
rates. Variants share source material; these are conditional native probes, not
96 independent scenes or classifier holdouts. Independently discretizing an
analog lowpass prototype through SciPy's bilinear transform reconstructs all96
native outputs. Maximum waveform discrepancy is1.5088922423e-7 and mono-fold
discrepancy3.7252902985e-8. Neutral/dry routes preserve original PCM bits. Gate
thresholds were fixed before these renders (2e-6 waveform and5e-7 mono error), and
all cases pass. No DSP/control parameters were tuned against this evaluation.

Widening produces a maximum sample peak2.0536417961. Bounded width controls do
not imply a full-scale ceiling or a desirable scene setting. Future learned
proposals must explicitly qualify headroom and compare side-energy/mono/listening
consequences, rather than silently normalizing audio. These probes show native
behavior only, with no perceptual or learned-benefit claim.

Retained diagnostics: initial Python lint required bound local closure defaults
and explicit subprocess check handling. The first native probes used20Hz low
crossover because the legacy constructor accepts it; host validation instead
requires80Hz. Both rejected probes are in `python-tests-initial.*`. Qualification
now uses the stricter registry range80..1000Hz, and explicitly tests rejection
of20Hz. No existing production range or compatibility behavior was changed.

Commands, exit codes, timings, report/analysis and source hashes are adjacent.
Raw input/output requests remain under
`/Volumes/home_tmp/tmp/ml-stereo-imager-native-20261025`. They are generated data,
with no recordings or model weights downloaded or redistributed. Native binary
SHA256 is retained in `evaluation.json` and checked again after native QA.
Source-labelled model fitting, uncertainty/fallback, actual setting desirability,
fresh source/level/rate shifts, listening and host/resource/cross-platform
acceptance remain required. This infrastructure alone does not complete the
Stereo Imager ML proposal or the full audit.
