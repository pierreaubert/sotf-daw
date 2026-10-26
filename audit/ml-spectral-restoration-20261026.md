# Spectral compressor restoration: native integration checkpoint

Issue #10, plugin 39: an explicit learned restoration target mask, with latency
and artifact comparisons against classical compression. Implementation is in
progress. This checkpoint establishes the native boundary and shared evaluator;
it does not establish restoration quality or production admission.

`sotf-restoration-mask` extracts the expander's existing fixed evaluator into a
small library. Both DSP crates depend on it only through `qa`. The expander's
public model/feature/counter paths are compatibility reexports. The model format,
coefficients, features, smoothing, fallback and finite-EOF behavior are retained.
The compatibility format remains `sotf-expander-denoising-mask-v1` rather than
silently presenting the same trained asset as an independently trained model.

The spectral compressor has an explicit `configure_restoration` operation before
audio, requiring initialized mono/stereo FFT1024. Default compression has no
model. A validated optional mask can replace only synthesis gains; detector,
temporal envelopes, adaptive threshold and tonal/transient histories continue
independently. Learned gains are linked across channels from maximum FFT
magnitudes. Uncertain/missing/invalid evidence, DC/Nyquist and unsupported clocks
retain the configured classical synthesis gain, including its channel-link and
target-mode behavior. Native dry/wet and delta monitoring remain post-stages.
The prepared QA mode requires finite input bounded by64 transactionally; ordinary
native input policy is unchanged.

The shared fixed model has six causal normalized features, eight tanh units and
one sigmoid Wiener amplitude ratio. The bounded loader checks exact hash, format,
provenance, shapes, coefficient bounds and false admission. Selection uses the
existing heuristic 0.15/0.85 thresholds, which are not calibrated confidence.
Selected amplitudes remain [0.1,1] with 5/30ms hop-rate smoothing and30ms feature
history. Supported learned clocks are44.1/48/96kHz. No runtime file loading,
hashing, allocation, locking or model replacement occurs during processing.

With FFT1024, both classical and restoration paths retain1024 samples of native
latency; the model adds zero lookahead. The ordinary default FFT2048 remains
unchanged. EOF freezes learned history, decisions and source counters while
classical finite continuation proceeds. Existing finite support remains through
the last source-containing window. Reset and reinitialize clear model history
without silently removing the explicitly configured asset.

New native tests exercise exact missing/uncertain/unsupported-clock classical
fallback, mono/stereo, three supported clocks and192kHz fallback, all three target
modes, adaptive history independence, callback/drain partitions, reset/reinitialize,
full-output energy bounds, preparation guards, transactional invalid input and
process/reset/drain allocation/deallocation guards. Existing expander and spectral
regressions remain required because the evaluator extraction affects both QA paths.
Command records are under `audit/artifacts/ml-spectral-restoration-20261026`.

The combined native suites pass218 tests (166 expander,52 spectral compressor).
After isolating the QA hook from the unchanged production gain branch, the
52-test spectral suite passes again. Shared/DSP strict Clippy, default-feature
builds, edited Rust formatting and replay-script Ruff/format checks pass.
The rebuilt expander renderer replays all39 frozen waveform outputs, gain traces
and native metadata byte-for-byte/field-for-field identically. The model bytes
are unchanged. This verifies extraction compatibility; it is not new quality
evidence for the spectral compressor.

The frozen expander candidate is available for subsequent cross-plugin waveform
qualification. Its severe clean-material failure still applies; it is not an
admitted spectral restoration model. Existing expander quality figures cannot
prove spectral-compressor quality because uncertain bins use a different
classical algorithm. Native renderer/conditional WOLA comparisons, clean damage,
noise leakage, onset/modulation/spectral-artifact diagnostics, equal-attenuation
controls and runtime/heap budgets remain required for this consumer. Recorded
material, calibrated perceptual acceptance and cross-platform DAW tests remain
outstanding across the program. No production selector or admission is introduced.
