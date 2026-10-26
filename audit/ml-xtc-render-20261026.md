# Rendered learned XTC: independent WOLA and measured-ear validation

Issue10/plugin44. A new QA renderer exports complete native speaker audio,
prepared filter matrices, the analysis window, normalization and fixed profile.
Read-only QA accessors expose accepted matrices; installation and processing
behavior from the preceding checkpoint are unchanged. The original measured
model/proposal remains frozen and unadmitted.

The renderer accepts finite stereo source values bounded by64, at most one
million frames and callbacks1–1024. Request/proposal reads and finite drain are
bounded, output creation is exclusive, and process/drain/reset heap guards stay
active. Invalid proposals retain native filters and report the rejection. The
qualification copies the executable before use; frozen SHA256 is
`1ea63525cb955e337cecfacecd0a02351d9f1fa9d42e4431ec0e2f0e6c2c2614`.
Model/proposal identities and the unchanged measured SOFA hash are in the report.

Profile: stereo44.1kHz, FFT2048/hop512, centered±30-degree speakers at1.4m,
AutoGain off and native final linked safety on. Classical native preparation uses
spectral normalization on, with its native default beta/Neumann/gain settings.
The learned proposal imports its already prepared matrices; native analytical
spectral normalization is not reapplied to them. Full parameters are retained
with every render. Total declared delay is2048 samples and model lookahead0.

Independent double-precision NumPy reconstruction derives periodic Hann analysis
and synthesis,75% overlap,1.5 COLA normalization, negative-origin startup windows
and finite source padding. It checks the exported window/scale conventions and
uses the actual f32 prepared matrices. Identity and nonsymmetric constant-matrix
tests independently establish window normalization, delay and column ownership.
The numerical reference does not model limiter activation; all tested low-level
sources remain in its inactive regime.

There are38 native renders:7 exact FFT-bin tones near108/258/495/991/2003/4005/
8010Hz in both input cues for learned and classical native filters, plus one
fresh deterministic broadband source and four impulses across two source columns
and within-hop offsets0/77. Maximum native/reference sample error is4.713e-9,
below the pre-set2e-7 gate. Callback1/257 outputs are bit-identical. Rejected hashes
preserve complete ordinary native output exactly.

At these interior FFT-bin tones, dual periodic Hann processing has a known
effective transfer: `(W[k-1] + 4W[k] + W[k+1]) / 6`. Carrier projection of actual
native output agrees within2.351e-7, whereas using only raw `W[k]` differs by up
to0.268 in complex speaker-transfer magnitude. This distinction explains why
ideal per-bin filter diagnostics are insufficient rendered validation. The
reference agreement supports implementation parity at these settings; it does
not prove arbitrary waveforms sound good.

Actual speaker waveforms are convolved with the original measured KEMAR HRIRs
for8 yaw configurations:0,±5,±10,±20 and90 degrees, at the same acquisition
radius. The filter remains centered throughout; these are measured plant
mismatches, not updated learned position estimates. Both ears and both input
cues produce224 measured-ear projections. Direct measured-IR DFTs composed with
independently projected speaker drives agree with ear-waveform projections
within2.522e-10. All referenced SOFA source indices and ear-waveform hashes are
recorded. Reusing these known mannequin directions is a diagnostic replay, not a
fresh subject/recording holdout or a test of radial translation.

Each row below contains14 tone/cue measurements. Values are signed and intended
gain is retained alongside normalized separation, so muting cannot masquerade
as cancellation:

| Fixed-filter plant yaw | Learned median/worst separation dB | Classical native median/worst separation dB | Learned worst intended gain dB |
| --- | ---: | ---: | ---: |
| 0 | 31.09 / 7.67 | 13.16 / 10.47 | -12.45 |
| ±5 | 25.35 / 7.20 | 13.65 / 10.28 | -12.43 |
| ±10 | 18.88 / 6.17 | 12.76 / 10.28 | -12.47 |
| ±20 | 15.95 / -1.81 | 12.54 / 4.10 | -12.66 |
| 90 | -0.003 / -24.21 | -0.018 / -20.61 | -19.16 |

At nominal yaw, the learned method already has two negative raw-attenuation
rows despite its better median separation. At each±20-degree mismatch, one
learned separation row becomes negative while all classical separation rows
remain positive. The90-degree mismatch fails severely for both methods. Absolute
gain values retain the original measurement scale; this is not calibrated human
SPL. These failures, the preceding broader-direction failures and missing
listener/host evidence prevent admission. No model, profile, threshold or
coefficient is tuned using these results.

Three Python tests execute, including real native/reference, callback and fallback
checks. Strict all-target QA Clippy, the feature-disabled build, Ruff and focused
format checks pass. Independent analysis verifies114 raw input/output/metadata
hashes. Source identities, commands and reports are in
`audit/artifacts/ml-xtc-render-20261026`; raw requests, outputs, metadata and the
frozen binary are in `/Volumes/home_tmp/tmp/ml-xtc-render-20261026`.

Remaining: improve the transfer/phase fitting and robustness on development data,
then freeze new independent final material; validate other poses/operating
profiles, broadband and transient ear artifacts, true peaks, runtime memory and
contended/platform deadlines. Real human calibration, listening and product
activation semantics remain. Full31-plugin/five-program scope stays open.
