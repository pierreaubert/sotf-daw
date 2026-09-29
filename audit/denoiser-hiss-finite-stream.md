# AUD073 / AUD084 / AUD085: Denoiser and spectral Hiss finite-stream corrections

Verified 2026-09-28. Source frozen after focused gates. Changes are restricted to
`sotf-plugin-denoiser` and `sotf-plugin-hiss-reducer` source/tests/docs. No shared
host, plugins-denoiser backend, manifest, SpeechDenoiser, MIDI, or IAMF edits.

## Implemented contracts

- Denoiser N=512/2048, H=N/2: prime one negative-time sqrt-Hann analysis window,
  discard its negative synthesis prefix, publish output-ready hops only from
  origin0, preserve N startup frames and N reported latency. All source samples
  are consumed before output writes; existing callback capacity is preserved.
- Denoiser note detection now consumes exactly the source slices reaching each
  large FFT boundary. The former whole-callback feed exposed future notes and
  caused max waveform differences0.0873–0.1186 in all four FFT/multi-resolution
  configurations; corresponding PND-disabled controls were bitexact. The same
  public partition regression is now green. Small multi-resolution analysis
  remains gain-only and is fed accepted source, without synthetic prehistory.
- Denoiser and spectral Hiss render the zero-input continuation through
  `E=2N+floor((T-1)/H)*H`, with drain suffix `E-T` after T>0 source frames.
  Maximum tail metadata is2N−1; prepared per-call cache isH frames/channel.
  Hiss retains its existing correct N1024/H256 scheduler and latency.
- Only explicit Denoiser noise-profile capture accumulation is frozen during
  drain. Its count, target, partial accumulator, stored profile, capture-active
  flag and profile settings remain untouched. Adaptive audio gains/noise
  continue normally. Reset follows prior capture semantics: discard partial
  measurement and preserve current stored profile/settings.
- Empty stream completes without closing input. First successful nonempty
  drain closes input/control mutation until reset/reinitialize; zero-frame
  processing remains a no-op. Wrong rates or pending zero/unaligned destinations
  do not consume audio. Completion is stable, unused destination samples retain
  sentinels, and reset/rate reinitialization match fresh processing.
- Denoiser reinitialize resets retained program state, preserving profiles;
  process contexts must match its configured sample rate.
- Conventional Hiss remains Unknown tail, max-drain0 and legacy COMPLETE0.
  No new errors or restrictions are imposed on its drain/playback/control path.
  Its recursive audio tail remains explicitly unresolved.

## Independent evidence

Denoiser integration tests:

1. Every initial-window phase for N512/2048: 2,560 paired first/final impulse
   cases, including T1, varied callback partitions and drain capacities, reset
   before every case. Oracle is exact source shifted by reported N; |error|<2e-6.
2. Dense unity in six FFT/layout combinations (1/2/6 channels), exceeding10N
   source frames and crossing output-ring wraps. Same independent delay oracle.
3. PND/MR callback partition eight-mode matrix, 32,768-frame changing-note
   program. Large4096 callbacks agree with1/7/137 callbacks within2e-6.
4. Nonlinear zero continuation across24 FFT/MR/PND/layout combinations. Exact
   derived output count, separate callback partitions, and extra5N silence past
   endpoint establish retained transform support without reading internal windows.
5. Eight-mode lifecycle/error/reset/reinitialization matrix with untouched twins.
6. Explicit cold allocator/deallocator measurements for24 fresh-thread mode/
   layout configurations, including first-frame EOF, first transform in drain,
   UI publication boundaries, reset and repeat. Every measured count is0/0.

Denoiser private-state regression:

- Eight FFT/MR/PND cases begin explicit capture with an existing stored profile,
  exactly one measurement frame short of completion. Drain audio matches ordinary
  zero continuation on a twin whose explicit capture is frozen. Every measurement
  and profile array/count remains unchanged through drain; reset clears only the
  incomplete capture. It cannot accidentally finish capture on padding.

Hiss spectral integration tests:

1. Every256-frame hop phase × 1/2/6 channels × enabled/disabled =1,536 paired
   first/final marker cases at strength0, checked against1024-frame delayed source.
2. Three nonlinear layouts versus separate zero continuation, continuing4096
   frames past endpoint to catch stale ring data.
3. Invalid input/output/control lifecycle, reset/rate initialization, metadata,
   and explicit legacy classic-mode continuation checks.
4. Six cold mode/layout configurations, first-frame EOF and longer transforms,
   completion/reset/repeat: explicit0 allocations and0 deallocations.
5. Three live bypass/strength transition cases spanning EOF, bitexact with
   separately zero-padded processing at different callback partitions.

No tolerance was weakened and no test is ignored. Existing suites also pass.
The process allocation measurements are on the current Linux build; Denoiser's
realfft convenience entry points currently need zero scratch for these prepared
power-of-two plans. No universal claim about arbitrary future FFT plans is made.

## Verification

- `cargo test --offline -p sotf-plugin-denoiser -p sotf-plugin-hiss-reducer --lib --tests`
  **117 passed, 0 failed, 0 ignored**; `/tmp/sotf-denoiser-hiss-eof-tests.log`.
- `cargo clippy --offline -p sotf-plugin-denoiser -p sotf-plugin-hiss-reducer --all-targets --all-features -- -D warnings`
  passes; `/tmp/sotf-denoiser-hiss-eof-clippy.log`.
- rustfmt and scoped `git diff --check` pass.
- Baseline failing public regression log: `/tmp/sotf-denoiser-hiss-eof-red.log`
  (Cargo stopped at the four failing Denoiser tests; Hiss loss was demonstrated
  by the earlier inventory/public zero-continuation probe, not this test log).
- Pre-fix PND public probe: `/tmp/sotf-denoiser-pnd-partition-probe.rs` and `.log`.
- Read-only derivations and unresolved recursive/timing findings:
  `/tmp/sotf-denoiser-followup-plan.md`, `/tmp/sotf-denoiser-tail-probe.rs` and `.log`.

## Remaining scoped findings

AUD083 SpeechDenoiser still has wet main-impulse delay960 versus dry480 while
reporting480, and its highpass creates recursive tails. It remains unchanged
pending a complete alignment/compatibility design. Conventional Hiss recursive
continuation likewise remains unchanged; do not mark all Hiss modes EOS-complete.

## AUD088 — optional separator reset follow-up

Independent review enabled harmonic/percussive separation through the public
scalar setter and found retained history across reset. Warm tonal input followed
by reset and a different program differed from a fresh configured instance by
up to 0.00986868 in the permanent regression. The existing configuration struct
silently ignores this option, so JSON-only fixtures did not enable the affected
mode; that separate persistence gap is AUD089.

Reset now calls each existing separator's allocation-free reset and clears three
shared scratch arrays. Backend implementation, settings and captured profiles
are unchanged. Twelve FFT/layout/reset-or-reinitialize cases match fresh output
bit for bit, and six warmed-history resets on fresh threads allocate/free zero.
The final all-feature Denoiser suite passes **78 tests**, with zero failures or
ignores; strict all-target/all-feature Clippy and scoped formatting pass.
Logs: `/tmp/sotf-denoiser-hpss-reset-{red,green,full,clippy}.log`.
