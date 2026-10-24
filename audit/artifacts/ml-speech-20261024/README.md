# Local mono DeepFilterNet2 implementation checkpoint

The shared chronological worker, opt-in speech backend, verified weights,
host-rate conversion, typed engine configuration, and model-specific telemetry
are implemented. The existing three RNNoise model IDs and omitted-policy
presets retain their behavior. Mono DFN2 currently refuses stereo explicitly.

Validation: 17 bridge unit tests; 4 actual-model adapter tests; 24 feature-enabled
speech tests (7 unit, 4 model/host, 13 legacy integration); 21 feature-disabled
speech tests (7 unit, 14 integration); engine test compilation; and one targeted
engine serialization/conversion test. Combined CREPE + DFN2 engine test
compilation also passed. Strict all-target Clippy passed for the
bridge and both denoiser crates. `runtime-selection.json` records metrics;
`remaining-validation.json` and the adjacent stdout files capture the remaining
commands and their real exit codes. The initial engine check omitted the facade
QA helper feature; that failure is retained, and the corrected check passed.

Focused rustfmt passed on all 15 changed Rust files, and Ruff passed for both
Python utilities. Workspace-wide `cargo fmt --all -- --check` could not obtain
metadata: driver-common resolves to conflicting parent and DAW workspaces.
That failure and the successful focused check are recorded in
`format-validation.json`; no parent workspace configuration was changed.

The unity-mask test measured the 2400-frame intrinsic delay within 8.95e-8
maximum waveform error. Pristine reset and wrapper consistency errors were zero.
The serving test used 16 playout packets, with 79 timely packets, 5 fallback/
warmup packets, no dropped inputs, and no worker errors. It establishes functional
serving under that explicit budget, not default-budget or real-DAW capacity.

The local weights/runtime integrity checker is `scripts/ml/verify_deepfilter.py`.
`source-manifest.json` identifies the final source and model files. Wrapper
consistency uses the same extracted runner; it is not independent evidence of
port equivalence or enhancement quality. Linked stereo, independent golden audio,
held-out speakers/noises, controlled listening, user-facing backend selection,
and real-DAW/default-budget deadline acceptance remain open. No Linux/Windows
DAW or unavailable held-out recording tests were claimed.
