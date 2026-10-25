# Experimental offline speech-conditioned de-esser keys

This experiment compares the native absolute-band detector, a relative spectral
key, and keys conditioned by the existing RNNoise/Silero speech models. VAD
scores provide context; they are not sibilance labels. No dedicated classifier
was trained, no production de-esser mode was added and no live DSP defaults
changed. The current evidence does not justify production adoption.

At gain 1 (fixtures peak-normalized to 0.5), the Wideband profile uses 6 kHz,
Q 0.7, threshold −36 dB, ratio 8:1, 1 ms attack, 80 ms release, 6 dB Range and
2 ms lookahead. Unwanted music attenuation is:

| Key | Violin | Cymbal |
|---|---:|---:|
| Native absolute | 5.691 dB | 5.038 dB |
| Relative spectral | 0.000 dB | 0.394 dB |
| RNNoise-conditioned | 0.000 dB | 4.213 dB |
| Silero-conditioned | 3.032 dB | 4.160 dB |

The relative key also loses up to 4.779 dB of high-band attenuation present in
the native baseline on an annotated “sh” region. Corresponding worst measured
losses are 0.595 dB for RNNoise conditioning and 2.567 dB for Silero conditioning.
These are conditional attenuation tradeoffs under fixed settings, not classifier
accuracy or listener preference. The five recordings include just one isolated
phoneme speaker, one violin and one cymbal. Level and topology variants remain
correlated development observations; upstream model training overlap is unknown.

Build the two diagnostics:

```sh
cargo build --release -p sotf-plugin-de-esser --features qa --bin qa-de-esser-render
cargo build --release -p sotf-plugin-gate --features qa,silero-speech --bin qa-speech-observe
```

Run the comparison into new output/work paths:

```sh
python scripts/ml/evaluate_deesser_conditioning.py \
  --renderer /path/to/qa-de-esser-render \
  --observer /path/to/qa-speech-observe \
  --silero crates/sotf-plugins/models/silero-vad/silero_vad_16k_batch1.onnx \
  --work /path/to/new-native-work --output /path/to/new-report.json
```

The native renderer accepts a bounded JSON request containing raw float32-le
input/output paths, an optional same-shape key, explicit sample rate/channels/
frames/callback size and native de-esser constructor parameters. The external-key
flag must match key presence. It uses the validated constructor, compacts program
output from the native interleaved program/key layout, checks normal key samples
remain unchanged, and guards native processing/drain against allocation and
deallocation. Output is created exclusively after successful rendering. Limits
are 1–8 channels, 44.1–192 kHz, four million program samples and 1–4,096 callback
frames. This geometry support does not establish learned quality across it.

Raw output retains latency and the complete public drain. Python returns the
input-duration segment after removing reported latency. The existing minimum-
phase recursive ringing is truncated at native drain completion; its full IIR
support is not finite. FIR support and program delay follow the public contract.

The observer accepts a mono 48 kHz raw float32-le file and an explicit existing
Gate model selection. It invokes the verified public model path in control-time
`ProcessingMode::Offline`, reads immutable 30 Hz diagnostic snapshots and records
their receipt after each 480-frame packet. Snapshot observation counts distinguish
new results. It performs no EOF padding and no retrospective score application.
RNNoise uses independent 100 ms windows; Silero retains recurrent history and
its existing warmup/resampling policy. Scores are not calibrated probabilities.
Age includes the native 100 ms allowance plus endpoint lag, not the entire
Silero recurrent history. This observer is not a realtime inference API.

The relative feature is the fraction of **raw one-sided rFFT squared bin
magnitudes** between 4 and 10 kHz in a trailing 960-sample Hann window. It is not
Parseval-normalized PCM energy; interior bins are not doubled. Fixed opening/
closing thresholds are 0.35/0.25, with two low packets to close after opening.
The completed window affects only the next packet. Key transitions use a 5 ms
smoother. This is an engineering baseline, not a reproduction of a published
de-esser's exact critical-band model.

For a warmed, valid observation that remains within the 150 ms age budget through
the entire nominal next packet, score ≤0.2 selects a reduced key and score ≥0.8
permits the relative policy. Missing, uncertain, invalid, startup or stale scores
select the classical key, returning smoothly after earlier protection. The
nominal packet allowance is retained at partial EOF to preserve prefix invariance.
Calling the key policy without observations gives an exactly classical key from
startup. The comparison CLI requires its explicit models and aborts on load
failure. The age cap governs new packet decisions; the 5 ms key transition and
native envelope can retain earlier history during fallback. Fallback retains
the baseline's known unwanted attenuation; it is not evidence of protection.

Modulating a key can introduce bandpass transients. Even though the key's sample
amplitude never exceeds the original, its filtered detector response and native
reduction need not remain pointwise below the absolute baseline. The existing
Range control remains independent. Actual Wideband sample gains are checked
against the requested 6 dB cap; no analogous raw-sample bound is inferred for
split-band sums. Native reference output uses Range 0 through the same topology.

Evidence is in `audit/artifacts/ml-deesser-conditioning-20261025`: 120 conditional
comparisons, four real-model prefix checks, seven Python/native integration tests,
112 native de-esser tests and strict lint checks. Independent reconstruction
checks every key stream's chronology/expiry and recorded metric, with maximum
key discrepancy zero and measurement discrepancy 2.69e-6 dB. Maximum used score
age through a packet is 148.042 ms. The initial packet-expiry bug, failing test
and first run are preserved; the corrected run uses bit-identical model traces.

A dedicated sibilance detector needs adequate speaker/source-disjoint labels,
including initial/final/whispered and voiced fricatives, real music/noise shifts
and listening acceptance. Realtime worker/lifecycle/fallback integration and
CPU/memory/deadline/host validation remain open. No new weights or recordings
were redistributed: RNNoise and Silero retain their existing BSD-3-Clause/MIT
provenance; phoneme/violin CC BY-SA and cymbal public-domain attribution remains
in the existing recorded-fixture manifest. Experiment code is GPL-3.0-or-later.
