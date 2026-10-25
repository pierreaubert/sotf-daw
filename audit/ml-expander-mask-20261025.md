# Explicit expander denoising: native integration checkpoint

Issue #10, plugin 35. The ordinary dynamics mode remains the default. A separate
QA-only configuration can select learned spectral synthesis gains in the native
1024-point, 256-hop dual-Hann expander. The canonical single-band factory alias
still forces time-domain dynamics; no factory or production parameter changed.

The native boundary now includes a frozen synthetic candidate, separate from
production admission. Constant untrained networks still force selection or
uncertainty in boundary tests; the trained fixture has separate feature/network
parity and actual native waveform evaluation.

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

The frozen 65-coefficient candidate uses 32 training, eight validation and 16
final source seeds, with 256 uniformly sampled rows per source. The fixed
300-iteration optimizer stopped at its limit without convergence. The model was
written before final probes. Shift variants reuse final source seeds and are
correlated. Generated mono training at 48 kHz has explicit 44.1/96 kHz, noise-step,
tonal-noise, percussion, vibrato, clean-only, noise-only and level-shift probes.
No external audio or pretrained speech model was used. Identity:
`3b93c142cbcd5fc118bd2a075c6bca7d5bccc1dc94b9cc36338f9c9fd7899af9`.

Held matched mask MSE at 48 kHz is 0.03475 versus a simple flatness mask's 0.15055.
The flatness mask is not the native classical expander. Clean-only mask MSE is
0.58546, with 38.6% of sampled bins confidently attenuated despite desired energy
dominating. This exposes a serious distribution failure; no tuning used these
final results. Native/Python parity covers 18 rows including both spectral edges:
maximum feature error below 1e-12 and prediction error below 2e-7.

Thirteen fresh generated waveform cases produce 39 actual native outputs:
unity, classical ratio-four expansion and the explicit learned mode. They include
mono/stereo and 44.1/48/96 kHz, with separate clean/noise files. The learned mode
improves SNR in all 12 cases containing clean material against these particular
classical settings, by 0.53–4.00 dB. It usually attenuates noise less. Clean-only
component relative RMS error remains 0.473, so this is not an acceptable clean
preservation result. Noise-only SNR is deliberately undefined; residual attenuation
is reported instead. Actual per-hop/channel/bin gains are captured after native
selection. Independent float64 WOLA and mixture-conditional clean/noise components
match all outputs within 9e-8 per sample. Isolated stems are never reclassified.

`calibrate_expander_mask.py` adds an explicitly post-hoc diagnostic control:
classical/dry interpolation (or scalar classical attenuation), calibrated only
from source time 50–200 ms known noise lead, then frozen for the full waveform.
It does not optimize SNR or use later clean labels to select the scalar. This
is a comparison diagnostic, not preregistered admission evidence.
All 11 comparable noisy cases retain positive SNR differences against that
noise-lead-calibrated control, ranging 0.35–4.00 dB. Later full-waveform noise
attenuation remains different, so this does not establish equal-attenuation
superiority. Clean-only material cannot calibrate from noise and is excluded;
noise-only material has no defined desired-signal SNR.

Final focused results: 166 native tests, five Python tests, strict QA Clippy,
default-feature check, Ruff and Python format checks passed. Training replay
reproduced model and full training report bytes exactly. Native process/reset/
drain allocation guards remain active with preallocated trace storage. No model
or renderer rebuild occurred while waveform evaluation consumed them.

Remaining implementation: independent onset, pumping and musical-noise metrics;
confidence calibration and remedy for clean-material failure; complete measured
memory and callback budgets; then recorded material and cross-platform DAW validation.
Product controls and click-free activation require an independently demonstrated
quality benefit. No production admission or full audit completion is claimed.
