# AUD126: Start/Pause/Continue/Reset UI and Control Path

**Status:** design and implementation accepted by independent review. This file
records the design; implementation evidence is in
`audit/coupled-integrated-lra-pause-ui.md`. AUD126 is complete. MIDI and IAMF
remain excluded, and AUD127/AUD128 plus the broader audit remain open.

## Goal and surfaces

Add explicit Integrated Loudness/LRA lifecycle controls, separate from the
Loudness Monitor's existing destructive `Enabled`/bypass control. The GPUI
controls live in the already mounted Studio Loudness Monitor custom panel at
`crates/app-gpui/components/plugins/ui_loudness.rs`, reached through
`custom_view_registry/render.rs`. The TUI exposes the same actions in the
existing LevelMeters focus mode and reports the output meter's latest published
state in `ui/draw_meters/draw.rs`.

The selected GPUI Loudness Monitor instance is the control target. Resolve its
stable graph node/instance to the current engine index; never use the selected
linear index as an engine index or assume the monitor is first/last. The TUI
LUFS panel reads `playback.loudness_info` from
`PluginGraph::output_monitor_engine_index()`, so its controls must target that
same output monitor engine index. Input and output monitor data and control
status remain independent.

## User-facing behavior

- **Start** clears the prior measurement epoch and begins a new Running I/LRA
  epoch. It is available at startup and can restart an already-running epoch.
- **Pause** freezes only Integrated Loudness and LRA accumulation. Audio keeps
  passing through; Momentary/Short-term, their maxima, sample/true peaks,
  correlation and true-peak maximum continue to reflect live input.
- **Continue** resumes the same I/LRA epoch without including paused samples.
- **Reset** clears measurement history and maxima while preserving Running or
  Paused. Reset while Paused remains Paused; live M/S/peak measurement continues
  on subsequent input.
- The plugin Enabled/bypass switch remains a separate destructive operation.
  Its existing reset-and-Running semantics are unchanged.

Use explicit text status (`Running`, `Paused`) and controls with accessible
names. The latest `LoudnessData` is a best-effort snapshot: if publication is
blocked by retained readers, show the last published state and a separate
`Pause requested` / `Reset requested — waiting for meter update` status. Do not
claim the measurement has changed until the snapshot acknowledges the control.
An immediate route error clears pending state and presents an error. A stale
snapshot keeps the request visibly pending; it is not treated as an implicit
failure and must not automatically repeat Start/Reset. The user can deliberately
retry the same action. TUI key help names all four operations and its status
line uses the same latest-snapshot/pending distinction.

At narrow GPUI widths, stack the lifecycle controls and status below the meter
summary; keep the current meter bars visible and let the panel scroll rather
than overlapping or clipping the controls. Provide translated strings for every
supported locale and use the existing design-system button, text, color and
spacing APIs.

## Control and acknowledgement protocol

The accepted core exposes the idempotent `integrated_running` Boolean parameter
and `LoudnessData.integrated_measurement_running`. That Boolean cannot
acknowledge Start or Reset: Reset while Paused and Start while Running leave the
Boolean unchanged. A generation comparison alone also cannot correlate a
publication to a particular request: a delayed earlier snapshot could arrive
after a newer request. Each plugin construction receives a fresh, nonzero,
host-issued `integrated_control_instance_id`, published in `LoudnessData`.
Checked exhaustion fails construction; it never wraps or reuses an ID.
Reinitializing the same plugin object preserves its identity and request
high-water/receipt. Replacing or reconstructing the plugin assigns a new
identity even if its graph node and engine index are reused.

Add `integrated_control_instance_id: u64` and
`integrated_control_request_id: u64` to `LoudnessData`, with serde defaults of
zero for old snapshots. Each app process allocates nonzero, checked,
monotonically increasing request IDs for explicit actions. IDs are runtime-only;
they and pending UI requests are never restored from a preset or serialized app
state. The host echoes both target instance and request ID only after the
request is applied, in the same complete snapshot as the running flag. A
blocked write leaves the previous receipt intact; a later writable publication
carries the latest applied receipt. IDs are request receipts, not audio-time or
measurement-epoch counters.

Route Start, Pause, Continue and Reset through one command-style String plugin
parameter, `integrated_control_command`, whose canonical ASCII value is
`<instance-id>:<request-id>:<operation>` with no leading zero, whitespace or
extra separator. Both ID fields are checked `u64` decimals; request IDs are
nonzero. The maximum length is bounded to 50 bytes. Its metadata default and
getter are the empty string. Request IDs are nonzero, checked `u64` values
allocated by one app-session counter; overflow stops new requests instead of
wrapping. They are not restored or persisted. Each plugin instance retains its
last applied request ID and operation across Start/Reset, measurement reset,
cache rebuilds, and same-object reinitialization. A target instance mismatch is
rejected before mutation so a queued command cannot affect a replacement at a
reused engine index. A newer ID executes once and is echoed as the published
receipt. A duplicate of the last ID with the same operation never re-executes
and may republish the existing receipt. Reusing that ID with a different
operation and any lower ID are rejected without state mutation or acknowledgement
change; gaps are allowed. Unknown operations and malformed IDs are rejected
before mutation. This parameter is not part of `PluginSettings`,
presets, or restored plugin state. Parsing is bounded and allocation-free in the
plugin callback. Start resets the measurement and enters Running. Reset resets
the measurement and preserves Running/Paused. Pause and Continue change
`integrated_running`; if the requested state already holds, the host leaves
measurement state unchanged but still acknowledges that request ID and
attempts a snapshot publication. This handles a UI race without leaving a no-op
request pending. Rejection at the control submission boundary clears the
pending request with a visible error.

Each UI pending record stores the exact `(instance_id, request_id)` pair and
operation. It clears only when the matching runtime instance publishes that
same receipt. It
never acknowledges from a matching Running/Paused Boolean, cold-looking values,
or an unrelated generation change. An older delayed publication cannot match a
new request ID. If the selected monitor is removed or replaced while the
request is pending, cancel the old pending record with a visible
`Monitor changed; request not confirmed` status; never transfer it to the new
instance. The TUI keys its pending action to the actual output monitor instance
ID in the latest snapshot, not the selected channel group or a reused engine
index.

While a request awaits publication, renders and timer polls do not resend it.
A visible Retry control allocates a fresh request ID and deliberately submits a
new action. This is explicit because retrying Reset or Start begins another
epoch. If the player rejects the queue submission, clear pending and show the
error immediately. If the host accepts the queue item but no audio block runs,
retain a `Waiting for meter update` state; do not report success or failure
until a matching receipt is published. A deliberate retry is always safe from
false acknowledgement because its ID is distinct.

The host control update remains allocation-free and must publish the full
current snapshot candidate without consuming meter queries or interval peaks.
An allocation test covers Start, Pause, Continue and Reset while all readers are
released and while strong/nested-Weak readers retain generations.

## Implementation scope

Expected sibling paths are limited to:

- `../sotf-daw/crates/sotf-plugins/crates/sotf-host/src/analyzer.rs` and
  `analyzer_loudness_monitor.rs` from the sibling checkout, for the receipt and
  command parameter contract;
- `crates/app-gpui/components/plugins/ui_loudness.rs`,
  `components/plugins/custom_view_registry/render.rs`, and the owning app/plugin
  control state for mounted buttons and per-instance pending status;
- the existing dev API query allow-list and
  `tests/e2e/scenarios/plugin_rack/mute_solo_level_meter.rs` for routed
  observation/click coverage;
- `crates/app-tui/events/level_meters.rs`, the keybinding catalog, the TUI
  parameter-update queue, `main/misc.rs` and `ui/draw_meters/draw.rs` for the
  output-monitor controls/status;
- existing GPUI/TUI translation tables and focused tests for complete locale
  coverage and narrow layouts.

Do not alter plugin chain topology, playback transport, host branch queues,
manager concurrency protocols, or unrelated meter rendering. Preserve all
existing sibling work and its restored Cargo.lock. Record the temporary tested
lock and restored-lock hashes separately if Cargo requires temporary offline
resolution; do not claim the restored lock was tested unless a `--locked` gate
actually used it.

## Acceptance tests

1. **Host command contract:** Starting cold, Start while Running, Reset while
   Running, Reset while Paused, Pause, Continue, repeated same-state Pause and
   Continue, malformed/zero/overflow IDs, duplicate and reordered older IDs,
   and invalid operation names. Assert
   Running/Paused state and the exact echoed request ID for each applied
   command. Same-state Pause/Continue leaves measurement state unchanged but
   acknowledges the request. Verify the command metadata/getter are empty,
   repeated new IDs each execute, duplicate IDs execute at most once, old IDs
   are rejected, and command values are absent from preset/config restoration.
2. **Live-vs-integrated signal behavior:** Build populated I/LRA and live M/S/TP
   histories, pause via the public plugin parameter route, feed non-silent audio,
   and compare to a no-pause control. I/LRA remains held and excludes paused
   samples; live M/S and true-peak/current and maxima advance. Reset while Paused
   clears the measurement epoch but remains Paused; live M/S/TP continue on
   subsequent callbacks. Continue matches the A+C concatenation reference, and
   Start after a populated epoch begins a clean Running epoch.
3. **Retained snapshot acknowledgement:** With strong and nested-Weak readers
   retaining published generations, issue one command and assert the old
   snapshot/receipt remain visible and UI status says requested/waiting. Queue a
   second request before publication and deliver the earlier receipt afterward;
   it must not clear the newer pending request. Release readers, permit one
   writable publication, and assert only the exact matching receipt clears
   pending. Reset while Paused and Start while Running prove acknowledgement
   does not depend on the Boolean state. Inject rejected submission and verify
   pending clears to an explicit error. Verify stale receipts never auto-retry
   Start/Reset and a deliberate retry uses a new ID.
4. **Mounted GPUI route:** In `Screen::Studio`, select an output monitor and an
   input monitor in turn. Click each actual dev-tracked button; assert the
   intended engine plugin instance receives the command and the other monitor
   is unchanged. Query both published running flags/receipts after accepted
   publication. Exercise Start, Pause, Reset while Paused, Continue, Reset while
   Running, a retained-snapshot wait and acknowledgement, error/retry, translated
   labels, compact panel and narrow viewport bounds. A helper-only render test
   does not satisfy this gate.
5. **TUI routed controls:** Dispatch the documented LevelMeters keys through
   the actual event dispatcher. Assert commands target exactly
   `output_monitor_engine_index()`, never the input monitor or selected channel
   group. Cover pause/reset/continue/start, no-op state behavior, latest
   published status, requested/pending acknowledgement, and translated help.
   Rebuild/replace the graph instance while a command is pending and verify the
   old request is canceled, never transferred to a new monitor with a reused
   engine index.
6. **Build and static gates:** Run focused host, TUI and GPUI tests, then host
   strict all-target Clippy, app-tui checks/tests, app-gpui all-target check and
   relevant E2E tests, design-token and pseudo-locale checks, formatting, and
   `git diff --check`. Capture exact source/Cargo.lock manifests at every build
   snapshot; serialize Cargo operations against the parallel Upmixer track.

The staged acceptance criteria were met for the reachable GPUI and TUI
controls, host command/receipt semantics, live-meter behavior, reset while
Paused, and snapshot-lag behavior. Astra accepted this AUD126 stage on
2026-09-29. AUD127 and AUD128 remain separate open issues.
