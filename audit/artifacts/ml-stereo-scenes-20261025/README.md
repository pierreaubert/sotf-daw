# Offline source-labelled Stereo Imager scene suggestions

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

Implemented a source-labelled regularized Gaussian discriminant classifier,
training/proposal CLIs, bounded model verification, source bootstrap/support
abstention, native render/reopen qualification and optional preset export. The
class-to-settings policy is supplied explicitly by the user. The classifier does
not learn preferred width, run on the audio thread or update a live plugin.
No default model is selected. The classical Stereo Imager remains unchanged.

The explicit synthetic model is in
`crates/sotf-plugins/models/stereo-imager-scenes/model.synthetic.json`, SHA256
`ce5eee04ac549ef193935d8599a3369c416308522a749bc1c45f303820e2ece6`.
It is626463 bytes. Training uses24 generated records grouped into eight sources.
No pretrained model, recordings or listening labels were obtained. The example
binds the previously qualified local native renderer SHA256
`a5cb532aef4e33d15c5b7ead22c569258bfb9e9220a0f078079935857be7fd6b`.
The training CLI reproduces the same model bytes as the fresh evaluation fit.

Sixteen gain-invariant spatial/band/envelope features use full20ms periodic-Hann
frames and10ms hops; the final real frame covers actual EOF without padding.
Models use equal total source weight, scale floor0.05, equal class priors and
20% covariance shrinkage toward identity. Fifty whole-source bootstrap draws
refit normalization and all classifier parameters. Whole-source holdout admission
requires no confident errors and coverage>=0.25 overall and for each class across
its sources. Inference requires bootstrap fifth-percentile score>=0.95 and nearest
same-class standardized RMS feature distance<=1.5. These scores are heuristic,
not calibrated probabilities or confidence about listener preferences.

Native qualification requires exact executable identity, current host-valid
controls, actual sample peaks<=0.999, RMS level change within+/-6dB, original mono
energy fraction>=0.01 and mono-fold relative RMS error<=1e-6. Exact JSON preset
reopen at4096 versus193-frame callbacks must differ by at most1e-7. Both renders
append4096 declared observation zeros, without claiming a complete finite IIR
tail. There is no gain normalization, clipping repair, true-peak ceiling or
perceived-loudness matching. Only successful proposals export preset.json.

Fresh evaluation contains24 procedural source/scene examples in eight held groups
at three correlated gains (72 labelled conditions), plus five structural and
intended-use cases. All72 labels are correct for both the full model and the
one-feature nearest-centroid baseline: no classification advantage is established.
Native checks admit50 proposals, including two identical-audio intent cases.
Twenty-four loud variants abstain for headroom; silence abstains for excitation,
one silent channel for labelled-support distance, and pure antiphase for mono
energy. Accepted reopens are bitwise identical. These are not77 independent
recordings or human scene/preference observations.

The same wide-effect PCM labelled as desired width versus unwanted side spill
receives identical settings. Its chosen policy reduces side energy3.3736174574dB.
That can be undesirable for the first intent. Classification cannot establish a
listener's intended policy from identical observations; output remains a manual,
optional suggestion.

Independent analysis uses SciPy STFT and a cross-spectrum identity for features,
prefix-energy envelope calculations, Cholesky covariance inversion and the linear
discriminant scoring form. It reconstructs100 unique audio identities,459 grouped
closed-form fits,24 holdout decisions,96 baseline decisions and the fresh native
metrics/admission. Maximum feature error8.972e-15, parameter error2.316e-13,
score/support error1.410e-14 and native-metric error3.997e-15. Training and held
audio hashes are disjoint; gain variants share source groups, and the intended-use
pair explicitly shares one PCM identity. These checks establish numerical and
functional consistency, not real-scene accuracy or perceptual benefit.

Seven final Python tests pass, covering unequal source weighting, independent
normal equations, gain/EOF behavior, conflicting labels/group shortage, malformed
model/policy/provenance, native headroom/reopen, context/renderer fallback and actual
CLI output preservation. Successful fit and proposal CLI runs are retained, as
are missing-model/loud CLI abstentions with no preset export. A manually rounded
48144-frame smoke argument was correctly rejected against the48143-frame file
(`proposal-cli.*`); the verified command derives the exact retained frame count.
The initial unused-json lint occurred during core construction; the completed
loader uses it and final Ruff checks pass. No post-evaluation model, numeric
feature or admission policy was tuned against these fresh cases.

Raw PCM/native files and CLI outputs remain in
`/Volumes/home_tmp/tmp/ml-stereo-scenes-20261025`. Adjacent files retain session,
fit/evaluation/analysis, commands, exit codes, durations, source copies/hashes and
the successful proposal report/preset. Only generated model/data descriptions
are committed, with no audio redistribution. Real source labels and policy
evidence, stronger baselines, independent source/rate/duration/channel shifts,
calibrated abstention, listening and host/resource/cross-platform acceptance
remain open. This checkpoint does not complete Stereo Imager adoption or the
full ML audit.
