# AUD-010 oversampling proposal and AUD-069 finite-stream evidence

Status: isolated numerical prototypes executed against the current compiled public limiter/host APIs. No production limiter, host, factory, or native-wrapper changes. This is a design for review, not an implemented feature.

## 1. Native limiter EOS defect (AUD-069) — fix first

`eos_probe.rs` / `eos_probe.log` exercise `ParametricInPlacePluginAdapter<LimiterPlugin>` as a public `Plugin` at 44.1/48/96/192 kHz, 1/2/6 channels, and four lookahead/ISP settings. Each input has 121 frames with independent low-level first and final markers. The reference is both (a) a separately initialized limiter explicitly fed enough zero continuation, in different callback partitions, and (b) the exact signal `[declared-delay zeros] + input`; these are bit-identical.

Of 48 cases, all 36 with nonzero delay return public `drain()` COMPLETE with zero frames, losing the queued suffix. Delays range 220–3840 frames. All 12 zero-latency controls preserve the entire stream. At 48 kHz, 5 ms lookahead + ISP declares 258 frames of delay: the 121-frame program never emerges at all before the default drain completes. This is a preexisting defect independent of oversampling.

### Narrow proposed correction

- Keep the current native processing arithmetic and parameter defaults unchanged. Extract a private shared block kernel only to allow public process and zero-continuation drain to call the same DSP.
- Track whether nonempty input was accepted and the draining/complete phase. Once a nonempty stream begins valid drain, processing requires reset. Empty-stream drain remains an explicit no-op, matching the host oversampler convention.
- On first valid nonempty drain, latch exactly `lookahead_len + isp_delay_len` remaining frames. The latter is `3 * detector_delay` only when ISP is enabled. This is a finite audio-support bound: release/detector histories cannot generate nonzero audio after the two audio rings empty.
- Emit bounded chunks by zeroing the caller's output slice and running the in-place kernel; no owned zero-input allocation is needed. For example, an advertised maximum of `min(delay, 256)` bounds per-call work. Honor any smaller nonempty whole-frame output capacity and touch only emitted samples.
- Validate initialized state, sample rate, frame alignment, checked dimensions, and required capacity before latching or decrementing EOS state. Invalid drain remains retryable with exact original history. Complete drain is idempotent. Freeze controls once EOS starts so changing ISP/ceiling/release cannot change the latched contract mid-drain.
- `tail_length()` can return cached `Finite(delay as u64)` after initialization without loading telemetry or allocating. Reset clears EOS and signal histories; initialization recomputes the rate-dependent bound.
- Regressions: independent pure delay at first/final markers; nonlinear zero-continuation reference; tiny drain capacities, final-partial capacity, output canaries, wrong-rate/unaligned retry transactionality, processing/parameter mutation after EOS, empty stream, reset, and cold allocation **and** deallocation count.

## 2. Oversampling experiment (AUD-010)

### Executed composition

`protection_probe.rs` implements a numerical stand-in, deliberately using public plugin composition only in `/tmp`:

1. At 1×, the existing limiter runs alone, without extra processing.
2. At 2×/4×, the existing host oversampler runs a fully wet limiter core. Its ISP output correction is off. In true-peak mode its input detector stays enabled and user lookahead is 0.25 ms; sample-peak mode uses zero lookahead.
3. After downsampling, a separately prepared native limiter protects the final stream. Sample-peak mode uses zero lookahead. ISP mode uses exactly the native detector delay `D` of lookahead plus its existing `3D` output correction, so the measured final protector adds `4D` frames.

The production implementation should extract these DSP stages and keep a single public parameter owner; it should not construct two independently parameterized public `LimiterPlugin`s.

### Measured peaks and realtime properties

- 840 burst cases: rates44.1/48/96/192 kHz × ISP off/on × factors1/2/4 × seven lengths1/2/3/5/13/31/127 × five deterministic patterns. Every final sample is within 0.00001 dB of the −12 dBFS ceiling. Every ISP-enabled independent reconstructed peak is within 0.1 dB; observed worst is approximately −11.999999 dBTP (roundoff at the ceiling).
- Complete zero-padded tone streams also remain protected with ISP enabled. Measurements include actual startup/shutdown samples, not an artificially cut steady-state slice. Spectral amplitude alone uses a coherent steady-state window.
- The reconstruction oracle independently generates f64 49-tap Hann-sinc kernels from the mathematical formula and measures the final emitted output, including 25 trailing zeros. It does not call production detector/kernel code. Its factor follows the current plugin contract (4× below96k, 2× below192k, sample peak thereafter). This establishes the current finite-reconstruction contract, not an ideal continuous-time supremum.
- 24 prepared combinations of four rates, 1/2/6 channels, and 2×/4× produce bit-identical output across callback lengths1/127/257/8192, including reset between renders. Separate low-level first/final impulses have their largest samples exactly at origin + declared latency for every channel and prepared instance; source origins0 and2047 are both checked.
- Explicit allocator instrumentation records **zero allocations and zero deallocations** on a cold valid process call and reset for all 24 combinations. Constructor, initialization, and probe result storage occur outside measurement. This does not yet prove the proposed control queue or composed EOS path.

### Alias measurements

48 kHz, amplitude0.9, threshold−12 dBFS, release10ms. Third-harmonic/folded spur levels in dBc:

| Tone | Mode | 1× existing | 2× + final guard | 4× + final guard |
|---|---|---:|---:|---:|
|7kHz|sample peak|−56.727|−58.138|−58.304|
|11kHz|sample peak|−53.857|−67.248|−83.112|
|17kHz|sample peak|−56.951|−94.802|−83.396|
|21kHz|sample peak|−50.762|−57.297|−62.057|
|7kHz|true peak|−127.546|−102.794|−96.017|
|11kHz|true peak|−87.934|−95.675|−99.181|
|17kHz|true peak|−121.117|−116.430|−100.501|
|21kHz|true peak|−78.419|−85.452|−92.632|

The 7kHz third harmonic is in-band; other rows measure folding. Oversampling improves the measured sample-mode folding substantially but is not monotonic across factor/frequency, and true-peak processing already suppresses some modulation harmonics more effectively at1×. Keep1× as the unchanged default and do not promise universal distortion improvement. The first version with high-rate input true-peak detection also disabled was materially worse in true-peak mode; `protection_sample_core.log` preserves that comparison.

Prototype latency at48k: existing1× sample0/ISP30 frames; protected2× and4× sample512/ISP548 frames. At96k protected ISP584 frames; these include the chosen0.25ms core lookahead. The extra final pre-detector lookahead is part of measured behavior. Removing it and keeping only3D correction requires a new independent proof.

## 3. Proposed maintainable public design

### Parameter and DSP ownership

Retain the public `LimiterPlugin`, its constructors, canonical parameter IDs/order, serialization keys, scalar getters, and adapter/factory/native entry points. Append one structural oversampling choice (1× default,2×,4×); construction/initialize can prepare a new rate path, while active setters reject rate-path changes. The public plugin owns one canonical control snapshot and schema. Private kernel structs own envelopes, smoothers, delay lines, meters, and derived coefficients, not independent external parameter registries.

A private rate-path enum can hold:

- `Native`: the existing native kernel, retaining exactly its operation order and float arithmetic. Capture representative golden output before extraction and require bit equality afterward.
- `Oversampled`: a `sotf_host::oversampling::OversampledPlugin<LimiterWetCore>` plus native `FinalProtection`, dry delay, and prepared scratch/control storage. `LimiterWetCore` is private, fully wet, and has no public parameter authority; its trait parameter methods can expose no independent controls. The enclosing plugin updates derived DSP state through `inner_mut()`.

The public plugin returns no preferred-oversampling request. Its direct, factory, FFI, and native paths all construct the same internal rate path, avoiding host wrapping twice or relying on host-only behavior. No host implementation changes are required merely to use the existing generic wrapper and its public inner accessor.

### Dry/wet and delay

Mix exactly once at the final native output. Save the original dry input in a prepared ring delayed by the total reported integer signal latency. Oversampled processing stays wet; the final guard processes the wet branch. As with the existing limiter, the final ceiling guarantee applies to fully wet output; ISP retains its existing requirement `mix == 1` and hard limiting.

For2×/4×, choose the lookahead in base-rate integer frames first, then set the private high-rate core length to `base_length * factor`. This avoids a half-base-frame physical delay caused by separately truncating milliseconds at the higher rate. It preserves requested milliseconds as the public value, uses the existing1× quantization at the input clock, and permits an exact integer dry alignment. Document the quantization. Use checked sample-rate/factor/capacity arithmetic. The final protector contributes the measured4D delay in ISP mode; the FFT pair and fixed buffer contribute their existing declared delay. Tests must independently locate low-level impulse timing, prove the dry-only integer delay, and compare mixed output with the explicit weighted sum. A pure dry delay and the up/downsampled wet path are not sample-identical filters, so an exact wet-versus-dry null is not promised.

### Control timing — decision needed before implementation

The generic host oversampler buffers256 input frames. Simply forwarding the latest parameter value causes a change accepted inside a pending chunk to govern earlier buffered input when that chunk executes. The prototype only establishes constant-parameter behavior. This is not solved by setting the inner ProcessContext sample position.

Proposed precise contract: changes enter at their accepted input frame; maintain a bounded, preallocated per-input-frame control timeline (or coalesced bounded records with a proven worst-case capacity) alongside the pending chunk. The private high-rate core consumes these controls at factor-spaced boundaries, using its own rate-correct smoothing. The base-rate final ceiling and outer mix need an explicit output-time policy: preserve immediate current-ceiling protection at the audible output while applying mix exactly once at the final output. Threshold/release/link events therefore have one canonical owner but deliberately affect separate derived stages on documented clocks. Validate events before queueing and never allocate/free in setters or processing. An initial implementation should test sample-exact event positions at every chunk phase, repeated events, and oversized host callbacks before claiming automation parity.

### EOS and reset

Implement AUD-069 first. The high-rate core must then declare/emit its finite lookahead tail; the existing host wrapper can drain partial input and both FFT overlaps. Feed every drained wet frame through final protection while advancing the dry branch with zeros, then drain final protection and any remaining dry delay. The composition needs an explicit sequential drain state machine and exact no-input/reset behavior; tail bound must include both filter overlap support and retained dry samples, not only group delay. Output-capacity errors must precede state mutation. Use prepared scratch sized to the advertised drain maxima; no realtime heap fallback.

## 4. Remaining acceptance gates before production feature

1. Review/implement native finite EOS separately.
2. Extract DSP state with exact1× golden output and direct/factory parameter compatibility.
3. Implement prepared rate path, full final guard, dry alignment, and accepted-input control timeline.
4. Independent impulse/group-delay + phase, dry-delay, and weighted-mix tests, all supported rates/channels/factors and reset.
5. Final emitted sample/reconstruction ceilings for dense tones, bursts, two tones, threshold steps, fastest release and minimum lookahead; coherent alias measurements as above.
6. Exact finite-stream comparison to separately zero-continued reference, first/final impulses, tiny outputs and invalid retries.
7. Cold process/setters/drain/reset allocation **and** deallocation gates, including native wrapper and factory roundtrip of the new structural choice. Preserve all legacy IDs and1× default.

Primary product references: FabFilter describes oversampling as an audio-path alias-reduction feature and requires limiting to remain safe after downsampling ([oversampling](https://www.fabfilter.com/help/pro-l/using/oversampling)). Its true-peak design also separates input detection from correction of peaks produced by limiting ([true-peak limiting](https://www.fabfilter.com/help/pro-l/using/truepeaklimiting)). These motivate the criteria; they are not evidence that this prototype matches that product's implementation.
