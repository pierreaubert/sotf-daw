# FIR band-center versus weighted RMS feasibility, 2026-10-24

**At 96 kHz, the fixed centered support cannot satisfy the natural 500 Hz
±0.05 dB constraint while retaining the anchor pilot dense RMS nonregression
guard, according to independently checked numerical dual evidence.** At 48 kHz,
the unconstrained RMS optimum already satisfies the center constraint.

No production or Rust code changed. No performance tests ran. The analysis plan
was published to private issue 9 before code (`issue-plan-publication.json`).
The frozen reconstruction and target model are copied from the rejected anchor
pilot; their provenance is in `source.json`. Audit probe frequencies do not enter
the optimizer. The target bands are the configured 500 Hz peak, 2000 Hz peak,
and 150 Hz low shelf. Only the natural first configured peak center is constrained.

| Rate | Baseline RMS | Unconstrained optimum RMS | Constrained optimum RMS | Numerical constrained RMS lower bound |
| --- | --- | --- | --- | --- |
| 48 kHz | 0.0002356595114 | 0.0002104326452 | 0.0002104326452 | 0.0002104323750 |
| 96 kHz | 0.0027072781575 | 0.0022770167186 | 0.0029256582255 | 0.0029256582061 |

The 96 kHz lower bound exceeds baseline by 8.0664%. Both positive and negative
signed-response intervals were checked, so the conclusion does not assume a
positive magnitude branch. The negative branch has RMS about 0.09835 and cannot
avoid the lower bound. The constraint endpoint's displayed dB error differs from
−0.05 by floating-point rounding; its direct amplitude residual is under 5e-16.

This addresses **only the pilot uniform-grid RMS guard plus one natural center**.
It is not a proof that the original four isolated probe requirements alone are
infeasible, nor that all possible finite FIR designs cannot satisfy them. The
other original gates, dense maximum error, phase, latency, and streaming behavior
were not relaxed. The positive 96 kHz optimum also worsens dense maximum relative
error (0.0286866 to 0.0528223); no designer implementation is proposed.

## Method and bound

Use the production integer-centered symmetry convention: a 1024-element stored
FIR centered at index 512 has h[−k]=h[k] for k=1..511 and the unmatched h[−512]=0.
Thus there are 512 independent coefficients. This keeps the frozen prototype's
support and center; it does not substitute a half-sample-centered Type II filter.
Write x[0]=h[0], x[k]=sqrt(2) h[k], and
B[j,0]=1, B[j,k]=sqrt(2) cos(pi*j*k/M), M=4096. The full grid includes both
endpoints with equal weights, exactly as in the anchor pilot. With target t[j],

    f(x) = mean((B*x/t - 1)^2)
    Q = B' diag(1/t^2) B / (M+1)
    r = B' (1/t) / (M+1)

FFT synthesis and its exact even-extension adjoint apply Q without forming a
matrix. CG solves Q*x0=r and Q*z=a, where a is the cosine basis at the configured
500 Hz center. The minimum over each signed response interval is x0 when its
response lies inside; otherwise it is

    xc = x0 + lambda*z
    lambda = (c - a'*x0)/(a'*z)
    f(xc) = f(x0) + (c-a'*x0)^2/(a'*z)

where c is the nearest interval endpoint. The multiplier sign is checked against
the interval inequality. The orthonormal cosine Gram under trapezoidal endpoint
weights is M*I. Equal endpoint weights add a positive semidefinite term, hence

    Q >= m*I, m = min(1/t^2)*M/(M+1).

For any approximate xc and multiplier lambda, the Lagrangian evaluated at xc
minus ||Q*xc-r-lambda*a||^2/m is a dual lower bound in exact arithmetic. The
implementation evaluates the Lagrangian through residual squares to avoid
cancellation and subtracts a floating roundoff safety allowance as well. Both
signed intervals are bounded separately; the minimum of their lower bounds
bounds the magnitude-constrained union.

`result.json` preserves coefficient vectors, CG iterations/residuals, direct
operator checks, multipliers, constraint residuals, and every bound component.
`verify-certificate.py` independently re-evaluates all saved constrained responses,
normal gradients, and dual bounds with scalar cosine sums, using neither CG nor
FFT. Its maximum normal residual is 1.48e-13. The 96 kHz direct lower bound is
0.002925658206080432 RMS, agreeing with the FFT result.

These are floating-point numerical certificates, **not interval-arithmetic
proofs**. Residual and roundoff allowances are retained explicitly; the conclusion
has a much larger margin than the observed numerical differences.

Reproduce with `rtk proxy python3 audit/artifacts/remaining-fir-weighted-feasibility-20261024/feasibility.py`
and `rtk proxy python3 audit/artifacts/remaining-fir-weighted-feasibility-20261024/verify-certificate.py`.
