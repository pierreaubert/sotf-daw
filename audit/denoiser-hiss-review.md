# Independent Denoiser / Hiss review and AUD088 correction

2026-09-28. Scope: current source and `/tmp/sotf-denoiser-hiss-eof-verified.md`.
MIDI/IAMF excluded. TokenSave used for source navigation and current slices;
the graph index predates recent aggregate changes, so cached graph relationships
were not treated as proof of current implementation. No Hiss source changes or
broad Hiss test reruns were performed in this independent review.

## Finite support and acceptance review

No new blocker was found in the implemented finite-stream/startup accounting.

- Denoiser uses N512/2048, H=N/2 and D=N, one negative-origin window, explicit
  negative synthesis discard, and a ring sized for the largest accepted callback
  plus one FFT. It consumes the full in-place input before output copying. Input
  slices are limited by the next FFT boundary, so note and multi-resolution
  analysis sees only the matching accepted source history.
- Denoiser and spectral Hiss use suffix `2N-H+((H-S%H)%H)` after S>0 frames,
  and native response bound `2N-1`. Gain estimators multiply each current FFT;
  they do not synthesize an independent audio floor. Hiss's existing N1024/H256
  prefix and sample scheduler remain consistent with D=N. Its dry delay is no
  longer than the spectral support. Conventional Hiss remains unresolved,
  with Unknown metadata and legacy COMPLETE0 as explicitly documented.
- Their prepared hop cache decouples drain capacity from the kernel cadence.
  Capacity/rate validation occurs before EOS mutation; unused output remains
  untouched, and remaining counts advance only for published frames.
- Explicit Denoiser capture is deliberately frozen during padding, whereas
  normal adaptive audio gains continue. The private regression's reference
  only disables capture accumulation, which is the same stated policy. Its
  stored profile and partial capture checks meaningfully test this distinction.
- Existing cold allocation/deallocation tests establish the selected FFT plans
  on this platform. RealFFT convenience calls may allocate for other plans;
  the report should retain that practical qualification.

## Confirmed preexisting defect: AUD088

`DenoiserPlugin::reset` omitted every tonal/transient separator's time history.
The new initialize-to-reset path inherited this omission. This affects enabled
harmonic/percussive processing; default-mode reset tests could not expose it.
The option must be enabled through the public scalar setter: the separate
missing JSON field is tracked as AUD089 and is untouched here.

Public standalone evidence is retained in `denoiser-reset-probe.rs/.log`:
32,768-frame tonal warmup, reset, then a deterministic dense signal differed
from a fresh instance by 0.009599838406 at normal latency and 0.003398563713 at
low latency. The permanent public-setter test uses a slightly different tone
calculation and records a red maximum 0.00986868 in mono/normal latency.

The narrow correction calls the backend's already-existing allocation-free
`separator.reset()` for every channel, even if the option is currently disabled,
and fills the three shared HPSS scratch vectors with zeros. It does not replace
objects, alter settings, erase the stored noise profile, or change ordinary
processing arithmetic. No backend modification was necessary.

Files changed in this follow-up only:

- `sotf-plugin-denoiser/src/lib/denoiser_plugin.rs`: reset history/scratch.
- `sotf-plugin-denoiser/tests/finite_stream.rs`: two public integration tests.
- `sotf-plugin-denoiser/CHANGELOG.md`: reset correction entry.

## Executed verification

- Permanent regression red before the fix; see `/tmp/sotf-denoiser-hpss-reset-red.log`.
- Focused green: two tests, including 12 exact warm/reset-or-reinitialize/fresh
  comparisons across N512/2048 and 1/2/6 channels. Enabled setting remains true.
- Six first-reset-on-a-new-thread cases with warmed HPSS history measured
  **zero allocations and zero deallocations**, with no warm reset on that thread.
- `cargo test -p sotf-plugin-denoiser --all-features`: **78 passed, 0 failed,
  0 ignored**. Existing captured-profile preservation regression also passes.
- `cargo clippy -p sotf-plugin-denoiser --all-targets --all-features -- -D warnings`:
  clean. Scoped rustfmt and diff whitespace checks clean.
- Logs: `/tmp/sotf-denoiser-hpss-reset-{green,full,clippy}.log`.

Source frozen. No JSON/wiring, host, engine, or unrelated lifecycle changes.
