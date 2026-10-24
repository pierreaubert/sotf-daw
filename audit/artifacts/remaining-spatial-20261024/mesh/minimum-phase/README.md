# Rejected minimum-phase plus residual-phase trial

`predeclared-protocol.json` was recorded on private issue 9 before code changes.
The candidate factors each donor into a finite-periodic full-magnitude cepstral
minimum-phase component, detected onset delay, and measured residual unit phase.
All components use the same canonical supporting-mesh weights. This retains
nonminimum measured phase; it does not assume mannequin responses are minimum
phase. The official [SciPy homomorphic documentation](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.minimum_phase.html)
supports the numerical factoring approach, not listener acceptance. No PDF was
read for this trial.

Independent references passed: minimum/nonminimum two-tap and pure-delay error
`2.63630e-7`, delayed analytic mixture error `1.02152e-6`, complex edge/diagonal
continuity jump `4.93185e-6`, all below the fixed `2e-5` gate. All 710 measured
nodes also passed the unchanged complex DFT tolerance.

**The candidate was rejected.** Complete leave-one-out checking hit the fixed
undefined-residual conditioning guard at ear 0, Nyquist bin 512. No bin was
skipped and the conditioning threshold was not relaxed. The separately completed
43-direction diagnostic also worsened maximum wrapped IPD error from `2.94789`
to `3.02650` rad, failing the fixed phase guard independently. There is no valid
complete-dataset quality aggregate for this trial.

Signed real DC/Nyquist responses require an explicit interpolation contract,
but an endpoint correction alone cannot rescue the failed measured phase guard.
All candidate source bytes matching the recorded pre-build hashes are preserved.
Production was restored to `1b9e6e1`; the candidate was QA-only and production
call sites were never switched.

The paired output's neighbor/weight columns describe its baseline lookup.
Candidate donors come from the archived `HrtfInterpolation::select` mesh selector
and are remapped from the view with the held-out measurement removed; those
baseline columns must not be interpreted as candidate donor metadata.
