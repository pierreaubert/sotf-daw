# AUD-069 native limiter finite drain — verified checkpoint

## Behavior and scope

Implemented in `sotf-plugin-limiter` plus the explicitly approved minimal host adapter contract. No audio-path oversampling, engine transitions, host DAG queues, MIDI, IAMF, factory, or native-wrapper edits.

- The public limiter now emits the entire retained suffix: exactly active `lookahead_len + (isp_mode ? isp_delay_len : 0)` frames after the last accepted input. The ISP audio ring is three detector delays. Allocated lookahead capacity, detector history, sliding-maximum storage, and release times are not counted as additional audio support.
- Each drain call emits at most256 output-rate frames and honors smaller whole-frame destinations; only returned samples are overwritten. Output capacity comes from the destination, not `context.num_frames`.
- Valid nonempty-stream drain enters EOS, including zero-delay completion. New nonempty input and changed controls require reset/reinitialization. Identical control snapshots are allowed without retargeting smoothers, including bulk snapshots. Existing structural rebuild restrictions remain unchanged outside this EOS no-op handling.
- Empty-stream drain is a no-op. It neither enters EOS nor freezes controls. Complete drain is idempotent for valid destinations/rates; zero-frame input remains harmless.
- Initialization/rate, frame shape, and nonempty required capacity are validated before entering EOS or changing history/output. Invalid retry leaves the complete retained waveform intact. Processing also rejects uninitialized/wrong-rate input before modifying DSP.
- `tail_length()` returns `Unknown` before initialization and `Finite(active delay)` afterward, allocation-free. It is the full response bound, not a decrementing drain counter; native tail consumers can count silence independently.
- Envelopes, detector history, and pending smoothers continue naturally under zero input. Parameter targets are fixed during drain; learned/gain state itself is not frozen.

Compatibility assessment: the generic drain API does not promise parameter changes while draining, and the existing oversampler/adaptive drains already require reset before input continuation. Native callbacks that keep processing silence do not call this finite-stream drain API and retain ordinary automation behavior. Explicit control freezing supplies a stable finite-stream policy; idempotent snapshots remain compatible with hosts resending state. Presets, IDs, defaults, and normal initialized processing arithmetic are unchanged.

## Minimal required host addition

`ParametricInPlacePlugin` previously had no drain methods, so implementing an inherent limiter drain could not affect its public adapter. Added default `drain_output_frames_max() == 0` and `drain() == COMPLETE` plus forwarding through both `Plugin` and `InPlacePlugin` adapter implementations in `sotf-host/src/parametric_in_place_plugin.rs`. Existing plugins retain the same zero-tail defaults. Host source ownership was handed back to plugin_chain immediately after this edit.

New `sotf-host/tests/parametric_drain.rs` proves unchanged default behavior and forwards explicit output-channel frame counts, capacity/rate errors, output canaries, and completion through both adapter traits. It deliberately uses differing input/output channel counts to catch accidental input-layout assumptions.

## Evidence

Before production changes, the isolated public API probe (`eos_probe.rs`, `eos_probe.log`) found36 nonzero-delay failures among48 cases: default drain completed without emitting any retained frames. The independent low-level oracle was exactly `[delay zeros] + input` and separately zero-continued processing matched it bit-for-bit.

New production integration tests first compiled and ran with six failures (`/tmp/sotf-limiter-finite-red.log`). With the implementation all six passed; a seventh malformed-input transaction regression was then added and passed in the final full suite.

`tests/finite_stream.rs` covers:

- 180 exact first/final-marker cases: four rates44.1/48/96/192kHz, 1/2/6 channels, zero/native dry/mixed/ISP/maximum-lookahead configurations, and1/121/delay+31-frame streams.
- 108 dense nonlinear comparisons to a separate zero-continued limiter with different callback partitions, including hard/soft, wet/mixed, ISP, dual release, and independent/partial/full channel linking. Extra zero continuation after the declared audio bound is exactly silent.
- Partial drain/canaries, wrong-rate/unaligned/empty-capacity retries before and during EOS, retained history after failed requests, post-EOS control and input behavior, bulk snapshot idempotence, reset/reinitialize, empty input, and zero-delay ISP at192kHz.
- Explicit allocation **and deallocation** instrumentation across12 prepared rate/channel/configurations. A cold valid process, scalar tail query, complete drain, unchanged setters between drain calls, reset, and subsequent process all perform0 allocations and0 frees.

The existing process body was moved to a shared private `process_stream` function without changing its bytes except the function name. `/tmp/sotf-limiter-final-protection/native-process-before.txt` retains the prior body; a script verified byte equality before and after the final source edit. Existing native limiter tests also remain green.

## Final checks

- `cargo test -p sotf-plugin-limiter`: **123 passed,0 failed,0 ignored**. Log `/tmp/sotf-limiter-finite-full.log`.
- `cargo test -p sotf-host --test parametric_drain`: **2 passed,0 failed**. Log `/tmp/sotf-parametric-drain.log`.
- `cargo clippy -p sotf-plugin-limiter --all-targets -- -D warnings`: clean. Log `/tmp/sotf-limiter-finite-clippy.log`.
- `cargo clippy -p sotf-host --test parametric_drain -- -D warnings`: clean. Log `/tmp/sotf-parametric-drain-clippy.log`.
- Scoped `git diff --check`: clean.

Oversampling remains the separate unimplemented design in `design.md`; its prototype does not establish a shipped feature. TokenSave saved over30,000 tokens across this investigation's focused source reads.
