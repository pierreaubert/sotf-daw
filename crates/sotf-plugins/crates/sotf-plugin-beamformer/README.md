# sotf-plugin-beamformer

Beamformer plugin — MVDR, superdirective, and GSC beamformers.

## What It Does

Combines signals from multiple microphones to focus on sound from a specific direction while rejecting noise and interference from other directions. Supports three beamforming algorithms for different use cases.

## Features

- **MVDR beamformer**: Minimum Variance Distortionless Response with scale-independent look-source protection and adaptive interference covariance
- **Superdirective beamformer**: Maximum directivity for diffuse noise fields
- **GSC beamformer**: Generalized Sidelobe Canceller — adaptive interference rejection
- **Configurable steering**: Point the beam in any direction

## Array and realtime contract

The exposed geometry is a 2–8 microphone linear array. Looking from above:

```text
               0° broadside
                    ↑
mic 0 — mic 1 — … — mic N-1  → +90° endfire
                    ↓
              180° broadside
```

Microphone count, spacing, steering angle, and algorithm are construction-time
graph state. Change them by rebuilding the plugin; live setters reject them so
the audio thread never allocates, replans FFT state, resets adaptation, or
changes latency unexpectedly. Serialized algorithms use `"MVDR"`,
`"Superdirective"`, and `"GSC"`; legacy indices 0, 1, and 2 are accepted on
load.

MVDR learns covariance only when a frame is not dominated by the configured
look direction. This estimator is normalized by frame energy, so its decision
does not depend on microphone gain or absolute FFT scaling. GSC aligns every
microphone before both fixed summation and blocking. It smooths spatial coherent
and total power over 10 ms, freezes learning when the coherent fraction reaches
0.65, and fades learned cancellation toward fixed summation over 5 ms. This also
protects a target arriving after prior noise learning. All timing uses the
prepared sample rate. Unusable evidence leaves recursive detector state intact.

MVDR and superdirective report 512 samples of STFT latency. GSC reports the
ceiling of its maximum fractional steering-compensation delay. All warmed
processing paths are allocation-free.

MVDR normalizes solved weights with wider arithmetic and validates every weight
before installing a frequency bin. Quiet covariance decay therefore preserves
finite weights; failed normalization uses the existing steered delay-and-sum
fallback. GSC retains adaptive weights and reference histories in f64 so finite
f32 input cannot overflow a blocking reference or its squared power. Its public
audio format remains f32; finite output beyond that range saturates at the
sample boundary, and invalid arithmetic cannot poison retained weights. This
changes rounding rather than promising bit-identical GSC output.

MVDR keeps its ordinary f32 detector and covariance arithmetic. Overflowed
detector powers are retried in f64; unusable FFT frames do not update learned
state. Covariance updates are validated as complete frequency bins, with a
wider retry when an intermediate product overflows. An unrepresentable candidate
leaves the previous bin intact, allowing subsequent ordinary learning to resume.
Representable large covariance is retained and decays at the existing 0.95-per-hop
rate. Recovery time therefore depends on overload magnitude; it is not immediate.

## Weight robustness

MVDR and superdirective weights now retain a conservative white-noise gain
floor of 0 dB for physical unit-magnitude steering vectors. When squared weight
norm exceeds one, shrink its component orthogonal to steering toward steered
delay-and-sum. This preserves the look response and bounds gain, delay and
geometry perturbation error by the perturbation's Euclidean norm. Existing
weights below the limit retain their exact values. The calculation uses wider
arithmetic and existing storage; MVDR weight updates remain allocation-free.

This trades diffuse-noise directivity for microphone-error tolerance. The
independent array fixtures measure up to 4.02 dB less diffuse directivity than
the original loaded solution, while remaining no worse than delay-and-sum.
They cover 189 fixed frequency/geometry/angle points, 567 gain/delay/position
perturbations and 72 learned MVDR close-interference points. GSC separately bounds total microphone-space FIR energy at one, scaling only
its blocked adaptive component. Its calculation uses an exact blocking-Gram
identity in O(taps × references), with existing storage. The corresponding frozen-weight sensor-white-noise gain
is bounded even after noise learning; coherent target protection
selects fixed summation when mismatch could leak a target into learned references.
This conservative fallback can forgo adaptive noise reduction in target-dominated
mixtures.

## Moving sources and microphone mismatch

`tests/audit_moving_sources.rs` exercises all three public algorithms in 162
rate/microphone/scene combinations: 16/48/96 kHz, 2/4/8 microphones, static and
±3° moving targets, ±0.5 dB gain/±0.05-sample delay/±1 mm position mismatch,
seven-direction isotropic-field quadrature, mixed target/noise and noise followed
by a moving mismatched target. The independent continuous-time fixtures compare
bit-identical regular/irregular callbacks, exact declared transport latency,
reset replay and finite draining. Target distortion and mixture error are
reported separately relative to a nominal uniform-array target reference.

The reproduced 48 kHz/four-mic GSC mismatch failure improves from 0.61 dB to
55.49 dB target SNR. An eight-mic/96 kHz target after noise learning improves
from 3.30 dB to over 54 dB. Across the final matrix, isolated target SNR exceeds
19 dB for every algorithm (GSC exceeds 54 dB after noise learning); mixed-scene
error is no worse than uniform summation to numerical precision. These are
engineering multitone fixtures, not speech-corpus or perceptual guarantees.
Public motion uses broadside steering and 5 cm spacing; separate frequency-domain
matrices cover other spacing/look angles.

`tests/gsc_reference.rs` compares 168 scale/rate/microphone/delay combinations
against direct f64 time-indexed equations and explicit microphone FIR energy,
including reset and full-range saturation. Detector construction/reset/rate
changes and frozen detector/mix state during drain have independent unit checks.
The QA binary measures eight-microphone callbacks with active covariance and
adaptation rather than identical constant channels. Exact commands, baseline
failures and measurements live under workspace `audit/artifacts/plugin-audit-20261018/beamformer-*.json`.

## Stream boundaries

MVDR and superdirective start analysis with a half-window zero prefix and discard
negative-time synthesis. This reconstructs the first input sample without a
startup fade and retains the declared 512-sample latency. MVDR solves initial
weights but does not learn covariance from the synthetic-prefix frame. GSC
startup is unchanged.

Call `drain()` after the final input until completion. Spectral algorithms emit
all pending synthesis windows with fixed covariance and weights; GSC flushes its
fractional steering delay and adaptive FIR history with fixed weights. A short
MVDR stream can solve its initial dirty weights during drain, using the existing
covariance without learning from padding. This policy differs from sending
ordinary silent input, which continues adaptation.

For FFT size N=512 and final hop phase r in 1..=256, spectral drain emits
`2*N-r` frames. GSC emits `ceil(max steering delay)+31` frames for its 32-tap
adaptive FIR. Bounds include potentially silent padding. Drain accepts any
positive mono capacity and emits at most 256 frames per call; empty streams
complete immediately. Invalid capacity/rate requests preserve state. After drain
starts, new input or parameter changes require reset or reinitialization. Cold
and reset/drain paths allocate and deallocate nothing.
Native host tail metadata reports a conservative 1024 output frames for the
spectral algorithms and the prepared maximum steering delay plus 31 frames for
GSC. This bound includes physical delay once and stays constant through ordinary
processing, drain and reset; reinitialization updates it for the new clock.
An unclocked, zero-rate instance reports `Unknown`. Ordinary zero input continues
adaptation and still becomes exactly silent within this bound.

Finite extreme input can still overflow the internal f32 FFT. Invalid spectral
synthesis samples are replaced by zero at the output read-and-clear boundary;
ordinary finite samples, latency and support are unchanged. This suppresses
overload artifacts and prevents nonfinite public audio, but does not reconstruct
the correct waveform beyond the internal FFT range. Retained covariance remains
finite, so later ordinary adaptation can recover without reset. The finite-tail
bound concerns stored audio and does not bound this adaptive settling time.

## Architecture

```
src/
├── lib.rs              # BeamformerPlugin
├── mvdr.rs             # MVDR beamformer
├── superdirective.rs   # Superdirective beamformer
├── gsc.rs              # Generalized Sidelobe Canceller
├── steering.rs         # Steering vector computation
└── params.rs           # Parameters
```

## Testing

```bash
cargo test -p sotf-plugin-beamformer
```

## License

Part of the SOTF (Sound of the Future) project.
