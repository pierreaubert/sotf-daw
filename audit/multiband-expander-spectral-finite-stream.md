# MultibandExpander spectral startup and finite EOS — verified implementation

2026-09-28. AUD094 startup; AUD073 finite spectral support. Scope is the MultibandExpander crate and its approved proposal. No parameter IDs/defaults, host traits, engine logic, MIDI, or IAMF changed.

## Executed failures before implementation

Public unity impulse checkpoints at source offsets 0, 256, 512, 768, with input amplitude 0.5, returned `[0, 0.083333336, 0.4166667, 0.5]` at the declared 1024-frame delay. These match the independent omitted-window prediction `[0, 1/6, 5/6, 1]` for amplitude gain. A final-marker stream of 1025 frames returned no drain frames, against a derived 2047-frame continuation. Both permanent tests failed before production edits: `/tmp/sotf-mbe-spectral-red.log`.

## Scheduling and support

The existing N=1024, H=256 dual periodic Hann transform keeps its 1/(1.5N) normalization and public 1024-frame latency. Input is primed with N-H zero samples. The first three synthesis windows have negative origins; their negative prefixes are discarded rather than wrapped into the ring. No output is published until origin zero has all preceding-window contributions. Reset restores that same origin, fill, discard count, output delay, and zero buffers.

For T>0 accepted input frames, the last potentially source-bearing scheduled window begins at `S=floor((T-1)/H)H`. Its emitted support ends at exclusive frame `E=2N+S`. The chosen finite render length is therefore `R=E-T=2N-H+(H-(T mod H)) mod H`, between 1792 and 2047 additional frames. This is a conservative window-support bound, not a promise that its final frame is nonzero. Settled dry retains its exact N-frame delay; wet or fading spectral output uses the full window bound. Recursive detector/hold/envelope state continues naturally and cannot synthesize audio from a zero spectrum. No adaptive profile or learned model exists to freeze.

Drain uses an additional preallocated H×channels f32 cache (1024 bytes/channel), processing ordinary zero continuation in canonical blocks. It serves cached output exactly once, including 1-frame destinations. Remaining frames include the unread cache. The full-capacity work bound is `bool(unread>0)+ceil((remaining-unread)/H)`, at least one terminal call. A full-support epoch flag keeps tail metadata at2047 through completion even when an in-flight mix fade settles dry during refill. Reset clears the flag; already-dry EOS retains1024. Structural capacity H remains advertised before initialization/input and after completion, supporting 2×/4× wrapper preparation.

Existing invalid-rate/whole-frame/zero-capacity checks precede state consumption. Empty drain remains a no-op. A valid nonempty drain freezes controls and input until reset or reinitialize; identical parameter snapshots are still accepted. Invalid calls do not latch EOS. Time-domain finite eligibility and the unresolved recursive crossover policy are unchanged.

## Independent evidence

- 2048 first/final impulse fixtures: mono/stereo × every initial phase 0..1023; compare every emitted sample against an independently delayed source, tolerance 2e-6, plus the independently derived full length.
- 72 dense unity runs: mono/stereo × 1/2/3/5 bands × 3 callback partitions × fresh/reset/reinitialized epochs. 17N+13 source frames cross the OLA ring repeatedly. Unity tolerance 2e-6; partition and reset/reinitialize output comparison is bit exact.
- 256 nonlinear zero-continuation cases: mono/stereo × 1/2/3/5 bands × ordinary/solo/bypass/EOF wet-to-dry transition × 8 lengths around hop/window boundaries. Ratio4, hold7ms and knee6dB exercise stateful gain. Drain with mixed capacities1/17/255/256 equals an independently zero-padded public process twin exactly; a further2N frames are exactly zero. Tests request the supported maximum5 bands explicitly; the initial proposal's8 would be constructor-clamped to5.
- 512 lifecycle/cache-bound cases: mono/stereo × all256 hop phases. Invalid rate, empty capacity, and incomplete stereo frames preserve destination/history and leave controls legal. After valid1-frame EOS: identical bulk snapshot succeeds, changed single/bulk controls and new input fail, remaining tail matches an untouched twin, full-capacity observed call count equals the query, and repeated completion is stable.
- 36 fresh callback-thread allocation fixtures: mono/stereo × native/2×/4× wrappers × lengths1/257/8193 × wet/nearly-settled dry fade; each exercises two epochs including reset. First process, begin, bound/capacity queries, complete drain and reset measure **0 allocations and 0 deallocations**. Initialization and fixture destruction are outside measurement.
- Metadata regression: a11000-frame wet-to-dry fade finishes during the first canonical refill, leaving255 cached frames and most of the original support unread. Before the epoch flag the test observed Finite1024 instead of2047 (`/tmp/sotf-mbe-spectral-metadata-red.log`); it now verifies2047 through completion and1024 after reset. A public process twin confirms the fade itself has settled.
- Existing time-domain, dry spectral, lookahead, parameter, ordinary spectral and transfer-law tests remain passing. Older fixtures were updated from spectral Unknown metadata to the new bound, with an independent phase-specific expected length rather than treating tail metadata as an exact countdown.

The existing steady coherent-tone gain oracle initially failed at 96kHz by -0.0336dB: its fixed32768-frame stream represented only0.341s. Correct startup exposes partial analysis windows that legitimately request additional attenuation; the configured50ms release had not settled. Warmup is now derived as12 release time constants, rounded to complete N periods, plus2N transform support before the four measured periods. `60dB*exp(-12)<0.0004dB`; the original0.01dB law tolerance is unchanged. This measures steady transfer separately from the independently verified startup waveform.

## Verification

- `cargo test -p sotf-plugin-multiband-expander --lib --tests --offline`: **146 passed**, zero failed/ignored. `/tmp/sotf-mbe-spectral-full.log`.
- `cargo clippy -p sotf-plugin-multiband-expander --all-targets --features qa --offline -- -D warnings`: passed. `/tmp/sotf-mbe-spectral-clippy.log`.
- `cargo run --release -p sotf-plugin-multiband-expander --features qa --bin qa-multiband-expander --offline`: passed expansion, latency, zero-allocation and performance diagnostics. `/tmp/sotf-mbe-spectral-release-qa.log`.
- Formatting and scoped diff whitespace checks passed.

## Files in this checkpoint

Production: `src/lib/spectral_state.rs`, `src/lib/multiband_expander_plugin.rs`.
Tests: `src/lib/tests.rs`, existing `tests/finite_stream.rs`, new `tests/spectral_finite_stream.rs`, new `tests/spectral_finite_allocations.rs`.
Docs: crate README/CHANGELOG and `audit/proposals/multiband-expander-spectral-finite-stream.md`.
Other dirty crate files belong to earlier accepted audit work and were not changed in this checkpoint.

## Limits

Spectral nonlinear startup intentionally changes because all preceding windows are now represented. The existing detector amplitude convention, hard gate-state threshold interaction with the soft knee, gain approximations, and time-domain recursive tail policy are unchanged. This work does not claim a new detector standard or proprietary processor parity.

## Independent reviews

Dynamics reviewed final source and startup/cache/lifecycle fixtures without new
Cargo execution and found no blocker. Root independently reviewed priming,
negative-prefix clearing, support and cached-output arithmetic. Root caught a
metadata-shortening edge during a nearly settled wet-to-dry drain; the permanent
red-to-green regression and frozen epoch declaration described above resolve it.
