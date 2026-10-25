# Beamformer spatial noise-mask prototype

This is an unadmitted QA model for MVDR noise covariance estimation. It estimates
the noise-energy fraction of a target-direction/interference mixture from one
multi-microphone spectrum. It does **not** recognize speech identity. No recorded
speech, measured room or physical microphone data enters training. There is no
production factory parameter, default model or automatic activation.

The fixed 6→8→1 tanh/sigmoid network uses look-direction energy fraction, mean
steering-aligned pair phase coherence, minimum/maximum sensor power ratio,
reference/total power ratio, normalized frequency and microphone count. Features
are causal and invariant to global gain/phase within the bounded excitation
domain. Real/imaginary f32 evidence is promoted to f64 for feature/inference
arithmetic. Native features reject nonfinite, silent/overloaded or nonphysical
unit-steering evidence. Model JSON is limited to 64 KiB, exact shape/policy and
finite coefficient magnitude ≤16, with an explicit SHA256 supplied by the caller.

`train_beamformer_noise_mask.py` generates 64 training, 16 validation and 32 final
scene seeds, with 256 examples per scene. Each scene fixes array geometry/rate,
look/interference angles and generates complex target/interference and sensor
noise components. The target is their oracle noise-energy fraction. Groups use
equal example counts; seeds and feature/label hashes are retained. The model and
fixed 0.85 decision threshold are written before final scenes are generated.
The final near-interference, sensor-gain and reverberant-target variants reuse
the 32 final seeds and are correlated shift probes. They are not independent
recording datasets. The fixed 300-iteration L-BFGS-B fit reached its iteration
limit; its report does not claim convergence.

The native `qa` path maintains the original MVDR covariance and weights
independently on every frame. A parallel learned covariance uses
`beta=(1-alpha)*noise_mask` and retention `1-beta`. High predicted noise fractions
(≥0.85) permit weighted updates; low fractions (≤0.15) hold the bin. Uncertain or
invalid bins copy current native covariance and weights exactly. Missing,
disabled or unsupported models use the independently running native path.
Neural decisions never replace diagonal loading, distortionless normalization,
white-noise gain bounds or the native invalid-evidence handling.

The declared generated model context is linear 2–8-microphone arrays with
1–10 cm spacing, look angle within ±60°, and 16/48/96 kHz. Other production-valid
contexts use fallback. Installation/replacement allocates on the control thread
and is restricted to a fresh MVDR stream. All valid callbacks, reset, inference
and drain are allocation/deallocation guarded. There is no worker queue or
future-context analysis: the model adds zero lookahead to the existing 512-sample
MVDR latency. The synthetic startup frame is excluded from model inference.
Drain freezes the last selected weights and all inference/covariance counters.
Reset clears both adaptive histories and counters while retaining enable choice.
Selection changes are immediate QA probes; prior overlap-add audio may remain
for up to two FFT windows. This is not a click-free product activation contract.

```sh
python scripts/ml/train_beamformer_noise_mask.py --output /new/training-directory
cargo build -p sotf-plugin-beamformer --features qa \
  --bin qa-beamformer-mask-render --release
python scripts/ml/evaluate_beamformer_noise_mask.py \
  --renderer /absolute/path/qa-beamformer-mask-render \
  --model crates/sotf-plugins/models/beamformer-noise-mask/model.synthetic.json \
  --work /new/raw-waveform-directory --report /new/report.json
```

Generated final mask MSE is 0.0373 matched, 0.0641 near-interference, 0.0518 sensor
gain and 0.1257 reverberant target. Confident noise errors on target-dominant
examples persist and increase under reverberant shift. The per-bin coherence
comparator in the training report differs from the native frame-level gate;
its mask MSE is not an audio-quality comparison.

Nine actual native waveform probes compare the model with classical MVDR at
2/4/8 microphones and 16/48/96 kHz, including target-only and shift conditions.
Steady output SNR changes range from −0.1612 to +0.3068 dB. Four probes improve,
four regress and the target-only probe is unchanged. That target-only output
has relative RMS error 1.51e-7 after the declared 512-sample offset. There is no
fitted delay, fitted gain, normalization or replacement limiter. Waveform results
establish no general neural advantage. Generated voiced/noise envelopes are not
recorded speech; the fixture uses linearly interpolated fractional propagation
delays and fixed broadside target, with limited acoustic realism.

The native/NumPy reference covers 35 feature/network cases: maximum feature
discrepancy 3.34e-16 and probability discrepancy 2.79e-8. Independent reconstruction
of final mask scores and 18 native waveform outputs agrees to floating-point
precision and checks hashes, prefix latency and frame counters. Six Python tests
and 103 native tests pass. The standalone local deadline probe measures 200
callbacks per route/rate at eight microphones, using an 80% callback-duration
budget declared before measurement. All nine probes passed locally; worst
neural callback was 0.310 ms against a 1.608 ms budget at 96 kHz. This does not
establish portable platform deadlines or complete heap memory accounting.

Evidence: [audit](../../../../audit/ml-beamformer-mask-20261025.md),
[manifest](manifest.json). Real multilingual speech and microphone/room captures,
unseen geometry/source motion and frequency-edge calibration, perceptual artifacts,
confidence calibration, full memory/platform deadlines and a reason to prefer
the learned route remain open. Production admission stays false.
