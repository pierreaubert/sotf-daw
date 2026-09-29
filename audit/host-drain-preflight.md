# AUD090: host drain output validation before DSP

2026-09-28. Narrow implemented fix; AUD077 call-budget design remains separate and unimplemented.

## Defect and independent proof

`DawHost::drain` passed its full native scratch capacity to a plugin, processed the resulting tail through downstream nodes, then checked the caller's output length. A recoverable-looking destination-size error therefore consumed the tail permanently.

Public isolated probe `/tmp/sotf-host-drain-capacity-probe.rs` and `.log` uses a deterministic four-frame native tail: a three-sample destination returns `need 4 samples, got 3`; full-size retry returns zero/complete while an untouched host twin emits `[1, 2, 3, 4]`. Caller sentinel remains unchanged, making the consumed internal state especially easy to miss.

## Exact source scope

- `crates/sotf-plugins/crates/sotf-host/src/host/daw_host.rs`: drain capacity documentation plus validation after graph mutation adoption/build and linear-graph validation, before BufferGuard/native drain/downstream process. Reject zero-channel geometry, partial output frames, multiplication overflow, and insufficient conservative capacity.
- `crates/sotf-plugins/crates/sotf-host/src/host/tests/drain.rs`: five permanent regression tests appended to the existing shared test file; existing tests preserved.

No engine change, cursor, drain-call contract, plugin DSP change, or manager/queue protocol change. Existing large host-file changes belong to prior work; this fix is confined to the drain capacity query documentation and drain preflight.

## Public behavior and limits

A nonempty host destination must hold whole output-channel frames and at least `drain_output_frames_max() * output_channels()` samples. This conservative requirement applies even if the next result is shorter. Previously some undersized buffers happened to succeed for a short final block; that accidental acceptance is no longer supported. Extra whole-frame destination storage remains untouched after the returned valid sample prefix.

The check uses graph state after already-supported queued adoption/build. Such graph changes may have occurred before a size error; no native audio drain or downstream processing has occurred. A caller that queried capacity before a queued layout/rate change may need to query again after the safe error.

Empty graphs remain immediate COMPLETE without storage. All-bypassed and native-zero-capacity completed graphs still complete using an empty destination. An already-completed native plugin that continues advertising a nonzero maximum still requires that capacity; the host currently has no independent completed-prefix cursor to detect otherwise.

This is output-validation transactionality only. A native plugin Err or downstream DSP failure is not made rollback-safe. The defensive post-DSP actual-size check remains for a plugin violating its advertised bound; no claim that an invalid third-party declaration becomes transactional.

## Executed evidence

- Red: `cargo test -p sotf-host --lib host::tests::drain --offline` — four failures, five passes. New replay, partial-frame, post-queue-adoption, and conservative-short-final requirements fail on old production. `/tmp/sotf-host-drain-preflight-red.log`.
- Green: `cargo test -p sotf-host --lib --tests --offline` — **617 passed, zero failed/ignored, 16 suites**. `/tmp/sotf-host-drain-preflight-tests.log`.
- Replay matrix: channels 1/2 × downstream frame ratio 1/2; after rejected capacity, three subsequent results and samples exactly match an untouched twin, including short final tail/terminal complete and trailing sentinel.
- Additional permanent cases: misaligned output with more than minimum samples is rejected before either native drain or downstream process; queued downstream 2× stage changes required capacity before DSP; conservative maximum is enforced on a short final tail; empty and zero-capacity completion remain stable.
- `rustfmt` applied to both touched files; scoped `git diff --check` clean.

Strict Clippy passed: `cargo clippy -p sotf-host --all-targets --all-features --offline -- -D warnings`, exit 0. Log: `/tmp/sotf-host-drain-preflight-clippy.log`. Sources are frozen at this checkpoint.
