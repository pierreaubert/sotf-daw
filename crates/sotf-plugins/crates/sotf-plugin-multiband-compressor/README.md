# sotf-plugin-multiband-compressor

SOTF Multiband Compressor plugin for multiband dynamic range compression.

Splits the signal into frequency bands and applies independent compression to each band for frequency-selective dynamics control.

## Crossover precision and validation

Crossover coefficients, state and serial split intermediates use f64; band audio
returns to f32 before dynamics processing. This corrects low-cutoff rounding
error at high sample rates and changes multiband numerical output. Parameter
formats, automation, latency and one-band processing retain their contracts.
Processing adds no heap allocation or deallocation. CPU cost has not been
benchmarked for this precision change.

The existing cascaded LR4 topology remains: each split operates on the preceding
high-pass output. With three or more bands, the sum is not generally a flat
all-pass response because low bands do not receive the later splits' phase
rotation. No-gain-reduction reconstruction is checked against this topology,
including its interband phase and transient timing.

`tests/audit_signal_paths.rs` uses independent f64 Tustin/DF-I filter references
and detector/gain trajectories. Its 252 cases cover 1–5 bands, 44.1–192 kHz,
solo/sum closure, M/S, lookahead, peak/RMS detection, knee, attack/release,
continuous stereo linking and near-threshold modulation. Successful callbacks
are checked for finite output and zero heap activity. Release allocation checks
require `--features qa`.

## Finite streams

Call `initialize` before processing and repeatedly call `drain` after the last
input block. Drain writes only its returned output frames, up to 256 per call.
Invalid rate, incomplete channel frames or insufficient capacity for a pending
tail leave drain history untouched and can be retried. A supported nonempty EOS
freezes controls and new input until `reset` or reinitialization; identical
parameter snapshots remain accepted. Empty-stream drain is a no-op. Reset
retains parameter targets and clears EOS and audio history.

Exact finite drain is supported for one-band time-domain processing and a dry
mix whose current smoothed value and target are both exactly zero. The bound is
the active lookahead delay, including the one-frame minimum for a positive
sub-sample request. Lookahead metadata uses the actual ring delay. A target of
zero during a wet-to-dry fade does not establish finite support.

Multiband wet audio includes recursive crossover response even with ratio 1 or
per-band bypass and reports `TailLength::Infinite`. The default retains its
legacy zero-frame COMPLETE drain behavior. Native Rust render jobs can choose
an explicit cutoff before wrapping the plugin or starting drain:

```rust
use sotf_plugin_multiband_compressor::MultibandCompressorPlugin;

let mut plugin = MultibandCompressorPlugin::new(2);
plugin.set_recursive_tail_render_frames(Some(48_000)).unwrap();
```

At 48 kHz that requests 48,000 zero-input output frames after the final input.
The budget includes remaining lookahead, then deliberately truncates the
recursive response at the selected frame. Zero explicitly discards the recursive
wet tail; `None` restores the legacy policy. Choose the cutoff and any final
fade at the render-job level. This is not a silence detector or a proof of finite
IIR support: mathematical metadata remains Infinite while recursive wet audio
can contribute. Exact one-band and settled-dry support takes precedence over
this recursive-only policy.

The first valid supported nonempty drain freezes the selected extent, including
while mix settles dry. Reset/reinitialization rearm EOS and retain the configured
frame count; a rate change does not rescale it. Identical policy writes after
EOS are allowed and changed policies require reset. Rejected rate/capacity
calls do not start EOS. The option is native job configuration, separate from
serialized parameters, factories and FFI preset restoration. Existing presets
retain their behavior. Detector, gain and smoothing state evolves normally
through the chosen zero continuation.

`drain_output_frames_max` is a per-call capacity; `tail_length` is a bound from
the last input and does not count down. Queries are allocation-free. The generic
in-place adapter can copy input to output before an inner `process` error;
processing errors do not promise destination preservation. Drain capacity
errors and direct/compiled EOS preflight retain their transactional guarantees.

The native adapter and a same-width DawHost chain are verified to emit the
selected extent. Host paths requiring finite mathematical metadata, including
channel-changing drain, retain their existing Infinite-tail restrictions.
