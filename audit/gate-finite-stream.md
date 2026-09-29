# AUD-071 native Gate finite-stream checkpoint

Implemented only Gate DSP, new Gate integration tests, README and CHANGELOG. Limiter remains frozen; parent owns NIH/vendor bus corrections identified in the separate read-only review. The existing host parametric drain hooks from AUD-069 are reused without further host edits.

## Reproduction

Public `ParametricInPlacePluginAdapter<GatePlugin>` probe tested 288 cases: four rates (44.1/48/96/192 kHz), 1/2/6 channels, Downward/Upward/Duck, internal/external detection, and 0/0.001/5/20 ms lookahead. Dry program first/final markers were independently compared with `[actual-delay zeros] + program`; a separately zero-continued reference matched that exact oracle.

- All 216 positive-lookahead cases previously returned COMPLETE with zero drain frames, discarding the queued program suffix.
- All 72 zero-lookahead controls retained their complete stream.
- All 72 tiny positive 0.001 ms cases physically delayed audio by one sample while reporting zero latency. The shared ring clamps every positive active delay to at least one sample.

Artifacts: `probe.rs`, `probe.log`, and its matched compiled-library paths in `artifacts.json`. New production tests ran red before the implementation: `/tmp/sotf-gate-finite-red.log` (five failures).

## Corrected contract

- Finite tail and latency use the actual active `LookaheadBuffer::delay()`, or zero when lookahead is disabled. The bound counts retained program audio once; it does not count maximum allocated capacity, HPF/RMS history, hold time, or envelope release as audio support.
- Drain processes zero continuation through the unchanged native processing kernel, at most 256 frames per call. External mode supplies zeros for both program and keys in prepared widened scratch, returning only the compact program channels.
- Existing detector, filter, hold, attack/release, and smoother state continues naturally under zero input. Parameter targets remain fixed once EOS starts. This policy is compared with ordinary zero continuation, not a frozen-gain reference.
- Capacity comes from the destination's whole output-channel frames; drain ignores `context.num_frames`. Initialization, sample rate, output alignment, and nonempty required capacity are checked before output/history/EOS mutation. Rejected requests remain retryable with exact history.
- Empty-stream drain is a no-op. Valid drain on a nonempty zero-delay stream completes without emitted samples and enters EOS. Changed controls or new nonempty input then require reset/reinitialization; unchanged scalar/bulk snapshots remain accepted. Zero-frame processing is harmless. Existing structural restrictions remain in force.
- Reset clears EOS/history without heap activity. Initialization now also resets existing envelope/hold state, so reinitialization matches a fresh plugin, not just fresh delay/filter buffers.
- `tail_length()` returns `Unknown` before initialization and `Finite(active delay)` afterward. The scalar bound remains the full response bound rather than a decrementing drain count.

Preservation: the entire previous `process_in_place` body was moved to `process_stream` unchanged apart from its name, verified by byte comparison with `native-process-before.txt`. Downward/Upward/Duck arithmetic and default zero-lookahead waveform are unchanged. The bounded-subdivision opt-in from plugin_chain is retained.

## Independent regression coverage

New `tests/finite_stream.rs` has five tests covering:

- 864 exact delayed-identity cases, including first/final markers, 1-frame streams, streams shorter/longer than lookahead, all modes, channel layouts, rates, external-key strides, zero/tiny/ordinary/maximum delay, and output canaries.
- 288 nonlinear waveform comparisons against a separate zero-continued instance with different callback partitions. Variants cover Peak/RMS, linked/independent channels, second/fourth-order HPF, hold, knee/hysteresis, dry/wet mix, and a pending threshold smoothing ramp. Additional zeros after the bound are exactly silent.
- Invalid empty/unaligned/wrong-rate drain calls both before and during EOS, unchanged follow-on program/key history, completion idempotence, same-value versus changed scalar/bulk state, reset/reinitialization, and zero-input behavior.
- 54 explicit cold callback heap measurements: 1/2/6 channels × all modes × internal/external × zero/tiny/20 ms delay. Process, tail query, full drain, same-value setters, reset, and subsequent process perform **zero allocations and zero deallocations**, with RMS and fourth-order HPF enabled.

## Final checks

- `cargo test -p sotf-plugin-gate`: **93 passed, 0 failed, 0 ignored**. Log `/tmp/sotf-gate-finite-full.log`.
- `cargo clippy -p sotf-plugin-gate --all-targets -- -D warnings`: clean. Log `/tmp/sotf-gate-finite-clippy.log`.
- Scoped `git diff --check`: clean.
- Shared DSP kernel byte comparison: identical; subdivision marker retained.

Source is frozen for the aggregate gate. No active Cargo process remains from this task.
