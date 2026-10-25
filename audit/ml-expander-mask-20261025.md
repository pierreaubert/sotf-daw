# Explicit expander denoising: native integration checkpoint

Issue #10, plugin 35. The ordinary dynamics mode remains the default. A separate
QA-only configuration can select learned spectral synthesis gains in the native
1024-point, 256-hop dual-Hann expander. The canonical single-band factory alias
still forces time-domain dynamics; no factory or production parameter changed.

This checkpoint implements the native boundary, not a trained quality result.
There is no trained or admitted model asset yet. Tests use constant untrained
networks to force confident selection or uncertainty. Such fixtures do not
establish denoising accuracy or measured musical/noise separation.

The bounded model contract is six normalized causal features, eight tanh hidden
units, and one sigmoid target amplitude ratio. Coefficients are fixed arrays,
finite and bounded by 16; JSON is at most 64 KiB with required SHA-256 identity,
exact format/provenance and false deployment admission. Features use linked
maximum channel magnitudes, Hann amplitude normalization, local spectral
contrast/flatness, 30 ms power history, normalized bin and broadband level.

The explicit mode is prepared on the control thread before audio, in initialized
mono/stereo spectral processing. It supports learned inference at 44.1/48/96 kHz.
Missing models, unsupported clocks, invalid evidence, uncertainty (0.15–0.85),
DC and Nyquist use classical synthesis gains. Confident gains are bounded to
[0.1,1] and smoothed at hop rate with fixed 5/30 ms attenuation/recovery constants.
This replaces synthesis gain rather than multiplying the classical gain; per-band
bypass, inactive and solo decisions retain priority. Classical gate, hold and
attenuation histories update independently and never consume learned gains.

Existing 1024-sample spectral latency and finite overlap support are retained;
the model adds no lookahead. EOF freezes learned gains, features and counters
while classical finite continuation proceeds normally. Reset and reinitialize
clear model history/counters while retaining the explicitly configured model.
Configuration cannot replace a model after input until reset.

Focused command logs are in `audit/artifacts/ml-expander-mask-20261025`.
Tests compare complete output against exact classical fallback across three
supported clocks and 192 kHz, mono/stereo, different callback and drain sizes;
verify independent classical envelopes; and check reset/reinitialize identity,
latency, feature/model domains, and zero allocations/deallocations in prepared
processing, reset and frozen drain.
An independent full-complex float64 FFT/dual-Hann overlap-add oracle also
reconstructs the constant-mask complete stream, including startup, hop-rate
smoothing, frozen EOF gains and 1024-sample latency, within 2e-7 per sample.
It checks the complete output energy against the no-boost input energy bound.
Final focused native tests, strict all-target QA Clippy, edited-file formatting
and the default-feature build passed. The model type cannot be deserialized
publicly; the bounded identity/provenance validator is its construction boundary.

Remaining implementation: train and freeze a source-disjoint generated-music
candidate; independent Python/native feature/network parity; actual native output
renderer and independent conditional WOLA references; clean damage, noise leakage,
onset, pumping and musical-noise metrics against classical dynamics and matched
attenuation controls; model/feature distribution shifts; measured memory and
callback budgets; then recorded material and cross-platform DAW validation.
Product controls and click-free activation require an independently demonstrated
quality benefit. No production admission or full audit completion is claimed.
