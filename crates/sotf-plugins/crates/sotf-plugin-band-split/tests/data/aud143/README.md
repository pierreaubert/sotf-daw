# AUD143 original legacy audio fixtures

These losslessly compressed archives preserve the original BandSplit audio
captured before the phase-compensation implementation. They are compatibility
oracles, not expected output for `PhaseCompensated` mode. Keep them unchanged;
write subsequent captures to a separate path.

Both archives contain 48 kHz, distinct stereo inputs with a 1,100 Hz probe,
12,000 settling frames and 24,000 measured frames. The public split archive
contains all ten original LR24/LR48 cases: two bands, close/wide three bands,
and close/wide four bands. It retains every per-band output sample. The host
archive contains both slopes through the actual three-band DawHost split →
merge chain. The headers, input vectors, case settings and complete output
vectors are part of each archive.

| File | Decompressed bytes | Decompressed SHA-256 |
| --- | ---: | --- |
| `legacy-split-48k-stereo-v1.bin.gz` | 9,504,620 | `73b30f81f14464e87fc6982ddcd5c2e3c618ca41486a95ec9b840a3debc16ebc` |
| `legacy-host-48k-stereo-v1.bin.gz` | 864,126 | `e9efc5c52975d07357bde1e36dae67a6ac471b42cd6f8d0508543799783923bc` |

Compressed-file SHA-256 values:

- Split: `e45166957b2ec671fcb3b5d98987b1c2ca3d9ccf8e200c1666d8e6edadbb53ff`.
- Host: `40447b3febc5b2e6ee7501a71787f5635094df743aaec57b6dbbb393ef56dc53`.

Gzip compression uses a zero timestamp. Decompressing each repository fixture
was verified to reproduce the original saved archive byte for byte. Original
archives remain in the audit target directory; replay must not depend on that
ignored directory being present.

## Format and provenance

The capture writers define version 1 of the binary format:

- Public plugin: `tests/aud143_phase_capture.rs` in this crate.
- Actual host: `crates/sotf-plugins/tests/aud143_bandsplit_host_chain.rs`,
  relative to the repository root.

The format uses a tagged header followed by little-endian integers and floating
point samples. Preserve complete bytes, including case ordering and metadata.
The original captures did not have run-bound source manifests. The audit report
records that limitation, original command logs, independent archive parsing,
and the post-edit legacy replay that matches the full split archive exactly.
See `audit/band-split-phase-compensation-gap.md` at the repository root.

These fixtures cover the stated legacy cases only. New compensated responses,
other sample rates and layouts, automation, reset, heap behavior and application
controls require separate tests.
