# Hiss Rust interval inference checkpoint

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

`interval-proposals` is an optional Hiss crate feature. It exposes an explicitly
hash-selected, context-matched model loader and allocating background/control
thread inference. No native audio, parameters, latency, profile or capture
behavior changes. No worker is spawned and no default model is selected.

The existing synthetic model is unchanged (SHA256
`bae4c80e241f85e98edda3d017bc48a964e9496d10cd436b58dad9fe978c32de`).
It is a functional example, not a model trained on recorded user data. Conditional
scores and source bootstrap bounds are not calibrated probabilities or semantic
safe-noise guarantees. Existing identical-PCM intended-use ambiguity remains.

The Python reference generator creates ten deterministic probes across
44100/48000/96000/192000 Hz, mono/stereo/eight channels, and 44150 Hz with an odd
883-point FFT. Each PCM SHA256 matches exactly in Rust. All sixteen features,
main and bootstrap lower scores, support distances and decisions agree within
absolute tolerance 1e-10. Out-of-context scoring compares private arithmetic
only; the public API rejects those contexts. These are numerical portability
probes, not classifier accuracy or generalization evidence.

Four new native tests additionally cover integrity/schema/policy/context
rejection, incomplete/nonfinite/out-of-range PCM, silent physical channels and
extreme finite parameter overflow. The complete feature-enabled Hiss suite has
135 passing tests, zero ignored; strict all-target Clippy passes. The default
feature-disabled library compiles (it contains no unit tests). Focused rustfmt,
Python Ruff and diff checks pass. Command arguments, exit codes, timings and
test output are adjacent. Python references regenerate byte-for-byte.

The initial compile failed on sha2 0.11 digest hex formatting and a serde_json
conversion; `initial-build.json` records both, corrected before successful
validation. Subsequent changes to lib.rs only sorted the new module declaration.
`source/` and `source-sha256.json` retain the implementation/reference inputs.

Still required: bounded causal worker delivery, source epochs/discontinuities,
overload/staleness handling, plugin/host integration, explicitly reviewed profile
application, recorded-source qualification, calibrated abstention, listening,
resource/deadline and cross-platform acceptance. This checkpoint does not
complete the Hiss proposal or the wider ML audit.
