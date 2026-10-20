# sotf-host

Core traits, host, and shared utilities for SOTF audio plugins.

Foundation crate for the SOTF plugin system providing:
- Plugin and analyzer traits
- Parameter system with automation support
- Plugin host for chaining processors
- Smoothing utilities for click-free parameter changes
- SIMD helpers for audio processing

## Hosting external plugins

Native hosting is opt-in: enable `external-plugin-clap`,
`external-plugin-vst3`, or (on macOS) `external-plugin-au`. A disabled or
failed backend is a graph-construction error; SOTF never silently substitutes
dry passthrough for a requested native processor.

`PluginScanner` performs filesystem discovery only. Its descriptors use zero
channels to mean "metadata not probed" and must not be used to size isolated
IPC. In-process loading replaces discovery metadata with the native ABI
metadata. Isolated callers must supply probed channel metadata.

Native instances allocate planar scratch for their negotiated maximum block
size. `ExternalPlugin::new_with_max_block_frames` exposes that contract;
isolated workers use the maximum from their shared-memory layout. The legacy
constructor uses an 8192-frame upper bound.

Isolated processing keeps shared memory mapped for the worker lifetime and
preallocates all block scratch. A wait deadline is capped at 75% of the current
block period. Runtime worker failures use dry audio delayed by the worker's
reported latency and a bounded 64-frame transition; startup failures abort
graph construction because latency is not yet known. The sandbox is the
process boundary—the mapping is not a security boundary against code already
running inside the worker.

The version-2 isolated protocol has bounded MIDI and parameter-event rings plus
a separate 64 KiB non-realtime control region. It exposes parameter discovery,
set/get, and native save/load state; `capture_worker_state` refreshes the
restart sidecar before project save. MIDI and parameter sample offsets and
transport metadata survive the process boundary. Oversized event/control
payloads fail explicitly rather than allocating on the audio thread.


## Loudness measurement retention

`LoudnessMonitor` owns integrated-loudness history independently of the backend
meter's whole-program storage policy. Both built-in and explicit channel layouts
use the same prepared history. The programme meter supplies momentary and
short-term windows with backend integrated-history accumulation disabled.

The default `IntegratedLoudnessMode::Rolling` retains the latest 36,000 overlapping
400 ms observations on the accepted programme's 100 ms grid, approximately one
hour. Quiet audio and silence advance this window and can age earlier loud audio
out. Pausing freezes the programme clock while live meters continue. Reset clears
the history and cached result while retaining prepared storage.

`WholeProgram` retains all observations up to the prepared capacity, then reports
`IntegratedProgramCapacityExceeded` and marks integrated loudness unavailable.
Continuing live measurement after exhaustion does not grow a second backend
history. Either mode prepares 288,000 bytes of energy payload, plus bookkeeping;
optional LRA storage remains separate. Queries use cached results refreshed by a
bounded two-pass scan after new observations.

The second gating pass respects both the relative threshold and the -70 LUFS
absolute floor. Below-floor blocks cannot re-enter when the relative threshold
is lower. NaN contamination remains visible until measurement reset. Loudness is
reported in LUFS; the hour-long retention/heap checks do not establish true-peak
conformance at their fallback test rate. Independent gating and LRA vectors,
true-peak vectors and retained-reader tests cover those separate contracts.
