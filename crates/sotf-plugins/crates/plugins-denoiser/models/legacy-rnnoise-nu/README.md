# Legacy RNNoise-nu model weights

These two real trained weight files come from [GregorR/rnnoise-models](https://github.com/GregorR/rnnoise-models/tree/3eee541a283fd3b8f81b85b1748e3b9ccbefa04d). The immutable commit, original URLs, sizes and SHA-256 values are recorded in `source-manifest.json`; each original `info.txt` is retained.

The maintainer's [pinned provenance statement](https://github.com/GregorR/rnnoise-models/blob/3eee541a283fd3b8f81b85b1748e3b9ccbefa04d/README.md) states that model weights and associated non-tool material are not creative work and are not subject to copyright. We record that statement without assigning an invented SPDX license. It does not establish permission to redistribute every underlying training recording; training audio is not bundled here.

Model names are arbitrary upstream identities. `sh.rnnn` is described as speech in recording noise (fans, AC, computers); `lq.rnnn` is described as voice including other human sounds. These descriptions are provenance, not measured quality guarantees for SOTF.

Both files use RNNoise-nu text format version 1, 42 input features, 22 gain bands and 48 kHz training input. VAD and denoise GRUs use Tanh, unlike the bundled model's Relu; loaders must honor the file's activation values. Existing default RNNoise weights, labels and frozen audio references remain unchanged.

Staged for SPEECH-DENOISER-R2 implementation. Structural/indexing checks are in `audit/continuation-2026-10-01/speech-model-assets-r1`; actual alternate-model audio, realtime lifecycle, quality and whole-chain acceptance are still required.
