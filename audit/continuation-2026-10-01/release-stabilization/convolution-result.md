# Convolution stabilization result (source-only assessment)

Date: 2026-10-03. Scope: `sotf-plugin-convolution` only. No commits, no push,
no delegation, no shell (root runs tests). TokenSave MCP tools were not
available in this session; diagnosis is from direct source inspection.

## Failing gate

`convolution_reset_clears_processing_state` in
`crates/sotf-plugins/crates/sotf-plugin-convolution/tests/integration.rs:246`
(`assert_eq!(&output[1024 * 2..], &input[..1024 * 2])`):
left starts near 0 then `0.19999388`, right `0.2`.

## Cause: incorrect async fixture, not a reset regression

The fixture set `ir_file` (which starts an asynchronous worker-thread load
via `begin_async_load`), processed a single 128-frame zero block hoping the
load would finish, called `reset()`, then asserted inactive (dry,
latency-matched) output.

Three facts make that race deterministic-failure-prone rather than testing
`reset`:

1. `reset()` (`src/lib/convolution_plugin.rs:1499`) clears signal state only
   (FDL, buffers, rings, smoothers, transitions, tails). It intentionally
   keeps the loaded IR and does not cancel a pending completion. This matches
   the host contract (`Plugin::reset`: "clear buffers, reset filters";
   native backend: "without changing persisted parameters") and the
   `direct_convolution.rs:346-348` fixture, which relies on reset preserving
   a loaded IR while ending its replacement fade.
2. Every `process_in_place` call runs `accept_ir_completion()` first, so the
   IR can activate during the warmup block *or* at the top of the final
   assertion block, depending on worker timing.
3. The failure values prove activation: the delta IR `[0, 32767]` decodes to
   `[0.0, 0.9999695]`, and `0.2 * 32767/32768 = 0.19999388` is exactly the
   reported left value; the leading near-0 samples are the one-sample IR
   shift (plus a possible 128-frame replacement fade) inside the wet path.

The old fixture was also weak when it passed: its pre-reset history was all
zeros, so it could not distinguish a correct reset from a no-op.

## Fix (fixture only, exact assertions preserved)

Rewrote `convolution_reset_clears_processing_state` to remove the async IR
load: it prefills stream state with 1500 nonzero frames, calls `reset()`,
then keeps the original exact assertions (first 1024 frames all `0.0`,
remainder bit-equal to the delayed input). Without `reset`, the delay line
would emit stale `0.9` samples, so the test now genuinely guards reset
semantics. No production code changed; no equality/bound weakened.
Reset-with-a-loaded-IR remains covered by the synchronous `from_params`
fixtures in `direct_convolution.rs` / `finite_stream.rs`.

Changed file: `crates/sotf-plugins/crates/sotf-plugin-convolution/tests/integration.rs`
(only the one test; `write_delta_ir` is still used by
`convolution_process_with_ir`, so no dead code).

## Gates for root to run

- `cargo test -p sotf-plugin-convolution --test integration convolution_reset_clears_processing_state`
- `cargo test -p sotf-plugin-convolution` (full crate: unit + integration +
  direct_convolution + finite_stream)
- `cargo clippy -p sotf-plugin-convolution --tests` if lint gate covers tests

## Residual risk (not changed, out of minimal-fix scope)

`convolution_process_with_ir` (same file) uses the same single-warmup-block
pattern and asserts output energy `> 0.0`; it is flaky in the opposite
direction (fails only if the worker is slower than one block). Left untouched
per smallest-fix scope; consider a bounded poll loop if it ever flakes.
