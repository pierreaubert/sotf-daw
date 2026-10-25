# Paired refinement-label experiment

Issue: private sotf-daw #10, EQ proposal; baseline 6408f09c.
Base: frozen eq-multiband SHA 671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015.
Data: fresh deterministic synthetic native EQ targets; exclusion manifests recorded per run.
Method: warm-start both arms; identical differentiable response loss + 0.1 normalized masked parameter loss. Control uses original generator labels; candidate uses base neural proposal refined by bounded SciPy least-squares. Unimproved labels retain original neural proposal; no hard case is removed.
Optimizer: Adam 1e-4, batch32, gradient norm cap10,2 CPU threads, fixed batch seed.
Selection: development fixed-budget solver qualification, refined MSE, calls; initial checkpoint eligible. Final holdout is neither generated nor scored during training.
Smoke:10 training,4 dev,2epochs,10nfev teacher/dev. Full planned:256training,12dev,12epochs,40nfev teacher/dev, independent seed from smoke.
Adoption: experimental only; preserve existing default model and classical candidates. Full export gating and independent response checks are required after checkpoint freeze.

Primary research:
- Nercessian, DAFx2020, Neural Parametric Equalizer Matching Using Differentiable Biquads: https://www.dafx.de/paper-archive/details/lpNI_8szg2jo_eJO1RnKxw . Keep response loss; parameter matching alone can mislead.
- Sambharya et al., JMLR2024, Learning to Warm-Start Fixed-Point Optimization Algorithms: https://www.jmlr.org/papers/v25/23-1174.html . Evaluate downstream fixed-budget residuals; theoretical fixed-point guarantees do not transfer automatically to this nonconvex solver.
- Mockenhaupt et al., DAFx2024, Automatic Equalization for Individual Instrument Tracks Using CNNs: https://www.dafx.de/paper-archive/details/3sfOGwNdQGaZJWVmX79IkQ . Synthetic-only improvement cannot establish measured-target quality.

Decision is an inference from these sources: use solver labels as auxiliary supervised targets with response loss, and compare equal-step continuation to isolate extra training effects. No external dataset or audio used. No tracker configured in environment.
