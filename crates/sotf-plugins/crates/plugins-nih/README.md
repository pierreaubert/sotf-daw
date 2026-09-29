# plugins-nih

VST3/CLAP plugin wrappers for SOTF audio plugins via nih-plug.

## What It Does

Wraps every SOTF audio plugin as both a VST3 and CLAP plugin using the nih-plug framework. Each plugin is built as a separate dynamic library (cdylib) selected by feature flag, producing a `.dylib` that exports both VST3 and CLAP entry points for use in any DAW.

## Features

- 29 audio plugins available as VST3 and CLAP
- One cdylib per plugin (feature-flag selected)
- Automatic parameter mapping from SOTF ParamSpec to nih-plug parameters
- Buffer format conversion (nih-plug planar to SOTF interleaved)
- Allocation detection in audio thread (`assert_process_allocs`)

## Usage

```bash
# Build the EQ plugin
cargo build --release -p plugins-nih --features eq --no-default-features

# Build the compressor
cargo build --release -p plugins-nih --features compressor --no-default-features

# Build all plugins
just build-nih-plugins
```

The resulting `.dylib` files can be loaded by any VST3 or CLAP compatible DAW.

## Available Plugins

| Feature | Plugin Name | CLAP ID |
|---------|-------------|---------|
| `eq` | SOTF: Parametric EQ | `org.spinorama.sotf.eq` |
| `compressor` | SOTF: Compressor | `org.spinorama.sotf.compressor` |
| `limiter` | SOTF: Limiter | `org.spinorama.sotf.limiter` |
| `gate` | SOTF: Gate | `org.spinorama.sotf.gate` |
| `gain` | SOTF: Gain | `org.spinorama.sotf.gain` |
| `delay` | SOTF: Delay | `org.spinorama.sotf.delay` |
| `upmixer` | SOTF: Upmixer | `org.spinorama.sotf.upmixer` |
| `binaural` | SOTF: Binaural | `org.spinorama.sotf.binaural` |
| `xtc` | SOTF: Crosstalk Cancellation | `org.spinorama.sotf.xtc` |
| ... | *(29 total)* | |

## Architecture

```
lib.rs      -- Feature-gated plugin definitions (one per feature)
params.rs   -- DynamicParams: runtime nih-plug parameter generation
wrapper.rs  -- sotf_nih_plugin! macro: generates full nih-plug impl
```

## EQ controls and channel layouts

The EQ wrapper exposes 20 bands with stable `band_N_freq`, `band_N_q`,
`band_N_gain`, `band_N_filter_type`, and `band_N_order` IDs. Each band starts
as a neutral peaking filter. Existing global parameter IDs are unchanged.
Frequency, Q, gain, and type automation use preallocated DSP transitions;
band order, topology, and oversampling are restored during initialization.

Channel-changing plugins advertise their actual DSP widths: mono-to-stereo
uses 1→2, upmixer and AAE use 2→6, ambisonics uses 4→6, and band split uses
2→4. NIH exposes only output channels in its main processing buffer, so
band merge uses a stereo main input plus a stereo auxiliary input, while
AEC and beamformer use a mono main input plus a mono auxiliary input.
These auxiliary buses carry the remaining DSP input channels in order.

## Restoring structural settings

Saved construction settings are restored on activation, including limiter
lookahead/ISP mode, room presets, crossover types, and dynamic EQ bands.
The wrapper preserves historical DAW parameter IDs and translates them to
the DSP constructor's keys and choice values. Settings that require a bus
layout outside the packaged wrapper's declared layout fail activation.
Unsupported legacy compressor sidechain settings also fail explicitly when
their saved values differ from the historical defaults.

Structural settings retain their existing hidden, non-automatable host
presentation. If a host restores different structural state while audio is
active, processing returns an error and silence until the host deactivates
and reactivates the plugin. Automatic host reactivation is not implemented.
Ordinary realtime parameter automation continues without reconstruction.

Saturation includes saved mode and oversampling choices. Native wrappers apply
the plugin's requested 2x/4x oversampling before initialization and report the
resulting filter latency to the DAW. Continuous saturation controls use direct
scalar access without rebuilding parameter maps on the audio thread.

## Native automation timing

Gain and Gate enable NIH's sample-accurate parameter dispatch. The native CLAP tests
verify the first event in a callback, equal-time ordering, multiple events,
irregular buffer sizes, and Gain's independent 10 ms smoothing envelope.
Gate's waveform is checked against independent sample-split DSP processing through
its hold and smoothing periods. The pinned NIH copy in
`crates/3rdparties/nih-plug` fixes first-event dispatch and
seconds-only transport offsets during buffer splitting; its README records
the original commit and the exact behavioral changes.

EQ and LinearPhaseEQ retain their existing sample splitting settings. Other
plugin families currently receive native parameters at callback boundaries.
Buffered and oversampled processors need separate event-queue handling before
sample splitting can preserve the timing of audio retained between callbacks.

## Native tail reporting

Native tail queries use each DSP's output-rate zero-input response bound, including
physical buffering delay once. This differs from per-call drain capacity and the
latency used for PDC. Finite bounds are cached after initialization and processing;
CLAP receives change notifications on the audio thread. Queries and notifications
remain valid across reset and restart, including a GUI state restore at callback end.

Unknown, recursive, or unrepresentably long responses request continued processing.
This protects retained audio, but unaudited plugins can use more CPU while idle.
Gain reports zero; Delay feedback is recursive, and Convolution/oversampling report
their conservative finite bounds where available. A VST3 query before activation
reports unknown because the DSP has not yet been initialized for the selected rate.

## Ownership and lifetime

Each VST3/CLAP plugin wraps a SOTF plugin instance created through
`plugins-bridge::create_plugin()`. The generated wrapper owns the instance via
`Box<dyn Plugin>` and uses `plugins-bridge::state` for preset serialization.
All audio buffers are converted from nih-plug's planar layout to SOTF's
interleaved layout inside pre-allocated scratch memory. Native regression tests
also check the selected parameter getter/setter and DSP paths for allocations.

## Testing

```bash
cargo check -p plugins-nih --features eq
cargo test -p plugins-nih --features eq --lib
```

## License

Part of the SOTF (Sound of the Future) project.

### Gate modes and key input

Gate preserves its original downward mode and adds upward expansion and ducking.
The mode is a structural choice restored during activation; maximum boost is a
realtime control. CLAP keeps its original stereo layout as configuration0 and
adds `Stereo + Sidechain` as configuration1. VST3 initially advertises the latter
so hosts can discover its key bus; the original stereo-only layout remains
selectable for saved-style negotiation. Both layouts keep stereo main input/output.
Use the key-bus layout before activating saved `sidechain_external=true` state.
Internal detection ignores this bus. External detection uses the key channels while emitting only
the stereo program, including linked or independent channel detection. A saved
external-key state with the stereo-only layout fails activation explicitly.

Actual CLAP and VST3 tests verify port discovery, layout selection, activation and
independent-key audio. Native tests also cover missing key input across changing
callback lengths and rejection of incorrect VST3 key widths. Generated wrapper tests cover all three modes, invalid
key buffers, reset, default-layout compatibility and allocation-free callbacks.
Changing detector routing remains structural and requires host reactivation.
