# AUD073 finite dynamics cases and AUD075 latency — verified checkpoint

2026-09-28. Source frozen. No host, engine, native wrapper, vendor, dependency, MIDI, IAMF, or oversampling edits in this scope.

## Implemented behavior

| Plugin / state | Native response bound | Drain |
| --- | --- | --- |
| AnalogLimiter, color identically zero since initialize/reset | Core lookahead plus active true-peak output-protection delay | Core finite output through unchanged prepared color stage |
| AnalogLimiter, any nonzero color target during epoch | Infinite | Existing COMPLETE / zero frames retained |
| MultibandCompressor, one band or exactly settled dry | Active lookahead ring delay | Exact zero continuation through unchanged kernel |
| MultibandExpander, time-domain one band or exactly settled dry | Active lookahead ring delay | Exact zero continuation through unchanged kernel |
| MultibandExpander, spectral and exactly settled dry | 1024 frames | Exact delayed dry program |
| MultibandCompressor / time-domain Expander, multiband wet | Infinite | Existing COMPLETE / zero frames retained |
| MultibandExpander, spectral wet | Unknown | Existing COMPLETE / zero frames retained |

Before initialization, metadata is Unknown. Each finite drain call emits at most 256 frames and touches only the returned prefix. The bound is measured from the last input; it is not a countdown or a per-call capacity.

Zero-color eligibility is deliberately conservative: even an overwritten 0 → nonzero → 0 target requires reset before finite support is claimed. Model replacement while the epoch remains zero preserves eligibility and core history. For multiband dry eligibility both the smoother's current value and target must equal zero; a pending wet-to-dry fade cannot qualify. One-band time-domain detector/filter/gain state cannot produce audio after delayed program samples become zero, even though control state continues evolving.

AUD075: positive sub-sample multiband lookahead actually delays one frame. Latency now queries the active dry ring instead of recomputing milliseconds as zero. Disabled lookahead remains zero, and spectral dry remains 1024. DSP arithmetic is unchanged.

## Lifecycle and real-time contract

- Successful initialize/reset starts a fresh audio/EOS epoch. Reset retains parameter targets.
- A valid finite drain after accepted input freezes changed controls and new positive input until reset/reinitialize. Identical scalar or whole snapshots remain accepted no-ops; changed bulk snapshots are rejected before mutation.
- Empty-stream drain does not freeze. A nonempty finite zero-delay stream completes without output and does freeze.
- Invalid rate, incomplete channel frames, and missing positive capacity while audio remains are retryable and preserve output/history. Repeated completed drain returns COMPLETE / zero frames.
- Unsupported recursive states retain their previous COMPLETE behavior without freezing or emitting audio. This is intentionally incomplete support, not a claim of a zero tail; AUD073 remains open for those states.
- MBC/MBE reuse existing scratch and run the old processing kernel. The extracted kernels were compared against saved pre-edit bodies: identical after whitespace and `process_in_place` → `process_stream` normalization, including recursive subdivision calls.
- Analog uses core drain then the ordinary color stage on exactly returned frames. No additional scratch.
- The generic parametric adapter already copies input into output before the inner `process` call. `Plugin::process` does not guarantee destination preservation on error. Adapted post-EOS tests check rejection and unchanged DSP/history, not an unsupported output-canary promise. Direct/compiled EOS preflight and all drain capacity errors preserve their output canaries. No shared lifecycle hook was added.

## Executed evidence

Initial public API probe: `probe.rs` / `probe.log` in this directory. It reproduced lost final impulses for all three classes and reported-zero/actual-one-frame multiband lookahead. Colored and multiband wet zero continuations remained nonzero well after lookahead, proving core-only support was insufficient.

Red tests before fixes:

- `/tmp/sotf-multiband-lookahead-red.log`: both time-domain metadata tests fail actual one-frame delay vs reported zero; spectral control passes.
- `/tmp/sotf-multiband-finite-red.log`: new exact public finite-stream dry waveform tests fail due to missing delayed suffix.
- `/tmp/sotf-analog-limiter-finite-red.log`: all-model zero-color delayed program loses suffix.

Green new regression matrices:

- 20 finite-stream test functions: 7 AnalogLimiter, 6 MultibandCompressor, 7 MultibandExpander.
- 576 exact dyadic delayed-program fixtures: first/final samples, 44.1/48/96/192 kHz, mono/stereo/6-channel, short/long inputs and multiple lookaheads, irregular process callbacks and smaller drain calls. Oracle is an independently constructed zero-prefix plus input, not the production delay implementation.
- 54 active nonlinear fixtures compare drain with independently scheduled ordinary zero continuation and check exact silence after the proved bound. These paths deliberately retain ordinary gain/smoothing evolution; no adaptive-freeze reference is being claimed.
- 18 additional spectral dry finite fixtures, all with exact 1024-frame delayed program, plus reset.
- Control-transition proofs, bulk atomic rejection, same-value snapshots, capacity/rate/shape failures before and during drain, reset/reinitialize, empty and no-delay cases, model replacement and compiled rejection.
- 73 prepared cold-thread cases explicitly count both allocations and frees across process/query/drain/reset/process. All record 0 allocations and 0 frees. Large time-domain blocks exercise existing subdivision and telemetry. Scalar control writes outside these measured paths are not claimed allocation-free.
- AUD075 additionally has 480 time-domain dry latency runs and 12 spectral dry runs, checking full exact waveforms across reset and callback partitioning.

Final commands:

```text
cargo test -p sotf-plugin-analog-limiter -p sotf-plugin-multiband-compressor -p sotf-plugin-multiband-expander
300 passed; 0 failed; 0 ignored
Log: /tmp/sotf-finite-dynamics-full.log

cargo clippy -p sotf-plugin-analog-limiter -p sotf-plugin-multiband-compressor -p sotf-plugin-multiband-expander --all-targets -- -D warnings
Passed
Log: /tmp/sotf-finite-dynamics-clippy.log
```

Scoped rustfmt --check and git diff --check passed. The final full suite preceded only replacement of the two drain literals with an equal named constant and README wording; final all-target Clippy compiled that exact Rust source.

## Remaining support and offline duration

Recursive wet output is not automatically drained. Infinite metadata stays Infinite. Spectral wet Expander stays Unknown pending its own response/startup audit. An amplitude threshold or lookahead-only truncation would not prove completion for these states.

Parent reports an additive `render_offline_with_tail(config, Duration, progress)` implementation, with existing `render_offline` delegating to `Duration::ZERO`. It supplies ordinary zeros through the unchanged chain to an explicit export-clock program-plus-tail endpoint. This subagent did not modify or execute that engine implementation; parent owns its analytic echo/resampling/progress verification. This explicit duration policy is compatible with the remaining Infinite/Unknown metadata and does not establish automatic real-time EOS completion.

## Scope of source changes

Current wave changed only each class's primary plugin source, README, CHANGELOG, and new `tests/finite_stream.rs`; AUD075 additionally added `tests/lookahead_latency.rs` to both multiband classes. Other existing diffs in these crate directories belong to earlier audited work and were not reverted.
