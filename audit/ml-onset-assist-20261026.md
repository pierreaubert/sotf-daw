# Transient shaper onset assistance: native boundary checkpoint

Issue #10, plugin42. The optional classifier must modulate bounded controls with
independent deterministic fallback. Implementation is in progress. This checkpoint
implements the native boundary and untrained test fixtures; it does not provide
a trained onset classifier, classification accuracy or production admission.

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

Tests use constant logits0/±10 to force uncertainty or bounded decisions. These
are explicitly untrained analytic fixtures, not generated-source training data.
Native tests check exact missing/uncertain/wrong-clock fallback, callback and
channel invariance, independent native envelopes, control/peak bounds, zero audio
tail, same-clock initialization continuity, explicit reset identity, rate-change
state preservation, invalid-window reentry, zero-attack sustain identity,
configuration guards and transactional invalid input. Process/reset allocation
and deallocation guards remain active.

Command records are in `audit/artifacts/ml-onset-assist-20261026`.
All60 native tests, strict all-target QA Clippy, the default-feature build and
edited Rust formatting pass. The tests include preserved zero-attack sustain
output and two-window invalid-energy fallback before classifier reentry.
Remaining: freeze a classifier trained on disjoint generated sources with explicit
causal event labels; independent Python/native feature/network parity; native
waveform and event-time comparisons, false-onset/missed-onset and automation
artifacts; confidence/level/noise/rate shifts; complete memory and callback/host
budgets; recorded/perceptual and cross-platform DAW validation; and product
selection/activation semantics. No production selector or trained asset is added.
