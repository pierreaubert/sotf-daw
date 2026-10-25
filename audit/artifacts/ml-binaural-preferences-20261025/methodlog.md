# Method basis

- Primary study: [A Perceptual Evaluation of Individual and Non-Individual HRTFs: A Case Study of the SADIE II Database](https://www.mdpi.com/2076-3417/8/11/2029). Publisher search text describes preference testing on 16 re-recruited listeners with brightness/richness/externalisation/preference judgments. Full publisher retrieval returned HTTP429 this pass. This supports separating perceptual criteria; it does not validate this fitter, thresholds or synthetic cases.
- Primary abstract: [Stimulus-Dependent HRTF Preference, AES2010](https://aes.org/publications/elibrary-page/?id=15690). Authors evaluate selected HRTFs with speech/music and spatial criteria. Only the abstract/search text was accessible. Inference: group whole programmes and test preference beyond repeated excerpts; no claimed replication.
- [H5py quick-start documentation](https://docs.h5py.org/en/stable/quick.html) supplies synthetic HDF5 dataset creation API usage. Native compatibility was verified by actual decoder rendering; HDF5 validity alone was insufficient.

Engineering choice: reuse the existing categorical Bradley-Terry preference
model, programme bootstrap and leave-one-programme validation, now shared with
crossfeed. No new supervised target or pretrained model weights were trained.
Fixed policy thresholds come from the prior crossfeed prototype, not HRTF
population calibration. Dedicated listener development and frozen held-out
validation remain required.

Synthetic acoustic generator: three invented causal 128-tap HRIRs over four
horizontal directions, 48 kHz, different delayed spectral taps. Eight independent
noise signals use seeds20531026..20531033, 12001 stereo frames, scale0.025.
Post-render per-programme gains target -36 dBFS full-tail stereo RMS. Choices are
fixed synthetic utility rankings with ties/side-bias/group-heterogeneity controls.
No measured human dataset is used or implied. No new audio/SOFA assets or weights
are committed; all are generated locally from the source case script.
