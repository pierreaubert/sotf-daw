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
sizes, not aggregate RSS or a CPU deadline claim. Optimized/advanced-family extremes, mixed realizations and oversampled
host rate changes remain audit work.


Exact serialized `Q = 0.1` is accepted at the documented minimum. Configuration,
filter-bank replacement and type-change clamping use the authoritative f64
minimum from `params.rs`; only f32 parameter descriptors narrow it. The immediately
lower representable f64 Q remains rejected.

Standard 1x extremes are checked against independent prewarped analog-polynomial
references for peak, low/high shelf, low/high pass, notch, bandpass and allpass:
44.1/48/96/192 kHz, 20/1000/20000 Hz, Q endpoints 0.1 and 20 (40 for notch),
gain -24/+24 dB, orders 2/4/6/8, both direct forms and legacy/ordered Stereo
routes. All 6,144 unit-impulse prefixes (4096 frames each) meet the fixed 2e-5
absolute bound; measured maximum error is 7.46e-9. Direct checks certify all
3,840 realized section denominators as strictly stable. Standard shelves keep
their fixed-slope convention; their Q metadata does not change shelf slope.

A further 384 retained-node `DawHost` cases change EQ input from twice the host
rate to the host rate at 44.1/48/96 kHz. A synthetic upstream node duplicates
frames and declares the doubled rate, then is removed before a control-thread
rebuild. Independent populated-state references verify legacy history retention
and ordered-route reset at the new rate. Compiled-linear and graph paths are
asserted explicitly. Maximum error is 2.33e-10; measured post-build callbacks are
heap-free. Recursive tail metadata stays `Unknown`, while the existing empty-bank
finite-stream policy remains covered separately. The fixture validates rate
metadata and history handling; it is not a physical resampler.

These matrices cover standard 1x filters and stated endpoints. Optimized filter
families, advanced extremes, mixed-realization trajectories, oversampled host
rate changes and broader tail-policy review remain separate audit requirements.


Global SVF processing is verified against independent analog-polynomial models
in 3,168 endpoint cases, including eight families and the three optimized-name
aliases that map to SVF. The matrix covers 44.1/48/96/192 kHz, frequency/Q/gain
endpoints, both routing modes and saved oversampling selections 1/2/4. SVF runs at
the base rate with zero transport latency for every selection. Maximum impulse
error is 3.73e-9 at the fixed 2e-5 bound. SVF shelves preserve user Q; raw SVF
bandpass has center gain Q.

The shared `math-audio` shelf correction is commit `434dadf`, and its existing
deterministic differentiable wrapper is aligned in `bceee04`. Low shelf uses
`g / sqrt(A)` with `m1 = k*(A-1)`; high shelf uses `g*sqrt(A)` with
`m1 = k*A*(1-A)`. Analytic response now follows `input = HP + k*BP + LP`.
This intentionally changes previously incorrect shelf audio and response plots;
parameter updates still preserve integrator history. The dependency's own
independent analog tests cover f64/f32 impulses and complex response.

A further 3,168 retained-SVF `DawHost` cases verify rate rebuilds at base
44.1/48/96 kHz after removing the synthetic doubled-rate upstream node. Both
routes reset SVF integrators, retain the selected factor, keep zero latency and
Unknown recursive tails, and use explicitly checked compiled-linear/graph paths.
Maximum new/old-rate reference error is 9.32e-10. Tested processing callbacks stay
heap-free. Native optimized biquad designs and advanced/mixed realizations retain
their separate audit requirements.
