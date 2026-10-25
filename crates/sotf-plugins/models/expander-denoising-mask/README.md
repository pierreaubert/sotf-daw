# Experimental expander denoising mask

This fixed 6→8→1 network is synthetic qualification material. Deployment admission
is false. Explicit native selection is gated by the expander crate's `qa` feature;
ordinary dynamics and the canonical one-band expander factory remain unchanged.

The target is a per-bin Wiener amplitude ratio from generated music/noise stems.
The six causal features are normalized power, local spectral contrast/flatness,
30 ms temporal power contrast, normalized bin and broadband power. Native linked
detection uses the maximum magnitude across channels. Training is mono at 48 kHz;
stereo and other supported clocks are explicit evaluation shifts.

There are 32/8/16 disjoint training/validation/final source seeds with 256 sampled
rows per source. Shift variants reuse final seeds and are correlated. The fixed
300-iteration optimizer reached its limit without convergence. No recorded music
or external training data was used; generated assets follow GPL-3.0-or-later.

Native inference uses fixed arrays and preallocated history. The existing
1024-point/256-hop STFT and 1024-sample latency remain; the model adds zero
lookahead. DC/Nyquist, uncertainty, missing/invalid models and unsupported clocks
retain independent classical gains. Confident gains remain in [0.1,1] with 5/30 ms
smoothing. EOF freezes learned gains, features and counters. Reset/reinitialize
clear history. Model installation is allowed only before audio or after reset.

Matched spectral error improves over a simple flatness mask, while clean-only
mask error is severe. Thirteen native waveform cases improve SNR versus the
chosen ratio-four classical settings but usually leak more noise; clean-only
damage remains unacceptable. This does not establish a deployment advantage.
Source-labeled onset/envelope diagnostics and residual spectral-isolation proxies
are recorded separately from perceptual acceptance. All 18 local callback budget
probes pass, covering mono/stereo, three clocks and classical/missing/learned modes;
this does not establish contended host deadlines or complete heap accounting.

See `audit/ml-expander-mask-20261025.md` and its artifact directory for frozen
source identities, parity, waveform/component errors, and post-hoc noise-lead
calibration. Training and evaluation entry points are
`scripts/ml/train_expander_mask.py` and `scripts/ml/evaluate_expander_mask.py`.
