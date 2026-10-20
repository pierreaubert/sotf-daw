These three speech excerpts derive from **Mini LibriSpeech dev-clean-2**,
published by OpenSLR under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).
LibriSpeech was prepared by Vassil Panayotov with assistance from Daniel Povey,
using recordings by LibriVox volunteers. The dataset paper credits Vassil
Panayotov, Guoguo Chen, Daniel Povey and Sanjeev Khudanpur.

Sources: [Mini LibriSpeech](https://www.openslr.org/31/),
[LibriSpeech](https://www.openslr.org/12/).

Changes: decode from 16 kHz FLAC, resample to 48 kHz mono PCM16, apply gain 0.2,
crop leading quiet samples using the fixed rule in `manifest.json`, and retain
up to four seconds. No denoising or equalization. Source/archive/derived hashes,
utterance and speaker IDs, decoder and exact selection rule are in the manifest.

These are development-set excerpts. Overlap with RNNoise model training is
unknown; this attribution does not imply model-held-out or population validation.
