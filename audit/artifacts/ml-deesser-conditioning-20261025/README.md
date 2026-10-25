# De-esser speech-conditioning development experiment

Actual native rendering and an explicit Offline observer compare absolute,
relative-spectral, RNNoise-conditioned and Silero-conditioned external keys.
No trained sibilance classifier or production mode is claimed. Both pretrained
models use their existing verified Gate implementation and artifacts.

Five existing recordings, two gains and three topologies produce 120 correlated
comparisons. The phoneme demonstrations share one speaker; violin and cymbal are
single examples. This is development material already used by the audit, not an
independent final corpus. No weights or thresholds were trained/tuned from these
observations. `development-tradeoffs.json` records target/music tradeoffs rather
than phonetic classification accuracy or population statistics.

At gain 1, Wideband native attenuation is 5.691 dB on violin and 5.038 dB on
cymbal. The relative key changes these to 0 and 0.394 dB but loses up to 4.779 dB
of the baseline's high-band attenuation on an annotated “sh” region. RNNoise
conditioning changes music attenuation to 0/4.213 dB, with worst target HF loss
0.595 dB. Silero produces 3.032/4.160 dB, with worst target HF loss 2.567 dB.
No approach resolves the demonstrated tradeoff; no production adoption follows.

The causal key applies only observations received by packet start. It reserves
the entire nominal 10 ms packet against the source-age deadline, including at
partial EOF. A new boundary assertion exposed that the initial implementation
could retain a score up to 10 ms past expiry. Initial policy, protocol, failing
test and results are retained. Corrected policy/results are separate; all 20
model traces are bit-identical across the fix. Model weights, score/band
thresholds and source audio were unchanged.

Four real-model prefix comparisons (s/violin, RNNoise/Silero, nonaligned EOF)
and pure-policy prefix tests pass. Native finalized-prefix tests exclude the
declared pending audio latency. Seven Python tests, 112 native de-esser tests,
both strict diagnostic-binary Clippy checks and formatting checks pass. Native
renderer process/drain are guarded against allocations and deallocations; the
Offline model observer intentionally makes no realtime heap/deadline claim.

Independent analysis reconstructs key smoothing with a recursive filter,
trailing FFT features through a separate FFT API/window formula, and measurement
filters with direct windowed-sinc convolution rather than the evaluation's
`firwin`/FFT convolution. It verifies 30 distinct key streams, all 120 conditional
comparisons and Wideband range bounds. Maximum key discrepancy is zero, bin-power
fraction discrepancy 3.34e-16 and attenuation discrepancy 2.69e-6 dB. Maximum
used score age through a packet is 148.042 ms, including the native allowance.

The spectral feature is explicitly raw one-sided squared-bin magnitude, without
interior-bin doubling; it is not a Parseval-normalized PCM energy estimate.
Centered LF/HF filters are measurement-only and do not feed decisions. Key
modulation can excite detector transients; a lower key amplitude does not prove
pointwise reduction below baseline. The native Range remains explicit.

Native request/response/audio/score traces are retained externally under
`/Volumes/home_tmp/tmp/ml-deesser-conditioning-20261025`. Existing fixture PCM,
licenses and hashes are in the de-esser recorded-fixture manifest. No new audio
or model weights are committed. A dedicated classifier, adequate new labels,
worker/lifecycle and resource budgets, real-material shifts, listening and
host/UI integration remain open in the implementation ledger.
