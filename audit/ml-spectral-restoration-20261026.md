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

## Native waveform and artifact qualification

`qa-spectral-restoration-render` records actual output, the native Hann window,
and every per-channel/bin synthesis gain, including frozen EOF continuation.
Input/request/model bounds, fixed source/trace counts, complete drain and native
process/reset/drain allocation guards are checked. The shared Python render
protocol checks complete typed parameter echoes and fixed FFT1024/H256 geometry.
Expander and spectral-compressor parameter profiles remain explicit. Independent
float64 WOLA and conditional clean/noise decomposition use the mixture's gain
trace; isolated sources are never reclassified.

The frozen model is still SHA256
`3b93c142cbcd5fc118bd2a075c6bca7d5bccc1dc94b9cc36338f9c9fd7899af9`.
There is no new training or asset admission. The initial probe set reused some
source-component seeds from the earlier expander evaluation. Those reports and
raw outputs are retained under `overlapping-consumer-seeds-*`; they did not
overlap model training. `disjoint-source-plan.json` records the mechanical seed
repair before generating the replacement. The model, controls and13-case scenario
matrix were unchanged. The primary first seed is20289120 with1000 channel offset.
All18 resulting source seeds are verified disjoint from this model's training,
validation/final groups and the earlier expander waveform components. These are
generated-family probes, not recorded or out-of-family generalization evidence.

The primary13 cases produce39 outputs: unity, classical compression and learned
restoration; mono/stereo at44.1/48/96kHz plus clean-only, noise-only, dense
percussion, noise-step, tonal-noise, vibrato and level shifts. Maximum native/WOLA
sample discrepancy is1.04e-7; conditional stem-sum discrepancy is1.14e-7.
Both classical and learned routes have1024-sample latency at this FFT setting;
the model adds no lookahead. Unity independently reconstructs delayed input.

The clean-target SNR difference is +0.52 to +10.44dB across12 cases with desired
material against the chosen ratio-four, -40dB threshold classical compressor.
Restoration and ordinary compression have different targets; these scores assess
the chosen restoration target and parameters. Learned residual noise attenuation
is2.41–9.71dB on noisy cases containing desired material, and10.74dB on noise-only.
Clean-only component relative RMS error remains0.256. This is substantial damage
despite its better score than the chosen classical setting.

Against the post-hoc noise-lead-calibrated classical/dry/scalar diagnostic, the
learned route improves SNR in10 of11 comparable noisy cases and regresses in one, with differences
-0.59 to +13.15dB. Only source time50–200ms calibrates the scalar; later full-wave
attenuation differs. This is not equal-attenuation or preregistered admission.
Clean-only lacks noise calibration and noise-only lacks a defined desired SNR.

Independent source-energy onset diagnostics expose a dense-percussion case with
mean30ms relative RMS onset error0.681 learned versus0.081 classical. Other onset
results vary; these sparse generated labels do not establish perceptual quality.
Active-envelope error standard deviation ranges0.35–1.50dB in the learned route.
Positive residual local-spectral-contrast p99 increases in all12 noisy cases,
by0.15–1.00dB. Envelope variation and spectral isolation are descriptive proxies,
not validated pumping, musical-noise or audibility scores. The candidate remains
unadmitted.

Three native-Python integration tests and all seven expander Python compatibility
tests pass after protocol sharing. The52-test native spectral suite, strict QA
Clippy and default-feature check pass with the recorder. Local callback budgets
are separately recorded for mono/stereo, all three clocks and classical/missing/
learned modes; this does not prove complete heap accounting or contended hosts.
All18 local cases pass the fixed80%-of-callback-duration budget with16 warmups
and300 measured193-frame callbacks, run alone. The worst observed callback uses
4.79% of its deadline at the declared quality-comparison parameter profile.

Remaining: clean/onset damage and confidence calibration, independent recorded
and perceptual acceptance, complete heap accounting, contended/cross-platform
DAW deadlines, and product selection/activation semantics. Existing generated
results do not authorize automatic production activation.
