# Method basis and limits

Primary-source literature pass:

- [Hestermann and Deffner, 2020, Enhanced De-Essing via Neural Networks](https://link.springer.com/chapter/10.1007/978-3-030-43887-6_49): addresses confusion between vocal sibilance and other high-pitched sounds. Only the abstract was available; it does not provide a reproducible architecture, causal budget or transferable thresholds for this implementation.
- [Linhard, Bulling and Wolf, DAGA2017, Frequency Domain De-Essing for Hands-Free Applications](https://pub.dega-akustik.de/DAGA_2017/data/articles/000071.pdf): motivates relative band/average-level detection, including quiet sibilants. Its hands-free speech setting does not establish separation from instruments in music. The current FFT feature is an engineering comparison, not a reconstruction of that critical-band detector.
- [Valin, MMSP2018, A Hybrid DSP/Deep Learning Approach to Real-Time Full-Band Speech Enhancement](https://arxiv.org/abs/1709.08243): RNNoise combines learned gains with DSP for speech enhancement. Its speech outputs are not phonetic sibilance labels.
- [Silero VAD authors' repository](https://github.com/snakers4/silero-vad): primary implementation source for speech activity. This experiment uses the already pinned artifact/runtime, not current upstream replacement weights.

Design inference: benchmark relative spectral features and generic speech
context before introducing a realtime learned gate. Retain native range and
classical fallback, keep unknown/stale observations separate from low scores,
and demonstrate prefix invariance. Scores received at a packet end cannot be
applied retrospectively without buffering and reported latency.

Unvoiced /s, sh/ must not require periodicity or an earlier voiced segment. New
training/evaluation needs initial, final, whispered and voiced fricatives across
speakers/languages, plus legitimate music highs. Whole speakers/recordings and
their gain/EQ/noise variants must remain grouped before data splitting.

The current five clips do not justify training a new classifier or claiming
population accuracy. This experiment keeps its method fixed and reports the
observed attenuation tradeoffs. Range/heap/prefix checks prove implementation
properties, not phonetic or perceptual correctness. No production adoption.
