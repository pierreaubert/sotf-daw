# AUD010 phase 2: contributing-gain telemetry and EOS/metadata audit

2026-09-28. Approved design addendum, now implemented; see the
[implementation report](../limiter-oversampling-implementation.md). The original
read-only proposal below supersedes the rejected energy-ratio telemetry design.
Native phase 1 behavior remains preserved.

## 1. Public meaning

Keep `gain_reduction_db` as a **conservative indication of contributing limiter
gain reduction**, and `is_limiting` as whether that indication exceeds 0.01 dB.
Neither is computed from dry/output energy. A neutral resampling filter or
destructive wet/dry phase combination must produce exactly 0 dB and false when
both limiter kernels applied unity gain.

The new meter includes both limiter stages, their audio-delay association and
current outer mix. It is not an exact ratio between final audio amplitude and
an unprocessed reference: FIR interpolation combines samples with positive and
negative coefficients, and no single multiplicative gain generally describes
that result. A minimum gain over a proven finite contributor interval is an
honest conservative control indication. The temporal overestimate is bounded
and documented below. No extra public energy diagnostic or reference DSP path
is needed. Native 1x telemetry remains unchanged.

## 2. Record applied gains, not independent envelope maxima

Private stage processing observes existing arithmetic without changing it. For
a finite nonzero pre-gain audio sample `x` and its finite limited output `y`,
record `g = clamp(abs(f64(y)/f64(x)),0,1)`. This includes the gain computer and
its final sample clamp. For `x == 0`, record 1: a decaying release envelope acting
on exact silence must not manufacture a new audio contributor. Convert to dB
only at publication; retain scalar linear gains internally. Native process has
no observer and keeps its exact existing path.

- High-rate wet core has mix 1 and ISP correction off. Observe the `delayed`
  sample and `wet` sample at `native_kernel.rs:580-586`. Its gain label belongs
  to the **core's emitted high-rate frame**, after its own lookahead. Do not add
  the wet core's lookahead or upsampling delay to that label again.
- Final guard observes both its first limiter output and its ISP correction.
  The same channel's first-stage gain must be carried through the actual ISP
  audio delay before combining with its output-stage gain. Summing current
  stage envelope maxima would be temporally wrong.
- Per-channel association is retained. Taking a core maximum from channel A
  and a guard maximum from channel B and adding them would invent a combined
  reduction that occurred in neither channel.

An instrumented observer needs only prepared scalar arrays/rings, no registry,
public cache or heap event ownership. It observes exact existing delayed/gained
samples and does not replace any signal calculation.

## 3. Source-derived downsampler support

Let `C = sotf_host::oversampling::OS_CHUNK_SIZE = 256`, factor `F` be 2 or 4,
and `U=C*F`. The production host uses registry Rubato 1.0.1, not the private
asynchronous resampler fork. Its `synchro.rs` is byte-identical to the workspace
copy inspected here; SHA-256:
`886c97c1926b908bd7608977e09c7003eaea69d440539624249dfcc2688fcfc0`.

1. `oversampling/oversampler.rs:94-101` constructs the downsampler as
   `Fft::new(F,1,U,1,channels,FixedSync::Input)`.
2. Rubato `synchro.rs:238-258` computes gcd=1, `fft_chunks=C`, input FFT unit U,
   output FFT unit C. `:438-472` consumes exactly one such input unit per call;
   `saved_frames` is zero for these exact-sized calls. There is no additional
   multi-chunk input backlog inside this configured downsampler.
3. `FftResampler::resample_unit`, `:143-190`, zero-pads the current unit, computes
   a 2C-frame result, adds only the previous C-frame overlap to its first C
   frames, and **overwrites** that overlap with the current result's second C
   frames. Thus the output of downsampled block b depends only on high-rate
   core output blocks b and b−1. It cannot depend on b−2. This is a structural
   dependency proof, not an assumption based on filter group delay or nominal
   attenuation; frequency truncation within the FFT cannot add another state
   block.
4. Host `Oversampler::new/reset` prepares exactly C startup output zeros;
   `process_chunk/process_down_chunk` append all subsequent native blocks in
   order without rewriting delivered output (`:667-758`). Therefore output
   block b is delivered at native indices `[C+b*C, C+(b+1)*C)` before the guard.

For each channel define `a_b = min(g[q])` over core output indices
`b*U <= q < (b+1)*U`, using unity for zero padding. Define `a_-1=1` and startup
output indication 1. The conservative indication for downsampled block b is
`h_b = min(a_(b-1),a_b)`. This can spread a core reduction over at most two C-frame
output blocks (512 native frames, about 10.67 ms at 48 kHz), including frames
whose actual coefficient is zero. It is explicitly not a minimum physically
audible duration or an exact gain curve.

Do not hardcode an unexplained two-chunk rule in the implementation. Name this
backend-specific dependency rule, cite these pinned source operations, and
add an independent dependency-support regression so a future host/Rubato
backend change fails this assumption visibly. The generic public tail bound
alone is not precise enough to identify contributing gain intervals.

## 4. Final guard and mix clocks

At native rate let guard lookahead be D and ISP audio delay J=3D (sample mode
has D=J=0). If `v[n]` enters the guard, its first stage emits
`z[n] = v[n-D] * p[n]`, and its ISP stage emits
`w[n] = z[n-J] * r[n]`, where p and r include their corresponding sample clamps.
For telemetry the limiting contribution at final frame n is therefore:

`g_wet[c,n] = h[c,n-(D+J)] * p[c,n-J] * r[c,n]`.

Prepare one G=D+J delay for h and one J delay for p; both start at unity.
Their advances occur with their corresponding native guard audio frames.
Current `p[n]*r[n]` is not the correct association. The source core's own delay
is already represented in h and must not be added again.

With the **actual advanced outer mix smoother** value m[n], form the conservative
indicator `g_mix = (1-m[n]) + m[n]*g_wet`, clamped to [0,1] against rounding.
Accumulate the minimum g_mix across channels/frames in each existing native
publication interval `max(1,R/10)`. Publish `-20*log10(max(g_min,1e-6))`, limiting
the display to 120 dB; exact g_min=1 publishes exact 0 dB. `is_limiting` uses
the same published value and the existing 0.01 dB rule.

This affine mix expression is a gain-control indication only: it does not
model cancellation between differently phased wet and dry audio. That is why
phase-only cancellation cannot turn the limiter indicator on. Fully dry m=0
always yields 0 dB even while hidden wet kernels remain active.

Original-input peak and final-output reconstructed ISP remain separate input/
output-clock measurements as in the broader plan. No core-stage cache is
mistaken for final-output telemetry.

## 5. Bounded metadata storage and EOS association

Keep per-channel minima keyed by sequential core-output block number. Do not
infer their index from `ProcessContext` or the most recent setter. The core
output cursor advances for all generated high-rate frames, including residual
padding, up-filter overlap and the child's finite lookahead drain. The child's
drain can return a partial U-frame block: retain its partial minimum until
later child output or final zero padding completes the downsampled unit.

The outer path submits at most the remaining C-frame input phase. Ordinary
processing can generate at most one U-frame core chunk before delivering its
native slice. During begin, at most two further U-frame chunks are generated
before any delivery. Afterward the wrapper only requests another child result
when no native result is queued; the planned private child drain maximum is
256 high-rate frames, which is <=U. Hence the generated core-output frontier
leads delivered native output by less than 2C at the largest EOS setup. Allow
another 3C for rounding a partial high-rate unit, its saved overlap and the
fixed startup shift. Eight prepared block descriptors (per-channel minima plus
indices) conservatively cover that <5C frontier/support span and the previous
contributing block. This deliberately leaves slack rather than relying on the
tighter full-unit-only bound. Use checked sizing
at setup and verify high-water occupancy for all 256 residual phases and tiny
drain capacities. No realtime descriptor growth/fallback is permitted.

Before a partial final unit is rendered by the downsampler, unfilled positions
are zero and their gain indication is unity; absence of a new gain record is
not permission to reuse a previous minimum. For the final down-filter overlap
block use the preceding real core minimum plus a unity current block. Once
the wrapper finishes, incoming h is unity while the guard's metadata delays
flush with its actual audio. Final-only and dry-only drain stages do not invent
core activity. Bound/metadata queries cannot consume descriptors or advance
any meter. Zero-frame/incomplete progress cannot advance output-clock rings or
publication time. Reset clears every pending descriptor and primes gain-delay
rings with unity, rather than zero.

## 6. EOS canonical snapshot audit

At the first **valid nonempty** begin/drain, freeze the currently accepted
canonical settings C*. Preflight rate, shape, scratch capacity and lifecycle
before this mutation. All queued real-input snapshots remain unchanged.

- If S real native frames were accepted, their existing records govern core
  frames `0..F*S`. Do not immediately set the historical core's target to C*.
- At the first synthetic frame `F*S`, after all pending real records have been
  consumed, adopt C* and hold it for subsequent padding and native child drain.
  A valid setter after the final input but before EOS therefore affects padding
  and the output-side guard, without rewriting buffered real-input history.
- The final guard and outer mix retain their current live smoother states and
  accepted targets. Freezing must not reset or re-target an identical value.
- The outer lifecycle freezes first, but the child may still receive the
  wrapper's residual/up-overlap ordinary processing before its own begin/drain.
  Distinguish 'timeline exhausted, use frozen controls' from 'audio drain
  complete, reject all new processing'.
- Empty begin remains a no-op; invalid begin/drain is retryable. Repeated valid
  begin and identical setter resends are no-ops. An internal DSP failure after
  partial work enters Failed; it must not pretend that rewinding the control
  cursor restored audio. Reset reopens the epoch with current canonical values.

Specific acceptance cases: p=0/1/255 pending phases, different last two real
snapshots, changed threshold/release/link after the final input, a zero-length
process call, wrong-rate/empty-capacity retry, explicit repeated begin, partial
child drain, and reset. Compare against an offline absolute event schedule
whose last change occurs exactly at S; inspect gain/target traces independently
of throttled public telemetry and compare final audio as well.

## 7. Pre-initialization factor metadata

Use a distinct unprepared new-path state. The configured oversampling choice
is canonical and reported correctly immediately, even though no FFT/kernel
path has yet been allocated or initialized. It must not accidentally inherit
native 1x delay or enable `PluginCompiledOp::Limiter` from the old native member.

Recommended conservative conventions for selected 2x/4x before initialization:

- `preferred_oversampling() = None` always, to prevent external double wrapping.
- `compile_metadata`: stateful nonlinear boundary, no compiled operation or
  fusion, FFT cost class. The factor controls this decision, not the presence
  of an already prepared backend.
- `latency_samples() = 0` **only as an explicitly unprepared sentinel**, not a
  zero-latency processing promise. The API has no unknown-latency variant and
  processing is rejected until successful initialization. `tail_length =
  Unknown`, `drain_call_bound = None`, structural drain capacity 256.
- After successful initialization use the actual prepared wrapper + integer
  core/final-guard delay. Stable structural drain capacity does not change.
  Existing 1x constructor-time metadata remains exactly unchanged.
- Rate/factor multiplication and channel/buffer dimensions are checked before
  constructing a new path. A failed reinitialization of an already prepared
  2x/4x instance retains its live path, clock and metadata; an unprepared failed
  initialization remains unprepared. Metadata queries never allocate.

Host `daw_host.rs:456,597` initializes nodes before final compile metadata is
used at `:842`; adapter initialization likewise initializes before preparing
its bounded processing storage. Tests must still exercise direct metadata
queries before initialize, including factor changes before first initialization,
to catch accidental fallback to a live native member. This is a new-path
convention; no native 1x contract is changed.

## 8. Independent controls that must prevent misleading telemetry

| Case | Required independent expectation |
| --- | --- |
| Neutral -60 dBFS sine at 1 kHz, 0.45R and 0.49R; hard ceiling -1 dBFS; factors 2/4 | Both stages' applied gains remain exactly unity. Public GR stays exactly zero and activity false even where independently measured filter attenuation exceeds 0.01 dB. Include startup, all residual phases, reset and EOS. |
| Low-level impulses and dense deterministic audio, mixes 0/0.5/1 and live mix ramps | No limiting indication merely from filtering, delay overlap or mix transition. All real core/guard reductions are independently absent. |
| Synthetic meter-only phase controls: dry sine, wet sine with phase pi/2 or pi, all stage gains 1, mix 0.5 | Even exact cancellation produces GR=0/inactive. This deliberately has a large/infinite energy-ratio 'attenuation' and rejects the previous flawed mapping. |
| Wet kernels strongly limiting, fully dry outer mix | GR=0/inactive; final samples match the independent delayed dry stream. |
| Synthetic contributor gain 0.5 in one core block, otherwise 1 | Only the two source-proven native contributor blocks can carry its indication, shifted by startup C and guard G. Expected max indication is 6.020599913 dB at mix1 and 2.498774732 dB at mix0.5. |
| First guard gain 0.5 at k, ISP gain 0.25 at k+J, same channel | At final k+J the composed guard indication is 18.061799740 dB, not the product of unrelated current p/r values. Repeat with gains on different channels to reject cross-channel summing. |
| Core-only, guard-only, both-stage overload bursts | Indicator sees each actual stage and composes correctly; independently reconstructed final ceiling remains the separate audio acceptance criterion. |
| Canonical chunk partition and EOS phase sweep | Exact metadata/audio partition parity against absolute offline gain/event records, including initial C zeros, partial high-rate final chunk and down-filter overlap. |

The synthetic controls establish the conservative convention independently of
production queues; the public audio controls prevent it from becoming a purely
self-confirming metadata implementation. Record cold allocation/free counts
with descriptors/rings active and repeated bound/metadata queries. None of
these new checks has been run yet; this is the plan for review before phase 2.
