# Primary sources and engineering choices

- [Scheirer and Slaney1997, Construction and Evaluation of a Robust Multifeature Speech/Music Discriminator](https://www.ee.columbia.edu/~dpwe/e6820/papers/ScheiS97-mussp.pdf): primary feature-family motivation for temporal/spectral statistics and source-aware evaluation. Its speech/music task and reported results do not validate noise-only semantics or the current feature formulas/thresholds. The copyrighted radio corpus was not downloaded or used.
- [Geifman and El-Yaniv2017, Selective Classification for Deep Neural Networks](https://proceedings.neurips.cc/paper/2017/hash/4a8423d5e91fda00bb7e46540e2b0cf1-Abstract.html): motivates explicit rejection/coverage. This prototype does not implement their statistical risk-control construction, use their image datasets, or inherit their guarantees. Its bootstrap lower score/support cutoff remain heuristic.

Technique: small per-session regularized logistic model, source-equal weighting,
50 source-bootstrap models with refitted normalization, complete source-held-out
admission, fixed classical comparator. No pretrained/default model. Fixed policy
and feature formulas were established before the fresh evaluation; no threshold
was chosen from its outcomes. Intent ambiguity is evaluated explicitly rather
than treating high score as proof of unwanted noise.

Smoke/test source seed20721026. CLI evaluation training seed20771026,8groups,
noise/programme offsets+i*10+j. Fresh procedural seeds20781026+i*10+class offset.
Semantic counterfactual seed20811026. Gaussian and mildly coloured noise versus
amplitude-modulated tones are functional cases, not realistic programme coverage.
One existing recorded /s/ clip is development evaluation only, never training.
The native diagnostic is explicit: spectral mode, default threshold-30dB,
strength1, cutoff4kHz, captured profile enabled, flat curves, independent linking.
It is not invoked by the classifier API/CLI.
