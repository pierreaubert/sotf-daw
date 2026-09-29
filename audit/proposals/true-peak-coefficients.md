# AUD121 — true-peak coefficient correction

The public loudness monitor and pinned private math-dsp backend contain the
same incorrect FIR coefficients, labeled as BS.1770-4 Table 2. The host's
supposedly independent test repeats those coefficients. The ITU recommendation
actually gives the interpolation coefficients in Annex 2, printed pages 18–19;
Table 2 describes the loudness high-pass filter. The current first FIR phase
has 4.678 dB gain at 12 kHz/48 kHz by direct frequency-response arithmetic.

Authoritative source: [ITU-R BS.1770-5, Annex 2](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I!!PDF-E.pdf),
PDF pages 19–20 (zero-based). This source was opened and its coefficient table
retrieved independently of repository constants.

## Correction and invariants

Replace both coefficient tables with the published four columns, reversing
each column to match the existing oldest-to-newest history iteration. This
implements causal convolution with lag-zero coefficients from the first
published row. Keep rate selection (four phases at 44.1/48 kHz, two at
88.2/96 kHz), interval ownership, reset, publication and prepared storage.
High-rate support is a separate capability issue. No audio-processing filter,
gain controller, loudness energy/history or sample-peak calculation changes.

Before editing production, add public regressions against an explicit
zero-stuffed, full 48-tap convolution reference using the published row-major
integer coefficients, plus an analytic quarter-rate tone whose amplitude is
known without any FIR constants. Check host and backend separately; record
both real failures. Correct the misleading existing oracle and documentation.

Validate impulses, DC, tones, dense audio, channel independence, all currently
supported rates, arbitrary interval/callback boundaries, reset and retained
snapshots. Preserve other loudness and gain telemetry with before/after
captures. Check cold allocation/free behavior, package tests and Clippy;
record any changed true-peak results as the intended calibration correction.
