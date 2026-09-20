# sotf-daw

DAW core workspace for SOTF: audio engine, plugin family and host,
MIDI integration, IAMF support, and the driver transport crates.

- `crates/sotf-engine` — decode, process, playback, and manager runtime.
- `crates/sotf-plugins` — plugin registry, host (`sotf-host`), DSP plugins,
  and FFI/bridge/packaging layers.
- `crates/sotf-midi` — MIDI device management and control.
- `crates/sotf-iamf` — IAMF decoder.
- `crates/driver-common`, `crates/driver-hal` — driver protocol and macOS
  HAL-side shared-memory transport.

`sotf` (apps, player, services) and `sotf-systemwide` (daemon) depend on
this workspace. It must not gain dependencies on either: the only
sibling-`sotf` edges are the optional `sotf-engine[streaming]` integration
and dev-dependencies on `sotf-testkit`/`sotf-test`, which resolve from the
sibling `../sotf` checkout in the `all_of_sotf` layout.
