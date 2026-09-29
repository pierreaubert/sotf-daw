# AUD-092 terminal same-format replacement: verified

## Result

The worker now drains the currently committed host before publishing EOS when a same-format replacement interrupts the old host's terminal tail-frame send or pending EOS send.

Only `processing_thread/processing_state.rs`, its existing `tests/eos.rs` fixture, and `audit/proposals/engine-terminal-replacement.md` changed in this scope. The production delta is the reviewed private committed-host epoch, its successful-commit increment, an epoch check on native completion, and a local EOS restart on unsent stale completion. No DSP, host trait, manager command, acknowledgement, playback transition, format policy, bypass protocol, or pending-frame policy changes.

## Red evidence

The isolated `/tmp/sotf-terminal-replacement-probe` compiled the actual worker copied byte-identically from production. Both deterministic tests failed before the fix: after an acknowledged replacement, the worker emitted stale EOS with replacement native `drain_calls == 0`. Cases were old terminal one-frame output and old zero-frame complete result blocked on EOS. Rendezvous output and acknowledgement ordering eliminate scheduling dependence. Source hash evidence is in the harness `snapshot.json`.

- Positive regressions before fix: 0 passed, 2 failed, as intended.
- Compatibility controls before fix: 2 passed, covering eight metadata/stale-generation/prepared-ticket/request-ticket rejection cases at both boundaries.

Logs: `/tmp/sotf-terminal-replacement-red.log`, `/tmp/sotf-terminal-replacement-rejected.log`.

## Green evidence

`cargo test -p sotf-engine --lib --no-default-features engine::processing_thread::tests::eos::`: **26 passed, 0 failed, 0 ignored** (18 existing and eight new tests).

New permanent coverage:

1. Final old tail frame remains delivered once, followed by all three replacement tail frames and EOS.
2. An unsent old EOS is withdrawn, then all three replacement frames precede EOS.
3. Metadata, stale generation, cancelled prepared ticket and cancelled outer request preserve the valid old final-frame completion.
4. The same four rejection cases preserve the valid pending old EOS.
5. Stop during restarted replacement drainage discards its unsent tail and allows fresh input.
6. Shutdown during restarted replacement drainage exits and disconnects output without stale tail/EOS.
7. Empty replacement tails complete after one native terminal call, for both old boundaries.
8. Repeated successful generation-zero replacements retain the old pending frame exactly once and drain only the latest host's not-yet-emitted tail.

The first full run was 25/26 because the new Stop fixture initially assumed two identical output samples after reset. Existing equal-power crossfade behavior sums the two identical reset hosts at the second output sample, producing 0.7502454. The fixture now derives this unchanged 50 ms / 96 kHz transition independently in f64; no crossfade production code or numeric tolerance was changed elsewhere.

Strict lint: `cargo clippy -p sotf-engine --no-default-features --all-targets -- -D warnings` passes. Scoped `rustfmt --check` and `git diff --check` pass.

Logs: `/tmp/sotf-terminal-replacement-green.log`, `/tmp/sotf-terminal-replacement-clippy.log`.

## Lifecycle and limits

The new epoch identifies successful committed host swaps only. Rejected, cancelled and stale updates do not change it. Shared queued-request generation remains unchanged. Reset and Stop clear existing DSP/transition history through their original paths; they do not reuse a committed identity. Native quotas remain owned by each host.

The same-format pending audio frame remains delivered once. Only an unsent stale EOS is withdrawn. Already delivered EOS and existing changed-format/bypass policies are outside this change. Scalar epoch operations introduce no heap work by inspection; this report makes no new blanket realtime-allocation claim for the engine.

The defect predates AUD077. The approval-blocked broader manager transition and host graph queue proposals remain unapplied. MIDI/IAMF excluded.

Source is frozen for independent review. Copy-ready independent AUD077 review: `/tmp/sotf-aud077-host-independent-review.md`.

## Root independent review

Root read the exact implemented production delta and all eight new worker
fixtures. The scalar epoch changes only on successful commit; old terminal
completion is checked against that identity after pending delivery. The local
EOS restart withdraws only an unsent stale EOS and preserves established
pending-frame, format-change and command-outcome ordering. No blocker found.
The Stop oracle independently models the existing equal-power transition; it
does not weaken a production assertion or alter that transition. No additional
Cargo run was needed for this source review.
