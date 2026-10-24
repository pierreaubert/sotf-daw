# Optional DeepFilterNet2 backend

The archive is the upstream v0.3.1 DFN2 ONNX export, SHA-256
`412e81d8d1779b8f925ac4b53472655a9f866c4af73dbf930ab587359f631628`.
Its download source and member hashes are in `manifest.json`. The archive at
current upstream and at v0.3.1 is byte-identical. Runtime compatibility requires
the DFN2 runner, whose source, local changes, licenses, and pinned dependency
revision are in `vendor/deep-filter-runtime` at the workspace root.

Build with `deep-filter` on the speech-denoiser, facade, or engine crate. An
explicit `SpeechDenoiserPluginParams.deep_filter` policy supplies `model_path`,
`sha256`, and `playout_packets` (2–16, default 4). Paths are resolved on the
control thread during preparation. The mode accepts mono or linked stereo. Stereo shares ERB masks, complex
deep-filter coefficients, and stage decisions, with separate audio histories.
This is a local linked extension of the pretrained mono model. The three RNNoise model IDs remain stable.
Old presets omit this policy and retain RNNoise behavior. A request on a build
without the feature returns an error.

Inference runs chronologically on a bounded worker, never in the audio callback.
The adapter derives 2400 frames of intrinsic delay at 48 kHz from the upstream
STFT and lookahead configuration. Playout adds `480 * playout_packets` frames.
Host-rate conversion reports the corresponding prepared delay. Actual-model tests measured the 2400-frame unity-mask delay within 8.95e-8
maximum waveform error and restored a fresh-state waveform exactly after reset.
Mono host tests passed at 44.1, 48, and 96 kHz for exact delayed dry audio,
finite drain, failed-reprepare preservation, and zero processing/reset heap
activity. Actual serving passed with an explicit 16-packet budget. Consult the
implementation ledger for the remaining acceptance work before deployment.

Deadline misses use delayed source audio. Telemetry reports selected packets,
fallbacks, dropped inputs, recurrent resets, and local SNR. Local SNR is not a
speech probability and is not published as RNNoise band gains or VAD.

Held-out speakers/noises, controlled listening, deadline-tail
benchmarks, and real-DAW validation remain open. This artifact establishes no
quality advantage over RNNoise.
