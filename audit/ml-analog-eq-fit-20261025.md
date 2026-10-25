# Analog EQ offline native fitting — 2026-10-25

Plugin 7 now has an offline GP-assisted native preset fitter and an equal-budget
classical comparator. `qa-analog-eq-render` is feature-gated; no runtime inference,
production parameter, default model or realtime signal path was added. Reviewed
exports reconstruct through the production factory with the existing host adapter.

The fitter supports all 14 numeric native controls and six categorical color
models. The evaluation deliberately frees only low/high shelf gain and color,
with Harmonics/Static model selection. Generated pairs use the actual native
four-band EQ followed by its color stage. Twelve whole-source groups are split
8/2/2; held inputs have higher noise levels. The negative system adds an intrinsic
ten-sample delay that this plugin cannot represent. The two systems share sources
and are not independent datasets or measured hardware.

Both searches use a 128 unique-objective cap and the same local refinement.
Classical/GP use 127/127 objectives on the matched system and 126/127 on the
delayed system. Both matched fits qualify. Worst held relative RMS is
4.1999621847e-5 classical and 1.5706837834e-5 GP. Both delayed fits abstain with
validation and test waveform mismatch (relative errors 1.12–1.21). All four
refinements hit their function-evaluation limits; qualification does not imply
optimizer convergence. No general surrogate advantage is claimed.

`analyze_analog_eq_fit.py` independently recomputed all 507 recorded objective
losses from native PCM (maximum discrepancy 3.34e-16), held metrics and acceptance
decisions (maximum discrepancy 2.23e-16), candidate/output hashes, callback reopens
and GP mean/variance using a direct dense solve (maximum discrepancy 2.23e-16).
The raw sessions remain at `/Volumes/home_tmp/tmp/ml-analog-eq-fit-20261025`.
Reports and search observations are retained in
`audit/artifacts/ml-analog-eq-fit-20261025`; source hashes identify the implementation.

Nine Python tests cover independent SciPy four-biquad reference at four rates,
all six EQ→color signal paths, exact callback reopens, multichannel flat identity,
all-control search support, category independence, inward native f32 bounds,
preparation refusals, held PCM isolation, objective arithmetic, immutable candidate
checks, accepted native export and out-of-family abstention. Native plugin tests,
QA strict Clippy and default-feature check passed (20 native tests total). The focused production factory
test and CLI exit-code logs are retained with the other verification evidence.

No real calibrated hardware capture has been fitted. Measured hardware quality,
broader search spaces, configuration/rate/level shifts, identifiability and
perceptual acceptance remain unproven. The implementation ledger remains in
progress. The synthetic fixture is for regression tests and is never activated
automatically.
