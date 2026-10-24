# Rejected magnitude-only mesh trial

The baseline was captured with production commit `1b9e6e1`, after recording
`predeclared-protocol.json` on private issue 9. Baseline and candidate source
snapshots and unfiltered 710-direction outputs are retained.

The existing architecture interpolates log magnitude and onset-aligned phase
separately. This trial used certified canonical supporting-mesh donors for log
magnitude and multiplied a positive real ratio onto the existing complex response
uniformly at every frequency. No per-frequency or direction-dependent policy was
selected from measured scores. Production call sites were never switched.

All five measured integration tests passed. Across both ears and all 513 FFT bins
at 710 held-out directions, maximum phase deviation was `9.78672e-8` rad, below
the fixed `1e-6` phase-preservation guard. Maximum/RMS band-ILD errors improved
from `2.20956/0.610986` to `0.978053/0.228793` dB; baseline phase errors remained.

However **all eight** fixed max/RMS pre-onset and beyond-measured-support energy
guards failed across the 43/667 partitions. Whole-dataset maximum pre-onset
energy fraction increased `0.0384631` to `0.0561972`; maximum energy fraction
beyond the measured 512-sample support increased `0.0201441` to `0.0320403`.
The inverse uses the production real DC/Nyquist convention and retains all 1024
samples. These are normalized energy observations, not audibility judgments.

**The trial was rejected and production restored.** Improving magnitude scores
while retaining inconsistent phase can increase ringing. No thresholds were
relaxed. A future residual/minimum-phase approach needs its own physical rationale
and predeclared node, cue, continuity and time-domain guards.

`verify-quality.py` recomputes every partition's scores from raw data and
intentionally exits **1** for the preserved rejection.
