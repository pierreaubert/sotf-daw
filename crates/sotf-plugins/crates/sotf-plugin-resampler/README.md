# SOTF Resampler

Allocation-free streaming sample-rate conversion using rubato's asynchronous sinc resampler.

The plugin accepts interleaved audio and retains partial input until `chunk_size` frames are
available. The preallocated residual planes feed rubato directly without a second planar
full-chunk copy. `output_frames_for_input()` is a conservative destination-capacity bound;
`available_output_frames()` reports what the next call can emit immediately. The returned frame
count is authoritative and may be zero or larger than the input callback. `DawHost` preserves
that count rather than padding it with input-rate silence.

A fixed input callback size cannot also be a fixed output callback size when rates differ: 256
frames at 44.1 kHz span about 279 frames at 48 kHz. A device-facing fixed-frame consumer needs a
separate output-clock FIFO/pull scheduler at the clock-domain boundary. The plugin does not
silently relabel, duplicate, or discard samples to simulate one.

At end of stream, call the object-safe `Plugin::drain()` repeatedly until `complete` is true.
Drain submits the final partial chunk and pumps zero input to the programme endpoint described
below. This preserves converted programme duration and leading delay; it does not promise every
finite sinc-ringing sample beyond that endpoint.
The host propagates each upstream tail through downstream plugins before draining
their own state. Seek, stop, or graph replacement uses `reset()` and discards pending state.

Drain validates output capacity and input-clock rate before finalizing a stream. Rejected calls
preserve pending audio and permit retry; a rejected first drain also permits ordinary input to
continue. A valid drain, including an empty-stream drain, finalizes the stream. Ratio or mode
changes and further input then require reset. Repeated completion and unchanged mode/quality
settings are idempotent. Drain requires no output capacity when already complete or empty. With tiny chunks and low
ratios, a drain step can consume padding while returning zero frames and `complete=false`; keep
calling with the advertised capacity until completion.

Equal configured rates with dynamic ratio disabled use a bit-exact, zero-latency copy path.
Non-finite samples are copied by the direct plugin API; `DawHost` rejects them at the graph
boundary. For equal rates, enabling dynamic ratio changes the audio path and declared latency.
Mode changes are accepted only before any input or after `reset()`; rejected transitions preserve
all pending audio and filter history. Fresh transitions also clear dormant ratio ramps. Configure
the mode before activation and renegotiate latency after changing it. `initialize()` validates
the clock without clearing an existing stream. The runtime mode descriptor is Structural for
equal-rate instances. With unequal rates, mode changes remain Realtime: disabling automation
ramps the same prepared backend to nominal ratio while preserving its history.

`ratio` automation updates rubato in place without allocation. `quality` is structural:
canonical indices are Fast=0, Medium=1, High=2, and
an activated plugin rejects live changes so no pending audio or filter history is dropped.

Latency is reported entirely in output-rate frames: rubato's `output_delay()` plus the input-chunk
priming duration converted conservatively to the output clock. The host rate passed to
`initialize()`, `process()`, and `drain()` must equal the configured input rate, and downstream nodes are initialized and
processed at the Resampler's declared output rate.

Fast, Medium, and High use 64-, 128-, and 256-tap Blackman-Harris-windowed sinc filters with
rubato's Linear table interpolation. Longer filters provide a narrower transition band and more
stop-band rejection at higher CPU cost.

## Measured spectral coverage and limits

`tests/spectral_accuracy.rs` generates tones in f64, measures coherent 200 ms output windows,
and compares irregular callbacks with regular 256-frame callbacks, including complete drain.
The matrix covers every preset at 44.1→48, 48→44.1, 48→96, 96→48, and 96→24 kHz.
Gain error must stay below 0.01 dB and residual energy below -90 dB relative to the input tone
in the conservative flat-band regions below. These are tested sample points, not a claim of
continuous-band certification.

| Preset | Tested flat region, as a fraction of the lower Nyquist frequency |
| --- | --- |
| Fast | 0.05–0.20 |
| Medium | 0.05–0.50 |
| High | 0.05–0.75 |

The suite also requires at least 60 dB attenuation halfway between input and output Nyquist
when downsampling. It reports additional points through the transition band. The fixed input
filter length means rejection close to output Nyquist worsens at large decimation ratios:
at 96→24 kHz, a 12.36 kHz tone measures about -19.62/-22.09/-27.84 dB for Fast/Medium/High.
Those transition-band measurements are not equivalent to the deeper stopband guarantee.

### Filter-response definitions and per-preset quantification

Transition width is the span between the −3 dB and −60 dB crossings of the
analytic windowed-sinc magnitude curve, as a fraction of output Nyquist.
Passband ripple is the peak absolute deviation over the tested flat band.
Stopband quality is the attenuation at 1.5× output Nyquist (deep stop);
near-Nyquist transition points are reported separately and never equated
with the preset label. `tests/filter_response.rs` computes these from an
independent f64 analytic DTFT per preset at ratios 1.0, 0.5, and 0.25
(`RESAMPLER-FILTER` lines), asserts ripple below 0.01 dB and width
narrowing Fast → Medium → High, checks measured-vs-analytic agreement on
an integer-step 48→24 kHz transition sweep, and cross-checks end-to-end
amplitude against an independent f64 direct-convolution reference at
44.1↔48 kHz. QA Test 6 prints per-quality CPU lines for the
ripple/rejection/CPU tradeoff alongside the accuracy tables.

Group delay in output-rate frames follows
`((sinc_len / 2 − 1 / oversampling_factor) × ratio − 1)`, floored at zero
(unity passthrough reports zero). Worked examples: Medium 44.1→48 kHz ≈
68.656 frames; High 96→48 kHz = 62.998046875 frames. The implementation
matches this equation to 1e-9 and the measured impulse peak to 0.52
frames across the rate/chunk/partition matrix.

### Dynamic ratios and prepared cutoffs

`set_ratio_relative()` multiplies the current target ratio, including successive calls; rejected
updates leave both the reported ratio and the audio unchanged. Runtime ratio changes are
allocation-free and remain allowed from half to twice the nominal ratio.

EOF follows the interpolation positions actually emitted, including inverse-ratio ramps and
ratio changes while input is buffered. Let S be accepted real-input frames and L the sinc length.
An emitted trajectory that stays at one ratio r preserves the exact existing raw frame count
`ceil(S*r) + floor(L*r/2)`. Targets overwritten before emitting output do not alter that count.
Zero-output chunks establish no emitted clock. A one-output ramp can establish the target clock
when its represented final step equals `1/r`; multi-output ramps between different ratios are
conservatively classified as variable, including changes too small to distinguish numerically.

A variable trajectory emits through the first interpolation anchor at or beyond `S - L/2 + 1`.
Anchors describe the start of the sinc support, not physical signal delay. This right-bracket
rule overshoots that boundary by less than one actual output interval. In the ideal constant
limit it gives the legacy count or one extra frame; the explicit fixed branch preserves legacy
rounding. Integer submitted-input origins include padding and remain separate from accepted
source length. Neither physical signal-delay reporting nor realtime latency/PDC is changed.

`tests/dynamic_endpoint.rs` checks 1,728 variable cases using a separate scalar clock recurrence,
180 tiny/pre-output-ramp cases, 192 very-low-rate fixed cases, and 18 buffered-target equivalences.
Explicit-zero continuation checks the retained waveform and final impulse peak. At extreme
decimation, short kernels can miss an isolated impulse entirely; those all-zero full responses
provide count evidence but no peak-retention evidence. Cold process/drain/reset tests measure
both allocations and deallocations; invalid drain retries preserve the exact audio history.

The private Rubato fork prepares same-length cutoff tables during construction. Runtime
selection keeps the filter cutoff at or below both endpoints of a ratio ramp and retains
all input history and timing. After an upward ramp, the wider prepared table becomes active.
Reset restores the original nominal table. Eight intervals per octave cover the allowed
range; an additional 0.1% downward step covers small negative clock drift. Grid selection
can reduce bandwidth by up to 8.3%. High quality needs at most approximately 4.5 MiB of
coefficients, independent of channel count; a nominal unity instance uses about 2.5 MiB.

The enabled `measure_large_dynamic_downsampling_alias` regression requires 60 dB rejection
of an 18 kHz tone after a nominal 48→48 kHz High instance changes to ratio 0.5. It measures
about -136 dB in the current test. This deep-stop result does not certify attenuation close
to output Nyquist.

### Cutoff smoothing

Abrupt table widening can create spectral transients; timing continuity is
always preserved. The opt-in `cutoff_smoothing` parameter (bool, default
`false`, Realtime) slews upward widening at most one prepared table per
selection call — at most one grid interval, about 8.3% bandwidth, per
call — while downward narrowing still jumps to the safe narrow table
immediately, so no transient alias burst is exposed. Selection runs at
control time and once per backend chunk (ramped upward advances only per
chunk), hence the slew is deterministic in stream position and
independent of host callback partitioning. Selection never changes the emitted clock, frame
counts, latency, or drain completion: smoothed and instant streams have
identical lengths, bit-exact pre-transition blocks, bit-exact downward
transitions, and bit-exact reconvergence once the slew completes.
Smoothing changes are rejected after finalization (reset first); `reset()`
tracks the backend's return to the nominal table and keeps the flag.
Default-off preserves legacy audio bit-exactly. See
`tests/smooth_cutoff.rs` and QA Test 7 (unified 9..13 window).

Audio-thread automation should use `try_set_ratio`,
`try_set_ratio_relative`, and `try_set_cutoff_smoothing`, which return the
`Copy` typed refusal `ResamplerControlError` without allocating on either
path; the `String` API delegates with identical messages. Slew artifact
bounds (analytic HF derivation, 0.1 dB LF, −50 dB coherent residual via
the true 2×2 least-squares fit), floor-aware full-trajectory checks
(linear floor from the 2e-6 alias bound), nominal-0.5/2.0 bank units,
nominal-not-1, instant-upward, Fast/Medium downward, 8 ch / 96 kHz,
preemption, drain-mid-slew, per-block-count, latency, cumulative-clock,
unchanged-ratio control, and injected-spur regression coverage live in
`tests/slew_artifact_bounds.rs`, `tests/slew_coverage.rs`, and
`tests/typed_control.rs`; the asserted 96→24 near-cutoff sweep lives in
`tests/hard_decimation.rs`.

The fork also corrects inverse-ratio ramp frame sizing and deferred-input history bounds.
Its 656 upstream tests and eight additional strict regressions pass across both fixed modes,
extreme ratios and interpolation variants. See
[`SOTF_FORK.md`](../../../../../sotf-3rdparties/rubato/SOTF_FORK.md) for derivations and scope.

### Parameter and adapter integration

Parameters in order: `quality` (choice 0–2, Structural), `dynamic_ratio`
(bool, Structural for equal-rate instances, Realtime otherwise), `ratio`
(float, Realtime), `cutoff_smoothing` (bool, default false, Realtime).
Old identifiers, order, and defaults are preserved; `cutoff_smoothing` is
appended. Runtime values are live through getter/setter; `parameters()` is
a construction-time snapshot (rebuilding it allocates, so the audio path
never does). The static `params.rs` schema mirrors the same four entries
for generic UI/catalog consumers. The facade factory creates default
instances (Medium, non-dynamic, smoothing off); an optional
`cutoff_smoothing` factory field is proposed to the shared owner (see the
owned audit patches). The fixed-rate FFI rejects the resampler by design
(frame counts differ), and no native wrapper exists for the same reason;
both are documented limitations, not silent omissions. Mixed-rate branch
integration (AUD087) stays with the shared clock/host owner.

Run `cargo test -p sotf-plugin-resampler` and
`cargo run -p sotf-plugin-resampler --features qa --bin qa-resampler`.
