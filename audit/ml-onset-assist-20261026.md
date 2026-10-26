# Transient shaper onset assistance: frozen synthetic classifier

Issue #10, plugin42. The optional classifier must modulate bounded controls with
independent deterministic fallback. Implementation is in progress. This checkpoint
implements the native boundary, a classifier trained on generated signals and
independent native/Python probability checks. The classifier fails important
held-out quality checks and remains unadmitted, behind the `qa` feature.

The existing native `initialize` contract is preserved: coefficients/time
constants update while audio envelope history remains. The new assist also
retains its history on same-clock initialization. Changing clocks drops a partial
analysis window, preserves physical previous-energy/control history, and primes
valid causal evidence again. Explicit `reset` clears both native and learned
histories, control smoothing and counters. Installation is allowed only before
accepted audio since reset, in initialized mono/stereo. Reinitialization does not
permit replacing a model without reset. Native defaults and production parameters
are unchanged; the entire model path is gated by `qa`.

The fixed6→8→1 tanh/sigmoid model validates bounded JSON, explicit SHA256, exact
format/policy/provenance, coefficient bounds and false admission. Six causal
features summarize linked amplitude level/crest, hop energy change, native
transient ratio, rectified-amplitude variation and fast-envelope/RMS contrast.
Native fast/slow detectors always run independently. No learned decision writes
their states, timing coefficients or sustain control.

Analysis uses rounded2ms hops (88/96/192 frames at44.1/48/96kHz) with no waveform
buffer or lookahead. Decisions become available only after their causal window;
the5ms attack-control smoothing time constant is additional control response,
not zero detection delay. Audio latency remains zero. Supported learned clocks
are44.1/48/96kHz. The uncalibrated confidence thresholds are0.1/0.9. Selected
attack multipliers stay[0.75,1.25], then the native attack amount is clamped to
[-1,1]. Existing sensitivity gating, sustain, output gain, mix, linked gain range
and native peak protection follow normally. Zero attack remains zero. The model
cannot synthesize audio or create a tail from zero input.

Missing, uncertain, silent, invalid and unsupported-rate evidence uses exactly
the native attack amount. Within a decision interval, classifier state is held
for at most one hop. An overloaded source amplitude immediately disables learned
selection; its completed window cannot supply previous-energy evidence to model
reentry. A following clean window reprimes that history. The prepared QA mode
transactionally requires finite source values bounded by64; model evidence is
restricted to unit-amplitude/statistic ranges. Ordinary native input handling
is unchanged.

Boundary tests use constant logits0/±10 to force uncertainty or bounded decisions. These
are explicitly untrained analytic fixtures, not generated-source training data.
Native tests check exact missing/uncertain/wrong-clock fallback, callback and
channel invariance, independent native envelopes, control/peak bounds, zero audio
tail, same-clock initialization continuity, explicit reset identity, rate-change
state preservation, invalid-window reentry, zero-attack sustain identity,
configuration guards and transactional invalid input. Process/reset allocation
and deallocation guards remain active.

Command records are in `audit/artifacts/ml-onset-assist-20261026`.
All61 native tests, strict all-target QA Clippy, the default-feature build and
edited Rust formatting pass. The tests include preserved zero-attack sustain
output and two-window invalid-energy fallback before classifier reentry.

The frozen model is `models/transient-onset-assist/model.synthetic.json` beneath
`crates/sotf-plugins`, SHA256
`781433352e2fe070fe640909b0ebe422e785cd2523b55b3606228ea53bd0c88c`.
All sources are project-generated, under GPL-3.0-or-later, with no external audio.
Training uses 32 seeds20291120–20291151, validation8 seeds20292120–20292127,
and final evaluation16 seeds20293120–20293135. Each source has approximately
two seconds of audio. Training has32000 analysis windows and192 positive labels;
validation8000 windows and48 positives. Labels identify the causal window that
contains each generated desired-event source frame. Noise bursts have separate
nuisance tags. Invalid/silent feature windows are excluded from classifier fitting
and probability metrics, while their missed events still count.

Features come from the actual native detector through `qa-onset-assist-render`,
not a Python envelope approximation. Training copies the executable before use;
its SHA256 is
`f4df9facef01cd3ebd1a3d68f8755be8522a85a26bce5c06ee59bc8dc8b2abec`.
The65 bounded coefficients use class-balanced binary cross entropy plus ridge,
one initialization and fixed300 L-BFGS-B iterations. The optimizer reached that
limit without convergence. No model or confidence threshold was selected on the
final groups. Class-balanced fitting does not calibrate probabilities at the
natural event density.

The model is frozen before final evaluation at44.1/48/96kHz and attack/noise,
level/density, late-window phase, steady-only and silence shifts. Shared seeds
across variants are correlated; branching RNG draws and different sample counts
mean these are not pure single-factor attack or resampling ablations. Native
probabilities match independent NumPy inference within2.98e-8 across144032 valid
held-out windows. The committed17-row parity fixture also passes in Rust.

| Final group | Event recall within6ms | False-positive analysis windows/minute |
| --- | ---: | ---: |
| Matched48kHz | 1.000 | 11.25 |
| Matched44.1kHz | 0.990 | 31.87 |
| Matched96kHz | 1.000 | 16.87 |
| Slow attack | 0.698 | 16.87 |
| Nuisance noise bursts | 0.865 | 86.23 |
| Steady only | Undefined: no events | 234.33 |
| Level shift | 1.000 | 0.00 |
| Dense events | 0.911 | 7.50 |
| Late-window phase | 0.979 | 15.00 |
| Silence | Undefined: no events | 0.00 |

Decision time is the last sample of the completed causal window, not its exclusive
end index. Matched48kHz median delay is0.8125ms; late-window median is2ms.
Late-window strict positive-window recall is only0.0521, despite high6ms event
recall, because many decisions arrive in the next window. False-positive rates
count selected analysis windows outside the6ms event interval, not clustered
perceptual false-onset events. The fixed-feature rule in the report is a detector
comparison, not a native shaped-audio quality reference. Steady-only false-positive
rate is234.33 versus29.99 for that rule, and slow-attack event recall is0.698
versus0.729. These failures prevent admission.

Seven Python tests include independent finite-difference gradient checks, causal
timestamps, source identity, strict loading and native integrations. Actual trained
shaping at attack60/sustain20 is bit-identical across callbacks1/64. Rejected hashes
and unsupported192kHz evidence preserve actual native shaping. Main classifier
corpora use zero attack/sustain and verify exact input identity; their classifier
scores do not establish any audible benefit.

An additional72 actual waveform pairs use fresh seeds20294120–20294123 across
matched, slow-attack, nuisance-burst, steady-only, late-window and silent sources,
with attack/sustain pairs(60,20),(-60,-20),(0,20), mono48kHz. The model and renderer
remain frozen. `scripts/ml/evaluate_onset_audio.py` records waveform hashes,
first10ms desired/nuisance-event changes, outside-event changes, residual first
differences and counters in `audio-report.json`. Every zero-attack pair is
bit-identical to native sustain processing; silent output remains zero and all
peaks obey the native bound. At(60,20), relative RMS change versus native is
0.0506–0.0526 for matched sources and0.0113–0.0172 for steady-only sources.
This is unwanted control activity on material with no desired event tags. These
descriptive differences do not identify an ideal shaped waveform or establish
perceptual benefit. No threshold, coefficient or policy is changed after seeing
these cases. Raw requests/features/outputs are in the external scratch directory
`/Volumes/home_tmp/tmp/ml-onset-audio-20261026` and can be regenerated using the
recorded command. Source families remain generated and correlated across variants.

Remaining: improve training/calibration using development sources and freeze a
fresh final holdout before reassessment; independently validate waveform quality,
automation artifacts, stereo and difficult material; complete memory and
callback/contended-host budgets; recorded/perceptual and cross-platform DAW
validation; and product selection/activation semantics. No production selector is
added. Full31-plugin/five-program scope remains open.
