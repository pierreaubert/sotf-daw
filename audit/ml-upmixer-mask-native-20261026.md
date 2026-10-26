# Native Upmixer source-mask boundary

Issue10/plugin43, continuation of the frozen offline source-mask experiment.
The same synthetic asset is installed through an explicit control-thread API
under `qa`. Production parameters, presets, scalar vocal ML and default output
remain unchanged. The candidate remains unadmitted after its offline hard-pan
retention and correlated-bed leakage failures.

The native loader validates a64KiB limit before hashing, SHA256, exact model
format/policy/provenance, false admission and65 finite coefficients bounded
by16. Runtime storage is fixed arrays. No model I/O, worker, queue or allocation
is introduced into processing. Installation is allowed only before accepted
source audio since reset; even one partial input frame prevents replacement.
Reset retains the prepared model and clears observations/counters.

Selection is restricted to48kHz,512-point main FFT, HR disabled with no residual
HR envelope, and scalar ML disabled. Other configurations keep ordinary native
decomposition. Every completed main analysis frame computes the same32 eight-bin
covariance features as the offline reference. The native FFT's fixed-3dB headroom
is compensated by a factor2 in covariance power, so model excitation features
retain their training convention. Silent, invalid or out-of-range bands retain
native decomposition independently. DC/Nyquist and low-frequency pass-through
routing retain their existing native paths.

Selected masks are common to both stereo channels and stay[0,1]. Direct inputs
become the masked L/R spectra and ambient inputs their complementary residual.
Existing center phase alignment, coherence/dialogue weighting, transition-band
crossfade, panning and user gains then operate normally. The selected residual
already expresses ambient fraction, so it does not receive the native additional
sqrt-diffuseness attenuation. The unmasked secondary PCA source is suppressed for
selected bins to avoid duplicating it. Independent native covariance, coherence,
diffuseness and direction detectors continue updating from original input.
Final native output safety remains active.

The trained mask is an amplitude Wiener-style estimate of a known-stem power
fraction. This native routing is not equivalent to the offline stereo stem
extractor: center extraction, frequency pass-through and multichannel panning
still participate. Offline mask MSE and stereo leakage do not certify native
surround leakage or artifacts. No new semantic source-identification claim is
made. Masks have no extra temporal smoothing beyond the existing FFT overlap and
native downstream controls; click-free activation is not claimed.

Finite-stream generation freezes the last source decisions and their counters.
The ordinary native finite tail geometry is retained. A model adds no waveform
buffer or lookahead; actual impulse-delay measurements and focused native test
results are recorded in the sibling artifact directory. Missing or rejected
models use the existing native algorithm, whose full routing is broader than the
offline reference's scalar analytic-directness mask.

The focused QA suite reports236 passing tests and3 ignored historical rejected
candidates. Four golden-audio tests return early because generated golden files
are missing; they are not waveform-regression evidence. All7 newly added
source-mask tests execute and pass, including independent frozen Python/network
probabilities, closed-form covariance and headroom compensation, actual audio
callback1/257 identity, reset replay, silent/unsupported configuration fallback,
partial-input installation guards and process/drain/reset heap guards. The
native test compares stereo source-column impulses at amplitude0.1 using a1e-9
output threshold: both native and selected first outputs occur at sample512,
matching declared latency. Added onset delay is0 samples for those impulses;
total latency is10.667ms at48kHz. This is not proof of unchanged phase/group delay
or perceptual timing for arbitrary material. Output/tail frame counts match.

Strict all-target QA Clippy and the feature-disabled build pass. The first
compile failed because the selected SHA library digest does not implement
LowerHex; per-byte formatting fixed it. That failure and subsequent passing runs
are retained under `audit/artifacts/ml-upmixer-mask-native-20261026`.

The subsequent export qualification is in
`audit/ml-upmixer-mask-render-20261026.md`:200 cases expose20 direct-only feature
parity failures in very quiet bands despite exact inference parity on native
features. These failures remain open. Remaining: fix that numerical feature
policy on development data, actual multichannel desired-source leakage and artifact measurements, complete
heap/CPU accounting and contended/platform host budgets. Improve the model on
development data before evaluating fresh final material. Independent recorded
music, listening and production selection/activation semantics remain open.
