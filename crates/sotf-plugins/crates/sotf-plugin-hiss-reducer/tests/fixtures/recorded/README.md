# Manual-capture speech contamination fixture

`speech_s.pcm` is the first second of the attributed, decoded/peak-normalized
[/s/ recording](https://commons.wikimedia.org/wiki/File:Voiceless_alveolar_sibilant.ogg)
by Peter Isotalo. This PCM adaptation remains licensed under
[CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/), independently
of the Rust code license. No endorsement is implied.

Headerless mono PCM16 little-endian, 48 kHz; decode each sample /32768. The
manifest records source and derived hashes, original crop and normalization.
The excerpt contains an isolated /s/, a vowel and silence from one speaker.
It is not conversational speech or a population-scale speech benchmark.

Changes: use the first 48000 samples of the mono 48 kHz PCM16 adaptation
documented in the de-esser corpus, with no additional processing. The test
adds deterministic low-level noise to a separate copy. Clean and contaminated
manual captures use identical fixed settings. A second noise seed supplies
the holdout noise stream; repeated desired speech is measured separately.
No codec, network or sibling-crate fixture lookup is required at test time.

Manual capture measures its input, including speech. The test quantifies that
bias and downstream wanted-signal attenuation; it does not pretend to provide
automatic noise-only interval selection. Such classifier research is separate.
