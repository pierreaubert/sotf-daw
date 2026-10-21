# sotf-plugin-hal-output

SOTF HAL Output plugin for macOS audio HAL output.

Writes processed audio data back to the macOS CoreAudio HAL driver via shared memory, completing the system-wide audio processing chain.

The serialized construction state is `{ "channels": N }` for layouts from 1
through 16 channels. The legacy `output_channels` key is accepted when loading
presets. Channel layout is construction-only and is not exposed as an
automatable plugin parameter.

Successful `process()` calls perform bounded, allocation-free transport work. Partial
writes are retained in a preallocated complete-frame FIFO. When the FIFO is
full, the newest complete frames are dropped deterministically and counted in
`HalOutputTelemetry`; older queued audio always keeps its order. Transport
diagnostics are available through `telemetry()` as versioned 64-bit counters,
not through the automation parameter schema.

Daemon restarts, shared-memory remapping, configuration changes, and encryption
key rotation are serviced by `service_transport()`. Hosts must call it from a
non-realtime control thread; the audio callback never performs filesystem or
key-loading work. Re-service quiesces readiness, preserves the logical pending FIFO, flushes the
shared ring, and re-primes it before publishing readiness. Reset or successful
reinitialization explicitly cancels pending frames and counts those drops.
A failed prime leaves readiness false and the shared ring empty.

Initialization maintains a target shared-ring fill equal to the negotiated HAL
buffer size. `latency_samples()` reports that deterministic transport delay plus
the Swift virtual device latency (one negotiated device buffer) and its
zero-frame safety offset. `HalOutputTelemetry` v2 exposes target and observed
fill, device latency, safety offset, and their fixed boundary-latency sum.

Process and EOF drain use the same complete-frame FIFO flush, including a frame
split across the physical queue wrap. Aligned slices stay on the direct path;
split frames use prepared staging. A flush makes at most two writer calls;
process can make one further call for its new input only after older audio is
fully acknowledged. No callback allocation or queue-capacity shrink is needed.

`requested_frames` counts input owned by successful `process`/sink-append calls,
including frames explicitly dropped by the bounded queue policy. An errored
current request remains caller-owned, while valid writes of older queued frames
still advance `written_frames` immediately. A writer-contract fault gates later
writes until control recovery. An observed configuration change also stays
latched if another consumer clears the shared notification; only explicit
control-side format validation authorizes resumption. With saturating counters, logical ownership is
`requested = written + queued + dropped`; priming silence is excluded. These are
bridge acknowledgements, not proof that a device played the frames. A malformed
writer report leaves that batch's physical delivery unknown. Dynamic queue/fill
occupancy is separate from the fixed boundary latency.

The queue audit uses controlled transports, all 1-16 channel layouts, five ring
geometries and 44.1/96 kHz clocks. It verifies sample order, frame conservation,
partial writes, blocked states, recovery, reset/reinitialize and prepared storage.
Real driver restart/key rotation, cross-process saturation/clock drift and device
deadline behavior still require separate native runtime evidence. Native feature
compile checks do not execute the live HAL output.
