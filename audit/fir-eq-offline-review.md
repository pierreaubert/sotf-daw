# FIR EQ and offline timed-tail review

Reviewed read-only on 2026-09-28, then added only two parent-authorized engine regression tests and their module registration. Delay/Convolution and production renderer source remained untouched by this reviewer.

## FIR EQ drain

No new production blocker found in the reviewed finite-drain implementation.

- The response bound `32 + coefficient_count - 1` includes NUPC's emitted startup delay once. It also bounds the delayed dry path for both phase modes.
- Work is capped at 256 frames independently of destination capacity; whole-frame capacity, rate and prepared-state checks precede output/state mutation on active streams.
- Nonempty processing is rejected after accepted drain. Reset clears NUPC and dry-ring history and re-arms the stream; same-rate ordinary initialize retains history, while initialize after EOS or a rate change clears it.
- The processing kernel has no new allocation, Arc publication or destruction paths. The fresh-thread allocation/deallocation fixture covers all four FIR lengths, both phase modes, mono/three channels, first processing and first drain callbacks.
- Direct f64 convolution of prepared coefficients provides an independent numerical oracle for NUPC scheduling and support. This validates convolution timing, not the independent correctness of FIR coefficient design.
- Minor policy distinction: `apply_values` rejects all nonempty updates after drain, including unchanged values. Unlike the new Delay/Convolution policy, no-op writes are not accepted; the reviewed public documentation should not promise otherwise.

## Confirmed endpoint failures

New file: `crates/sotf-engine/src/offline_renderer/tests/endpoint_composition.rs`.

Command:

```text
cargo test -p sotf-engine --lib offline_renderer::tests::endpoint_composition -- --nocapture
```

Red log: `target/audit-offline-endpoint-composition-red.log`. Result: 0 passed, 2 failed, 0 ignored.

### Source-converter response truncated before downstream compensated endpoint

`render.rs` stopped source conversion at `target_frames + ceil(source_converter_signal_delay)` and then supplied zeros directly to the host. Downstream FIR taps can still depend on converter output beyond that boundary after the combined signal delay is trimmed.

Oracle: a final impulse in a 441-frame, 44.1 kHz source, converted to 48 kHz and passed through a nonflat FIR EQ, compared against the prefix of a much longer explicitly zero-padded 4410-frame source. This longer reference avoids sharing the candidate's early endpoint. With zero requested tail, sample 479 was 1.2170883 instead of 1.1734073, absolute error 0.043681026. The fixture also checks a one-millisecond tail once the first failure is corrected.

Small correction: when a source converter exists, continue sending source-rate zeros through that converter and the host until the final exported target has been written. Do not switch to direct host-input silence early.

### Completion budget omitted bounded chunk waiting

`drain_host_to_duration` budgeted only remaining output, remaining physical delay and eight extra callbacks. Physical signal delay intentionally excludes chunk assembly waiting, which can exceed eight small callbacks.

Oracle: one input frame at 48 kHz, configured resampler 48→96 kHz with a 4096-frame chunk, automatic terminal conversion back to 48 kHz, one-frame export callbacks and a one-millisecond tail. The renderer returned `Plugin host did not drain to the requested 49 frames (wrote 0)`.

Small correction: keep physical signal delay for trimming, but add conservative scheduler waiting to the work budget. The parent's proposed budget sums remaining exported frames, remaining trim, host terminal-clock latency, and source-converter output-clock latency; ceiling-convert that sum to source frames, ceiling-divide by callback size, and retain a small progress allowance. Use checked arithmetic for representability. The no-source-conversion path similarly needs host scheduling latency in its continuation budget.

## Scope and verification limits

These two red tests are the only new executions by this reviewer. The parent owns production fixes and subsequent green runs. No host DAG queues, manager protocol, MIDI or IAMF source was changed.
