# Recorded de-esser corpus

These offline fixtures are recordings, not synthesized speech or instruments.
`manifest.json` fixes annotations, processing, original/derived SHA-256 hashes,
authors and source URLs before measuring plugin output. The raw PCM is mono
48 kHz signed 16-bit little-endian with no header. Decode each sample /32768.

| Files | Original author | Source | License selected for the PCM adaptations |
| --- | --- | --- | --- |
| s.pcm | Peter Isotalo | [Voiceless alveolar sibilant](https://commons.wikimedia.org/wiki/File:Voiceless_alveolar_sibilant.ogg) | [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| sh.pcm | Peter Isotalo | [Voiceless palato-alveolar sibilant](https://commons.wikimedia.org/wiki/File:Voiceless_palato-alveolar_sibilant.ogg) | [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| p.pcm | Peter Isotalo | [Voiceless bilabial plosive](https://commons.wikimedia.org/wiki/File:Voiceless_bilabial_plosive.ogg) | [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| violin.pcm | Clngre~commonswiki | [Violin open string](https://commons.wikimedia.org/wiki/File:Violin_open_string.ogg) | [CC BY-SA 3.0](https://creativecommons.org/licenses/by-sa/3.0/) |
| cymbal.pcm | aradlaw | [Cymbal sounds](https://commons.wikimedia.org/wiki/File:Cymbal_sounds.ogg) | Public domain dedication by the author, as documented on the source page |

The four CC BY-SA PCM adaptations remain under CC BY-SA 3.0. This audio license
notice applies to those sound fixtures; it does not change the Rust source
license. No endorsement by the original recordists is implied.

Changes: decode with `ffmpeg -i source.ogg -ac 1 -ar 48000 decoded.wav` to PCM16,
crop at the manifest's sample-rounded bounds (or EOF), scale the cropped signed
integers by `0.5 * 32767 / maximum_absolute_sample`, round to the nearest integer
with ties to even, and write little-endian PCM16. No equalization or denoising.
The test also evaluates a fixed additional gain of 0.25. No downloading,
decoding software or codec dependency is needed to run the Rust test.

Annotations use the source's phoneme/instrument identity and conservative
intervals selected from spectrograms before measuring the processor. Vowels
after fricatives, plosive speech, bowed violin and a cymbal hit/decay are
non-target regions. Speech recordings are isolated demonstrations from one
speaker; these clips do not establish conversational or population accuracy.
The violin is one bright harmonic instrument, not a broad music benchmark.

The detector is based on absolute band level, not speech identity. Attenuation
on cymbals or other non-target sounds is reported as unwanted reduction for
this speech-oriented annotation policy; it is not silently relabelled as a
correct sibilance decision. The fixed test setting is 6 kHz, Q 0.7, threshold
-36 dB, ratio 8, attack/release 1/80 ms, range 6 dB, fully linked stereo and
2 ms lookahead. Range-zero output through the same topology provides the
phase-matched reference. Measurements use annotated intervals plus declared
latency, active 10 ms windows above -60 dBFS and a 1 dB reduction event threshold.
The LR4 observations use ordinary zero continuation, not a finite-tail claim.
