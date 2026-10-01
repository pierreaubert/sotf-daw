# sotf-plugin-delay

SOTF Delay plugin with feedback.

Audio delay line supporting up to 5000ms of delay with feedback control.

The default moving-head mode retains tape-style Doppler motion. The opt-in
`pitch_preserving` mode instead transitions between two fixed fractional-delay
taps for 20 ms. Every nonidentical tap change fades the old tap fully out before
fading the new tap in. This conservative switch avoids moving-head Doppler,
phase rotation, and destructive summing for arbitrary input. It is
callback-partition independent, adds no fixed latency, and performs no
processing-time allocation. Existing presets default it off. LFO rate and depth
must both be zero in this mode: input-agnostic modulation between differently
delayed taps cannot guarantee carrier and phase retention without nulls.

Delay-time automation is tape-style: moving a read head produces the expected
Doppler pitch glide. Four-point Lagrange interpolation controls fractional-tap
error but does not make time changes pitch preserving. LFO motion clamps only
the out-of-range half-cycle at a delay boundary. One shared LFO phase is
intentional: it preserves stereo/multichannel image coherence rather than
turning Delay into a phase-spread chorus.

`try_new_with_max_delay` and `new_per_channel_with_max_delay` declare the
maximum live automation range and size the ring accordingly. The ordinary
scalar constructor retains the full five-second range; the RoomEQ per-channel
constructor uses the largest configured route delay and exposes no effect
controls. Effect settings fail at construction, and runtime writes that
deviate from the pure routing values are rejected without changing the
accepted configuration or populated history; the parameter schema marks those
controls unsupported in per-channel mode.

## Native host tails

The zero-input response bound is unknown before initialization. With no feedback
history, the bound is the prepared ring capacity in output frames, covering
fractional interpolation, modulation, and both transition read heads. This is
conservative: it can exceed the currently selected delay time.

Any nonzero feedback keeps the tail classified as infinite until reset clears
the history with a zero feedback target. Setting feedback or wet mix to zero
does not immediately discard that classification, because smoothed feedback and
allpass history may still contribute. This metadata lets native hosts continue
processing through silent gaps.

## End of stream

With no recursive feedback history, native `drain` continues the existing DSP
with zero input for one prepared ring traversal. It preserves fractional taps,
LFO motion, per-channel delays, and unfinished delay transitions. Each call
writes at most 1024 output frames and accepts any positive whole-frame output
capacity. Only the returned prefix is written. The conservative bound can
include trailing zeros; no level threshold truncates the response.

A valid first drain freezes parameter changes and later nonempty input until
`reset` or successful `initialize`; unchanged parameter writes remain accepted.
Invalid rate, partial-frame, and insufficient-capacity calls preserve the
stream. Empty streams complete without entering EOS. Continuation and reset
reuse prepared storage without allocating or freeing on the audio thread.

Recursive feedback EOS remains unsupported: `drain` retains its previous
immediate-completion behavior, and tail metadata remains `Infinite`. This does
not mean recursive audio has been exhausted. No guessed cutoff or new render
error is introduced. A renderer-controlled infinite-tail policy is separate
work.

The engine currently limits a stream to 4096 drain calls. Very long/high-rate
rings or multiple serial tails can exceed that limit even though each plugin
makes finite progress; the plugin's drain does not shorten its response to
fit that engine policy.
