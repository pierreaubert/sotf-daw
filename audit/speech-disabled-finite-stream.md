# SpeechDenoiser disabled finite response

2026-09-28. AUD073 extension following AUD083's corrected dry/wet alignment.
Only the SpeechDenoiser plugin, its tests and documentation changed. The shared
RNNoise backend, model math, latency and enabled wet waveform are unchanged.

## Support proof and lifecycle

When EOF selects disabled, existing backend processing targets bypass mix one.
The mix advances before each emitted sample, and the explicit mix>=1 branch
returns only dry audio. From any accepted mix in [0,1], each unclamped f32 step
advances by more than 1/480−2^-24, so at most 481 emitted frames reach one. The
dry ring delays sanitized program exactly 960 frames. Consequently exactly 960
zero-continuation frames cover both an incomplete fade and every remaining dry
sample. Subsequent disabled output is exactly zero, even though recursive wet
model/high-pass state remains internally active.

Initialized disabled metadata is Finite(960); enabled or uninitialized metadata
is Unknown. The structural native capacity is 480 before input and after
completion. Full-capacity call bounds are ceil(remaining/480), minimum one;
partial calls update the remaining count exactly. Queries do not run the model.

Valid finite EOS after nonempty input freezes input and changed enabled controls
until reset or successful initialize. Same-value scalar and borrowed/owned map
snapshots remain accepted. Empty streams remain unfrozen. Enabled wet retains
its existing Unknown/immediate-completion/unfrozen policy; this change does not
claim finite model support. Invalid initialized rate, output alignment or zero
positive-tail capacity is rejected before output or stream state changes.

One drain call writes only min(destination frames,480,remaining), runs the same
backend and diagnostic publication as ordinary zero input, then returns that
prefix. No new model copy, buffer, callback allocation or deallocation is needed.
The owned parameter-map API retains its existing caller-ownership behavior; the
borrowed realtime map route is used for cold no-free guarantees.

## Executed evidence

- Independent final-marker regression failed before implementation: returned
  zero tail frames versus expected960. `/tmp/sotf-speech-disabled-drain-red.log`.
- 976 exact dry waveform cases: every model phase for mono/stereo, plus sixteen
  long dense/ring-wrap cases over one-frame, irregular and oversized callbacks.
  Complete process+drain equals 960 startup zeros followed by the entire source.
- 120 identical-history fade comparisons: immediately disabled wet history,
  interrupted reverse transitions and partial fades, varied model phases and
  drain capacities. Every returned sample equals ordinary zero continuation
  bit-for-bit; a further 3072 frames of disabled continuation are exactly zero.
- 60 actual native work-count configurations include arbitrary prior partial
  drainage, exact remaining call counts and stable terminal completion.
- Transactional invalid-call/retry, output sentinels, scalar/map control guards,
  empty streams, failed/successful initialization and reset/fresh replay pass.
- Fresh-thread cold checks: 16 configurations, two reset epochs each, zero
  allocations and zero deallocations through first drain, metadata, same-value
  controls, borrowed snapshots, completion, reset and subsequent processing.
- Full shared backend and SpeechDenoiser suites: **113 passed, zero failed or
  ignored**. Seven new tests; existing enabled wet/model timing evidence remains
  unchanged. `/tmp/sotf-speech-disabled-drain-full.log`.
- Strict all-target/all-feature Clippy passes:
  `/tmp/sotf-speech-disabled-drain-clippy.log`.

Focused matrices: `/tmp/sotf-speech-disabled-drain-matrix.log`.
Plan: [disabled finite-stream proposal](proposals/speech-disabled-finite-stream.md).
Independent dynamics review found no blocker in support, progress, preflight,
control or reset semantics; spectral independently reviewed the support proof.
This change awaits the next workspace integration checkpoint.

## Real host-chain follow-up

A factory-created SpeechDenoiser(disabled) → spectral stereo Downmix → dry
Limiter chain passes 48 complete waveform runs: four source lengths, each of
three single-stage bypasses or no bypass, and three ordinary callback sizes.
The oracle independently specifies model/queue960 + downmix2048 + limiter240
latency and composes the actual accepted-input phase at Downmix EOF. Every
returned sample matches the delayed source within1e-6, including the final marker
and structural zero suffix; exact total returned length is checked. Host reset
reuses each chain for the next partition. Both real-chain tests (including the
prior Gate/FIR EQ/Limiter suite) and scoped strict Clippy pass.

Logs: `/tmp/sotf-speech-downmix-chain-final.log`,
`/tmp/sotf-speech-downmix-chain-clippy.log`. The initial new fixture queried host
latency before building its graph; it was corrected to build before that query.
That fixture-only failure remains in `/tmp/sotf-speech-downmix-chain.log`; no
host production behavior changed for this follow-up.
