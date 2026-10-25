# Planned release fitting experiment

No model is trained in this checkpoint. This records the literature brief and
design constraints for the next experiment, rather than claiming model evidence.

Primary sources from the read-only literature pass:

- Colonel and Reiss, AES 2022, [Approximating Ballistics in a Differentiable Dynamic Range Compressor](https://aes.org/publications/elibrary/elibrary-page/?id=21915): inverse compressor controls from dry/wet pairs, including temporal controls. Approximate differentiable ballistics still require actual native qualification.
- Kates, JASA 1993, [Optimal Estimation of Hearing-Aid Compression Parameters](https://pubmed.ncbi.nlm.nih.gov/8354752/): fit temporal parameters to deliberately stepped sinusoidal responses using mean-squared error. This does not establish arbitrary programme identifiability.
- Sheng and Fazekas, DAFx 2017, [Automatic Control of the Dynamic Range Compressor Using a Regression Model and a Reference Sound](https://www.dafx.de/paper-archive/search?author%5B%5D=Sheng%2C+D.&p=1&q=neural): feature regression to compressor controls with processed/reference similarity evaluation; simplified material limits generalization.
- Simionato and Fasciani, DAFx 2023, [Fully Conditioned and Low-Latency Black-Box Modeling of Analog Compression](https://www.dafx.de/paper-archive/details/1iUC09PiIdeqD8hEE-xSsg): control-conditioned temporal modeling, with remaining heavy-compression difficulty. Does not validate a SOTF release estimator.

Engineering proposal, not a published architecture: bounded small MLP predicts
log release in 10–1000 ms from chronological dry/reference envelope features and
fixed configuration. Use energetic window gain estimates sum(x*y)/sum(x*x), not
unstable samplewise division. Include recovery structure and lag correlations.
Long-release observation context must be checked empirically, with longer-context
replays; an impulse followed by silence does not expose recovery without later
nonzero material.

The comparison must include log-spaced scalar grids, endpoints and local native
refinement around promising intervals. Inspect the objective rather than assuming
unimodality. Count actual native calls, loading/inference cost and total time.
The model only proposes an initializer; the native plugin retains the ceiling.

Separate signal identities before release/level/rate/callback variants. Smoke the
complete pipeline on approximately ten examples and a held-out identity before
scaling. Select checkpoints on development identities; reserve a fresh final set.
Stepped tones, burst/probe trains, percussion, sustained signals and stereo-linked
asymmetry need distinct coverage. No real user recordings or other host/CI
resources have been supplied.

Qualification requires native waveform residual, sample-ceiling checks and an
identifiability check. Broad or separated release regions meeting tolerance must
produce abstention. Silence, no limiting, continuously ceiling-bound signals and
insufficient recovery excitation are explicit negative controls. Synthetic native
parameter recovery is not evidence of preference, hardware matching or general
programme suitability. Analog coloration remains a separate open requirement.
