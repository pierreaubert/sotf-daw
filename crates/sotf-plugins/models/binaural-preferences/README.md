# Offline binaural HRTF preferences

`fit_binaural_preferences.py` learns a listener/session-specific categorical
ranking over **already auditioned** SOFA candidates. This optional offline tool
exports a manual native preset, selected SOFA snapshot and audition setup only
when statistical and actual native rendering checks pass. It does not change
active settings, interpolate a new HRTF, or add inference to head tracking.

There are no pretrained preference weights. Model utilities are fitted from
explicit pairwise listener observations, with a separate presentation-side bias
term. The shared `preference_ranking.py` core also serves crossfeed. Synthetic
functional checks pass; personalized listener benefit has not been established.

## Session and candidate identity

Use format `sotf-binaural-preferences-v1` and declare `data_origin` as either
`listener_observations` or `synthetic_validation`. Context records listener,
headphones, headphone EQ and session identifiers; sample rate, input layout,
FFT size, diffuse-field EQ, fixed zero head pose, `overall_preference` criterion,
and the actual native renderer SHA256. Other perceptual criteria need separate
sessions/methods rather than reinterpreting an overall preference score.

Each candidate contains a unique `id` and exact `parameters` fields `sofa_file`
and `sofa_sha256`. Each programme contains its whole-source `id`, raw
frame-interleaved float32 little-endian `audio_file`, `audio_sha256`, `frames`,
and `post_gains_db` mapping every candidate ID to its actual audition trim.
Paths resolve relative to the working directory. Candidate/audio snapshots are
bounded and hashed before any render. Different paths with identical SOFA bytes
are rejected, as are duplicate programme byte identities. Different metadata
with effectively identical native audition waveforms is also refused.

Trials record unique `id`, `programme_id`, candidate IDs `a`/`b`, and `choice`
`a`, `b` or `tie`. Protocol declares blinded and randomized order, fixed head
pose, whole-source grouping, and maximum RMS mismatch in 0..0.25 dB. These
playback/listener declarations are self-reported; hashes and rerendering cannot
prove what a person heard. Keep whole recordings and all gain/EQ/noise variants
in the same source group. Distinct file hashes alone do not prove independence.

A complete generated schema example is retained under
`audit/artifacts/ml-binaural-preferences-20261025/sessions/consistent.json`.
Its choices and acoustic data are synthetic, and paths reference external audit
fixtures. Use the evaluation script to regenerate the functional examples.
Do not present synthetic choices as listener observations.

## Ranking, abstention and native qualification

The fixed ranking policy uses ridge 0.01, at least 8 programme groups and 32
trials, equal total weight per programme, and both presentation orders for each
observed pair. Full and every held-programme comparison graph must remain
connected. Two to twelve HRTFs are supported. Repetition cannot create extra
independent programme groups.

Two hundred programme bootstraps use seed 20261026. A winner needs at least
0.9 winner fraction, fifth-percentile utility margin above 0.1, at least 0.9
connected bootstrap fraction, leave-one-programme-out log-loss improvement at
least 0.01 over uniform choices, and absolute fitted side bias no greater than
0.5. These are engineering admission thresholds retained from the existing
crossfeed preference experiment. They need development listener calibration;
they are not population confidence or established HRTF-quality thresholds.
Ties contribute label 0.5; no separate tie-probability model is claimed.

The CLI renders **every** declared programme/candidate with the actual decoder,
full native tail and recorded trim. Room reflection order is zero, late reverb
is off, head pose is zero, near-field strength is zero, externalization is one,
no SRIR or database override is used. Native normalization remains unchanged.
It refuses output sample peaks above 1, RMS at/below -120 dBFS, mismatch above
the declared bound, or candidate pairs whose complete audition waveforms all
agree within 1e-7. RMS uses both ears and the full aligned output including tail.
This numerical matching is not perceived-loudness or headphone-SPL calibration.

On acceptance, `selected.sofa` is byte-identical to the verified candidate and
`preset.json` contains the native decoder configuration with its explicit path.
Reopening the selected snapshot at callback size 17 must reproduce the original
callback-193 output within 1e-7 and the same native parameters. The audit observed
maximum 1.118e-8. `audition_setup.json` retains programme-dependent trim and
context separately: the native preset alone does **not** apply those trims or
establish one global playback gain. Moving the folder requires updating its
absolute SOFA path. No model inference runs during decoder processing.

On abstention, `report.json` records reasons and the declared fallback ID;
no selected SOFA or preset is exported, and active playback remains unchanged.
Invalid identity/files fail before fitting; native preparation/reopening errors
abort export. Use a new output directory: existing data is never overwritten.
The Python `fit()` function is statistical-only; use `run()` or the CLI for
native-qualified exports.

## Reproduce

```sh
cargo build --release -p sotf-plugin-binaural --features qa --bin qa-binaural-render
python scripts/ml/fit_binaural_preferences.py --session listener.json --renderer /absolute/path/qa-binaural-render --output new-result
python scripts/ml/evaluate_binaural_preferences.py --renderer /absolute/path/qa-binaural-render --work new-synthetic-work --output evaluation.json
python scripts/ml/analyze_binaural_preferences.py --evaluation evaluation.json --output analysis.json
```

Python runtime dependencies are NumPy and SciPy; generating synthetic HDF5 SOFA
cases additionally requires h5py. The native binary is the pinned repository
build, not a replacement Python renderer. Candidate paths/snapshots and the
renderer executable must stay unchanged during a run. Limits are 32 programmes,
4 million aggregate PCM samples, at least 250 ms per programme, 16 MiB per SOFA,
2 MiB session JSON, and native geometry limits. These bound inputs, not parser
expansion, total process RSS or realtime deadlines. Raw native evidence is
retained externally under `/Volumes/home_tmp/tmp/ml-binaural-preferences-20261025`.

## Evidence and remaining work

Six synthetic cases admit stable candidate and baseline preferences and abstain
on ties, heterogeneous preferences, side bias and level mismatch. Six HRTF
integration tests also exercise hash/context/source rejection, equivalent
transfers and CLI output preservation. Nine crossfeed tests pass, including a
new held-programme graph regression; four prior full reports match exactly.
Independent BFGS reconstructs 1200 bootstrap fits and verifies all 144 native
auditions, hashes, metrics and accepted reopening results.

These functional cases use three invented FIRs and eight synthetic noise
programmes. They provide no evidence of human localization, externalization,
HRTF suitability, generalization to head motion or listening benefit. Real
listener data, held-out directions/programmes, uncertainty calibration and
host/headphone/resource validation remain explicit ledger requirements.
