# sotf-iamf — v1.1.0 implementation scope

Pure-Rust IAMF v1.1.0 IA Sequence decoder: OBU parsing, substream codecs,
channel/scene rendering, and mix presentation. Verified against the
reference bitstreams in `tests/data/` (AOMediaCodec/iamf-tools
`iamf/cli/testdata/iamf`, see `tests/data/README.upstream.md`) by
`tests/corpus_conformance.rs`.

## Implemented

- **OBU framing + descriptors** — sequence header, codec config, channel
  and ambisonics audio elements, mix presentations, temporal units with
  parameter blocks and trimming.
- **Codecs** — LPCM (8/16/24/32-bit), FLAC (STREAMINFO carried in the
  codec config's FLAC metadata blocks, one raw frame per audio frame
  OBU), AAC-LC (AudioSpecificConfig, stereo/mono, 1024-sample frames).
  Opus reports `UnsupportedCodec`: no pure-Rust Opus decoder exists, so
  Opus substreams stay engine-side via the `SubstreamDecoder` trait.
- **Channel rendering** — discrete 1:1 routing plus the scalable
  pipeline: per-subblock demixing (`dmixp_mode` incl. per-frame mode
  updates), enhanced-sonic-head `w(k)` reconstruction, recon-gain
  smoothing, output-gain application.
- **Scene rendering** — mono and projection ambisonics modes.
- **Mix presentation** — v1.1 `num_layouts` loudness layouts with
  BS.2051 sound-system mapping, per-element and output MixGain with
  Step/Linear/Bezier animation across chained subblocks, runtime mix
  switching (`select_mix_presentation` rebuilds renderers + decoders),
  output-layout switching, seek with full state reset.
- **Conformance evidence** — `tests/corpus_conformance.rs` decodes the
  reference FLAC (bit-exact path) and 5.1 LPCM files end to end;
  codec vectors in `src/codec/symphonia.rs` are verified against
  ffmpeg (FLAC bit-exact, AAC within 5e-8); Opus reference files assert
  clean `UnsupportedCodec` errors.

## Known gaps (not in scope)

- **Opus substream decoding** — needs an engine-side `SubstreamDecoder`
  (libopus/concentus); the trait seam is ready.
- **Downmixing on layout mismatch** — rendering an element to a smaller
  target layout routes matching channels and drops the rest (e.g. 5.1
  element to stereo keeps L/R); no ITU downmix matrix is applied.
- **Encryption / content protection** — not implemented at all: protected
  content is neither detected nor decrypted.
- **Expanded loudspeaker layouts** beyond the base table — unknown
  BS.2051 sound systems are accepted at parse but dropped from the
  layout list (rendering falls back to known layouts, else stereo)
  instead of rendered.
- **Streaming / live edge cases** — the decoder assumes complete
  temporal units; parameter-only units render silence (or zero frames
  when the codec declares no frame size).
