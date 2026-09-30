# sotf-daw

DAW core workspace for SOTF: audio engine, plugin family and host,
MIDI integration, IAMF support, and the driver transport crates.

- `crates/sotf-engine` — decode, process, playback, and manager runtime.
- `crates/sotf-plugins` — plugin registry, host (`sotf-host`), DSP plugins,
  and FFI/bridge/packaging layers.
- `crates/sotf-midi` — MIDI device management and control.
- `crates/sotf-iamf` — IAMF decoder.
- `crates/sotf-streaming` — HTTP streaming input and live PCM output.
- `crates/sotf-testkit`, `crates/sotf-test-macros` — shared test fixtures.
- `crates/driver-common`, `crates/driver-hal` — driver protocol and macOS
  HAL-side shared-memory transport.

`sotf` (apps, player, services) and `sotf-systemwide` (daemon) depend on
this workspace. It must not gain dependencies on either: there are no
sibling-`sotf` edges, not even optional or dev-only ones. `sotf-streaming`
and the `sotf-testkit`/`sotf-test` test crates are local members.
