# Held-out recorded references

These two half-second excerpts are checked in so normal declick quality tests
require no external corpus, network or decoder. Headerless mono PCM16 little
endian at 48 kHz; decode each sample /32768. `manifest.json` fixes source and
derived hashes, crop bounds, authors, normalization and licenses.

- `cymbal.pcm`: [Cymbal sounds](https://commons.wikimedia.org/wiki/File:Cymbal_sounds.ogg),
  aradlaw; public-domain dedication documented by the source. First cymbal hit.
- `violin.pcm`: [Violin open string](https://commons.wikimedia.org/wiki/File:Violin_open_string.ogg),
  Clngre~commonswiki; this PCM adaptation is licensed under
  [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/).

Changes: originals decoded to mono 48 kHz PCM16 and cropped/peak-normalized as
specified in the attributed de-esser corpus; retain the first 24000 samples
without any further signal processing. The manifest records both the parent
PCM and excerpt hashes. The audio license does not change the Rust source
license. No endorsement by the original authors is implied.

Clean references stay unmodified; fixed deterministic corruptions are added to
copies. Here "clean" means the original desired recording before our injected
corruption, not a claim that microphone/codec noise is absent. The test does not select quiet click locations or tune controls to the
recordings. Synthetic percussion is a separate controlled reference. These
short excerpts are a held-out numerical regression corpus, not population-scale
music certification. The existing external piano/rock release corpus remains
a separate explicitly selected gate.
