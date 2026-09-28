# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Mix presentations with unknown BS.2051 sound systems in loudness
  layouts now parse: unrenderable layouts are dropped (rendering falls
  back to known layouts, else stereo) instead of failing the whole
  mix presentation.

## [0.2.0] - 2026-09-28

### Added
- Native pure-Rust AAC-LC and FLAC substream decoders
  (`codec::AacSubstreamDecoder`, `codec::FlacSubstreamDecoder` via
  Symphonia), wired into `create_substream_decoder`; Opus still reports
  `UnsupportedCodec` for engine-side handling.
- MixGain Step/Linear/Bezier animation with chained multi-subblock
  segments in the mixer; post-block gains persist at end values.
- Per-frame DemixingInfo/ReconGain routing into scalable channel
  rendering, element default demixing modes, and codec-dependent
  recon-gain overlap.
- v1.1 mix presentation `num_layouts` loudness layouts with BS.2051
  sound-system mapping (`SubMix.layouts`; `output_layout`/`loudness`
  follow the first layout).
- `tests/corpus_conformance.rs` over the reference `.iamf` bitstreams in
  `tests/data/` (AOMediaCodec/iamf-tools): FLAC and 5.1 LPCM full
  decode, Opus rejection, ambisonics element parse.

### Fixed
- Codec configs now retain decoder bytes (AudioSpecificConfig,
  STREAMINFO) instead of consuming them; FLAC metadata-block headers
  are skipped to find STREAMINFO.
- LPCM configs declaring `num_samples_per_frame = 0` derive the frame
  count from decoded payloads instead of decoding zero frames.
- Ambisonics element parsing matches the wire format (mono has no
  coupled count; projection has no channel mapping).
- `select_mix_presentation` rebuilds renderers, substream decoders, and
  scratch buffers, not just mix gains.

### Changed
- `RELEASE_SCOPE.md` rewritten to describe the implemented v1.1.0
  surface, conformance evidence, and known gaps (engine-side Opus,
  downmix on layout reduction, encryption, expanded layouts).

## [0.1.1] - 2026-07-08

### Added
- QA-IAMF-001 malformed-input test suite in `tests/qa_iamf_001.rs`:
  truncated headers, oversized descriptors, bad magic bytes, zero-length
  payloads, unknown codec ids, out-of-range counts, and unbounded allocation
  attempts.
- Synthetic end-to-end decode test for a minimal LPCM IAMF stream.
- Additional parser bounds: `num_subblocks` loops in parameter definitions and
  mix-gain configs are now capped by remaining bytes; parameter-block subblock
  allocation is kind-aware (MixGain/DemixingInfo capped by payload, ReconGain
  by `MAX_LEB128_CAPACITY`); mix-presentation rendering and loudness extension
  skips are bounds-checked.

### Changed
- Documented release support level as **Experimental** in new
  `RELEASE_SCOPE.md`; updated `README.md` to reflect actual codec support.

## [0.1.0] - 2025-05-13

### Added
- Initial release of pure-Rust IAMF decoder.
- OBU (Open Bitstream Unit) parsing for IAMF v1.1.0 descriptors and temporal units.
- Codec support for Opus, AAC, FLAC, and PCM substreams.
- Ambisonics and speaker-layout rendering via `sotf-plugin-ambisonics`.
- Pre-allocated decode path with zero heap allocations in the hot loop.
