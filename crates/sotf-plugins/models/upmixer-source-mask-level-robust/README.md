# Excitation-independent v2 mask candidate

QA-only generated-source candidate; deployment is not admitted.

`model.synthetic.json` SHA256:
`85d1609e715a8d558f4b85bf727b45f373d5ac59915d1ddf604082658c1f7d12`.
Project-generated panned harmonics/stereo noise beds, GPL-3.0-or-later;
no recordings or speech-identity labels.

This uses the existing v2 contract and the same fixed native renderer. The eight
input weights for absolute excitation (coefficient indices40–47) are constrained
to exactly zero. Predictions therefore depend on the remaining covariance and
frequency features. Native raw-power eligibility/fallback still apply, so very
quiet material can reach the existing absolute validity floor.

Training covers16 fresh source seeds across four conditions and gains−40/0/+20dB,
with230919 native rows; four validation seeds provide58254 rows. One fit converges
after520 iterations. A fixed development level gate passes before final evaluation.
The frozen candidate passes numerical and mask-level gain gates on192 cases
from eight further fresh source seeds. Earlier candidates remain separate.

Quality remains insufficient: hard-pan direct retention is worse than the scalar
analytic diagnostic, and correlated background leakage is high. The complete
nonlinear speaker renderer also retains substantial gain dependence. This is
a validated mask-level robustness improvement, not a production source-separation
or perceptual acceptance result. See `audit/ml-upmixer-level-robust-20261026.md`.
