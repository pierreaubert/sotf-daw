# sotf-plugin-gate

SOTF Gate plugin for noise gating.

Attenuates audio below a configurable threshold to remove low-level noise and unwanted background sound.

The plugin is stateful and must be initialized before processing. Each callback
must use the initialized sample rate and provide exactly `num_frames *
input_channels()` interleaved samples. External-sidechain mode uses programme
channels followed by matching sidechain channels in each frame and never writes
the sidechain samples.

Channel linking, sidechain HPF frequency/order, detection mode, external
sidechain mode, and lookahead are structural settings: change them by rebuilding
the graph. A runtime write of the existing value is accepted as a no-op.
A rejected live structural write does not move or clear the active delay line.
Hosts replacing a lookahead configuration must align the old/new plan latency
before crossfading; the plugin cannot compensate a graph by itself.
`range_db = 0` means unlimited attenuation with a finite 240 dB numerical ceiling.
Processing, realtime parameter writes, and reset are allocation-free; non-finite
audio and detector samples are treated as silence before entering DSP state.

## Finite streams and lookahead

After the final input block, call `drain()` until complete. It emits exactly the
active lookahead ring delay, including the final program samples. External-key
mode continues both program and key inputs with zero but returns only program
channels. HPF, RMS, hold, release, and smoothed targets continue normally with
zero input; those detector/envelope histories do not add audio support.

Latency and the finite `tail_length()` bound use the actual active ring delay.
A positive lookahead shorter than half a sample still uses the ring's minimum
one-sample delay and reports one sample; exactly zero remains zero latency.
Tail length is `Unknown` before initialization.

Drain accepts any nonempty whole-frame output capacity and writes at most 256
frames per call. The destination determines capacity, independently of
`context.num_frames`; the sample rate must match initialization. Invalid rate,
shape, or required capacity changes neither history nor EOS state. Empty-stream
drain is a no-op; zero-delay streams complete without extra samples.

Once valid drain begins for a nonempty stream, reset or reinitialize before new
input or changed controls. Identical scalar/bulk snapshots remain accepted;
existing structural rebuild requirements still apply. Reset and reinitialization
clear the prior stream. Normal silence processing retains ordinary automation.
Valid processing, drain, scalar tail queries, and reset allocate and free no
memory; widened external-key scratch is prepared during initialization.
