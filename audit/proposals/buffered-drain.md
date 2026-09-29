# Proposed narrow buffered-drain wave — read-only, approval pending

## Proposed ownership and shared needs

First implement **Declick** and **SpectralCompressor**, each independently
bounded and exposing delayed programme even at disabled/dry settings. They use
ParametricInPlacePlugin, whose new drain/max forwarding already exists; **no
host-trait or bridge/schema changes are required**. Keep parameter indices,
constructor defaults, gain law, detector calibration and advertised latency
unchanged. Root owns EQ/ParametricPlugin hooks/FIR crossover; other agents own
Delay/Convolution and AnalogLimiter/MBC/MBE. No overlap is needed.

Do not bundle SpeechDenoiser/RNNoise, HissReducer, or LinearPhaseEQ into the
first patch. They have materially different support questions:

- SpeechDenoiser: wrapper has a 480-frame dry queue, but enabled RNNoise also
  has model framing/overlap and input filtering. Derive that enabled response
  before choosing a fixed flush duration; recurrent gain state alone need not
  make audio support infinite.
- HissReducer: spectral path is a close follow-up (N=1024, H=256), but the same
  wrapper also exposes a recursive conventional lowpass/residual path. A mode-
  complete API needs that path's explicit bounded-render policy.
- LinearPhaseEQ: actual processing uses NUPC plus aligned dry rings; derive
  partition/history support and structural-reconfiguration history from that
  backend, rather than draining the obsolete-looking overlap field alone.

## Declick finite convention

The backend's ring is 17 frames; candidate index is current source time−8.
Each repaired candidate uses at most eight preceding/following frames.
Disabled mode is exact eight-frame delayed identity. For T>0 accepted frames,
continue with exactly eight zero-input frames; T=0 completes immediately.
Check the enabled repair law independently against a longer zero-padded
reference: candidates strictly after the original stream have eight following
zeros, so the excursion-length rejection should prohibit manufactured repaired
samples there. Prove this on boundary bursts/steps and all candidate phases
before declaring finite8 metadata.

Implementation shape: prepare a bounded zero/scratch buffer at initialize,
track whether real input was accepted, freeze remaining8 at first drain, and
write only returned output frames. No backend source change is expected: its
existing process method handles zero continuation and current control state.
If a source-level bound counterexample appears, report it before expanding
support policy. Max call capacity may be8 frames; accept smaller aligned
positive destinations without allocations.

## SpectralCompressor support derivation

Current source uses N-point periodic Hann analysis/synthesis, H=N/4, and emits
N startup frames. Analysis origins are0,H,2H,...; initial input_fill is0 and
reset restores0 (`stft_state.rs:96-107,124-131`). A frame whose origin is S
contributes N synthesis frames to the OLA ring, observable over output indices
N+S through N+S+N−1. For T>0 accepted source frames, a conservative final
input-containing origin is:

    S = floor((T−1)/H) × H
    exclusive_output_end = 2N + S
    drain_frames = 2N + S − T
                 = 2N − H + ((H − (T mod H)) mod H)

This includes the full transform response, not only the N-frame group/schedule
delay, and dominates the dry delay's remaining N frames. Continue all-zero
analysis hops as needed to make the remaining OLA intervals readable. Gain,
adaptive-threshold and tonal/transient histories can continue their ordinary
zero-input update: they multiply zero spectra after the input window empties
and do not independently generate audio. The proposed bound must be checked
against an independent direct DFT/WOLA reference with frozen unity gain, plus
ordinary zero continuation for nonlinear/adaptive modes. Do not assume decay
release time extends spectral audio support.

Important existing limitation: with empty initial history, a wet first-sample
impulse is multiplied by Hann[0]=0 and no preceding analysis frames exist.
The proposed drain does **not** repair startup reconstruction or relabel it as
unity. Use mix0 for exact delayed-identity EOS checks and the current analysis
origin convention for independent wet WOLA checks. If root wants initial-window
identity corrected, treat that as a separate reviewed scheduler change with
its own phase oracle (similar to the earlier Upmixer correction).

Implementation shape: bounded prepared zero buffer and output cache, fixed
canonical chunks no larger than min(H, current MAX_BLOCK_FRAMES), with smaller
consumer destinations reading the cache. Freeze the derived budget on first
drain, including any pending cached output. Store only a bounded input phase
mod H and has-input flag; do not accumulate an overflowing total-frame counter.
Use internal zero continuation without counting padding as newly accepted
programme. No callback allocation/free, locks or telemetry snapshot creation.

## Lifecycle and error contract for both

- Empty stream completes with zero frames. Completed drain is idempotent.
- Nonempty input or parameter mutation after drain starts rejects until reset;
  zero-length input should be an explicitly tested no-op.
- Wrong sample rate, zero/nonaligned pending destination capacity, or impossible
  capacity rejects before consuming budget/state; trailing destination sentinel
  stays unchanged. Output width is the plugin's declared output layout.
- Reset clears EOS/cache/history and restores fresh-target behavior; reinitialize
  rebuilds prepared storage and starts a fresh stream. No extra parameter or
  native-tail claim beyond the independently established support bound.
- Keep native `tail_length` separate from remaining EOS budget. Declick may
  publish finite8; SpectralCompressor may publish the phase-independent safe
  bound2N−1 (or a justified stronger bound) including startup exactly once.
  Confirm prior transitions/configuration cannot retain a larger old transform
  before publishing that bound.

## Required focused evidence

1. Red public final-marker reproduction before changes (inventory already
   demonstrates disabled Declick8 and dry SpectralCompressor2048 losses).
2. All supported FFT sizes, mono/stereo/multichannel, T=1,H−1,H,H+1,N−1,N,N+1,
   plus all source phase offsets through at least one hop. First/final impulses
   separately; don't silently relax existing latency assertions.
3. Dry exact delayed identity; independent f64 direct DFT/WOLA using specified
   Hann windows and analysis origins at unity gain; nonlinear wet/delta/adaptive
   output vs a separate zero-padded process reference. No production detector
   coefficients reused in the independent transform oracle.
4. Partitions1/7/113/hop/large, drain capacities1/mixed/oversized, zero/noncomplete
   progress, completed repeat, invalid-call transactionality, reset/reinitialize
   and the explicit post-drain control policy.
5. Enabled Declick clicks/steps/bursts crossing the original endpoint, linked
   channels and bypass transitions; full eight-frame edge interpolation check.
6. Explicit cold allocator AND deallocator counters for first process/drain,
   repeated drain, reset and first post-reset drain. Focused package tests,
   rustfmt and strict Clippy only after source stabilizes.

No implementation has started. Review this bounded scope and support equations
before production changes or expanding to other restoration families.
