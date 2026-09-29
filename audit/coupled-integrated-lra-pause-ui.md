# AUD126: Start, Pause, Continue, and Reset UI

**Status:** reachable TUI/Studio controls and layered host integration accepted
by Astra on 2026-09-29. AUD126 is complete. AUD127, AUD128, and the broader
metering audit remain open. MIDI and IAMF remain excluded from workspace gates.

## Behavior and routing

The LevelMeters TUI screen and mounted Studio loudness panel expose Start,
Pause, Continue, and Reset for Integrated Loudness/LRA. These commands do not
pause audio playback or live M/S, true-peak, sample-peak, and correlation
measurement. Reset clears I/LRA data while preserving the current Running or
Paused state.

Commands are transient and are never stored as preset parameters. A command
identifies the host-issued runtime instance and carries a monotonically
increasing request ID. The host rejects malformed, duplicate conflicting, and
stale commands before changing measurement state. A matching published receipt
acknowledges a request. Retained snapshots may lag under publication pressure;
they do not acknowledge a newer command. Retry is available only for an
unresolved request to the same live monitor and uses a fresh request ID.
Replacement or disappearance cancels the old request instead of transferring
it to a reused engine index. A late exact receipt or replacement identity is
more authoritative than an earlier transport error.

The TUI targets the output monitor from the LevelMeters key dispatcher. The
Studio panel controls the analyzer role whose snapshot it displays, which may
be the input or output analyzer; the visible user graph node is not itself the
audio host. The live dev-api reports the selected engine index, exact command,
request, error, and latest snapshot state.

## Validation

DAW host controls and allocation evidence:

- `cargo clippy --offline --locked -p sotf-host --all-targets -- -D warnings`
  — pass.
- `cargo test --offline --locked -p sotf-host --lib integrated_control_command_tests`
  — 2 passed.
- `cargo test --offline --locked -p sotf-host --test loudness_control_commands`
  — 4 passed.
- `cargo test --offline --locked -p sotf-host --test loudness_pause_realtime public_string_command_applies_borrowed_control_without_allocating_with_retained_readers -- --nocapture`
  — 1 passed. The already-prepared `ParameterId` and command `String` are
  dropped inside the guarded public setter; parsing, measurement-state change,
  and best-effort publication allocate zero bytes while three generations are
  retained (the guard records 0 allocations and 2 frees). This does not claim
  that building or transporting owned command strings is allocation-free.
  Log: `/tmp/sotf-aud126-host-public-string-realtime-final.log`.

TUI coverage:

- `cargo test --offline --locked -p sotf-tui --lib` — 365 passed in the full
  final3 run. Log: `/tmp/sotf-aud126-tui-lib-final3.log`.
- `cargo test --offline --locked -p sotf-tui --lib level_meter_key_commands_reach_only_the_output_host_and_acknowledge_real_receipts -- --nocapture`
  — 1 passed after the final helper refactor. The real key dispatcher sends
  Start, Pause, Reset while paused, and Continue to one of two actual host
  instances, reads back its receipts, and verifies that the input host is
  unchanged. It also covers malformed-command error and fresh-ID retry.
  Log: `/tmp/sotf-aud126-tui-host-route-final.log`.
- `cargo clippy --offline --locked -p sotf-tui --all-targets --no-deps -- -D warnings`
  — pass. Log: `/tmp/sotf-aud126-tui-clippy-final2.log`.

Mounted GPUI coverage:

- `cargo test --offline --locked -p sotf-gpui --features dev-api --test e2e programme_maximum_loudness_queries_resolve_live_app_state -- --nocapture`
  — 1 passed. The mounted Studio test exercises the actual control click and
  dev-api query, then applies the exact recorded engine index and command to
  two initialized `LoudnessMonitorPlugin` instances. The selected output
  host's published snapshot is fed back to the panel; UI status and receipt
  are checked after Start, Pause, Reset while paused, and Continue, while the
  independent input host remains unchanged. Log:
  `/tmp/sotf-aud126-gpui-live-route-host-bridge-final.log`.
- `cargo check --offline --locked -p sotf-gpui --all-targets --features dev-api`
  — pass on that source snapshot. Log:
  `/tmp/sotf-aud126-gpui-check-final4.log`.
- GPUI package formatting and the prior TUI/GPUI localization, design-token,
  and diff checks pass. The final all-target check emitted existing warnings
  for two `sotf-player` imports and test-only `rate`/`ratio` helpers; no
  compile error was reported.

The GPUI fixture separately covers delayed, older, late exact, and replacement
receipts. The host bridge is intentionally layered: the test applies the
recorded UI payload directly to the real plugin through `Plugin::set_parameter`.
It does not run a real `Player` processing thread or prove end-to-end engine
transport delivery. The DAW host test measures the owned public setter after
the caller created its strings; it does not measure allocation in the whole
transport path.

## Dependency lock and source snapshot

Sibling checks used the reconstructed temporary offline resolver lock
SHA-256
`7a03b9f561ee929aa189ee70881d84563ab1b1d0036b3a76078d50c033a5af3a`.
Earlier candidate UI checks used a separate temporary lock; no sibling result
is claimed against the restored lock. The exact original dirty sibling lock
was restored from `/tmp/sotf-aud126-original-Cargo.lock` and verified at
SHA-256
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`.
Offline `--locked` resolution with that original lock is unavailable in this
environment because the local math-audio checkout does not match its locked
math-optimisation version.

The final GPUI/TUI source manifest is
`/tmp/sotf-aud126-sibling-source-final-accepted.sha256`, aggregate SHA-256
`bd97ee929fff3fae4e4759298eae442a827e37ca8549451addc9818e1a1b7075`.
The AUD126 host source manifest is
`/tmp/sotf-aud126-host-source-final.sha256`, aggregate SHA-256
`59ae5f4146e0313536084d18104755a37af8f002af4c2bdbaf3da8e567d6a2f3`;
the realtime test source also matches the host regression start/end manifests.

## Remaining audit work

AUD126 is complete. AUD127 first-minute LRA stability, AUD128 official corpus
coverage, and unrelated open audit items remain separate work. This acceptance
makes no EBU Mode or certification claim and does not close the broader audit.
