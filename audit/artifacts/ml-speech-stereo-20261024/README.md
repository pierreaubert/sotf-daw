# Linked-stereo DeepFilterNet2 checkpoint

Builds on local mono checkpoint `db1b94a`. The two-channel runner shares mean
ERB masks, mean complex deep-filter coefficients and a mean local-SNR stage
decision. Audio STFT/rolling histories remain separate. Post-filtering runs once
on the common mask. A common per-frame full-scale bound avoids independently
clipping the stereo channels. No new model weights, RNNoise IDs, or scalar
parameter IDs are introduced.

The pre-fix real-model regression produced 0.0030891672 maximum channel-relation
error for right = 0.25 * left. With linked coefficients, positive/negative ratios,
a silent right channel, and common post-filtering all have zero measured error.
Channel swap and complete two-channel reset also have zero measured error.

Final validation:

- 7 actual-model adapter tests passed, including an unchanged-upstream mono
  golden fixture. `golden-validation.json` and
  `adapter-with-upstream-golden.stdout.txt` contain final commands/results.
- Mono and stereo unity-mask measurements both establish 2400 frames of
  intrinsic delay with maximum waveform error 8.940697e-8.
- Mono port versus unchanged upstream v0.3.1 golden waveform and scores:
  maximum errors 0. The source-verified independent reference and locked offline
  replay are documented under the denoiser test fixture directory.
- 26 feature-enabled speech tests passed: 7 unit, 6 model/host, 13 legacy
  integration. 21 feature-disabled tests passed: 7 unit, 14 integration;
  unavailable-backend requests are checked for both mono and stereo.
- Real stereo serving at 44.1/48/96 kHz selected 79 timely model packets, with
  5 warmup/fallback packets and no dropped inputs at each rate. Channel-relation
  error was 0; processing and reset heap activity was (0 allocations, 0 frees).
  The explicit 16-packet worker budget establishes functional serving, not
  default-budget or real-DAW deadline acceptance.
- Strict all-target Clippy and combined CREPE + DFN2 engine test compilation
  passed. Initial SHA-format fixture compile errors and the pre-fix stereo
  failure are retained separately; no numerical thresholds were relaxed.

`validation.json` records the initial stereo gates; `golden-validation.json`
supersedes its adapter/Clippy rows after adding the independent golden and
stereo-delay cases. Production DSP was unchanged between those host gates and
final adapter validation. `source-manifest.json` records final file identities.
The earlier checkpoint records the parent/DAW workspace metadata conflict that
blocks workspace-wide formatting; focused Rust formatting is checked here.

Full candidate acceptance remains open: held-out speakers/noises, controlled
listening, real-DAW/default-budget memory/deadline comparisons, and user-facing
structural-backend configuration. Linux/Windows hosts and unavailable held-out
recordings were not tested. Linked stereo is a local extension using pretrained
mono weights, not a separately trained spatial model.
