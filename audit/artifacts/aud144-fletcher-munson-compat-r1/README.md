# AUD144 legacy FletcherMunson migration regression

The unchanged probe succeeds against the immutable pre-validator library and
fails against the immutable r2 envelope-validator library. The document is a
genuine C API export, with no identity mutation. This distinguishes the intended
legacy migration from a forged family label. The bridge explicitly routes the
legacy FletcherMunson constructor to LoudnessCompensation; the engine also
migrates its typed defaults to LoudnessCompensation.

Pre-validator import returns 0. Saved state equals the fresh FletcherMunson
source, and all 9,464 interleaved output samples match exactly. Default-output
sensitivity is 0.27916882932186127. The validator returns -8 before importing,
leaving the default LoudnessCompensation output. Full states, genuine documents,
complete little-endian f32 vectors, library/probe hashes, and exit codes are
retained here. No library was rebuilt for this comparison.

The first diagnostic script completed with exit 0 for both libraries while
recording the mismatching return values. This archived version makes preserved
migration an explicit gate: pre-validator exits 0; envelope-validator exits 1.
The original preset-envelope refusal probe and both sealed library packets
remain unchanged. Reverse migration was not tested by this probe.

Run checksum verification from the repository root.
