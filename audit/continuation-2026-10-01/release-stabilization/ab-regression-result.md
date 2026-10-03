> SUPERSEDED BY ROOT RUNTIME EVIDENCE: the static conclusion below is incorrect. `build()` subsequently calls `rebuild_graph_drain_plan()`, which increases emission scratch to 262146 samples. Instrumented focused execution measured required/prepared output 262146/262146; the fixture therefore fits. Temporary instrumentation was removed. See `drain-fixture-result.md` and `ROOT-RECEIPT.md` for the corrected disposition. This report is preserved as review history, not acceptance evidence.

# AB regression result: aud140 drain-preflight failure (release blocker)

## Verdict

No defect exists in the current tree. The broad failure
(`drain_preflight_rejects_expansion_and_contraction_scratch_extents`,
`aud140_channel_changing_eof.rs:460`, got
`PluginDrainResult { frames: 1, complete: false }`) is **not reproducible
from current source**: the exercised preflight provably rejects on the
current tree, and the suspected cause (the checkpoint revert of the
channel-changing preflight envelope preference at `daw_host.rs:2268`) is
**refuted** — it is behavior-neutral on this graph. No production or test
change was made, deliberately: any edit would risk the green host-665 and
AB gates to chase a phantom. Root should re-run broad on a clean tree;
capture instructions if it reproduces are below.

## Refutation of the 2268-revert hypothesis

The failing test exercises the **chain** preflight
`validate_channel_changing_drain_scratch`
(`crates/sotf-plugins/crates/sotf-host/src/host/daw_host.rs:2227-2300`),
reached via the chain drain path (`drain`:4710-4739). Line 2268 there reads
`downstream.output_frames_for_input(frames)` (live-only after the
checkpoint revert; envelope-preferring in R29). Both fixture nodes publish
**no** `output_frames_envelope` (FIR fixture: no override;
`AmbisonicsDecoderPlugin`: no override;
`plugin.rs:820` default is `None`), so R29's envelope preference falls back
to the live query and yields the identical value (43691) as R28's live-only
read. The revert cannot change any verdict on this graph. The graph
preflight fold (`min_input_frames_for_drain_output_frames`, ~2247-2270 in
older numbering) is a different function and does not run here.

## Proof the current tree rejects (expansion sub-case, runs first)

All links read from live source; arithmetic exact:

1. Fixture capacity: `PREPARED_GRAPH_SCRATCH_SAMPLES / 6 + 1`
   = 262144/6 + 1 = **43691**
   (`aud140_channel_changing_eof.rs:20,473`; helper :444-448, :453
   asserts the host query equals it).
2. `add_plugin` x2 populates `chain_nodes=[fir,amb]` plus the linking edge
   (`daw_host.rs:2385-2388`): chain topology holds (`:4635-4644`).
3. `build()` prepares `scratch_output.len() = 262144`:
   `prepared_sink_frames=8192` (`:1379-1388`),
   `max_graph_frames=8192` via live fallback (`:1394-1403`;
   `path_output_envelope` returns `None` per `:2778-2793` + `plugin.rs:820`,
   `path_output_frames` returns 8192 per `:2733-2751` + identity lives),
   `graph=8192*32=262144` (`:1404-1406`); merge joins skipped (single
   predecessor, `:1510-1514`); chain envelope prep skipped (no
   `drain_frames_envelope`, `:1529-1531` + `:4545-4575` `?` at `:4557` +
   `plugin.rs:514` default `None`); allocation at `:1564-1567`.
4. `process` of 1 frame cannot grow scratch: per-node need is
   `live(1)=1` frame (`:3175-3198`, `:7172-7182`).
5. `drain()` takes the chain path (`:4710`), `channel_changing_drain=true`
   (`:4713`, width check `:2080` sees ambisonics 4->6), chain validation
   passes (`:4735`), chain scratch preflight runs (`:4738`).
6. Preflight requires `43691*6 = 262146` output samples: source frames
   43691 (`:2247`, fixture `:243`), downstream live 43691 (`:2268`,
   ambisonics identity `:527-529` of
   `ambisonics_decoder_plugin.rs`), equality passes the `:2269` floor,
   product at `:2275-2278`.
7. `262146 > len 262144` errors at `:2293-2298` with an "output scratch
   samples" message containing "scratch" — `expect_err` at test `:460`
   passes, and `:461-467` (message + no-begin/no-drain + output
   untouched) follow. Contraction sub-case likewise rejects
   (262208 > 262144 at `:2287`).

Sibling aud140 tests passing in the same broad run (fitting extents,
cap-1 drain convergence, alignment/capacity errors) are consistent with
this analysis: only the 2-sample margin case is sensitive.

## The tripwire

Required 262146 vs prepared 262144: a **2-sample** margin. Any +2 growth of
prepared chain scratch (build sizing, merge terms, envelope prep) or any
structural preflight change flips exactly this test while leaving host-665
and AB green. That selectivity matches the broad log (single failure at
6854/7395, siblings green).

## Consistent explanations for broad.log

Static analysis excludes every in-tree mechanism (all sizing, fold,
dispatch, default, and growth paths verified above; trait defaults sane at
`plugin.rs:496,514,820`). Remaining consistent explanations:

- (A) Transient interference: another lane's uncommitted host change
  present when broad ran (multi-worker tree; other authors active on
  shared host files), absent now. Most likely.
- (B) Stale test binary reused by the runner (cargo fingerprinting
  normally prevents; exotic).
- (C) A static-analysis blind spot (none found after exhaustive tracing).

## Root action: re-run + capture

1. Confirm clean tree state (`git status`, no other lane's uncommitted
   host/plugin edits), then re-run the focused binary followed by broad:
   `cargo test -p sotf-plugins --test aud140_channel_changing_eof`
   (expect 15 pass + 1 pre-existing ignore), then the broad gate.
2. If green: blocker cleared with zero changes; this doc is the record.
3. If red again (unexpected): capture before anything else —
   `RUST_BACKTRACE=1` focused output, `git status` + stash list at run
   time, test-binary mtime vs source mtimes, and the exact
   `required/prepared` values (temporary `eprintln!` at `:2287-2298`
   behind `#[cfg(test)]`, or a debugger watch). Do not weaken the test;
   the calibrated margin is the tripwire, not the bug.

## Gates and files

- Gates executed by this worker: none (shell disabled; analysis only).
  Relied-upon prior gates: host-665 pass, AB all-targets pass (R28
  restoration, per root); broad.log single-failure record read at
  `audit/continuation-2026-10-01/release-stabilization/checkpoint-gates/broad.log:7566-7583`.
- Files changed: none. Owned scope (`host/daw_host.rs` + targeted tests)
  inspected read-only; AB algorithm untouched; no other passing test
  affected.
- Known limitation: conclusion is static-proof-based; final confirmation
  requires root's re-run (step 1 above).
