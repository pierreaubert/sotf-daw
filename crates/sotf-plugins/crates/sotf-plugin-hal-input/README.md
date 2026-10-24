# sotf-plugin-hal-input

SOTF HAL Input plugin for macOS audio HAL input.

Reads audio data from the macOS CoreAudio HAL driver via shared memory, acting as the input source for system-wide audio processing.

`input_channels` is structural and must match the negotiated HAL stream. Runtime
status is available through `HalInputPlugin::diagnostics()` rather than plugin
automation. When `needs_control_recovery` becomes true, a control thread may call
`refresh_transport()` to validate a replacement without changing the active
reader on refusal. A liveness supervisor should call `service_transport()` instead:
if broker attachment, key loading or format validation fails, it latches the
source disconnected and silent until a validated replacement succeeds. A healthy
probe retains the active reader and its partially consumed decrypted record;
periodic checks must not discard those source frames. Both
operations can open mappings and load keys and must never run in the audio callback. The plugin reports zero graph-compensation
latency because shared-memory capacity is not measured signal latency.

Each nonempty callback validates readiness and format before and after its single
transport read. A detected disconnect or key fault discards the entire callback
and returns silence. An unavailable or changing format (including capacity during
a read), or an oversized reader count, clears output and returns an error.
These observations do not provide an atomic native epoch fence or roll back
frames already consumed by the transport.

`missing_frames` counts replacement silence in successful callbacks, including
startup, disconnected and key-recovery silence. Rejected callbacks do not add to
that count. Underruns count short reads only after the current stream has supplied
a positive read; disconnect/recovery and successful initialization or reader
replacement restart that arming state. Rejected initialization/replacement leaves
the active state intact. Empty callbacks observe readiness without reading audio
or adding missing frames.

Connection generations count observed readiness edges. Recovery and format
change generations count continuous fault episodes; only a validated nonempty
read or successful control preparation closes an episode. `last_error` describes
the most recent detected condition or processed callback, rather than permanent
fault history. A healthy idle observation does not close pending recovery.

Controlled-reader release tests cover 1–16 channels at 44.1/48/96/192 kHz,
mid-read faults, exact output/counter transitions and callback heap activity.
Routine native `hal` checks are compile-only. The explicit isolated integration
target below additionally exercises real mapped transport and writer processes;
Swift/HAL callback races, source-clock drift and device deadlines remain separate.

The ignored `native_transport` target requires an explicit fresh owned fixture
root. It spawns a bounded child with shared-memory, key and runtime paths all
redirected before any plugin constructor runs. Invoke only this target, rather
than the full native suite whose other constructors may use live defaults:

```bash
SOTF_HAL_AUDIT_FIXTURE_ROOT=/absolute/fresh/private-dir cargo test \
  -p sotf-plugin-hal-input --release --features hal,sotf-host/qa \
  --test native_transport -- --ignored --nocapture --test-threads=1
```

Native worktree evidence covers 1–16 channels at 48/96 kHz, actual plain and
encrypted reads, partial records, key rotation with old staged plaintext,
transactional failed refresh, format refusal and exact first frames after
mapping replacement. A separate writer process is killed abruptly: the retained
immutable mapping remains readable, queued frames stay exact, empty reads become
counted silence, and explicit refresh attaches the new writer's first frame.
These callbacks assert zero allocations. The evidence records the current
uncommitted driver dependency hashes; it does not certify a clean-driver build
or real Swift/device operation.

`connected` observes shared `driver_ready`; it is not broker-process liveness.
After abrupt writer death with that flag still set, missing frames and underruns
advance but automatic control recovery is not requested. A control supervisor
must schedule `service_transport()` to check broker availability. After a failed
service, stale readiness cannot rearm callbacks: missing frames advance without
new underruns or reader consumption, reset does not clear the latch, and
initialization refuses until control preparation succeeds. A failed transactional
`refresh_transport()` continues to preserve its prior reader and diagnostics.
Hardware clock drift, callback-time format races and automatic service scheduling
remain outstanding.
