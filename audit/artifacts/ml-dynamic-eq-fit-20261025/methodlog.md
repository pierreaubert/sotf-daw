# Method basis and experiment provenance

Primary method reference: [Snoek, Larochelle and Adams, NIPS2012, Practical Bayesian Optimization of Machine Learning Algorithms](https://papers.nips.cc/paper_files/paper/2012/hash/05311655a15b75fab86956663e1819cd-Abstract.html), also [authors' paper](https://www.cs.princeton.edu/~rpa/pubs/snoek2012practical.pdf). It motivates GP objective modeling and expected improvement, and emphasizes that prior/inference choices affect optimizer behavior. The current fixed-kernel recipe is an engineering prototype, not reproduction of their hyperparameter integration or a guarantee for audio fitting.

Classical implementation reference: [SciPy minimize source and supported bounded methods](https://github.com/scipy/scipy/blob/main/scipy/optimize/_minimize.py). The current comparator is a shared Sobol design plus bounded Nelder-Mead, not a claim to exhaust every classical optimizer. Both methods receive the same proposal allowance; early convergence and bound-limited sensitivity probes can change actual render counts, which are reported.

Fixed policy: normalized T/logR/logAttack/logRelease; lengthscale0.25,
Matern5/2, jitter1e-6, log-loss scaling floor0.1, EI over256 global+256 local
candidates, seed20581026; first8 common observations,32 global proposals at
allowance160, remaining bounded Nelder-Mead. Native scoring is equal-source
mean squared relative RMS. At least2 training+1 validation whole-source clips;
validation scores are not fed into optimization. Admission and sensitivity
thresholds were fixed in code before the fresh six-case comparison.

Smoke seed20591026..28 is separate development material and fails validation
for both methods. Evaluation case seeds are20611026,20612026,20613026,
20614026,20615026,20616026, each with +0/+1 training and +2 validation.
Signals are24001 mono48k frames with seeded level steps, mixed1k/400/5100Hz
tones and noise. Native references use fixed hidden dynamics stored only in
truth records, apart from the explicitly known-control wiring case. Near truth:
T=-23,R=4.5,A=3.5ms,release80ms; harder:T=-32,R=7,A=.8ms,release180ms.
These are synthetic native reconstruction probes, not human/hardware data or
an independent final generalization set. No pretrained weights were trained.
