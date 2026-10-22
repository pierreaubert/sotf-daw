# sotf-plugin-convolution

SOTF realtime impulse-response convolution with uniform and non-uniform partitioned FFT backends.

Normal UPC/NUPC operation reports and preserves 1024 samples of latency, including while no IR is
loaded or a replacement is pending. The optional NUPC time-domain head reports zero latency.

## True-stereo routing

Enable the structural `true_stereo` option for a stereo input/output plugin and a four-channel IR.
IR channels must be ordered **LL, LR, RL, RR**, with the first letter identifying the input:
`wet_left = left * LL + right * RL`, `wet_right = left * LR + right * RR`, where `*` is convolution.
UPC, NUPC, and the NUPC direct head support this routing. Mix, gain, latency, and drain behavior
follow the same contracts as ordinary convolution.

The option defaults to `false`. Existing four-channel IR presets retain their original mapping:
left uses IR channel 0, right uses channel 1, and channels 2 and 3 are unused. True-stereo mode
requires exactly two plugin channels and exactly four loaded IR channels. It can also be selected
before loading an IR; the plugin then uses its normal latency-aligned dry path.

Factory JSON and engine presets persist `true_stereo`. Direct Rust callers can use
`from_params_with_routing(channels, sample_rate, params, true_stereo)`; the existing params struct
and `from_params` constructor retain their default routing. Changing the mode requires a
control-thread rebuild. The realtime scalar setter rejects mode changes.

See [usage examples](USAGE.md#true-stereo) for the channel map and configuration.

## Lifecycle

`tail_length()` reports a bound in output-rate frames, including that latency once. After a
history reset, the bound is `latency + longest_resampled_IR_length - 1`; with no IR it is the dry
delay alone. The getter uses cached scalar metadata and does not clone the active IR pointer.
While an asynchronous replacement is pending, it reports `Unknown`. Accepted replacements and
clears retain the prior bound and add the 128-frame held-output fade horizon, so a shorter IR
cannot prematurely stop a host that is still counting silence from earlier input. This can
over-report the tail after several live replacements; `reset()` clears that retained history.
`initialize()` ordinarily preserves DSP history; after accepted EOS it clears history and rearms
input together. A rate-change reload remains unknown until its prepared state is accepted.

IR decoding, resampling, FFT planning, and backend construction run off the audio thread. Async
requests are generation-tagged; failed or stale replacements leave the last-known-good IR active.
Old large states are reclaimed in the background. Use `load_status()` to distinguish idle, loading,
ready, and failed states.

The shared asynchronous loader permits at most four active workers and sixteen
queued, unprepared requests. IR selection and rate-change submission run on the
control thread. A full queue returns a retryable error immediately; an unavailable
loader also returns an error. Both refusals preserve the preceding request,
mailbox, generation, load status and live IR. Rejected rate initialization keeps
the preceding prepared clock and smoother timing. Callers must retry a refused
selection; it has not replaced the prior pending selection.

A later accepted selection, clear, successful synchronous `load_ir`, or plugin
destruction cancels earlier work. Workers skip cancelled queued requests before
file decoding or FFT planning. Preparation already underway may finish, then its
cancelled result is destroyed on that worker. The bounded queue can remain full
until a worker becomes available to remove cancelled requests. This bounds the
request backlog, not aggregate memory across arbitrary plugin instances; active
and prepared IRs still use the existing per-IR resource checks. Cancellation
never drops worker tokens or completion storage in process/drain/reset.

WAV, FLAC, and AIFF IRs are supported. The loader requires valid sample-rate metadata and enforces
32 channels, 30 seconds, and a 512 MiB estimated realtime-backend budget.

Resource admission follows the partition layout for both ordinary and true-stereo
routes. UPC counts every retained IR spectrum and each input channel's FDL. NUPC
counts shared IR spectra/plans once per used IR channel, then the history, delay,
head and FFT workspace for every mapped output. Fixed plugin buffers and the
existing planner/metadata allowances are included. Some large multichannel IRs
that passed the former sample-count estimate are now correctly refused before
FFT backend construction. The 512 MiB limit is unchanged.

This estimates persistent backend payload; it is not a peak RSS or aggregate
process-memory limit. Decoded/resampled copies, temporary preparation storage,
active/retired replacement overlap and other plugin instances require additional
memory. File metadata is checked before decoder construction: valid source rate,
at most 32 channels, at most 30 seconds, and at most 512 MiB of owned decoded-f32
sample-vector capacities. Known oversized frame counts are refused immediately.
Every decoded packet is checked before appending, so unknown or understated
length metadata cannot bypass the bound. Geometric reservations stop at the cap;
oversized input returns an error rather than a truncated impulse response.
Decoder/probe scratch and encoded packets remain outside this sample-buffer cap.
Backend admission follows bounded decoding and does not prove that a configuration
can meet the audio callback deadline.

## End of stream

Native `drain` emits zero-input continuation for the active UPC, NUPC, direct-head,
or inactive dry-delay path. The bound is the greater of `latency + IR length - 1`
and the remaining held-output replacement fade. Already-retired IRs do not add
another response; the native tail metadata can conservatively advertise more
history than this frozen stream actually retains. Latency is counted once.

Each call writes at most 1024 output frames and accepts any positive whole-frame
capacity. Only the returned prefix is written. Invalid rate, partial-frame or
insufficient-capacity calls leave the stream and destination unchanged. An
empty stream completes without entering EOS. The first valid drain freezes
control changes and new nonempty input until reset or successful initialize;
unchanged parameter writes remain accepted.

An asynchronous result, including one already ready when drain begins, remains
in its mailbox while the active response finishes. Workers cannot mutate active
DSP state. Reset permits ordinary processing to adopt the completion again.
Pending load metadata remains `Unknown` throughout this frozen drain.

The active state is an audio-owned Arc, so cold callbacks do not allocate an
ArcSwap reader slot. Replaced states move into existing background retirement
queues. Both consumed completion endpoints remain owned through process, drain,
and reset: releasing the receiver alone could free its message storage in the
callback. A subsequent control-thread load/clear or teardown releases them.

The engine's separate 4096-call drain limit can still reject long high-rate IRs
or multiple serial tails. This plugin makes bounded finite progress and does
not truncate its response to fit that policy.

## Admission-boundary load diagnostic

Run the explicit local release measurement with:

```bash
cargo run -p sotf-plugin-convolution --release --features qa --bin qa-convolution-load
```

The 63 geometry cases cover 48/96/192 kHz, 2/12/32 output channels with mono
and full-channel IRs, plus two-output/four-path true stereo. Each runs UPC,
NUPC, and a 257-tap direct head. It finds the longest equal-channel source IR
allowed by duration, decoded-buffer and backend admission, confirms the last
accepted and next refused length using real file preflight, then constructs
one candidate at a time. Discrete 32-channel fixtures use AIFF.

Complete host-adapter callbacks first populate the full IR history at 128
frames. Steady measurements use 128 and 512 frames for at least four largest
partition periods and 256 callbacks. Every phase reports raw physical and
reported-horizon misses, p50/p95/p99/max, finite output, allocations and frees.
There is no timing multiplier. Any miss, boundary mismatch or functional
failure makes the diagnostic exit nonzero after retaining all case results.

Construction reports net and peak requested-allocation deltas during
`from_params_with_routing`, including temporary decoding/preparation. These
process-wide allocator deltas include background bookkeeping and are not RSS
or allocator usable-size bounds. Adapter preparation is outside that measured
construction interval. No device/native scheduling compliance is inferred.
The older `qa-convolution` target keeps its timing measurements diagnostic
and now labels its functional success separately from physical deadline misses.

The recorded local run in
[the load artifact](../../../../audit/artifacts/plugin-audit-20261018/convolution-maximum-load.json)
exits 1: 61/63 cases miss deadlines, with 51,122 misses across 532,086 callbacks.
All finite-output, callback heap and retained-estimate checks pass. The highest
construction requested-allocation delta is about 551 MiB, including temporary
preparation beyond the persistent payload estimate. These results establish a
measured capacity limitation; they do not justify enlarging deadlines or claiming
native compliance. Scheduling mitigation remains open; the worker/retirement
overlap validation below covers ownership with shorter IRs.

## Worker and retirement overlap regression

`cargo test -p sotf-plugin-convolution sustained_mixed_instance_switching`
runs 18 instances through a shared production worker pool and two shared
retirement queues. It covers ordinary and true-stereo UPC/NUPC/direct-head
routes, 2/12 outputs and 48/96 kHz, using real 4097-frame files. The test sends
9,216 burst selections while retirement is blocked, checks transactional queue
refusals and fenced live roots, then releases a separate reclaimer thread.
Every instance adopts an actual current failed decode without replacing its
live IR, and finally converges to a valid selected IR. Reset streams and complete
tails match fresh instances exactly, with zero callback allocations or frees.

The pool uses a dedicated instance of the production factory to isolate queue
saturation from other unit tests. Worker and reclaimer handles are joined before
fixture cleanup. This validates ownership and recovery; it does not establish
full-size simultaneous RSS or deadline compliance. Debug/release outcomes are
recorded in
[the overlap artifact](../../../../audit/artifacts/plugin-audit-20261018/convolution-worker-overlap.json).

### Real-input NUPC transforms

NUPC uses real forward/inverse transforms for each `N=2B` partition level and
stores only the `N/2+1` nonnegative-frequency bins in kernels, per-engine FDLs
and convolution sums. Interior bins are not doubled: these are convolution
coefficients, not a one-sided power spectrum. The inverse keeps its single
`1/N` normalization. A prepared real workspace holds the transform input/output;
FFT scratch is sized for the larger of forward and inverse requirements.

Partition plans, absolute offsets, direct-head taps, latency, output delays,
channel mapping and callback-thread execution remain unchanged. Floating-point
roundoff may change because the transform algorithm changes. Independent f64
convolution and existing host/EOF tests establish waveform accuracy; memory and
speed reductions do not by themselves establish callback deadlines.

The 512 MiB admission estimate deliberately retains its conservative full-complex
storage/plan allowance. The saved complex-buffer allowance also covers the new
real workspace, and tested materialized backends remain below that estimate.
Consequently the admitted source lengths and full load diagnostic matrix do not
change with this implementation. The estimate is an upper bound, not a statement
of actual half-spectrum storage or total process RSS.

Before level coalescing, the retained strict 63-geometry load run failed: 62 geometries and 48,377
physical deadline misses over 532,086 complete callbacks, versus the historical
61 geometries / 51,122 misses. Of 42 NUPC geometries, 39 record fewer misses and three the same count; the
additional failed geometry is an unchanged UPC route. The observations are not
an interleaved or controlled timing comparison. Every new case is finite and
has zero callback allocations/frees, unchanged admission boundary/latency and
retained requested bytes below the conservative estimate. NUPC retained backend
allocation is about 62.25–64.29% of the historical value at the same geometry;
these counters are not RSS or an aggregate process-memory bound. Large inline
FFT bursts and full maximum-load deadline compliance remain open.

### Coalesced equal-size NUPC levels

Preparation combines adjacent equal-block partition groups with contiguous IR
coverage into one runtime level. Both groups originally transformed identical
source blocks. Keeping the first offset and concatenating their IR partitions
lets older FDL spectra express the second group's delay, with one forward FFT,
one inverse FFT and one overlap-add accumulator per block size. Total partition
coverage and tap positions stay the same. Direct-head taps, public latency,
callback-thread execution and finite tail policy are unchanged.

The public partition planner and conservative admission estimate still describe
the original groups, so admitted workloads do not expand. Numerical comparisons
use the explicitly unmerged construction and independent f64 convolution; bit
identity with the earlier accumulation order is not required. Each current
renderer still has exact reset and callback-partition invariance. This reduces
redundant transforms and per-sample bookkeeping, but does not distribute a large
remaining FFT across callbacks or establish maximum-load deadline compliance.

With coalesced levels, the complete unchanged load matrix still fails in 59/63
geometries, with 45,578 physical misses over 532,086 callbacks. The preceding
real-FFT run failed in 62 geometries with 48,377 misses. Forty NUPC geometries
record fewer misses and two the same count; none record more. Retained NUPC
requested bytes are 58.06–81.66% of the preceding values. These are unmatched
local observations, not causal timing or native-device acceptance. All cases
remain finite and within the conservative retained-storage bound, with zero
callback allocations/frees. The shared linear-phase EQ 108-row matrix also
remains red: 28 failed rows / 9,237 misses. Both complete datasets and independent
consistency verifiers are retained under `audit/artifacts/plugin-audit-20261018/`.

### Direct-head accumulation

The zero-latency head retains its original causal taps and one circular history.
Each sample splits that history into two contiguous slices and computes reversed
four-lane dot products, then wraps the write position once. This removes per-tap
ring branches without adding persistent storage, latency or dependencies. The
portable Rust implementation permits compiler SIMD; final linked ARM64 release
profiling code contains packed multiplications. Accumulation order can change,
so accuracy is checked against f64 convolution across short, odd and lane-boundary
lengths, all history positions, reset and complete tails.

An explicit ignored-by-default diagnostic can be run with
`cargo test --release -p plugins-spatial direct_head_paired_callback_cost_abba -- --ignored --nocapture --test-threads=1`.
It compares the original scalar loop and current head in ABBA order using
preallocated, explicitly touched output buffers, populated history and 32-channel
128-frame callbacks. The captured 257/512-tap median p50 ratios are about
0.3045/0.3027 against the original loop, satisfying the predeclared 0.9 criterion.
These isolated wall-clock measurements do not include the rest of the plugin or
a device callback. The regular test suite skips the diagnostic; it is executed
separately for this optimization, while existing functional ignores are retained.

The complete unchanged direct-head load run remains red: 58/63 geometries and
43,261 physical misses over 532,086 callbacks, versus 59 / 45,578 in the prior
coalesced run. Direct-head cases total 2,379 misses versus 4,678: ten geometries
record fewer, ten equal and one more (23 to 24). These full-load comparisons are
unmatched wall-clock observations. Every case is finite, fits its conservative
retained-storage estimate and records zero callback allocations/frees. The
captured linear-phase EQ 28/108 failing-row result remains pending; its ordinary
path does not instantiate direct heads, and no new EQ load result is claimed.

### Real-input UPC transforms

Uniform convolution uses a real 2,048-point transform for each 1,024-frame
partition. Prepared IR spectra, channel FDLs and convolution sums store 1,025
nonnegative-frequency bins. A preallocated 2,048-float workspace holds transform
input/output; complex scratch covers both transform directions. DC and Nyquist
remain real, interior coefficients are not doubled, and overlap-add retains one
`1/2048` inverse normalization.

This preserves ordinary/cyclic and true-stereo routing, 1,024-frame latency,
callback partitioning, complete native tails, mix/gain timing and asynchronous
state ownership. Independent f64 DFT checks validate file-prepared spectra;
the existing direct-convolution, reset, routing, drain and callback heap tests
cover streaming behavior. Floating-point roundoff may differ.

The conservative full-complex admission arithmetic is deliberately retained.
Its spare complex-buffer allowance covers the real workspace, and real FFT
plans fit the existing planner/scratch reserve. Admitted source lengths and
all 63 load geometries stay unchanged. Smaller spectra reduce persistent
storage and multiply-accumulate work but do not distribute partition-boundary
bursts across callbacks or establish deadline compliance.

The complete UPC real-FFT load run still fails in 58/63 geometries, with 43,053
physical misses over the same 532,086 callbacks, versus 58 / 43,261 in the
preceding direct-head run. UPC misses total 40,164 versus 40,357, while unchanged
NUPC paths total 2,889 versus 2,904. Retained UPC requested bytes are about
50.09–50.17% of preceding values; these counters are not RSS. All cases remain
finite, heap-free in callbacks and within the unchanged retained estimate.
Historical timings are unmatched; maximum-load scheduling remains open. Full
raw results and their consistency verifier are retained in
`audit/artifacts/plugin-audit-20261018/upc-real-fft-*` and
`verify-upc-real-fft.py`. Linear-phase EQ has no new load acceptance result.
