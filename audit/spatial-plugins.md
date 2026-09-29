# Spatial, adaptive, and HAL audit

Read-only source review, 2026-09-27. No source changes or new test runs were made for this report. Existing tests described below are inspected coverage, not independently executed results. TokenSave was consulted first; its graph was rebuilding, so source reads were used for conclusions. MIDI and IAMF were excluded.

Paths below are relative to `/home/pierre/src/all_of_sotf/sotf-daw`. Plugin paths share `crates/sotf-plugins/crates/`. Line numbers describe this checkout and may move during concurrent work.

## Three ranked implementation opportunities

### 1. Preserve buffered audio and tails at end of stream

**Evidence:** `sotf-host/src/plugin.rs:230-246` defaults to zero maximum drain frames and immediate `PluginDrainResult::COMPLETE`. Binaural, Upmixer, XTC, AAE, Beamformer, AEC, and PND have no trait override. They contain pending input, overlap-add output, delay lines, or reverberation. Relevant examples are AEC `src/lib/aec_plugin.rs:340-425`, Beamformer `src/lib/beamformer_plugin.rs:303-443`, Binaural `src/lib/binaural_decoder_plugin.rs:1891-1950`, Upmixer `src/lib/upmixer_plugin.rs:2481-2538`, XTC `src/lib/xtc_plugin.rs:1720-1735`, and PND `src/lib/phase_vocoder_channel.rs:425-435`.

**Consequence:** finite streams ending between processing hops can omit programme samples as well as effect tails. Processing manually appended silence in a numerical test does not verify the host drain contract. AAE needs a documented finite reverberation completion policy even though its direct path reports zero algorithmic latency. Ambisonics' optional crossover also has filter state; its pure matrix path does not.

**Implementable scope:** per-plugin bounded, allocation-free drain state, with explicit completion criteria and adequate advertised drain capacity. Exercise a final-sample impulse with programme lengths 1, hop−1, and hop+1; call only `drain` afterward and compare the concatenation against an independently zero-padded reference. Include irregular output capacities, tail count, no duplication, eventual completion, and allocation checks. Reverb requires an energy threshold and maximum duration policy. This is separate from the rejected host graph queue proposal and unsupported DAG drain.

### 2. Honor SOFA `Data.Delay`

**Evidence:** `sotf-host/src/sofa.rs:6` reexports `sofa_reader`. The sibling `../sofa-reader/src/hrtf/sofa_file.rs:11-26` representation has no delay field, and `load_sofa` at lines 214-296 reads sample rate, positions, and `Data.IR` without reading or rejecting `Data.Delay`. Binaural `src/hrtf/interpolate.rs:26-36` derives arrival time solely from the raw impulse samples, then interpolates that estimate. A min-phase HRIR carrying its interaural delay separately therefore loses that delay.

The [SOFA SimpleFreeFieldHRIR convention](https://sofacoustics.org/mediawiki/index.php/SimpleFreeFieldHRIR) defines additional per-receiver/per-measurement delays in samples, with the same sampling-rate basis as the impulse responses. Dropping them can alter localization even if frequency magnitude is correct.

**Implementable scope:** extend the sibling loader, its cache representation, and consumers to preserve and apply integral and fractional delays; validate permitted broadcasting/dimensions. Add synthetic files with known unequal ear delays, measurement-dependent delays, and sample-rate conversion, then verify timing and cache round trips. This requires explicitly authorized work in the sibling repository. No sibling files were edited.

### 3. Remove cold audio-thread allocation in filter snapshots

**Evidence:** Binaural `src/lib/binaural_decoder_plugin.rs:539-544` calls `state.load()` in its block processor. XTC `src/lib/xtc_plugin.rs:1668-1670` calls `filters.load()` during processing; pending update and retirement paths also use ArcSwap operations around lines 922, 962, and 977. The pinned `arc-swap` is 1.9.2. Its default strategy enters thread-local debt bookkeeping: `strategy/hybrid.rs:189-190` calls `LocalNode::with`, and `debt/list.rs:223-228` obtains a node whose creation can allocate at lines 151-169. See the [primary crate source](https://docs.rs/arc-swap/1.9.2/src/arc_swap/debt/list.rs.html).

**Consequence:** preparation on a control thread and processing warmups do not establish an allocation-free first callback on the actual audio thread. Recycled global debt nodes can hide the issue. This is source-backed evidence; this audit did not execute a fresh-thread regression.

**Implementable scope:** bounded prepared publication and cached audio snapshots, preserving crossfades and deferred destruction. Test construction on a live control thread, the very first processing callback on another fresh thread, and repeated filter updates with retirement backpressure. Count both allocations and deallocations; removing allocation must not move final-Arc destruction into the callback. If using a mutex, prepare its platform resources before callbacks and only try-lock on the audio path.

## Per-crate capabilities and numerical evidence

### Ambisonics

**Implemented:** ACN/SN3D orders 1–3 (4/9/16 inputs), speaker-layout decoding with LFE excluded, regularized mode matching and AllRAD, max-rE weighting, and a 700 Hz LR4 dual-band decoder. `sotf-plugin-ambisonics/src/decode_matrix.rs:78-181` builds a Fibonacci virtual array, decodes to it, then composes VBAP gains for the physical speakers. `src/ambisonics_decoder_plugin.rs:323-403` validates buffers and rejects non-finite input. Zero fixed latency does not imply zero crossover group delay.

**Evidence:** closed-form ACN harmonic identities and normalization/orthogonality tests in `src/spherical_harmonics.rs`; matrix rank/reconstruction, max-rE, omni/LFE, front/back energy, and dense finite-energy tests in `src/decode_matrix.rs:674-814`.

**Limits:** this is a defined ACN/SN3D decoder, without observed N3D/FuMa adaptation or HOA rotation. Existing loose energy bounds do not establish full-sphere localization, velocity/energy-vector direction, or ripple accuracy. An independently generated matrix/reference grid would strengthen evidence. The [original AllRAD paper](https://secure.aes.org/forum/pubs/journal/?elib=16554) supports the method; implementing a virtual array does not establish parity with every grid/weighting choice in reference decoders.

### Binaural

**Implemented:** known surround layouts to stereo, SOFA convolution and resampling, arrival-time-aware log-magnitude/phase interpolation, near-field adjustment, diffuse-field EQ, asynchronous head-pose updates, filter crossfades, and deferred reclamation. Room rendering adds source-owned first/second-order reflections and optional FDN reverberation.

**Evidence:** `sotf-plugin-binaural/src/lib/tests.rs` includes a naive reflection oracle (around 290), reflection partition invariance (467), affine/constant interpolation fields (523/545), EQ balance (615), measured impulse latency (685), and direct linear-convolution comparison across boundaries (739). Crossfade and retirement behavior are also covered.

**Limits:** ranked SOFA delay, cold snapshot, and drain issues apply. Reflection direction currently uses broadband ILD rather than a complete per-reflection HRTF/ITD treatment. Synthetic convolution correctness does not establish personalized localization or externalization; a measured SOFA corpus and timed head-motion trajectories are needed for those claims.

### Upmixer

**Implemented:** stereo to surround/height layouts through FFT covariance/direct-ambient decomposition, principal-eigenvector direction estimates, VBAP, optional multi-source handling, decorrelation, transient/height controls, dual-resolution processing, LFE routing, and optional asynchronous vocal-probability inference. See `sotf-plugin-upmixer/src/frequency_domain`, `panning.rs`, `decorrelation.rs`, `height.rs`, and `hr_processing.rs`.

**Evidence:** complex eigenvector phase and energy tests in `src/frequency_domain/tests.rs:114-170`; streamed impulse latency and partition invariance in `tests/integration.rs:49,456`. The multi-source energy test around line 378 checks correlated, antiphase, quadrature, and independent inputs in 5.1 and 9.1.6 against a −4 to +2 dB budget after warmup.

**Limits:** `tests/upmixer_audio_regression.rs:247,277,305,333` returns successfully when golden files are absent; no matching golden assets were found in this checkout. A gate can therefore pass without those comparisons. Self-generated goldens and broad energy bounds do not independently establish dialogue placement, stability, or listening quality. No held-out vocal-model accuracy evidence was found. Drain is absent.

### XTC

**Implemented:** regularized modeled stereo transaural inversion, head/speaker geometry, Woodworth and Brown–Duda components, optional pinna and reflection models, SOFA transfer functions, imported RoomEQ filters, gain limits, auto gain, limiter, and asynchronous crossfades/reclamation.

**Evidence:** analytic geometric delay, model coefficients, unity STFT, independent small-matrix inversion, asymmetric normalization, gain budget, and matrix-column orientation tests in `sotf-plugin-xtc/src/lib/tests.rs`. `tests/xtc_validation.rs:31-90` compares ITD with a separate Woodworth formula. `tests/xtc_quality_tests.rs:7` checks sustained hot-input peak limiting.

**Limits:** `src/validation/measure.rs:14-17` explicitly evaluates cancellation with the same transfer model used to design filters. This demonstrates model consistency, not measured ear cancellation. Independent measured transfer functions, head-position error grids, room changes, and gain/robustness tradeoffs remain essential evidence gaps. Ranked cold allocation and drain findings apply.

### AAE

**Implemented:** stereo-to-multichannel feed-forward ambience enhancement: pre-delay, diffusion, VBAP early reflections, an eight-line time-varying Hadamard FDN, damping/tone controls, dialogue-aware wet ducking, auto gain, and an effects LFE send. Its dry route has no fixed algorithmic delay.

**Evidence:** allpass DC/energy, LFE behavior, delay transitions, channel-ratio preservation, and bounded energy in `sotf-plugin-aae/src/lib/tests.rs` and `src/lib/tests/misc.rs`. `src/quality.rs:443-518` independently checks Schroeder RT60 on a known exponential, sparse/dense echo density, coherent/diffuse spatial measures, known gain/phase, and distortion/detector measurements.

**Limits:** this is not a complete closed-loop microphone/loudspeaker acoustic enhancement installation with feedback identification and stability margins. `src/quality_validation.rs:1-6` and the manifest/protocol require external evidence; no accepted recorded validation run was found. Those metric unit tests do not supply a listening study. FDN and reflection drain are absent.

### Beamformer

**Implemented:** uniform linear array to mono, MVDR with target-presence-gated noise covariance, diagonal loading and distortionless fallback, superdirective diffuse covariance, and GSC with fractional steering delays, aligned blocking references, NLMS adaptation, and target-dependent adaptation freeze. See `sotf-plugin-beamformer/src/mvdr.rs:93-165`, `src/gsc.rs:116-207`, and `src/lib/beamformer_plugin.rs:303-443`.

**Evidence:** independent fractional-delay and plane-wave checks in `src/gsc.rs:359-380`; steering geometry/magnitude checks in `src/steering.rs`; distortionless fallback and scale-invariant presence tests in `src/mvdr.rs:376-389`; partition, causality, scratch reuse, and invalid-context tests in plugin tests.

**Limits:** no observed arbitrary-array calibration or moving-source tracker. Microphone gain/phase errors, reverberant speech, wind, broadband fractional-delay error, and white-noise-gain/SNR tradeoffs need quantified tests or recorded corpora. Finite outputs and ideal plane waves are insufficient real-room accuracy evidence. Drain is absent.

### AEC

**Implemented:** two inputs (microphone/reference) to mono, partitioned frequency-domain adaptive filtering, shared reference FFT, foreground/background filters, slower background adaptation during double talk, sustained-improvement filter transfer, causal partition constraints, and crossfaded residual suppression. Variable host blocks are buffered around a fixed internal block delay. See `sotf-plugin-aec/src/two_path.rs:88-143` and `src/lib/aec_plugin.rs`.

**Evidence:** causal partition and late-echo convergence tests in `src/pbfdaf.rs:437-539`; foreground stability during synthetic double talk and recovery after delay changes in `src/two_path.rs:320-392`; exact block latency, large-block queuing, allocation, speech-preservation, and echo-reduction tests in plugin tests.

**Limits:** no observed reference-clock drift tracker or independent delay alignment outside the finite adaptive tail. Nonlinear loudspeakers, varied real speech/music, and room changes lack broad external validation. The [Microsoft AEC Challenge](https://github.com/microsoft/AEC-Challenge) illustrates the needed breadth of single/double-talk devices, rooms, and perceptual/intelligibility evaluation; it does not require adopting a neural implementation. Drain is absent.

### PND

**Implemented:** a fixed-duration pitch-drift correction insert, not a device-clock sample-rate converter. Pilot-based or change-only detection uses channel consensus and drives a 2048-point, 75%-overlap phase vocoder with fixed 2047-frame latency, identity phase locking, transient resets, and optional formant-envelope preservation. See `sotf-plugin-pnd/src/lib/phase_vocoder_channel.rs`.

**Evidence:** amplitude-invariant pilot detection, colored-noise rejection, harmonic pilots, consensus permutation, exact impulse latency, and block/control partition tests. Phase-vocoder tests include unity SNR, impulse locality, arbitrary time origin, repeated attacks, interchannel phase, resolved partials, and broad formant peaks (`src/lib/phase_vocoder_channel.rs:604-948`).

**Limits:** a zero reference cannot determine an unknown constant tuning offset by design. Held-out polyphonic music and speech/formant listening evidence is absent. The [Rubber Band technical description](https://www.breakfastquay.com/rubberband/technical.html) offers a primary comparison for transient/phase behavior, not proof of equivalent output quality. Drain is absent.

### HAL input

**Implemented:** a macOS transport source with strict negotiated frame/channel/rate validation, whole-frame partial reads and zero fill, startup arming, typed 64-bit telemetry, and control-thread recovery for disconnects/key rotation. See `sotf-plugin-hal-input/src/lib/hal_input_plugin.rs:370-493`. Reported zero latency avoids treating ring capacity as measured source latency.

**Evidence:** fake-reader tests cover channel layouts, partial tails, armed underruns, replacement validation, invalid contexts before consumption, live format errors, reconnect/key events, corruption, and normal/starved allocation behavior (lines 763-930).

**Limits:** these are transport contract tests, not macOS CoreAudio/crypto/device-clock loopback measurements. No source-clock timestamps or end-to-end measured latency are exposed by this plugin. Native stress and recovery evidence remain necessary.

### HAL output

**Implemented:** a macOS sink with strict format validation, prepared bounded frame-aligned pending output, explicit overflow telemetry, control-thread reconnect, and priming. `sotf-plugin-hal-output/src/lib.rs:330-389` models target fill, virtual-device latency, and safety offset; `process:515-627` preserves pending data under backpressure. `latency:630-634` uses the declared target/device terms rather than ring capacity.

**Evidence:** tests cover counters beyond 32-bit range, priming latency, maximum-channel backpressure without allocation, invalid contexts, and capacity-versus-latency distinctions (lines 944-1164).

**Limits:** pending writes have no end-of-stream flush/completion acknowledgment. Callback acceptance does not prove acoustic playback completion. Native device loopback, sustained clock mismatch, driver restart, crypto/key rollover, and queue stress must be validated on macOS; changing queue fill is not a measured constant delay.

## Interpretation

The strongest existing evidence is independent analytic or time-domain reference comparison: convolution, delay geometry, matrix algebra, adaptation examples, and phase-vocoder invariants. Many advanced features exist, but model-self-consistency, optional missing goldens, and synthetic fixtures must not be presented as measured perceptual or real-room equivalence. The three ranked opportunities identify concrete correctness/realtime contracts, rather than requiring a particular branded algorithm.

## AUD045 Binaural end-of-stream checkpoint (implemented, 2026-09-27)

**Proven defect:** Binaural inherited the host's immediate-completion drain despite retaining partial input hops, overlap-add output, source reflection delays, and optional recursive reverb. For FFT size 64, a one-frame impulse with a nontrivial 49-tap HRIR emitted only one startup-silence frame and then reported completion. Its response was entirely lost. Reproduction: `/tmp/sotf-binaural-drain-repro.log`.

**Implemented finite bound:** For FFT size N, hop H=N/4, and positive input length T, the final input-containing transform starts at `floor((T-1)/H)*H` and contributes N output samples. Including the advertised N-frame scheduling delay, drain returns `N + (N-H) + ((H-T%H)%H)` frames after the input timeline. The bound is extended to at least `N + D`, where D is the longest source reflection tap delay. Reflection taps use original source channels and scalar ear gains; they do not have an additional per-tap HRTF convolution. This conservatively retains the implemented full-transform support, including diffuse EQ/LFE frequency-domain shaping, and may include trailing zeros. It does not claim such shaping is a shorter ideal FIR or independently prove its filter response.

**Recursive policy:** Enabled late FDN reverb adds three configured RT60s *after* that finite bound, then clears the feedback history. This is a documented bounded rendering policy, not exact finite support or a certified residual dB level. Maximum extra duration is 15 seconds. The existing amplitude floor and FDN processing equations are unchanged.

**Lifecycle/realtime:** The first drain freezes its budget and current HRTF snapshot; an existing crossfade finishes, while pending background tracking publications wait until reset. New nonempty input and parameter changes require reset after drain starts. Empty streams finish immediately; repeated completed drains return zero frames. Positive, stereo-aligned destination capacities are supported, each call writes at most one hop, and unused destination samples remain untouched. Invalid empty/odd capacities on a pending stream do not consume state. The zero buffer is preallocated. Existing publication retirement ownership remains intact.

**Independent evidence:** `src/lib/drain_tests.rs` compares final output with direct f64 convolution of known 49-tap HRIRs (including nonzero last taps), and with separately zero-padded input continuation. Eight input lengths cover incomplete/full hop and FFT boundaries with variable callbacks. A source-reflection oracle checks its final delayed impulse at exactly `T-1+N+D`. FDN tests compare zero continuation at exact binary RT60 choices 0.125/0.25 seconds, verify exact cap lengths, and cover destination capacities 1/16/513 frames. Further tests cover capacity-error retention, zero-signal progress, completed lifecycle, and bit-exact reset. `src/lib/realtime_tests.rs` verifies zero allocation/deallocation on cold-thread first drain and reset/drain; previous publication ownership tests pass.

**Verification:** 119/119 unit/integration tests and focused Clippy `--lib --tests -- -D warnings` pass; package format and diff checks clean. Logs: `/tmp/sotf-binaural-drain-tests.log`, `/tmp/sotf-binaural-drain-clippy.log`. Production and documentation are frozen for the aggregate gate.

**Remaining Upmixer work:** It also lacks EOS drain. Main/short-window transform support is finite; optional subharmonic synthesis has a recursive release envelope. A separate startup defect is now tracked as AUD049: zero-filled history is not prefixed to sqrt-Hann analysis, so the first sample is multiplied by zero and the initial half-window lacks its preceding overlap. Upmixer production remains unchanged pending an independent phase-offset oracle and scheduling proposal.

## AUD049 Upmixer startup checkpoint (implemented, 2026-09-27)

The startup reproduction exercised every sample phase in the first window with actual FFT/OLA processing under stereo identity routing and identity crossover tables. For N=64 and N=512, callbacks of 1, variable 17/3/97, and 512 frames all measured first-sample gain 0 and quarter-window gain 0.49999994. A 0.25-amplitude impulse had maximum error 0.25. The missing preceding analysis frame removes the complementary term: `w[n]^2 = (1-cos(2πn/N))/2`, but correct 50%-overlap identity reconstruction requires `w[n]^2 + w[n+N/2]^2 = 1` from the first sample. Log: `/tmp/sotf-upmixer-startup-repro.log`.

Main and HR analysis buffers now start with a zero-prefilled half-window. The first synthesis window spans negative and nonnegative timestamps; its negative-time half is cleared and discarded. Public N-frame startup padding is unchanged. Constructor, reset, initialization, and FFT reconfiguration restore the same boundary state. This fixes genuine startup amplitude loss without changing the documented latency.

The corrected initial half-window also exposed a previously dormant HR gain scheduling error in the existing stationary-noise partition test: max callback-dependent delta 0.00165963 versus its required 0.00002. The first full window following a half-filled window activates the transient path, whose gain ramp previously used the current host read length. HR gains now occupy a preallocated ring on the main output timeline, computed while the corresponding FFT analysis is current. Later analysis and arbitrary fractional-hop host reads cannot retarget an older hop. An independent numerical ramp oracle tests the expected `sqrt(2)/N × transient_gain × (sample+1)/hop` gain, including later queued analysis.

Verification: 138/138 unit/integration tests pass. New evidence includes every initial-window phase, first/final impulses in finite zero-padded streams, reset and FFT reconfiguration equivalence, cold callback/reset zero allocation and deallocation, and HR per-sample gain magnitudes. Existing ordinary spatial, multi-source callback-partition, and advertised latency assertions pass at unchanged tolerances. One internal reset-buffer assertion now checks the intended zero-prefilled half-window instead of an empty buffer. Logs: `/tmp/sotf-upmixer-startup-tests.log`, `/tmp/sotf-upmixer-startup-clippy.log`. Upmixer EOS drain remains the next AUD045 scope.

## AUD045 Upmixer end-of-stream checkpoint (implemented, 2026-09-27)

**Separate reproduction after AUD049:** With corrected startup, a one-frame neutral input still produced only its first startup-silence frame and immediately completed EOS, while its conservative finite timeline at N=512 extends to 1024 frames. The impulse at the advertised delay never appeared. Log: `/tmp/sotf-upmixer-drain-repro.log`.

**Finite support:** Main analysis uses N and H=N/2. The prefixed first window changes the starting boundary, but the last input-containing window still begins at `floor((T-1)/H)*H`; therefore the absolute finite end is `N + floor((T-1)/H)*H + N`. HR support extends this if needed to `N + floor((T+D-1)/Hhr)*Hhr + Nhr`, with D the existing alignment delay. These are full transform-support bounds, not idealized filter lengths. Crossover and decorrelation coefficient vectors act inside finite FFT blocks, not recursive audio IIR histories. Binaural preview is a static fold of rendered speaker channels, not a child Binaural decoder.

**Subharmonic policy:** Enabled synthesis or a still-releasing envelope adds `ceil(14*tau_effective_samples) + N` after the finite bound, where `tau_effective_samples = -1/ln(1-alpha)` uses the actual f32 release coefficient. Release coefficients update immediately; there is no pending release smoother to outlast that coefficient. The extra N covers synthesis overlap. After this cap, oscillator and envelopes are cleared. This is a bounded rendering/truncation policy, not a claim of exact recursive silence or guaranteed residual level. At ordinary sample rates the maximum configured 500ms release adds about seven seconds plus N. Degenerate zero coefficient at an extreme sample rate falls back to fourteen configured release times. Tests independently bound cap rounding from the analytical coefficient and f32 exp/subtraction quantization.

**Lifecycle/realtime:** The first drain freezes its budget, processes zeros in fixed hop-sized calls into preallocated output storage, and serves arbitrary positive frame-aligned destination capacities from that cache. Capacity changes therefore cannot retime callback-sensitive gain controls. Empty/unaligned capacities on pending streams reject before mutation. New nonempty input/parameter changes require reset after drain begins; completion is idempotent. Empty streams and hard bypass complete immediately. Reset and channel/FFT reconfiguration restore/rebuild bounded storage. Unused destination samples remain untouched.

**Evidence:** Seven stream-boundary tests include direct neutral first/final impulse expectations at partial/full hop/FFT lengths; independently zero-padded continuation; 5.1 HR rendering and stereo preview with auto gain enabled; recursive 20ms/50ms releases; exact bitwise capacity invariance for 1, mixed 13/1/4096, and 4096-frame destinations; zero/noncomplete progress; invalid-capacity history retention; lifecycle and reset equivalence. The full suite passes 142/142 tests. Cold first drain with both subharmonic synthesis and auto gain enabled reports zero allocations and zero deallocations; cold process/reset/drain also passes. Existing latency and ordinary spatial fixtures retain their tolerances. Focused Clippy and format/diff checks pass. Logs: `/tmp/sotf-upmixer-drain-tests.log`, `/tmp/sotf-upmixer-drain-cold.log`, `/tmp/sotf-upmixer-drain-clippy.log`.

**Limits preserved:** This work defines finite-stream behavior and fixes initial WOLA reconstruction/HR gain scheduling. It does not establish perceptual equivalence to commercial upmixers, general automation partition invariance for every existing gain controller, or ideal infinite-length IIR response from the sampled frequency-domain crossover. Optional ONNX inference was not enabled as an active detector in the numerical oracle.
