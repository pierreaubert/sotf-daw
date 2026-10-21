# sotf-plugin-hal-input

SOTF HAL Input plugin for macOS audio HAL input.

Reads audio data from the macOS CoreAudio HAL driver via shared memory, acting as the input source for system-wide audio processing.

`input_channels` is structural and must match the negotiated HAL stream. Runtime
status is available through `HalInputPlugin::diagnostics()` rather than plugin
automation. When `needs_control_recovery` becomes true, a control thread may call
`refresh_transport()`; that operation can open mappings and load keys and must
never run in the audio callback. The plugin reports zero graph-compensation
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
Native `hal` checks are compile-only; actual driver restart/key rotation,
cross-process epoch changes, source-clock drift and device deadlines still need
isolated transport/device validation.
