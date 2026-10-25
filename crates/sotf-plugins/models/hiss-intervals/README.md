# Optional Hiss interval review model

`fit_hiss_intervals.py` learns from explicitly labelled one-second examples.
`propose_hiss_intervals.py` returns review candidates and abstentions. It never
starts native capture, writes a profile/preset or changes stored/active profiles.
The existing explicit interval selection remains necessary. The CLI is offline;
the native Hiss crate also provides an optional background worker.

The Hiss crate also exposes `interval_proposals` behind the opt-in
`interval-proposals` Cargo feature. `IntervalModel::from_bytes` verifies bounded
model bytes, an explicit SHA256, policy and exact `IntervalContext` before use.
`evaluate` extracts features and returns review scores for exactly one second of
interleaved PCM. Both operations allocate and belong on a control/background
thread. Finite parameters that overflow inference arithmetic return an error.
No default model, preset field or automatic capture action is added.
Ten deterministic Python/Rust reference probes cover 44.1/48/96/192 kHz,
one/two/eight channels, and the odd 883-point FFT at 44150 Hz. PCM byte hashes,
all features, main/bootstrap scores, support distances and decisions match;
numeric comparisons use absolute tolerance `1e-10`. Out-of-context probes test
arithmetic only; public evaluation rejects their context. This is not additional
classifier quality evidence. References can be regenerated with
`python scripts/ml/hiss_rust_parity.py`; native validation is recorded in
`audit/artifacts/ml-hiss-rust-20261025`.

No default model is installed. `model.synthetic.json` is an explicitly named
functional example, SHA256
`bae4c80e241f85e98edda3d017bc48a964e9496d10cd436b58dad9fe978c32de`.
It was fitted by this repository's CLI from generated labels/audio, not obtained
as a pretrained model. It must not be presented as a real-material classifier.

## Label/session contract

Use `sotf-hiss-labelled-intervals-v1`, declare `data_origin` (`labelled_recordings`
or `synthetic_validation`), and `protocol: {"whole_source_grouping": true}`.
Context has exact sample rate, physical channel count, capture cutoff,
`capture_setup_id`, and `noise_definition: "unwanted_stationary_hiss"`.
Inference requires the same context, including recording/setup identity.

Supply16..256 exact one-second frame-interleaved float32 little-endian records.
Each contains `id`, `source_group`, `label` (`noise_only` or `programme`),
`audio_file` and `audio_sha256`. Gain/EQ/noise variants and excerpts of the same
source must share a group. At least8 whole-source groups and6 groups containing
each class are required, with at most32 groups. Grouping/labels remain
self-reported; different byte hashes alone do not prove independence.
Identical bytes under independent groups or opposing labels are refused, as are
opposing labels for features agreeing within1e-6. Repetition cannot create more
independent groups.

Session JSON is at most1MiB; aggregate training input at most32million samples.
Context rates are44.1..192kHz,1..8 channels, cutoff1..16kHz within0.45*rate.
Every channel must have one-second RMS>=1e-5; silence/near-silent channels abstain
rather than imply noise suitability. PCM must be finite within+/-32. Current
learned-quality evidence is48kHz mono; geometry support is not quality acceptance
throughout these ranges. Paths resolve from the working directory. The exact
synthetic schema is in the artifact's `training-session.json`; its paths reference
external generated data and can be regenerated from the case script.

## Features and fitting

Per-channel audio is normalized by its own interval RMS, excluding absolute
level from the learned features. Full20ms periodic-Hann frames use10ms hops,
without EOF padding. Features are mean log high-band flatness, high-band energy
fraction, dominant-bin fraction, envelope coefficient of variation, clipped log
95th/5th RMS ratio, mean/95th spectral flux and zero-crossing fraction. High-band
flatness uses raw bin powers; spectral ratios/flux use one-sided power with
interior-bin doubling (DC and even-N Nyquist once). Unlike native stored profile
bins, these ratios describe normalized spectral mass. The feature vector stores
channel minima and maxima for each of the8 features, preserving channel-order
and gain invariance. Features are not semantic proofs of unwanted noise.

Logistic fitting uses equal total weight per whole source. Feature scales have
floor0.05; ridge is0.01 on16 coefficients and1e-6 on the intercept. Fifty grouped
bootstrap fits use seed20711026 and refit normalization. At least48 must retain
both classes. Every held-source fold refits the complete model/ensemble/support
using only other source groups. Fit admission requires zero held-source
programme review candidates and noise coverage>=0.25.

A candidate's fifth-percentile bootstrap score must be>=0.95 and its nearest
labelled-noise RMS standardized feature distance<=1.5. These are fixed engineering
thresholds, not calibrated probabilities or a statistical risk guarantee. Unknown,
unadmitted, unavailable, silent or context-mismatched inputs abstain. Model files
are read once with a1MiB cap and explicit SHA256 before parsing; model format,
context, policy, shapes, finite values, positive scales and support are checked.
No silent pretrained-model fallback is supplied.

The classical comparator uses fixed flatness/HF-energy/dominant-bin/envelope
conditions, not learned weights. Both methods need meaningful source-level
validation. A successful conditional fit does not establish that it can identify
all legitimate noisy programme sounds.

## Proposal clock and commands

Offline proposals use nonoverlapping completed one-second windows. Frame times
start at the supplied file's frame0; `available_at_frame` equals interval end.
Incomplete EOF is unused, with no backward application or future interpolation.
Appending future samples leaves completed proposals unchanged. The native worker
uses the same completed-window rule with absolute host positions and epochs.

```sh
python scripts/ml/fit_hiss_intervals.py --session labels.json --output new-model
python scripts/ml/propose_hiss_intervals.py --audio input.f32le --frames 144000 --context context.json --model new-model/model.json --model-sha256 EXPLICIT_HASH --output new-proposals
```

Training writes a report and, only if admitted, model JSON. Proposal output is
only `proposals.json`; missing/invalid models produce abstentions and exit2.
Existing output directories are refused. NumPy/SciPy are needed; no model download
or neural runtime is added. Use the separate native capture tool only after an
explicit interval is selected, retaining its source/context identity. No capture
or profile publication is performed by this classifier's API or CLI.

## Evidence and semantic limit

Eight synthetic groups provide16 labelled training intervals. Six tests cover
independent optimizer agreement, gain/channel-order/prefix invariants, grouped
rejection, model verification, CLI fallback/output preservation and semantic
counterfactuals. Fresh procedural evaluation has16 noise and16 tonal programme
examples. Both learned and classical methods propose all16 noise and none of
the16 programme examples: no learned advantage is established. The existing
single-speaker /s/ development fixture is also declined by both; it is not a
multilingual/general speech result.

An unwanted hiss example and a **desired noise effect** with identical PCM get
the same review proposal. No audio-only classifier can infer the differing intent
from identical observations. Explicitly selecting that interval for a diagnostic
native spectral capture attenuates the desired effect by9.593dB. This is why the
proposal never silently learns programme material. The diagnostic deliberately
calls native capture separately; the successful proposal CLI writes only review
JSON and leaves profiles unchanged. There is no automatic-capture adoption.

Independent analysis reconstructs51 record features (50 unique audio identities),
459 BFGS fits including all source bootstraps/folds,16 held-source decisions,
35 evaluation decisions and the native consequence. Maximum feature discrepancy
is5.33e-15, parameter discrepancy4.74e-7 and score/support discrepancy2.19e-9.
Raw generated/native data remain under
`/Volumes/home_tmp/tmp/ml-hiss-intervals-20261025`. No audio is redistributed;
recorded /s/ is the existing attributed CC BY-SA3.0 development fixture and is
not used to fit the example model. Real grouped labels, stronger baselines,
calibrated abstention, full host/UI integration, listening, resource/deadline
and cross-platform validation remain open.

## Native worker and explicit application

After initialization, call `configure_interval_review(Some(model), max_age_frames)`
on the control thread. The model must match the current rate, physical channel
count and cutoff; its explicit setup identity must describe the recording chain.
`None` disables review. Configuration, replacement and destruction allocate or
join the worker and must never run on the audio callback. No model path or review
state is added to presets or the generic plugin factory at this checkpoint.

Feed actual source positions using `ProcessContext::with_sample_position` or
equivalent transport metadata. Repeated default position0 is a discontinuity,
not an implicit advancing clock. Complete nonoverlapping one-second intervals
start at the first source frame after preparation/reset/seek. Zero-length input
cannot complete a window; EOF drain never contributes synthetic audio. Audio
latency and native tail behavior remain unchanged.

Three preallocated source buffers and a two-slot queue bound retained PCM storage
to `3 * sample_rate * channels * 4` bytes (576000 bytes at48kHz mono;
18432000 bytes at192kHz/eight channels), excluding model/FFT working memory.
Only whole independent windows are submitted; the shared worker may supersede
older queued windows. Reported submissions therefore are not inference counts.
The audio callback does no FFT, hash, allocation, buffer destruction or waiting.
Result access uses the shared bridge's nonblocking try-lock and returns no result
on contention. This local functional contract is not a DAW deadline measurement.

`interval_review_status` omits previous-epoch, future, expired and dead-worker
results. Maximum endpoint age is explicitly configured from0 to ten source
seconds; it excludes the preceding analyzed second. Reset, transport jump, cutoff
or rate mismatch retires old results. Queue rejection and an occupied buffer pool
invalidate old proposals; an overloaded interval is discarded in full. Restoring
matching geometry starts a fresh epoch. Model failures/insufficient excitation
are explicit abstentions. No inference result calls `learn_noise`, clears a
profile or replaces one.

Every observation carries its epoch, source frame range, model SHA256 and exact
float32 little-endian PCM SHA256. After a person explicitly reviews a candidate,
`apply_reviewed_interval_profile(observation, original_dry_pcm)` can measure and
store that exact interval on a control thread. The caller must retain original
PCM; firing `learn_noise` after a proposal would capture later audio. Application
requires the current fresh candidate and matching full PCM digest, refuses active
capture/draining, preserves the profile-use flag and retires the token on success.
Rejected requests preserve the stored profile. The native measurement matches
ordinary explicit capture in both backends. This explicit method is never called
by the worker; identical audio with different intended uses still requires human
judgment. A UI and host configuration/review flow remain to be integrated.

Native evidence is in `audit/artifacts/ml-hiss-worker-20261025`. Tests include
in-flight reset, forced queue rejection, occupied-pool recovery, future/dead/stale
result refusal, geometry changes, allocation/deallocation guards, direct model
agreement, bitwise audio/profile preservation, EOF tails and explicit original-PCM
profile application. These establish local functional behavior, not real-source
classification accuracy or cross-platform acceptance.
