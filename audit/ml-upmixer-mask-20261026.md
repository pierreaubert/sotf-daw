# Upmixer source masks: offline generated-stem candidate

Issue10, plugin43. This experiment implements a learned direct/ambient source
mask and compares it to an analytic covariance mask. It does not replace the
existing native Upmixer's scalar vocal detector. No speech labels or identity
claims are attached to the model. Native integration and actual added latency
measurement remain required; production Upmixer behavior is unchanged.

The candidate uses six band-covariance cues: absolute L/R power imbalance,
signed real correlation mapped to[0,1], imaginary correlation magnitude,
complex correlation magnitude, normalized band frequency and log excitation.
Each eight-bin band has a shared stereo mask. The512-point periodic sqrt-Hann
analysis uses256-sample hops at48kHz. DC/Nyquist retain the analytic path.
The6→8→1 tanh/sigmoid network produces a bounded[0,1] mask. Missing or invalid
model coefficients retain independent analytic masks. This is a synchronous
offline reference; it establishes no worker freshness or realtime allocation
contract. Zero model lookahead in its metadata does not mean zero audio latency.

Training uses32 generated source seeds20295120–20295151, validation8 seeds
20296120–20296127, and final16 seeds20297120–20297135. Training retains every
other analysis frame, with no window-level split:49152 training rows and12288
validation rows. Source hashes include input, direct and ambient stems. The direct
source is a panned, enveloped harmonic signal; the ambient bed is independent
smoothed stereo noise. Targets use known stem powers, not pseudo vocal labels.
The sole fit uses bounded coefficients, soft-label cross entropy plus ridge,
one initialization and300 L-BFGS-B iterations. It reaches that iteration limit
without convergence. Final shifts are evaluated after freezing the model.

Model SHA256:
`2ca743740f6adeaa40738a82066c25f65d20633a9bc8a237692aa156818c6d6a`.
Asset: `crates/sotf-plugins/models/upmixer-source-mask/model.synthetic.json`.
All sources are project-generated, GPL-3.0-or-later; no recorded audio is used.
The report contains128 final cases across matched, hard-panned, correlated-bed,
antiphase-bed, direct-only, ambient-only, -40dB level and silence conditions.
Variants share seeds and are correlated; altered RNG draws in hard-pan cases
prevent treating every variant as a pure factor-controlled ablation.

Values below are means across16 sources. Relative RMS error compares the
estimated direct waveform against the actual generated direct stem. Leakage
measures the masked ambient component relative to its original energy; retention
measures the masked direct component relative to its original energy. Reporting
both avoids rewarding a model for muting sources.

| Condition | Learned/analytic mask MSE | Learned/analytic direct RMS error | Learned/analytic direct retention dB |
| --- | ---: | ---: | ---: |
| Matched | 0.00324 / 0.10551 | 0.644 / 0.746 | -4.48 / -3.86 |
| Hard pan | 0.01020 / 0.10505 | 0.801 / 0.693 | -11.47 / -3.17 |
| Correlated bed | 0.16307 / 0.93502 | 1.260 / 1.509 | -0.61 / -0.36 |
| Antiphase bed | 0.00531 / 0.00478 | 0.686 / 0.673 | -7.51 / -6.32 |
| Direct only | 0.08766 / 0.00582 | 0.0581 / 0.0587 | -0.48 / -0.54 |
| -40dB level | 0.00456 / 0.10551 | 0.653 / 0.746 | -5.23 / -3.86 |

Matched ambient leakage is-10.54dB learned versus-7.64dB analytic. On hard-pan
sources, lower leakage comes with severe desired-source loss: learned retention
is-11.47dB. Correlated-bed leakage is only-1.63dB despite better mask MSE than
the analytic rule. On ambient-only sources, leakage is-14.31dB learned versus
-9.67dB analytic. Silence has no valid evidence and retains zero output.
These findings do not support admission.

The report also records direct-component waveform damage, mask frame variation,
output hashes and reconstruction of the original mixture from estimated direct
plus residual. This residual identity is a bookkeeping check, not separation
quality. The known-stem-power mask is a comparison reference, not an attainable
ideal separator: even that mask has matched direct RMS error0.612. Common masks
cannot perfectly separate coincident complex components.

Tests check overlap-add unity against the original waveform, Fourier source
additivity, exact covariance extremes, gradient central differences, strict
hash/policy/coefficient loading and analytic fallback. A deliberate ambiguity
test gives identical stereo observations opposite intended source labels;
spatial covariance alone cannot resolve arbitrary semantic foreground intent.

Remaining implementation: native bounded source-mask inference and optional
selection, measured end-to-end extra latency, exact fallback/lifecycle/streaming
and heap contracts, leakage and artifacts through actual multichannel rendering,
calibration and stronger development data with a fresh final holdout. Recorded
music, listening, host contention/platform budgets and activation UI also remain.
The complete31-plugin/five-program ML audit remains in progress.
