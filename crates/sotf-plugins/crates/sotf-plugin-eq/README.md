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
sizes, not aggregate RSS or a CPU deadline claim. Additional optimized/advanced,
mixed-realization and host-rate validation is described below.


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

These matrices cover standard 1x filters and stated endpoints. Advanced/mixed,
actual oversampled-host and tail-policy checks have their own evidence below.


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
heap-free. Advanced/mixed realizations retain separate audit requirements.


Native optimized biquads use shared `math-audio` commit `63e8495`. PeakMatched
boosts keep their matched pole radius/angle and satisfy unity DC/Nyquist plus the
requested center gain. Cuts invert the corresponding minimum-phase boost. The
optimized shelves preserve their fixed-slope poles and endpoint/geometric-midpoint
constraints, using a cancellation-resistant positive numerator split. They now
select minimum phase for both gain signs. Negative-gain audio intentionally
changes: deep matched cuts reach the requested gain, and tiny optimized cuts
approach identity without a phase jump. Filter names and preset fields remain.

The independent native matrix covers 2,304 impulse prefixes at the same fixed
2e-5 bound (maximum error 2.99e-8), four rates, three frequencies, Q/gain endpoints,
orders 2/4/6/8, DF-I/TDF-II and both routes. Optimized shelves use independent
analog-polynomial references; matched peaks use endpoint constraints and complex
center-response spectral factoring. All 1,440 actual f64 sections have stable
poles and minimum-phase zeros, including long-tail sections beyond the 4,096-frame
prefix. Tested processing callbacks have zero measured heap activity. Shared f64
response tests verify center/endpoints, reciprocal phase and zero-gain continuity;
this does not establish an analog bandwidth match or CPU deadline. Mixed native
peaking trajectories and actual oversampled host rates are covered below.


Independent advanced validation covers 576 Kautz endpoint cases and 864 retained
host rate cases. The reference derives pole/basis/allpass coefficients and uses
its own DF-I recurrence with signed linear weights plus dry. It covers omitted
legacy sections, explicit/coincident/close modes, both routes and compiled/graph
host rebuilds. Maximum errors are 7.46e-9 and 1.82e-12, under the fixed 2e-5 bound.
Both routes rebuild modal history on rate changes; latency remains zero, tail
Unknown, and measured post-build callbacks heap-free. The host rate fixture
changes declared rate with frame duplication; it is not a physical resampler.

The Warped endpoint defect recorded in `eq-advanced-extremes.json` is corrected
by shared math-audio through commit `d515d16` (core `2e5c105`, fallback
compatibility `45438d7`, identical-retune guard `d515d16`). Processing uses an equivalent
normalized physical-coordinate realization instead of a nearly cancelling
prototype/implicit solve. All four input/output allpass histories remain populated
and map to the new coordinates during retunes; older serialized states restore
from those histories. Matched peaks transform boost poles before minimum-phase
spectral factoring, invert for cuts, and retain unity endpoints with a separate
dry-plus-residual form. The existing Biquad corner/rate/gain fallback domain is
preserved. Preset names and the accepted lambda range remain unchanged.

All 4,608 standard Warped endpoint cases now pass (maximum 9.32e-10), and the gate
runs in the ordinary suite. Another 1,728 optimized-name Warped cases match
independent analog or complex-pole/spectral-factor references (maximum 4.77e-7).
The fixed bound remains 2e-5. Both matrices include lambda -0.9999/-0.37/0/0.37/
0.9999/automatic and both routes. Matched transfer tests also compare 288 shared
f64 configurations with independent factored-prototype complex responses.

A further 2,304 retained `DawHost` rate changes cover the eight standard Warped
families at Q/gain/frequency endpoints and lambda -0.37/0.37/automatic. Independent
implicit state references verify populated legacy histories and ordered reset,
including recomputed automatic lambda, compiled/graph paths, zero latency,
Unknown tail and heap-free measured post-build callbacks. Maximum error is
9.32e-10. The rate fixture duplicates frames rather than physically resampling.

The original failing artifacts are retained. Current evidence and exact commands
are in `audit/artifacts/plugin-audit-20261018/eq-warped-conditioning.json`. These
f64 matrices cover the stated endpoints and native 1x host cases. Additional
mixed and actual oversampled-host checks are described below; arbitrary
trajectories, broad f32 accuracy and CPU deadlines are not established.


Mixed realization validation in `tests/audit_extremes/mixed.rs` combines the
independent native, Warped and Kautz models. It covers 960 static and 768
interrupted native-band trajectory cases at four rates, stereo/five-channel
layouts, six band permutations and legacy/Stereo/L/R/M/S profiles. The legacy
reference preserves native-first grouping and combined advanced-bank rounding;
ordered references preserve stored order and per-band rounding. Both callback
plans are bit-identical, with maximum reference error 9.32e-10 at the fixed 2e-5
bound. A noncommuting placement witness distinguishes order by 0.0003606.

A reproduced per-channel defect dropped Warped/Kautz banks when switching to
SVF: runtime banks existed, but structural rebuilding had no retained channel
configurations. Descriptions now retain shared or per-channel ownership, and
rebuilding selects each channel's descriptions. All 192 per-channel cases and
144 populated return switches pass; distinct parameters, unequal bank shapes,
empty/advanced-only channels and saved SVF factors are preserved. Public preset
fields and controls are unchanged.

Another 24 advanced-only topology/drain cases retain zero latency and Unknown
recursive tails. Legacy drain completion leaves its buffer untouched, and
subsequent audio matches a populated recursive reference with a nonzero tail.
That compatibility completion does not establish finite recursive support.
Measured native scalar setters (with retained host-owned parameter IDs), process
and drain calls have zero heap activity. Structural bank rebuilds remain control
operations. This standalone mixed matrix covers base-rate processing; see
`audit/artifacts/plugin-audit-20261018/eq-mixed-realizations.json` and the actual
oversampled-host coverage below.


Actual internal 2x/4x processing is validated through retained `DawHost` rate
rebuilds in `tests/audit_extremes/oversampled_host.rs`: 1,056 native-family,
576 mixed-placement and 144 heterogeneous per-channel cases. The reference
shares the existing FIR transport while independently deriving EQ coefficients,
histories, routing and rate epochs. Legacy advanced filters stay at base rate;
ordered advanced filters run at the inner rate. Partial old/new chunks,
retained legacy histories, reset ordered/modal histories, saved factor and
latency are checked. Maximum error is 7.16e-7 at the fixed 2e-5 bound. Nonzero
latency is an explicit host compilation barrier: both compile-enable settings
use the verified graph fallback. The upstream rate fixture duplicates frames;
the EQ itself performs real FIR up/downsampling.

Empty banks now publish geometry-only state-independent support: zero for
active 1x/SVF, four transport chunks for prepared oversampling. Process output
has an exact identity frame envelope; drain output has the fixed geometry-only
canonical refill bound. Recursive native/SVF/advanced banks keep Unknown tail
and unproven support, including zero-gain configurations. These declarations
let the host compose finite empty-bank support without using live drain progress
as an all-state proof.

In 216 finite host EOF cases, drain matches explicit zero-input FIR continuation
exactly, including the final marker, completion and reset replay. Actual emitted
frames stay within the composed host support. In the 1,776 recursive cases,
legacy completion does not discard queued transport or recursive history;
subsequent input matches the independent continuation and support stays unknown.
Measured process/drain calls perform no heap operations. Evidence and the
requirement-by-requirement EQ row audit are in `eq-oversampled-host.json` and
`eq-completion-audit.json` under `audit/artifacts/plugin-audit-20261018`. These
are bounded f64 integration checks, not an independent resampler-design or
CPU/f32-wide conformance claim.
