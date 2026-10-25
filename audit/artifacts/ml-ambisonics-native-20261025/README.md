# Native Ambisonics auditions for offline personalization

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The audit permits offline learned HRTF or layout personalization while retaining
the normative decoder. This checkpoint supplies native speaker-layout auditions
and explicit energy constraints needed before fitting listener preferences. It
does not implement the preference learner or complete the proposal.

`qa-ambisonics-render` constructs the unchanged Ambisonics decoder followed by an
actual diagonal Matrix plugin. It supports orders1..7, registered named layouts
and validated custom geometry, mode matching and AllRAD, max-rE on/off, and
single/dual-band decoding. Inputs are explicitly ACN/SN3D; other normalizations
are refused. Geometry uses the native channel order, with0degrees front,
positive azimuth left, positive elevation up. LFE decoder rows remain silent and
their correction gains must remain1. Custom layout angles and the effective
preset report the native float32 values. No production decoder matrix, parameter
schema, automation path, default model or processing behavior changes.

The QA-only optional Matrix dependency is activated by `qa`. The renderer requires
complete native parameters, float32 little-endian interleaved PCM within+/-32,
44.1..192kHz and1..4096-frame callbacks. Requests are capped at64KiB; combined
input/output sample geometry including explicit observation zeros is capped at
4million samples, with at most65536 appended zero frames. Output files are
created exclusively. Cold processing and reset of both native plugins assert no
allocations or frees. Every output frame is checked for complete overwrite and
finite values. Native latency is0; dual-band recursive tail remains unknown, and
explicit zeros are processed input rather than a finite-drain claim. These
functional checks are not end-to-end host or platform acceptance; custom output
counts above16 remain DSP-only until engine admission is validated.

The Python wrapper checks native context, actual controls, matrices, exact PCM
byte counts, hashes and latency. It reports per-speaker angular diffuse-energy
weights `sum_n D[s,n]^2/(2*l(n)+1)`, where `l(n)=floor(sqrt(n))`. This is the
mean electrical energy for uniformly distributed unit SN3D plane waves, not
room power, calibrated loudness or localization accuracy. Deterministic offline
normalization projects squared gains onto preservation constraints for each
matrix and every pairwise cross term. For dual-band processing this preserves
LF, HF and their overlap, including arbitrary complex crossover responses.
The pseudoinverse uses relative cutoff1e-10 and normalization is verified after
float32 conversion within2e-6. Gains outside[0.8,1.25] are rejected after
projection, without clipping. Positive diagonal correction bounds arbitrary
output energy ratios between the smallest and largest squared gains, but does
not preserve every source direction's energy. The native renderer itself applies
only the explicitly supplied gains and performs no normalization or limiting.

All six Python tests pass. Independent associated Legendre harmonics verify the
SN3D addition theorem through order7; Gauss-Legendre/azimuth quadrature verifies
the input covariance and energy formula. NumPy SVD and Legendre polynomial
roots independently reconstruct regularized mode matching and max-rE weights.
SciPy cascaded second-order Butterworth sections reconstruct the dual-band LR4
audio. AllRAD audio is compared with its native exported matrix; its VBAP matrix
construction is not independently reimplemented by these checks. Tests cover
custom32-speaker arrays, exact callback/preset reopen, silent LFE, invalid
normalization/schema/geometry, overflow, nonfinite/out-of-range PCM and energy
constraints. All77 existing native tests pass, zero ignored. Strict all-target
Clippy and compilation without `qa` pass.

The retained48-case evaluation uses12 generated source arrays reused across
algorithm and band variants, at44.1/48/96/192kHz, orders1/3/7 and named5.1,
7.1.4 and custom32-speaker geometry. Noise, leading silence and isolated impulses
exercise native processing. They are conditional functional probes rather than
48 independent programmes or listening trials. All48 cases pass fixed gates:
waveform error<1e-5, native correction composition<1e-7, independent mode-matching
matrix error<2e-6, and normalized band/cross energy errors<2e-6. Maximum waveform
error is1.9492858928e-6, composition error7.2684400720e-9, matrix error2.8841007138e-8,
band energy error5.5932114829e-8 and cross energy error2.7319454931e-8.
17/193/4096-frame callback paths and JSON reconstruction reproduce exact PCM.
Maximum sample peak is0.1996574104 for these low-level probes; this does not
establish a full-scale ceiling, true-peak bound or desirable correction.

The initial evaluation and the separate-band-only normalization revision are
retained for traceability. Review added the necessary LF/HF cross constraint;
`evaluation.json`, `summary.json` and `*-cross.*` contain final results. Native
metadata was also corrected to serialize actual float32 geometry rather than
echo requested higher-precision JSON. The first build required correcting a
moved Rust result variable; Python lint fixes were confined to these new files.
Commands, timings, versions and source hashes are adjacent. No external
recordings, models or datasets were downloaded or redistributed. Final raw
requests/PCM remain in `/Volumes/home_tmp/tmp/ml-ambisonics-native-20261025-cross`.
Final native binary SHA256:
`9a1d95fd2db8b871f6f6f0ff2c99faa1fa7ffa184f7a40c5ac0e97c068bf9369`.

Next work is source-grouped, blinded listener preference learning with exact
listener/setup/decoder context, uncertainty and abstention, bounded candidate
admission, native headroom checks, explicit review/export and fresh JSON factory
reconstruction. Real recordings, spatial/listening quality, source/rate/layout
shifts, and Linux/Windows DAW/CI acceptance remain outstanding. No learned
benefit, production readiness or completion of the ML audit is claimed here.
