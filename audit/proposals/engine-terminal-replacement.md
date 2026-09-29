# AUD-092: Worker EOS after a successful same-format host replacement

## Status and scope

Two deterministic actual-worker regressions reproduced red in an isolated `/tmp` copy. The narrow worker-only change was approved after the tenth aggregate gate passed (5702 tests across 301 binaries). It is implemented and verified: all 26 EOS tests pass, including eight new boundary tests, and strict all-target engine Clippy passes with default features disabled. The original review patch and unchanged-worker red harness remain in `/tmp`.

The defect is preexisting. AUD077's removal of the global EOS counter did not introduce it; the new near-limit replacement test exposed the missing terminal boundary coverage.

This proposal is confined to `crates/sotf-engine/src/engine/processing_thread/processing_state.rs` and additions to its existing `tests/eos.rs` fixture. It uses the already accepted `CommitHostUpdate` worker operation and its existing acknowledgement. It adds no manager protocol, Stop/Play transition, playback hold/configure/release handshake, output-format negotiation, bypass protocol, shared host trait, or wire message. The approval-blocked broader manager transition proposal remains unapplied.

## Executed deterministic proof

Harness: `/tmp/sotf-terminal-replacement-probe/engine`, a copy of the real engine crate. Only temporary manifest paths/package name and the copied EOS test module were changed. The production worker file is byte-identical to its copied counterpart; `/tmp/sotf-terminal-replacement-probe/snapshot.json` records SHA-256 `a3b9321826c7dcddb73e4a91ff40c6f45effff43aab097bf40e8185fe413ed1c` for both.

Added test source: `/tmp/sotf-terminal-replacement-tests.rs`.

The existing fixture's output uses `sync_channel(0)`. `TailPlugin` queues a host replacement from inside its native drain call before returning. The test waits for `PluginChainUpdated` before receiving any pending output. Thus replacement necessarily succeeds during backpressure, without sleeps or scheduler-dependent timing.

| Case | Old native result | Required observed sequence after commit ACK | Actual old behavior |
|---|---|---|---|
| Final tail frame | one frame `[1]`, `complete=true` | old `[1]`, new `[3]`, `[2]`, `[1]`, one EOS | old `[1]`, EOS; new native drain called zero times |
| Blocked EOS | zero frames, `complete=true` | new `[3]`, `[2]`, `[1]`, one EOS | EOS; new native drain called zero times |

Both hosts have one output channel at 96 kHz, with 48 kHz input. The replacement is deliberately prepared with three known pending finite tail frames, matching the existing accepted-replacement fixture's public state model. This proves worker ordering independently of a particular DSP algorithm.

Command executed:

```text
cargo test --manifest-path /tmp/sotf-terminal-replacement-probe/Cargo.toml \
  --target-dir /home/pierre/src/all_of_sotf/sotf-daw/target \
  -p sotf-eos-terminal-probe --lib --no-default-features --offline \
  terminal_replacement_ -- --nocapture
```

Result: 0 passed, 2 failed as expected. Each fails with `stale EOS discarded replacement tail`, expected replacement frame 3, `new_drain_calls=0`. Log: `/tmp/sotf-terminal-replacement-red.log`.

## Source-backed cause

- `processing_state.rs:1081–1126`: the interrupted tail-send path checks command outcome, bypass and output geometry. A successful same-format swap preserves the pending frame, which is the established policy. After delivery, however, `if drain.complete` still refers to the old host's native result and exits drainage.
- `processing_state.rs:1137–1172`: an interrupted EOS send discards the pending EOS only for Stop or changed output format. An accepted same-format swap leaves the old EOS pending and skips the replacement's drain entirely.
- `commit_host_update` already makes acceptance precise: stale expected metadata, request generation and ticket failures return before host replacement. The successful path swaps `self.host`, updates metadata, and returns the acknowledgement data.

## Exact proposed state and ordering change

Reviewable diff: `/tmp/sotf-terminal-replacement-proposed.patch`.
Standalone proposed source: `/tmp/sotf-terminal-replacement-proposed.rs`.

1. Add private `ProcessingState::committed_host_epoch: u64`, initially zero. Increment with wrapping arithmetic only at the successful end of `commit_host_update`, after the actual host swap. No new allocation, locks, atomics, messages, or DSP calls.
2. Snapshot this identity immediately before each `state.host.drain` call. After sending its tail output, honor `drain.complete` only while that same committed host remains active. An accepted same-format replacement therefore proceeds into the next drain-loop iteration instead of using the old completion bit.
3. Label the existing decoder-EOS arm with a local restart loop. When backpressure interrupts an unsent EOS, snapshot the current committed identity before executing the command. If the command successfully swaps hosts, output format is unchanged, and EOS is still unsent, abandon that pending EOS and restart native drainage. Publish one EOS only after the currently committed host completes.
4. Retain the existing early branches for Stop, Shutdown, bypass, errors, disconnection, and format changes. The restart condition is specifically a successfully committed same-format host replacement with an unsent EOS.

### Pending output fate

- A same-format already-rendered audio frame remains pending and is delivered exactly once, including the old terminal tail frame. The new host drains afterward.
- An unsent EOS from the replaced host is withdrawn. It contains no audio buffer to recycle. A new EOS is sent after the replacement finishes.
- An EOS already delivered cannot be withdrawn; the change only operates on the existing `Some(unsent)` backpressure path.
- Changed-format frames/EOS retain current discard behavior; this proposal establishes no new format-transition policy.

### Identity, reset and command semantics

- The existing shared `host_generation` is a request-validity counter, not committed identity. It can change for cancelled/rejected work, and generation-zero prepared fixtures are accepted. It remains untouched.
- Metadata rejection, stale request generation, cancelled outer request, and cancelled prepared-host ticket do not increment the new epoch and do not restart EOS.
- Each successful replacement increments the private epoch, including generation-zero requests. Repeated successful replacements are distinguished without comparing heap addresses.
- Reset/Flush clears plugin history through its existing path and leaves committed host identity unchanged. Stop still resets and abandons the current stream; Shutdown still exits promptly. Neither needs an identity reset, avoiding reuse of a prior epoch.
- Parameter reads/writes and bypass keep their existing behavior. This proposal does not reopen completed prefixes for parameter actions.
- Each replacement `DawHost` owns its own quota state. The worker neither copies nor replenishes native quotas itself.

## Permanent verification proposed after approval

1. Land the two actual-worker red regressions unchanged and confirm green with the worker-only fix.
2. Pin rejected/cancelled/stale replacement at these two boundaries: keep the old terminal frame/EOS, and never drain the rejected host.
3. Pin a replacement with no tail: one retained old frame when present, then one EOS; no duplicate EOS and no busy loop.
4. Pin repeated same-format successful replacements, including generation-zero requests, and ensure only the latest host's not-yet-emitted tail is subsequently drained.
5. Retain/execute all existing EOS Stop, Shutdown, bypass, changed-rate, disconnection, timeout, failure, 5001-call and near-limit replacement fixtures. Add one Stop/Shutdown interruption while restarted replacement drainage is blocked if existing fixtures do not exercise the outer label sufficiently.
6. Focused engine Clippy with `--no-default-features`; no broad test expansion until these boundaries are verified.

Before application, two rejection tests covering eight deterministic metadata, stale-generation, prepared-ticket and request-ticket failures passed against the original worker. After application, all 26 EOS tests pass. The new permanent tests also cover Stop and Shutdown during replacement drainage, empty replacement tails, repeated generation-zero commits and exact old pending-frame order. The Stop fixture checks the existing reset 50 ms equal-power transition against an independent f64 formula.

Final verification:

```text
cargo test -p sotf-engine --lib --no-default-features engine::processing_thread::tests::eos::
cargo clippy -p sotf-engine --no-default-features --all-targets -- -D warnings
```

Both pass, as do scoped rustfmt and diff checks. Final report: `/tmp/sotf-terminal-replacement-verified.md`. Logs: `/tmp/sotf-terminal-replacement-green.log` and `/tmp/sotf-terminal-replacement-clippy.log`. Source is frozen for independent review.

## Independent review handoff

The completed AUD077 host scalar/cursor review is ready to copy from `/tmp/sotf-aud077-host-independent-review.md`. It records the host paths reviewed, no introduced scalar blocker, missing focused evidence, and this separately classified preexisting engine edge.
