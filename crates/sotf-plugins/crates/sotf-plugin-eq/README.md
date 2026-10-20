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
oversampling uses bounded input-chunk spans; native callbacks use prepared
4096-frame scratch spans. Transition storage retires at the original callback
boundary, while completed ramps select the exact target coefficients immediately.
Native 1x peaking-band automation is bit-identical across tested callback partitions
for legacy and ordered DF-I/TDF-II routes. Oversampled biquad targets are retained
on the source timeline and delayed by the upsampler group delay before use.

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
all advanced realizations or host sample-rate changes.

At 2x/4x, each band retains exact coefficient targets with source-frame timestamps.
Repeated edits at the same source instant coalesce to the final target. A separate
internal transition consumes targets after the upsampler delay, preserving earlier
controls for buffered audio. Source-facing five-millisecond transition retirement
is independent of this delayed processing state. Scalar frequency, Q, gain and
filter-type edits reuse prepared storage. Reset clears queued controls and clocks
in place; structural reconstruction discards pending controls with its existing
filter transition boundary. Declared latency, presets and the native 1x path are
unchanged. Automated oversampled audio intentionally changes to follow source time.

Run the explicit gate with
`cargo run -p sotf-plugin-eq --example qa_eq_transitions --release --features qa`.
All 576 cold/warm 2x/4x cases now have bit-identical output across callback
partitions; 288 static controls remain identical. An independent control-timing,
RBJ and recursive-state reference checks another 288 cases, sharing only the
transport FIRs (maximum waveform error 1.87e-9). Dense edits exercise ring wrap,
same-frame coalescing and all 20 bands. Tested scalar setters, process calls and
reset have zero measured heap activity. Historical failures remain in
`audit/artifacts/plugin-audit-20261018/eq-oversampled-transitions.json`; final
checks are in `eq-oversampled-scheduling.json` in that directory.

History capacity is one transport input chunk plus the upsampler delay in source
frames, plus one boundary instant: currently 385 instants per band at 2x and 4x.
Exact coefficient payload storage is `385 * total channel-stage count * 40` bytes,
plus timestamps, indices and active endpoints. For 20 stereo second-order bands
this is 616,000 payload bytes; the tested 20-band/32-channel/eighth-order case uses
39,424,000 bytes. Native 1x allocates no history. These are prepared owned payload
sizes, not aggregate RSS or a CPU deadline claim. Broader filter-family extremes,
advanced mixed realizations and host rate changes remain audit work.
