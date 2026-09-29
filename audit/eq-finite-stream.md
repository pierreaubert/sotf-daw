# EQ finite drain and preparation checkpoint

2026-09-28. Production and tests frozen. No commits or pushes.

## Implemented scope

- `ParametricPlugin` now has additive default `drain_output_frames_max` and `drain` hooks. `ParametricPluginAdapter` forwards both; defaults remain zero complete. Gain source and its default drain behavior are unchanged.
- `Oversampler` exposes its existing checked `reserve_for_max_frames` preparation method, with an explicit control-thread allocation contract. EQ calls it whenever it constructs its internal 2x/4x state, preparing the advertised 4096-frame callback capacity. Other constructor allocations are unchanged.
- `Oversampler::passthrough_tail_frames` exposes the existing conservative four-chunk finite support bound. The internal generic-wrapper tail calculation uses the same helper; its numerical value is unchanged.
- EQ permits finite drain only when every channel's biquad, SVF and advanced bank is empty and no coefficient transition is active. Nonempty recursive banks keep immediate native completion and `Unknown` tail metadata. Neutral gain is not treated as proof of finite support.
- Eligible 2x/4x EQ emits 1024 native frames of zero continuation. A prepared 256-frame cache always processes four canonical refills and serves arbitrary positive frame-aligned output capacities. Each call performs at most one refill and emits at most 256 frames. Destination capacity cannot retime AutoGain or metering.
- Eligible 1x EQ reports finite zero and accepts EOS without emitting samples. Nonempty finite streams freeze after EOS; empty streams do not. Reset/initialize re-arm. Changed scalar/batch/filter replacement controls reject after accepted EOS; unchanged known primitive values succeed without rebuilding state.
- Invalid rate/capacity leaves output and EOS state unchanged. Nonempty ordinary and compiled processing rejects before copying output after finite EOS. Only emitted drain prefixes are written.

## Independent red evidence

1. Public-adapter finite marker: default drain returned zero samples where ordinary zero continuation retained 2048 stereo samples. Log `target/audit-eq-finite-drain-red.log`.
2. Fresh-thread maximum callback: every 2x/4x × mono/stereo first 4096-frame call returned `Ok(4096)` but performed one allocation and one deallocation. Isolated probe `/tmp/sotf-eq-cold4096-probe.rs`, log `/tmp/sotf-eq-cold4096-red.log`. The prepared output queue was smaller than the maximum accumulated chunk output before readback.

## Verification

227 distinct focused tests passed:

- EQ: 134, all features, including eight new finite-stream tests.
- Gain: 66, all features; no source changes.
- Parametric adapter drain suites: four, including two new out-of-place forwarding/default tests.
- Existing host oversampling subset: 23.

The new EQ matrix covers both factors, mono/stereo, chunk boundaries through 4096 input frames, irregular input partitions, destination capacities 1/17/256/257/4097, exact ordinary zero-continuation parity, output canaries, AutoGain enabled/disabled across its measurement boundary, rate/capacity transactionality, reset/reinitialize, scalar/batch/filter replacement freezes, first-channel-empty recursive banks, SVF/advanced banks, and removing recursive filters while oversampler queues remain.

Cold first maximum processing, cold first drain, all drain work and reset count **zero allocations and zero deallocations**. Cold drain crosses the existing ten-callback measurement/cache publication boundary. Measurement ran on Linux; no unexecuted platform runtime claim.

Commands/logs:

```text
cargo test -p sotf-plugin-eq -p sotf-plugin-gain --all-features --lib --tests
  target/audit-eq-gain-drain-tests.log
cargo test -p sotf-host --test parametric_plugin_drain --test parametric_drain
  target/audit-parametric-drain-hooks.log
cargo test -p sotf-host --lib oversampling::
  target/audit-eq-host-oversampling-tests.log
cargo clippy -p sotf-plugin-eq -p sotf-plugin-gain -p sotf-host --all-targets --all-features -- -D warnings
  target/audit-eq-drain-clippy.log
```

Focused Clippy and scoped `git diff --check` are clean. A fixture initially accessed the old `ProcessContext.sample_position` field; it was corrected to `context.transport.sample_position` and the forwarding tests rerun green. No ignored or intentionally failing tests remain.

## Cached compiled-plan source review

`CompiledOpKind` has no Copy variant. Eligible 1x EQ remains `EqBiquadBank`; static-gain elimination applies only to `ApplyGain` (`host/daw_host.rs:2617`). The cached EQ operation therefore dispatches via `process_f32_node` → compiled adapter → EQ's EOS guard. If the compiled operation returns an error, existing host isolation tries ordinary processing, which reaches the second guard before copying. Existing host failure-passthrough behavior is unchanged; this patch does not introduce a new host-wide post-EOS error policy.

## Limits

- Recursive EQ EOS remains unsupported; no guessed cutoff was added. Timed offline rendering can explicitly zero-continue these configurations.
- The finite bound is conservative and includes endpoint zeros; it is not minimal response length.
- Normal callback AutoGain measurement cadence remains the existing behavior. Finite drain uses its documented canonical 256-frame cadence.
- Native drain remains f32, matching the existing shared trait. No new f64 drain API is claimed.
- No schema/preset IDs, native wrapper layouts, engine protocols, DAG queues, MIDI or IAMF source changed. The parent owns aggregate verification.
