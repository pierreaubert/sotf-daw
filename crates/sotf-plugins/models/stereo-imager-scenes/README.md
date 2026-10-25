# Optional offline Stereo Imager scene suggestions

The classifier learns declared scene labels; the user supplies a complete native
Stereo Imager preset for each class. It can suggest that preset offline after
source-support and native audio qualification. It does not learn what width a
listener prefers, run in realtime, automatically update a plugin or install a
default model. Existing predictable width control remains unchanged.

`model.synthetic.json` is an explicitly named functional example trained from
24 generated one-second stereo records in eight source groups. Its SHA256 is
`ce5eee04ac549ef193935d8599a3369c416308522a749bc1c45f303820e2ece6`.
The three demonstration labels describe centered, diffuse and side-dominant
procedural signals. These are not recorded dialogue/music/ambience observations
or listening annotations. No pretrained weights or recordings were downloaded.

## Labels, features and fitting

Use `sotf-stereo-scene-labels-v1`. Declare exact context (`sample_rate`,
`channels:2`, `scene_definition_id`), data origin (`labelled_recordings` or
`synthetic_validation`), a native renderer SHA256,2..4 scene names with complete
eight-control `settings_by_scene`, and `protocol:{"whole_source_grouping":true}`.
Each of16..256 records declares `id`, `source_group`, `scene`, `frames`,
`audio_file` and `audio_sha256`. Raw audio is frame-interleaved float32
little-endian. Paths resolve from the working directory. Session JSON is capped
at1MiB and aggregate training audio at32million samples; at most32 groups are
accepted. Require at least8 source groups and6 groups containing each class.
Excerpts and gain/EQ/noise variants of a source must share its group. Grouping
remains self-reported; hashes cannot prove statistical independence. Identical
bytes under independent groups or conflicting labels are refused, as are
different labels for indistinguishable normalized features.

Feature input is1..10s,44.1..192kHz stereo, finite within+/-32 and at most4million
samples. RMS below1e-5 abstains. Normalize by one global RMS, preserving channel
imbalance. Full20ms periodic-Hann frames use10ms hops; a final real frame is added
when needed to cover the actual EOF without padding. One-sided spectral mass
doubles interior FFT bins, counting DC/even-N Nyquist once. Full/low(<250Hz)/mid
(250..4000Hz)/high(>=4000Hz) regions each provide side-energy fraction, signed
correlation and channel energy balance. Three relative band-energy fractions and
frame-envelope CV complete16 features. Unexcited bands have zero spatial features.

Training gives every source equal total weight, centers/scales each feature with
scale floor0.05, computes class means, and shrinks pooled within-class covariance
by20% toward identity. Class priors are equal. Fifty whole-source bootstrap draws
refit all normalization/means/covariance; at least48 must retain all classes.
Every leave-one-source-out fold refits that whole ensemble and labelled support
without the held source. Admission requires no confident held-source errors,
overall coverage>=0.25 and coverage>=0.25 for each class averaged over its sources.
The one-feature nearest-centroid comparator uses the same source folds and labels.

A prediction requires fifth-percentile bootstrap class score>=0.95 and nearest
same-class support RMS distance<=1.5 in standardized features. These fixed gates
are engineering abstention heuristics, not calibrated probabilities, statistical
risk bounds or evidence of preferred settings. Classification and desired width
are different targets; the explicit class-to-settings policy remains a user choice.

## Native export and fallback

Model files are read once with a1MiB cap and explicit SHA256 before validation.
Format, schema, labels, context, policy, parameter shapes/bounds, positive-definite
precision, scale floors, support and declared training identities are checked.
The renderer executable must match the model's declared digest. The example binds
the local binary in the manifest; another build/platform requires explicit new
qualification/binding, not silently replacing the digest.

Confident candidates render the original supplied PCM through the actual native
Stereo Imager at its sample rate. Controls use the host registry, including the
80Hz low-crossover minimum. Keep actual sample peak<=0.999, total RMS level change
within+/-6dB, original mono-energy fraction>=0.01 and mono-fold relative RMS error
<=1e-6. The same JSON controls must reopen a fresh native processor with max
sample error<=1e-7 at4096-frame versus193-frame callbacks. Both observations append
4096 explicit zero frames; these are not a complete finite IIR tail. No gain
normalization, clipping repair, true-peak protection or listener-loudness matching
is performed. These gates inspect these rendered clips, not arbitrary future audio.

Missing/invalid models, mismatched context/renderer, uncertainty, poor excitation,
native failures or failed audio gates produce no setting export. A neutral native
preset is included as a comparison/fallback reference; no running settings change.
Only an admitted proposal writes `preset.json` (`plugin_type:stereo_imager` plus
native parameters). Caller-owned output directories are refused. Native audition
data and a report are retained for review.

```sh
python scripts/ml/fit_stereo_scenes.py --session labels.json --output new-model
python scripts/ml/propose_stereo_scene.py --audio input.f32le --frames EXACT_COUNT --context context.json --model new-model/model.json --model-sha256 EXPLICIT_HASH --renderer /path/to/qa-stereo-imager-render --output new-proposal
```

Training admission does not establish native/perceptual acceptance; native checks
run again for each proposed clip. The fit CLI exits2 on failed admission. The
proposal CLI exits2 on abstention, with no preset file. NumPy/SciPy and the native
QA renderer are required. No neural runtime or inference dependency is added to
the realtime plugin.

## Current evidence and limits

Seven tests cover unequal source weighting, independent covariance equations,
gain invariance, final real PCM, grouped/conflicting-label refusal, model schema
and provenance checks, native headroom/reopen, context/renderer fallback and
successful CLI export with existing-file preservation.

Fresh procedural evaluation has24 source/scene examples in eight groups, each at
three correlated gains (72 labelled conditions), plus silent, one-silent-channel,
antiphase and identical-PCM intended-use probes. Both classifiers get all72 labels
right: no demonstrated classification advantage. Native checks admit50 proposals,
reject24 loud variants for headroom, and abstain on silence, out-of-support channel
imbalance and insufficient mono energy. All accepted preset reopens are bitwise
identical. An identical wide effect used with two different intentions receives
identical settings; the chosen policy attenuates side energy3.3736dB. That change
can be unwanted. This is why output is an optional user-reviewed suggestion.

Independent analysis reconstructs100 unique input identities,459 source-grouped
fits,24 held-source and96 baseline decisions, fresh scores, native metrics and
admission. Maximum feature error8.98e-15, parameter error2.32e-13, score/support
error1.41e-14 and native-metric error4.00e-15. The training CLI reproduces the same
model bytes. Evidence and raw data locations are in
`audit/artifacts/ml-stereo-scenes-20261025`. Real scene labels, stronger baselines,
source/rate/duration/channel shifts, calibrated abstention, listening and actual
host/resource/cross-platform acceptance remain open. This example is not a
production classifier or evidence of perceptual improvement.
