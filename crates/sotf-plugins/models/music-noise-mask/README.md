# Explicit musical denoising mask prototype

The `qa` feature adds a fresh-stream-only music-mask mode to the native denoiser.
It is trained on generated polyphonic/percussive music and noise, with no speech
pretrained model or external audio. There is no production factory selector,
default model or automatic activation. Deployment admission remains false.

The fixed 6→8→1 tanh/sigmoid network estimates a gain ratio from local spectral
contrast, broadband contrast, local spectral flatness, temporal power contrast,
normalized frequency and normalized level. Feature history is a causal 30 ms
power average. JSON is limited to 64 KiB with exact dimensions/policy/provenance,
finite coefficients bounded by 16, and an explicit SHA256. Preparation allocates
on the control thread; no model loading, hashing, locks or allocations occur in
the valid processing, reset or drain paths.

The native path keeps MCRA, decision-directed power and classical gain history
independent. It prepares a separate gain table and selects it only when multiplying
the spectrum for synthesis. Disabled/missing/invalid or unsupported model contexts
retain the exact classical gain. DC and Nyquist deliberately remain classical.
Tests compare all retained classical estimator/gain arrays after every callback
across both FFT sizes and four optional configurations. Selected output differs
while those classical arrays remain identical.

The model supports mono/stereo, 512/2048-point FFTs and 44.1/48/96 kHz. Other
native-valid contexts use fallback. Neural gain attack/release is fixed at 5/30 ms;
this research mode does not reinterpret the native classical attack/release
controls. Positive reduction shapes gain as `prediction^(reduction_db/20)` before
the native floor, reduction curve and transparency are applied. The 20 dB value
is a shaping reference, not calibration to the classical mode's achieved noise
attenuation. The last positive depth is retained while a zero-depth transition
uses the existing final unity blend. Fresh/reset zero depth has an exact unity
gain endpoint and reconstructs delayed input within native FFT roundoff.

The existing half-window prefix and periodic sqrt-Hann analysis are retained.
There is zero added model lookahead; native latency stays 512 or 2048 samples.
Inference/history runs only on source FFT frames. Full finite drain freezes the
last learned gains/counters while retaining the ordinary analysis/synthesis
support. Unprepared short streams use classical gains during drain. Reset clears
feature/gain history and counters, retaining model and selection. Disabled
qualification keeps the model's features/inference warm. A live QA toggle can
leave old overlap-add audio for up to two FFT windows and is not a product
click-free activation contract.

Training uses 32 generated source seeds and eight validation seeds, with 1,024
stratified rows per source (half foreground-dominant). Weighted gain MSE emphasizes
foreground. The fixed 300-iteration L-BFGS-B fit hits its iteration limit. Model
and policy are written before eight final source seeds are generated. Six final
variants reuse those seeds and are correlated. Mask-level analysis includes
defined zero-padded end frames; native inference instead freezes at EOF. These
pooled frame/bin scores are distinct from native waveform acceptance.

The model does poorly on pooled gain MSE: 0.2312 matched versus 0.1503 for a simple
flatness-mask reference, worsening to 0.3905 under a noise step. Clean-only
foreground-bin gain error is also high. Low-energy bins contribute to these
unweighted metrics; they do not directly determine perceived waveform error.

The actual native renderer evaluates nine new generated compositions/conditions
at both FFT sizes (18 correlated probes, 36 outputs), with complete bounded drain.
At the tested 20 dB setting, learned steady SNR exceeds classical by 0.29–26.62 dB,
but lead-in noise attenuation is only 0.75–8.43 dB versus 13.68–19.90 dB classical.
This is a preservation/noise-removal tradeoff, not an equal-attenuation comparison.
Clean-only learned output reaches about 27.3/29.7 dB SNR; unfiltered clean input
remains the ideal reference. Transient, stereo-balance and residual-noise spectral
crest diagnostics are recorded separately and are not perceptual acceptance.

An additional reference blends native classical output with latency-aligned dry
input. It chooses one coefficient from the known noise-only 0.15–0.19 s span,
ramps it over 10 ms before music starts and evaluates later musical content.
The learned route wins eight noisy probes and loses eight (SNR delta −1.87 to
+7.98 dB). Noise energy in the later 0.20–0.24 s check differs by −3.09 to +0.17 dB,
so this is an approximately matched, noise-span-calibrated diagnostic. It is not
a deployed native parameter or an independent model-selection gate. No coefficient
is fitted using subsequent clean musical target data.

```sh
python scripts/ml/train_music_noise_mask.py --output /new/training-directory
cargo build -p sotf-plugin-denoiser --features qa --bin qa-music-mask-render --release
python scripts/ml/evaluate_music_noise_mask.py \
  --renderer /absolute/path/qa-music-mask-render \
  --model crates/sotf-plugins/models/music-noise-mask/model.synthetic.json \
  --work /new/raw-waveform-directory --report /new/report.json
```

Validation passes 138 native tests, six Python/native tests, strict QA Clippy,
default-feature check and all 18 local stereo callback budgets. The model adds no
lookahead. Worst observed neural callback was 0.688 ms against the 1.608 ms local
budget at 96 kHz. These measurements do not establish complete heap accounting
or platform deadlines. Timing uses the other native parameter defaults and does
not cover every optional-mode combination. Feature/network parity, gradient/DFT
references, estimator
independence, exact fallback, unity endpoint, reset, callback/drain partitioning
and frozen EOS counters are covered.

Evidence: [audit](../../../../audit/ml-music-mask-20261025.md),
[manifest](manifest.json). Real licensed held-out music, broader instruments and
genres, stereo/transient listening review, parameter/profile semantics, portable
runtime/memory bounds and a demonstrated benefit at comparable noise removal
remain open. The current candidate is not admitted for arbitrary music.
