# Dynamics audit follow-up — 2026-09-27

## Evidence and scope

Read-only review of Gate/Expander (single and multiband), DynamicEQ, SpectralCompressor, TransientShaper, and analog-common/analog-eq/analog-limiter. No tests or DSP experiments were run for this report. Findings below distinguish source/algebra proofs from proposed runtime verification. MIDI, IAMF, and the compressor implementation owned by another agent were excluded. Existing AUD009 (upward/ducking gate modes) and AUD010 (limiter audio-path oversampling) remain separate.

Analog dependency behavior was checked against the actual Cargo-pinned math-audio checkout at `cabbc6dc1c3d0c8aad275ac33ec015d174859c89`, not a newer sibling checkout.

## Three priorities

| Priority | Finding | Evidence | Bounded implementation |
|---|---|---|---|
| 1 | Gate and single/multiband expander ratios use a compression-derived attenuation slope | Algebra and both production implementations | Correct conventional downward expansion law; independent transfer-curve and processed-signal regressions; document audible preset change |
| 2 | Analog model selection discards drive/color/character/trim settings | Model reconstruction and unordered bulk setters | Preserve shared settings across model changes; deterministic bulk application; model-switch regression across EQ and limiter |
| 3 | Spectral compressor detector overreads DC/Nyquist by 1.249 dB | Exact periodic-Hann DFT algebra | Correct endpoint normalization with independent absolute-level FFT-boundary oracles |

### 1. Conventional downward expansion ratio

`sotf-plugin-gate/src/lib/gate_plugin.rs:335` and `sotf-plugin-multiband-expander/src/lib/multiband_expander_plugin.rs:664` use attenuation `(T - x) * (1 - 1/R)` below threshold. The single-band Expander is an alias of the multiband implementation. Thus output slope is `2 - 1/R`, which cannot reach 2:1 even at arbitrarily high displayed ratios. With threshold −20 dB, input −30 dB, ratio 4:1, and a sufficiently large range, the current output is −37.5 dB. Conventional downward expansion requires `y = T + R*(x-T)`, hence −60 dB in this example. The [FabFilter Pro-G dynamics manual](https://www.fabfilter.com/help/pro-g/using/dynamiccontrols) describes ratio as increasing expansion strength, unity at 1:1, and high ratios as gating.

The existing `independent_single_band_peak_oracle` in multiband-expander tests repeats the production ratio expression and uses production fast logarithm/exponential helpers. It cannot identify this error. Existing gate knee-continuity assertions also derive their reference slope from the same expression.

Proposed checks: independent f64 hard/soft-knee equations, threshold/ratio/range sweeps, unity at ratio 1, knee value and derivative continuity, settled DC processing with sidechain HPF disabled, and a coherent-bin spectral-mode tone. Retain attack/hold/release and range behavior. The range cap must be included in expected output.

**Compatibility judgment:** correcting the existing displayed ratio is preferable to silently retaining misleading behavior, but it audibly increases attenuation in old presets. Existing JSON has no law-version discriminator. Document this as a behavior correction; approximate hard-knee legacy strength can be restored with `R_new = 2 - 1/R_old` (subject to range, detector and timing). A new opt-in compatibility parameter would preserve defective semantics indefinitely and expand public state unnecessarily. Parent should approve this explicit migration choice before release.

### 2. Analog model changes lose shared controls

`analog-common/src/lib.rs:142` replaces `AnalogModel` with `AnalogModel::from_id(id)` and prepares it. The pinned dependency constructs a fresh default model. The stage does not retain the drive/color/character/output-trim values. Both `analog-eq/src/analog_eq.rs:349` and `analog-limiter/src/analog_limiter.rs:297` apply a HashMap of parameters in iteration order. If model selection occurs after another control, that control is lost inside DSP although plugin getters still report its requested value. A later initialize repushes controls, but live changes and bulk updates are inconsistent.

Proposed checks: fresh fully configured reference versus changing among all six pinned models after setting nondefault shared values; reordered bulk maps; same-model selection behavior; scalar getters versus processed output. Preserve each model's own state-reset semantics during actual model replacement. Keep any new work bounded; preparing a new model must remain outside the audio process callback. Existing zero-color transparency tests do not cover this. The test named `oversized_blocks_are_chunked_without_allocation` checks finite output but contains no allocation assertion.

Related follow-up: model selection is marked setup but not structural; fresh construction/preparation and generic ParameterSet-based scalar setters are unsuitable for realtime automation. This report does not claim that fixing state preservation resolves that wider contract.

### 3. Spectral compressor FFT-boundary calibration

`spectral-compressor/src/lib/spectral_compressor_plugin.rs:195` uses `2/N` magnitude normalization at DC/Nyquist and `4/N` elsewhere. It sums five nearby bin powers and divides by 1.5. STFT uses the periodic Hann window (the [official SciPy Hann reference](https://docs.scipy.org/doc/scipy/reference/generated/scipy.signal.windows.hann.html) documents the periodic-window convention).

For a constant amplitude A, exact periodic-Hann DFT values are `X[0] = A*N/2` and `X[1] = -A*N/4`. With these normalizers both retained magnitudes become A. The detector therefore returns `sqrt(2/1.5)*A`, an overread of `10*log10(4/3) = 1.249387 dB`. The same issue occurs for an alternating Nyquist signal. At threshold −20 dB, A=0.1, ratio 4, and zero knee, this targets about 0.937 dB compression where an absolute-amplitude threshold oracle expects zero.

Existing FFT/bin-alignment tests cover bin 43 plus offsets 0, .25, .5 and compare against another instance of the same detector. They do not establish absolute calibration or cover boundaries. Proposed independent oracle: direct f64 DFT with literal periodic-Hann formula, DC/Nyquist and nearby sine/cosine phases, amplitude/threshold/ratio sweeps, FFT 1024/2048/4096, supported sample rates, plus final processed-output gain. Do not fix by copying production normalization into the oracle. Derive endpoint weighting for the declared amplitude convention and preserve interior behavior.

An additional sequencing issue merits a separate onset test: tonal/transient masks are used for the current frame before being updated from that frame's spectrum, so classification is one hop old. Reset initializes masks to one. Whether that is an intentional classifier delay needs a documented contract before correction.

## Coverage and parity matrix

| Family | Existing useful coverage/features | Material missing oracle or parity decision |
|---|---|---|
| Gate / Expander | Range, knee, hysteresis, hold, attack/release, lookahead, peak/RMS, link; gate external sidechain; time/spectral multiband modes and dry-delay tests | Correct absolute ratio law (above); independently measured timing/hold and range at all supported rates. Upward/ducking modes already AUD009. |
| DynamicEQ | Eight bell bands, per-band gain/threshold/ratio, shared timing/knee; link, dry/neutral behavior and coarse triggered-band attenuation tests | Independent complex-response oracle for the dry/full-EQ blend, not merely center-frequency algebra; absolute detector/GR sweeps, cut and boost, attack/release. Per-band timing, shelves, external sidechain, mid/side and below-threshold modes are product scope choices. |
| SpectralCompressor | Reconstruction/delay, dry mix, callback partitioning, FFT size/bin offset relative checks, linked channels, finite inputs, reset | Absolute DC/Nyquist and near-boundary normalization; current-frame versus previous-frame mask contract; final audio GR reference. |
| TransientShaper | Stereo linked gain, neutral overrange identity, parameter smoothing, callback invariance, reset and finite-control handling | Independent f64 two-envelope reference and step/decay oracles; scale invariance above sensitivity, rate/channel matrix, safety-clamp distortion. Existing pulse and sustain tests only assert coarse inequalities. |
| Analog common/EQ/limiter | Six pinned model IDs, zero-color transparency, coarse EQ center gain and harmonic behavior, color-off limiter ceiling | Model-control preservation (above); measured final-output ceiling after color/trim; THD/IMD/aliasing and model latency against independent analysis; real allocation guards. |

These commercial references define comparison criteria, not mandatory feature requests: [Pro-G timing](https://www.fabfilter.com/help/pro-g/using/timecontrols) and [expert sidechain/channel controls](https://www.fabfilter.com/help/pro-g/using/expertmode); [Pro-MB processing modes](https://www.fabfilter.com/help/pro-mb/using/processingmode) distinguish minimum/linear/dynamic phase; [TDR Nova GE manual](https://docs.tokyodawn.net/nova-ge-manual/) documents independent band dynamics and operating quadrants; [Pro-Q spectral dynamics](https://www.fabfilter.com/help/pro-q/using/spectral-dynamics) provides frequency-selective comparison criteria.

TransientShaper references an SPL-inspired approach, but its percentage controls, sensitivity threshold, bounded composite gain and protective peak handling are not exact SPL behavior. The [licensed SPL Transient Designer manual](https://help.uaudio.com/hc/en-us/articles/33538335681428-SPL-Transient-Designer-Manual) describes level-independent processing and separate attack/sustain envelope behavior. Document the intended design and validate its own equations instead of claiming hardware equivalence from coarse tests.

AnalogLimiter currently limits first, then applies analog color and trim (`analog_limiter.rs:373`). Its threshold help says maximum output, although postprocessing can increase it. The file already explicitly excludes a strict ISP guarantee after color. A final-output ceiling test is therefore needed to settle whether threshold means a pre-color limit or an emitted-output guarantee. [Pro-L true-peak limiting](https://www.fabfilter.com/help/pro-l/using/truepeaklimiting) distinguishes input detection from correction of peaks created by processing. This is separate from AUD010, and no blanket attenuation should substitute for a stated contract.

## Status

This report is a source review and analytical reproduction, not a claim of passing regressions. Root authorized follow-up implementation of priorities 1 and 2 as AUD027/AUD028 after report completion. Priority 3 and the smaller coverage gaps remain unimplemented here.


## Implementation follow-up (after the read-only report)

Root authorized AUD027/AUD028. Independent regression runs reproduced Gate
and Expander output −35.0006 dB versus expected −40 dB for input −30 dB,
threshold −20 dB, ratio 2. Their actual transfer slopes are now corrected;
new f64 references use standard log10/powf, not production fast math. Global
and per-band time/spectral paths share the corrected law. Static auto-makeup
retains its documented bounded heuristic to avoid a speculative loudness
change. Crate changelogs document migration `R_new = 2 - 1/R_old` and the
static-makeup caveat.

The spectral audio oracle additionally reproduced preexisting input suffix
loss: with ratio 1, 257-frame versus 256-frame callbacks differed by 0.06324079
on an amplitude-0.03162 tone. With ratio 2 the final bin42/43/44 attenuation
was 18.50/10.19/14.10 dB instead of the coherent-tone 16.02/10/16.02 dB. The
local process loop now consumes all input as well as filling output; no new
queues or latency were introduced. Exact waveform comparisons cover small,
large, irregular and final partial callbacks. The absolute spectral oracle
uses the analytic dual-Hann weights 2/3 center + 1/3 side-bin gain.

Analog shared controls now survive model replacement, same-model selection
preserves running state, and EQ/limiter bulk updates select the model before
applying float targets. Their live smoothing trajectories and settled output
are checked across all six models and varied bulk insertion positions. The
analog-compressor consumer is tested but its source was not changed. Broader
analog realtime setter/model-prepare concerns remain separate follow-up.

Verification checkpoint: 247 tests passed across Gate, MultibandExpander,
analog-common/EQ/limiter/compressor. The same six crates passed clippy with
all targets and warnings denied. Logs: `/tmp/sotf-expansion-analog-final.log`
and `/tmp/sotf-expansion-analog-clippy.log`.
