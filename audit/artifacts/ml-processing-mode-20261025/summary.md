# Explicit offline speech inference checkpoint

Tracking: private sotf-daw issue 10. This is part of the full ML audit, not completion of Gate or the overall audit.

## Behavior

- New control-time `ProcessingMode` / `initialize_with_mode` contract. Existing constructors and direct initialization retain realtime behavior.
- Host mode is immutable; every added node and rate-driven reinitialization receives it. Parametric and in-place adapters and both oversampling wrappers forward it.
- Gate uses synchronous RNNoise/Silero inference only when explicitly prepared offline. Same model code, source packets, confidence/age policy, warmup, gain smoothing, and audio latency. Offline model ownership is boxed during preparation; realtime storage does not contain an inline offline model.
- File-chain rendering selects offline mode. The parallel API retains a single chronological stream for learned Gate configurations: finite slice priming cannot establish equal recurrent context or source-clock phase.
- Presets, scalar parameter indices, and default classical behavior are unchanged. Timeline graphs retain their preparation mode; automatic offline reconstruction/restoration remains open.

## Validation protocol

Host tests cover every adapter view, reset, explicit error propagation, automatic/explicit oversampling, default realtime selection, rate reinitialization, and nodes appended after build. Existing bounded-buffer and complete host library tests also run.

Gate tests render an existing 4-second development recording plus 13 final samples, without sleeping. Both RNNoise and Silero must produce more than 100 observations and actual protection, with zero dropped packets. Audio must agree exactly across fixed/irregular callback partitions, reset, reinitialization, and asymmetric external-key adaptation. External input/key samples remain unchanged. Existing realtime heap and backpressure regressions run separately.

The engine test writes the development source as float WAV and compares actual file rendering at 127/1024/3001-frame callbacks against direct synchronous Gate processing. It checks both models, actual learned gain differences from classical gating, partial final packets/blocks, and the parallel API.

Validation is complete for this checkpoint. Host: 714 library tests (one ignored), four execution-mode tests, and six bounded-adapter tests passed. Gate: 122 model-enabled tests (one ignored), 104 default-feature tests, and a fresh post-lint offline model retest passed. Engine: all 26 offline-renderer tests and a fresh post-lint model integration retest passed. Strict Clippy passes for host/Gate all targets and engine lib/tests with `--no-deps`. Focused formatting and diff checks pass. Command, exit code, elapsed time, and complete output are retained alongside this summary. Early engine release invocations omitted the allocation test's required dependency QA feature; those failures are retained and corrected with explicit feature selection. The first engine run passed 25 existing tests but excluded the new model test because the engine Silero feature does not itself set the engine RNNoise feature. The test module now accepts either speech feature; the successful 26-test run and final targeted retest both include the named model integration test. New test iterators were corrected after strict Clippy feedback, with the affected model tests rerun. The unrestricted engine Clippy attempt reaches a pre-existing `too_many_arguments` lint in `sotf-streaming/src/server/client_message.rs:18`; that dependency is unchanged and scoped engine Clippy passes. No held-out quality, real DAW deadline, cross-platform, or production adoption claim.

## Remaining work

- Automatic timeline track/master mode preparation and restoration.
- Review realtime learned Gate against the host's strict bounded-subdivision equivalence contract; asynchronous observation arrival depends on worker timing.
- Held-out multilingual/natural noise/music quality, quiet-plosive gain-shift robustness, stronger classical baselines, listening, frontend configuration, and actual DAW budgets.
- All other ML audit proposals remain tracked in the full implementation ledger.

`validation-summary.json` lists successful final checks. Earlier command/lint failures are preserved as diagnostics. `source-sha256.json` identifies final Rust sources; the pre-lint manifest and source hashes embedded in earlier run metadata preserve the distinction between test revisions.

Captured command text removes only surplus blank lines at EOF to satisfy repository whitespace checks.
