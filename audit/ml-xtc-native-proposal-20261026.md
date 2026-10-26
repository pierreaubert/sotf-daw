# Native prepared XTC inverse: QA-only resource path

Issue10/plugin44. The measured horizontal-transfer model and inverse proposal
remain unchanged. A new `qa`-gated control-thread API validates and installs the
existing centered proposal into native XTC's frequency-domain matrix processing.
Production defaults, source modes, presets and the native analytical inverse
preparation algorithm are unchanged. The measured cancellation/gain failures from
`audit/ml-xtc-transfers-20261026.md` still prevent admission.

`PreparedProposal::from_json` verifies a1MiB byte bound, proposal SHA256 and an
independently supplied model hash, strict metadata/shape/geometry, false admission
and1025 finite2×2 complex matrices. Matrix rows are speaker outputs and columns
intended-ear inputs. DC/Nyquist must be real. It computes the largest singular
value of each matrix through its Hermitian Gram matrix and rejects operator gain
above4. A shared real scaling guard accommodates f32 rounding near the ceiling;
the prepared f32 matrix is checked again. This preserves column steering up to
coefficient rounding, rather than applying independent coefficient knees. It
does not modify native analytical column knees or spectral normalization.

Installation requires initialized44.1kHz/FFT2048 stereo, reset/no accepted input,
native synthetic source mode, centered±30-degree speakers at1.4m, no head offsets,
AutoGain disabled, room resources disabled and filter bypass off. Model loading,
validation, allocation and replaced-resource destruction happen on the control
thread. Wrong profiles, accepted partial input and draining instances reject
before filter generation or audio state changes. The explicit resource API has
no production UI selector.

Successful installation advances the existing publication generation, replaces
the accepted filter bundle synchronously, clears previous crossfade/pending
publication ownership and pins the QA resource. Old worker publications cannot
replace it. Changed parameters reject while the resource is pinned except for
the native aligned enabled/bypass control; same-value snapshots are no-ops and
do not start workers. Reset retains the resource and clears audio history.
Successful initialize prepares and restores the configured native source before
clearing QA selection. Failed initialization retains the accepted epoch through
the existing transactional preparation path.

The resource uses native STFT processing and finite drain; no learned worker or
additional audio buffer is introduced. Initial small-impulse tests observe onset
and peak at sample2048 for both source columns, matching the declared46.44ms
latency at44.1kHz. This is not a claim that frequency-dependent phase, measured
ear responses or arbitrary waveform timing match the ideal supplied-matrix
calculation. Frequency-domain gain bounds likewise do not certify true peaks.
Native linked output safety and fixed-latency bypass remain active.

The original model SHA256 is
`157b1f395717fcebb669d57e61450214823695f520dcef83545d35ce7c161248`;
the imported proposal remains the frozen JSON from the preceding measured
checkpoint. The loader preserves explicit identity binding; it does not infer
measurement provenance or model quality from a supplied hash alone.

The focused native suite reports269 passing tests and1 ignored pre-existing
transition-timing benchmark. The final proposal target passes9 tests after
adding two lifecycle cases. Tests cover strict loader/gain rejection, coefficient
ownership/rounding, callback1/257 identity, reset replay, process/drain/reset heap
guards, rejection after one accepted input frame, unsupported clocks, successful
native restoration, failed-initialize history retention and exact aligned dry
output. A non-symmetric constant matrix agrees with independently computed
delayed stereo within9.32e-10 absolute sample error. An actual filter worker is
paused after its generation check, then released after native initialization and
QA proposal installation; it cannot replace the accepted proposal or trigger
callback heap activity. This is a controlled ownership test, not a contended
host deadline measurement.

Strict all-target QA Clippy and the feature-disabled build pass. An initial
full-suite compile failed on a relative fixture path, and the first Clippy run
reported constant-sized chunk iteration in tests; corrected runs and first
failures are preserved under `audit/artifacts/ml-xtc-native-proposal-20261026`.

The subsequent rendered qualification is in `audit/ml-xtc-render-20261026.md`:
38 native waveforms pass independent WOLA parity, while224 measured-ear
projections retain severe fixed-filter mismatch failures. Remaining: improve the severe
desired-gain loss and negative separation on development data, then freeze new
final material. Complete true-peak, memory/contended-host/platform, human
calibration/listening and product activation validation remain open. Full
31-plugin/five-program ML scope remains in progress.
