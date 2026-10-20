# sotf-plugin-eq

SOTF EQ plugin with parametric biquad, warped biquad, and Kautz modal filters.

Parametric equalizer supporting multiple filter types (peak, shelf, highpass, lowpass, etc.) with optional auto-gain normalization. Standard filters use cascaded biquads; advanced Roomeq paths can also load per-band `topology: "warped_biquad"` and `topology: "kautz_filter"` entries.

## Finite streams

When every channel's biquad, SVF and advanced filter bank is empty, `drain`
preserves the internal 2x/4x oversampler's retained audio. It emits a conservative
1024-frame zero continuation, including buffered delay and FFT overlap. Work is
limited to one 256-frame refill per call; a prepared output cache accepts any
positive whole-frame destination capacity without changing AutoGain timing.
Only the emitted output prefix is written. The equivalent 1x state has no tail.

After a nonempty finite stream accepts EOS, reset or initialize before new input
or changed controls. Unchanged primitive controls remain accepted. Empty streams
complete without freezing. Processing callbacks through the oversampled route
remain limited to the prepared 4096-frame maximum.

Nonempty recursive filter banks retain the existing unsupported native drain
behavior: immediate completion and unknown tail metadata. Zero gain or momentary
silence does not establish finite support. Timed offline rendering can continue
such filters with explicit zero input until its requested endpoint.

## AutoGain measurement timing

AutoGain ingests every native-rate input and uncompensated output frame, even
while compensation is disabled. It refreshes paired loudness/peak measurements
every `max(sample_rate / 10, 1)` frames (10 Hz at standard rates). The target
computed at a boundary starts affecting the following frame; later callback
samples cannot affect earlier compensation. Published peaks cover the entire
measurement interval. Retaining all telemetry snapshots can delay publication;
the audio clock and gain processing continue.

At 2x/4x, a prepared reference-only ring delays input measurements by the actual
oversampler latency. The output and declared audio latency are unchanged. Raw
oversampling still receives the original callback; native callbacks use prepared
4096-frame scratch spans. Transition storage retires at the original callback
boundary, while completed ramps select the exact target coefficients immediately.
Native 1x peaking-band automation is bit-identical across tested callback partitions
for legacy and ordered DF-I/TDF-II routes. Oversampled transition-clock
reconciliation remains separate validation work.

Successful initialization and oversampler reconstruction start a fresh paired
meter/reference epoch and clear meter diagnostics while retaining the current
and target gain trajectory. Existing post-EOS initialization performs a full
reset. Ordinary reset clears the new reference/clock state with the existing
filter and gain state; as before, its published snapshot updates at the next
measurement boundary. Reference storage is `4096 * channels` samples plus
`latency * channels` samples for the oversampled route. Processing, canonical
finite-tail generation, publication, and reset reuse prepared storage.

The audit transition regression uses an independent RBJ coefficient and filter-state
reference across 144 combinations of 44.1/48/96/192 kHz, 1/2/6 channels, orders
2/4/8, DF-I/TDF-II and legacy/ordered routing. Interrupted frequency, Q and gain
ramps cover Q 0.1–20 and gain -18 to +18 dB, with stable interpolated poles, finite
output and zero measured setter/callback heap operations. This does not establish
all advanced realizations, oversampled automation or host sample-rate changes.
