# Read-only next-step review: native Speech fixture and spectral MultibandExpander

2026-09-28. No production or repository test edits; no new test runs.

## Speech fixture correction: no findings

`crates/sotf-plugins/crates/plugins-nih/src/params_scalar_setter_tests.rs:6-7` defines independent `SPEECH_DELAY=960`, justified by model480 + adapterqueue480. `expected_sample` uses it for the changed-bypass scalar oracle (line28), and the oversized-callback test uses it for the exact mono/stereo shifted source and latency assertion (lines305-357). Existing3e-5 scalar tolerance, exact oversized dry equality, finite wet checks, sentinels and fresh-thread no-allocation assertions remain. Expected samples are not inferred from `latency_samples()`.

The fixture follows the accepted backend contract, `plugins-denoiser/src/rnnoise.rs:342` and the independent direct-model/transition evidence in `audit/speech-denoiser-latency.md`. Enabled wet startup still has only480 guaranteed zero queue samples; the changed fixture asserts exact960 delay only for bypassed dry. No incorrect wet pure-delay claim was introduced.

## Recommended scope: AUD094 MultibandExpander spectral startup + AUD073 finite EOS

### Verified source facts

- Only MultibandExpander has this spectral mode; MultibandCompressor remains the time-domain crossover family (`multiband-compressor/.../multiband_compressor_plugin.rs:1297`). Do not describe both as a shared spectral implementation.
- Spectral N=1024,H=N/4=256, dual periodic Hann and1/(1.5N) synthesis scale: `multiband-expander/src/lib/spectral_state.rs:105-123`; initialization sets input_fill0 and next_add_position0 at150-162, with declared/startup delayN.
- First input is multiplied by Hann[0]=0 in the only window that can contain it. No negative-origin windows exist: `multiband_expander_plugin.rs:751-762`, `:931-965`. This source-proves missing first wet sample under unity gain. Reset repeats the same origin policy.
- Independent unity startup coefficient for source n<1024 is sum_{q=0..floor(n/H)} w[n-qH]^2/1.5. With w[n]=(1-cos(2πn/N))/2, coefficients at n=0,256,512,768 are exactly0,1/6,5/6,1. Thus the current phase512 impulse test can pass its delay assertion while losing1/6 amplitude: `src/lib/tests.rs:434-475` only checks peak position. These are analytical predictions; no new public waveform probe was executed in this read-only step.
- `finite_response_frames()` at1457-1464 returns a finite bound only for settled dry or one-band time domain. Wet spectral returnsNone; drain at2076-2080 immediately returnsCOMPLETE.
- Spectral wet audio consists solely of finite input windows, finite IFFT/OLA and a pureN-frame dry delay. Per-bin envelopes/gate histories multiply the current spectrum; zero input windows remain zero irrespective of envelope release (`process_spectral_hop`,708-883). The recursive IIR crossover and sidechain program paths are not used by the spectral dispatch at1502-1503. Consequently spectral wet has finite audio support and does not need a recursive-tail truncation policy.

### Narrow implementation proposal

1. First execute red public unity oracles: input0 impulse, every first-window phase, dense dyadic source, final marker; wet gain unity by ratio1 plus explicit band overrides/bypass. Compare output to independent source shiftN, asserting amplitude and position. Include1-frame, hop-adjacent, irregular and oversized callbacks.
2. Prime input with N-H zeros and use negative analysis origins -(N-H),..., -H,0. Discard negative-time synthesis prefixes explicitly; advance OLA origin byH, withhold ready output until origin0 has full preceding-window coverage. Keep existingN startup output padding and N dry delay. Use already-reviewed SpectralCompressor origin/discard design as a model, without importing its detector or parameter law. Reset restores identical priming and clears all discarded regions so ring wrap cannot resurrect prefixes.
3. Derive finite support from origins. For T>0, last source-containing origin S=floor((T-1)/H)H; it contributes through exclusive output E=2N+S. Remaining full spectral continuation R=E-T=2N-H+(H-(T mod H)) mod H, so1792<=R<=2047. With priming, T modH is recoverable from input_fill-(N-H), or track a dedicated source phase if clearer. Latch R at first valid EOS. Existing settled-dry path can retain its exactN continuation; wet or changing mix uses the full finite WOLA bound.
4. Advertise stable structural native drain capacityH=256 for spectral mode even before input/after completion, preserving wrapper preparation. Tail metadata can conservatively report2N-1 for spectral response. Implement checked query count from actual remaining/canonical cache: if unread C>0,1+ceil((R-C)/H), otherwiseceil(R/H), minimum1. No output-capacity-as-progress inference.
5. Use preallocated canonicalH continuation cache if needed to guarantee identical detector/smoother scheduling for tiny drain destinations. Existing hop-local gain calculations and per-sample mix smoothing continue normally under zero continuation; no learned profile exists to freeze. Hold/attack/release histories do not extend audio support. Reject invalid sample rate/capacity before latching/processing; retain current reset-required lifecycle and control rejection after valid EOS. No parameter IDs/defaults or external wiring changes.

### Required acceptance evidence

- First/final impulse every256-hop phase (prefer all1024 initial phases), exactN peak and unity amplitude, dense all-sample identity including onset, ring-wrap/reset/reinitialize equivalence.
- Non-unity processing+drain versus independently zero-padded public process twin for1/2/3/5bands, mono/stereo, threshold/ratio/hold/knee/solo/bypass mixtures. Compare entire returned stream and exact derived length, not only last nonzero sample. The initial draft requested8bands; the constructor clamps to its supported maximum5, so the implementation matrix requests5explicitly.
- Exact phase-bound1792..2047 proof, native bound versus actual successful calls after prior1-frame/cached reads, empty/terminal stablecompletion, invalid destination/rate retry against untouched twin, no post-EOS input/control acceptance.
- Mixed capacity1/17/255/256 and mixed input partitions; first EOS/query/reset/process on fresh thread with explicit0 allocations and0 deallocations. Native wet spectral wrapped2x/4x tests ensure advertised capacity stays prepared.
- Full MultibandExpander tests/QA and focused strictClippy. Preserve existing expansion transfer law, knee and detector convention; endpoint detector amplitude/calibration and upper soft-knee behavior are separate potential accuracy issues, not part of this scheduler correction.

## Why Speech recursive EOS is a separate, larger decision

RNNoise audio includes a recursive high-pass biquad (`nnnoiseless/src/denoise.rs:384-392`, coefficients at430-442), analysis/synthesis overlap, pitch audio history and data-dependent RNN gain/pitch filtering. Silence bypass skips neural processing but still synthesizes the high-pass window; it is not an exact finite-tail guarantee (`:446-465`). Full wet EOS therefore needs an explicit render cap/threshold policy plus model-state continuation decision. A permanently settled dry960 path is finite, but enabled live fades and model-warm bypass need an eligibility latch and honest metadata before narrow dry-only support could be promised. This is less bounded than the spectral MultibandExpander correction.

XTC and terminal engine replacement remain with their assigned owners. No queue/manager protocol changes are proposed.

## Control/adaptation boundary

At accepted EOS, freeze only stream extent and render eligibility, following the existing reset-required control policy. Do not freeze per-bin gate/envelope state: it must evolve exactly like the same instance processing real-valued zero continuation. Unlike adaptive echo cancellation, no model updates can create audio independently of the finite input spectrum, and no capture/profile operation exists here. Reject controls only after valid EOS acceptance; invalid output/rate calls must leave the instance eligible for ordinary processing and controls. This separates finite audio-support proof from infinitely remembered detector scalars without changing their law.

Root reserved AUD094 for startup and AUD073 for finite-drain extension. All production remains held pending aggregate verification and explicit implementation review.
