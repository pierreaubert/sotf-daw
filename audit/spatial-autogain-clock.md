# AUD115 — shared spatial AutoGain clock: verified checkpoint

Status: implementation and test sources frozen. No host queues, engine protocol, native wiring, MIDI/IAMF, or decorrelation geometry changes. Approved plan: `audit/proposals/spatial-autogain-clock.md`. Independent caller/source review: `/tmp/sotf-spatial-autogain-callers-verified.md` (plugin_chain).

## Production changes

- `sotf-host/src/multichannel_auto_gain.rs`: additive `measure_aligned_and_apply` consumes aligned stereo input and raw output together. It folds/ingests bounded spans, applies the preserved scalar gain recurrence, then refreshes the target every `max(1, sample_rate/10)` active frames. A refreshed target first affects the next frame. Disabled/empty calls do not advance this clock; reset/rate preparation restart its phase. Existing gain and setter semantics remain intact.
- Its older separate input/output methods retain their block refresh schedule. Legacy output now ingests bounded folded spans with exactly one refresh after the original callback, before the original gain loop. No scratch growth above8192. Speaker order, center split, actual mono/stereo width, preview handling, LFE meter exclusion and all-channel gain application remain unchanged.
- The actual pinned `math-dsp::EbuR128` accumulates peaks across ingestion calls until query/reset. The initial concern based on the `prev_sample_peak` name was incorrect; all provisional local peak fields were removed. Permanent peak-marker tests pass using the existing accumulator only.
- AAE removes the early whole-input refresh and uses the paired pass after its unchanged acoustic renderer, before final limiter and audible bypass. ER/FDN feedback is untouched; initialize retains the existing gain history via `set_sample_rate`.
- Upmixer prepares AutoGain even when initially disabled and synchronizes constructor JSON enabled/max/smoothing values. Its new stereo N-frame reference ring supplies x[n−N] before overwriting with x[n], including callbacks longer than N. The reference advances while AG is disabled. Direct bypass uses x[n]; existing bypass reset, FFT resize, initialize and reset clear ring state. The raw renderer still runs once per original callback.
- Upmixer's canonical native drain uses the same paired pass when generating cached frames; copying a partial cache does not advance the clock. No tail-length or work-quota changes.

Changes are confined to the shared helper and AAE/Upmixer sources, new tests, their documentation, and the proposal. Existing unrelated dirty changes were preserved.

## Red evidence and exact compatibility

1. `/tmp/sotf-multichannel-autogain-legacy-red.log`: normal callbacks match an independent direct scalar AutoGain/fold driver exactly. 8193frames panics before the fix (`capacity16384 < required16386`). Afterward normal and32769-frame legacy callbacks both match exactly, including one original output refresh.
2. `target/audit-spatial-autogain-callers-red.log`: four permanent public-constructor timing regressions failed. At137vs512, AAEmax0.00013354793 and Upmixermax0.001336515. Same-prefix/different-future errors were AAE0.000041787513 and Upmixer0.00024104957, starting at outputsample30. The original public bridge probe additionally measures137vs8192 maxima0.0028697848/0.017378747; provenance remains `/tmp/sotf-spatial-autogain-clock-manifest.json`.
3. `target/audit-spatial-autogain-callers-green.log`: all7 public caller regressions pass **exactly** after the correction, including fixed-position enabled/max/smoothing controls, reset and warm Upmixer native EOS versus ordinary zero continuation at capacities1/17/hop.
4. AG-disabled pre/post raw audio is exact for1,728,102samples per plugin: AAE digest`e08772984d750b54`, Upmixer`77ea0858430cdf3d`. Saved before-edit evidence: `target/audit-spatial-autogain-callers-baseline.log`.

## Independent numerical, lifecycle and realtime coverage

- Host `tests/multichannel_autogain_clock.rs`:11tests. Independent fixed-boundary scalar driver, 44.1/48/96/192k, mono/stereo/5.1/7.1.4 plus actual-stereo preview;1/17/137/512/8193/32769frame schedules; both settled gain directions with amplitude-ratio oracle and0.005dB tolerance; exact center±10° fold controls, LFE/missing-channel negatives; future-prefix causality; paused state, same snapshots, reset/rate, invalid shape canaries; early peak markers, incomplete future interval and oversized legacy full-callback peaks.
- Host `tests/multichannel_autogain_heap.rs`: cold paired/legacy ingestion and true meter publication, setters, reset, >8192frames and192k intervals; zero allocations **and frees**.
- AAE `src/lib/autogain_state_tests.rs`: final AG changes output but does not change later raw acoustic history after disabling it; limiter remains inactive in this oracle. Initialize preserves a genuinely nonzero compensation value and reset clears it.
- Upmixer `src/lib/autogain_reference_tests.rs`:72prepared configurations (FFT256/512/1024/2048/4096/8192 ×44.1/48/96k ×HR/preview combinations). Every accepted frame is emitted; exact ring contents are checked before and after enabling, including8193-frame calls, startup/wrap, malformed output, empty call, reset and geometry change. Twelve neutral true-WOLA cases include FFT64 and192k: full waveform agrees with independent N-frame delayed identity within2e−6, compensation stays below0.0001dB, and level transitions do not create a false alignment correction. Direct bypass uses current source and clears ring state across mode changes.
- New public `tests/autogain_clock.rs` files (AAE3/Upmixer4) are independently owned/reviewed by plugin_chain. These cover default raw paths and fixed source schedules; they do not claim all unrelated raw parameter automation is callback-invariant.
- Each caller's `tests/autogain_heap.rs` measures a fresh callback thread at48/192k, initially off/on,32769frames, through first and subsequent meter refreshes, prepared enable/max/smoothing updates, reset and Upmixer EOS. Audio processing/reset/drain are0allocations/0frees.

Public setter work is measured separately, retaining the Arc-backed parameter ID owner outside the guard. AAE enable/max/smoothing each measure0/0. Upmixer each measures143allocations/143frees from the existing `rebuild_cached_parameters` hook; the immediately following process call is0/0. No claim that those public Upmixer setters are realtime-safe. Log: `/tmp/sotf-spatial-autogain-heap-final.log`.

## Gates

Commands use `TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-tmp` and the shared target.

- `cargo test -p sotf-host -p sotf-plugin-aae -p sotf-plugin-upmixer`: **927passed,0failed,8existing ignored doctests**. Host682including1doctest, AAE94, Upmixer151. This run also includes root's current AUD116 spectrum tests. Log `/tmp/sotf-spatial-autogain-full.log`.
- `cargo clippy -p sotf-host -p sotf-plugin-aae -p sotf-plugin-upmixer --all-targets -- -D warnings`: passed. Log `/tmp/sotf-spatial-autogain-clippy-final.log`. The first lint run found one test-only `1*6` identity operation; fixed without numerical changes.
- Focused host final12tests: `/tmp/sotf-multichannel-autogain-final-focused.log`.
- Focused private caller5tests: `/tmp/sotf-spatial-autogain-private-final.log`.
- Scoped Rust formatting and `git diff --check`: pass. No manifest/dependency changes. No optional ONNX feature build or universal feature-matrix claim.

## Cost and remaining limitations

- Reference ring:8Nbytes,16KiB atFFT2048, plus its fixed descriptor/cursor. Upmixer now also prepares the existing helper while disabled:64KiB stereo scratch plus its existing loudness-monitor storage. This changes constructor memory; the16KiB ring is **not** the entire disabled memory increment.
- Processing does bounded linear work; disabled Upmixer adds source-ring copy traffic. Derived loudness statistics refresh at10Hz of enabled frames instead of once per callback. The preserved per-frame scalar smoothing cost is unchanged.
- **Matched before/after CPU timing has not been measured.** Frozen raw digests establish waveform compatibility, not CPU cost. Remaining measurement: preserve/rebuild the pre-AUD115 artifact, run identical rate/layout/FFT/callback/input/enable schedules with the same optimized profile and warmup, report enabled steady work, refresh-boundary callbacks, and disabled ring-copy overhead separately. No percentage speedup or deadline claim is made from test wall time.
- An active Upmixer final safety cap still computes a whole-callback peak/release and can independently create partition/future-prefix dependence. Other callback-sized raw smoothers remain outside AUD115. Exact public timing tests use limiter/cap-inactive levels. AAE's recursive native drain policy is unchanged.
- A preexisting small-FFT/default-decorrelation panic found during the matrix is recorded separately for AUD118 in `/tmp/sotf-upmixer-small-fft-panic.md`; no decorrelation changes were made. Valid neutral FFT64 is retained in115coverage.

Audible compatibility: enabled AutoGain now follows the causal fixed measurement clock, so old presets can sound different from their former callback-dependent gain trajectory. Parameter IDs/defaults, raw AG-off audio, fold objective, scalar smoothing, declared latency, feedback, safety-stage order and finite Upmixer output counts are preserved.
