# AUD073 follow-up: Denoiser, HissReducer, SpeechDenoiser

Read-only review, 2026-09-28. Repository production/tests unchanged. Public probes
are `/tmp/sotf-denoiser-tail-probe.rs` and `.log`, executable under
`target/audit-finite-stream/denoiser_tail_probe`. Existing built artifacts only;
no Cargo resolution/build or dependency changes. MIDI/IAMF excluded.

## Verified inventory and mode eligibility

| Processor/configuration | Actual retained audio | Finite continuation? | Proposed next scope |
|---|---|---|---|
| Denoiser, N512 low latency or N2048 normal, each with/without multi-resolution and PND | N input window, circular overlap-add, N startup padding | Yes. Every audio bin is current FFT times a gain. MCRA, DD, captured noise, formants, spatial/tonal/PND and small FFT histories affect gains only. | Native finite drain plus first-window correction; fix PND scheduling if its callback oracle confirms the source concern. |
| Hiss spectral, enabled or disabled and through bypass transitions | N1024/H256 input/OLA, dry delay1024 | Yes. Minimum statistics and gain release multiply finite FFT windows. | Spectral-mode-only finite drain; no backend startup change needed. |
| Hiss conventional, enabled or transitioning | One-pole lowpass state contributes `(1-gain)*low` on zero input; gain/noise/bypass controls also retain history | Recursive audio. Implementation flushes lowpass below1e-20, but no fixed window support. | Leave unclaimed until a state-derived flush or explicit render cap is reviewed. |
| Hiss conventional, disabled from reset | Wet mix exactly0; no audible retained output | Exact no-tail in this restricted configuration. A current `enabled=false` getter does not establish settled bypass after automation. | Do not infer zero tail from the getter alone. |
| SpeechDenoiser enabled/transitioning, mono or stereo | 480 framing/queue; 960 analysis window; synthesis overlap480; mono pitch history1728; recursive input highpass | Not finite from transform length alone. Neural recurrence changes gains; the highpass is the actual unbounded audio source. | Separate timing/compatibility design before drain; no change now. |
| SpeechDenoiser disabled from reset | Exact dry queue480 while model continues warm | Finite480 only if bypass is already settled. A live disable crossfades for480 samples. | Cannot publish generic480 tail while wet can still contribute. |

All three currently inherit drain maximum0 and immediate COMPLETE0. The original
inventory independently demonstrated real suffix loss. Hiss `spectral_mode` is
structural after initialization; Denoiser `low_latency` and `multi_resolution`
are reconstruction-only. Hence proposed finite bounds cannot hide retained audio
from a prior larger active topology. The inactive classic Hiss reducer is not
processed in spectral mode and cannot become active without reconstruction.

## Exact STFT endpoint and public-probe evidence

For accepted source length T>0, FFT N, hop H and analysis origins on integer H:

    S = floor((T-1)/H)*H
    exclusive output end E = 2N + S
    drain D = E-T = 2N-H + ((H-(T mod H)) mod H)
    phase-independent tail declaration = 2N-1.

This is a conservative window-support bound and a deterministic suffix policy;
last returned samples need not be nonzero. T=0 returns COMPLETE0 immediately.
Store only source phase modulo H, not a growing frame counter. Freeze endpoint
on first successful drain. A prepared H-frame cache provides canonical internal
zero-input chunks independent of caller destination capacities. Accept all
positive channel-aligned capacities; write only returned frames. After drain
starts, reject parameter mutation and nonempty input until reset. Invalid rate,
capacity, and initialize(0) must preserve storage/history; zero-frame process
remains a no-op. Empty/completed drain is stable.

Public sparse input probes (first0.25, terminal-0.5) covered:

- Denoiser: 2 FFT modes × 2 multi-resolution modes × 2 PND modes, stereo,
  transparency1; T=N+73. All eight became exactly zero beyond E through another
  2N frames. Explicit process allocation/deallocation counters were0/0.
- Hiss spectral: enabled/disabled × strength0/1, stereo, T73. All four were
  exactly zero beyond E through another4N frames; counters0/0. Strength0 and
  disabled both preserved first0.25 at output1024 and last-0.5 at output1096.
- Conventional Hiss: 1s persistent low-level signal engages reduction before
  zero continuation. At cutoff1000/4000 Hz, suffix peaks0.0055245/0.0056213;
  last nonzero suffix indices313/79. This disproves treating latency0 as no tail.

These are small support probes, not full regression matrices or proof of all
model/control combinations. The algebraic gain-only architecture supplies the
finite support argument for the two STFT paths.

## AUD084: Denoiser startup correction proposal

Current source: `sotf-plugin-denoiser/src/lib/denoiser_plugin.rs` constructor
lines219-232, IO initialization399-406, FFT processing773-829, OLA870 onward,
reset1011 onward, process1064 onward. It uses square-root Hann analysis/synthesis
and H=N/2, but starts `input_buffer_fill=0`, first analysis origin0. Public probe
finds source0.25 produces exactly0 at outputN for all eight configurations.
The previous negative-origin window is absent; this is independent of denoising.

Narrow correction:

1. Prime `(N-H)*channels` zero samples in the existing linear input buffer.
2. Initialize OLA write cursor to ring capacity-H, read cursor0; discard H
   negative synthesis samples for the first frame. Do not publish a ready hop
   for that frame. Later origins0,H,... publish normal ready hops.
3. Retain existing N-frame public startup padding and callback output contract.
   The first sample receives the missing Hann[N/2]=1 contribution, together
   with Hann[0]=0. Two windows sum to1 for every phase. S/E/D above are unchanged.
4. Apply identical constructor/reset priming. Existing algorithm first consumes
   all callback input before writing output, so it does not need the incremental
   padding fix used in SpectralCompressor. Verify output capacity with the extra
   initial H samples; the current prepared ring already reserves an extra N.
5. Do not prime the small multi-resolution or PND analyzers with fake prior
   program. They analyze real accepted source/control time and only determine
   gains. Test low/normal plus multi/PND combinations separately: transparent
   large-path gain is not a unity oracle when small-path gains are enabled.

Required independent red/green checks: paired first/final impulses at every
initial-window phase for both N; dense unity on a genuinely unity configuration;
long silence past ring wrap; direct mathematical sqrt-Hann overlap identity;
nonlinear and profile/multi/PND modes versus separately zero-padded processing;
phase boundaries, mixed capacities, reset and rate reinitialization; explicit
cold allocation+free counters including T1 EOF before the first FFT.

### PND and capture policy need explicit handling

`process_in_place` currently calls each PND analyzer with the entire callback
(lines1111-1121), then iterates the large FFT windows. `calculate_polyphonic_gains`
reads its latest matched peaks. Thus a large callback may expose future notes to
an earlier STFT frame, unlike small callbacks. This is source-proven control
ordering; waveform magnitude has not yet been quantified by a dedicated probe.
Move PND feeding into the existing input-consumption slices capped at each large
FFT boundary, like `MultiResState::feed_and_process` already is. Deinterleave only
the bounded consumed slice into prepared scratch, before executing that frame.
First execute a public callback-partition regression; do not weaken its oracle.

An explicit noise-profile capture can finish during synthetic zero padding and
mutate the persistent stored profile (`noise_profile.rs:21-62`). Choose a clear
policy before implementation: ordinary zero-continuation semantics preserves the
same audio as user-supplied zeros but also continues capture; freezing explicit
capture avoids teaching padding as program, and needs a matching control-state
reference. Normal adaptive gain/noise updates do not extend audio support.
Do not silently change captured configuration or claim both policies equivalent.

## Hiss spectral implementation proposal

Source: `plugins-denoiser/src/spectral_hiss.rs:74-88,136-155,168-339` already primes
N-H zeros. It stores negative-origin output at ring index0 and emits only H
startup zeros. Therefore the output offset is H+(N-H)=N; wet and dry are aligned.
The source0/final marker public strength0 oracle passes. Preserve this established
scheduler and its possible non-unity pre-ringing; no duplicate startup fix.

Implement wrapper-owned source phase, endpoint and prepared H-frame drain cache;
call existing spectral backend with zeros. Metadata is Finite2047 and maximum
per-call256 only when structural spectral_mode=true. Conventional mode should
retain its existing unknown-tail/default drain behavior with an explicit
remaining-gap note; do not imply all Hiss configurations now preserve EOF.
Both setter paths must reject after a spectral drain starts; reset clears flags.
No shared host or plugins-denoiser DSP changes are necessary for this finite part.

## AUD083: Speech wet/dry timing mismatch, confirmed

For impulses at source indices0/72/479/480, mono and stereo public processing:

- Disabled peak is exactly source+480, amplitude0.75.
- Enabled main peak is source+960 (mono amplitudes~0.0988; stereo~0.1445).
- `latency_samples` returns480 in all cases.
- Wet output remains above1e-8 through approximately source+6698. This observed
  threshold is not an exact silence time or promised residual floor.

The direct vendored `process_frame_with_band_gains` with unit supplied gains also
places the principal impulse peak at480, proving the model's extra timing beneath
the wrapper's480 queue. That API is not a full-band identity: band interpolation
and analysis can produce preceding ringing, so main-peak evidence is paired with
source inspection, not mislabeled an exact pure-delay response.

Source: `plugins-denoiser/src/rnnoise.rs:106-125,178-244,256-276` enqueues raw dry
alongside model output. `nnnoiseless/src/denoise.rs:213-226` analyzes previous480
plus current480; synthesis374-381 emits first half plus prior overlap. Live bypass
therefore blends streams whose nominal timing differs by480. Correcting this
needs an aligned dry delay and compatibility/latency decision, not an isolated
constant tweak. Do not change Speech latency in the present wave.

The recursive highpass coefficients at denoise.rs432 and473 have denominator
`1-1.99599*z^-1+0.996*z^-2`. The complex-pole radius is sqrt(f32(0.996))≈0.997998.
A future bounded-render proposal could derive a cap from this radius and add
finite framing/analysis/synthesis/pitch support; no cap is adopted here. The
1728-sample pitch buffer is feed-forward stored input, whereas neural state
changes gains and cannot alone synthesize sound from fully zero audio spectra.

## Other initialization/RT review findings

- Denoiser initialize updates rates/PND objects but does not reset retained IO or
  noise state; process currently has no context-rate equality check. Source-
  verified, not yet exercised by this probe. A finite-stream reinitialize policy
  must reset program history while preserving intended captured configuration.
- Classic Hiss initialize changes coefficients without clearing lowpass/history;
  spectral initialize does reset. Conventional reset also retains the current
  smoothed cutoff alpha. Avoid claiming fresh-instance equivalence for that
  deferred path without a specific regression and agreed control policy.
- Denoiser uses realfft `process()` convenience methods (FFT audio and small
  analyzer), which create scratch vectors internally. The measured current
  power-of-two plans allocate/free0, apparently because required scratch is0.
  A portable cold guarantee should check actual plan scratch requirements or
  prepare explicit `process_with_scratch` storage; don't generalize observed
  architecture behavior to every future FFT backend.
- New Hiss/Speech cached analyzer publication paths reuse prepared triplet
  storage. Current isolated public probes measured0alloc/0free in callbacks.
  Tests must include EOF as the first transform/model callback and publication
  boundaries, not just a warmed steady callback.
