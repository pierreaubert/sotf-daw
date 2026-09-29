# AUD-101 Beamformer covariance and spectral overload recovery

2026-09-28. Production, tests and documentation frozen. No host, GSC, tail metadata, native-wrapper, graph queue or engine protocol changes.

## Correction

- `src/mvdr.rs`: preserves ordinary f32 detector/products/recurrence ordering. A nonfinite FFT estimation frame leaves all learned covariance untouched. Overflowed finite detector powers/projections are retried in f64 with the existing threshold and floor.
- Covariance candidates use the existing fixed scratch matrix. Each entire frequency bin is checked before copying into retained history. Overflowed intermediate products receive a f64 retry using the exact promoted f32 smoothing constants. A candidate that still cannot fit f32 preserves its previous complete bin; valid neighboring bins continue learning. The dirty flag reflects committed bins.
- `src/lib/beamformer_plugin.rs`: the spectral OLA read emits finite stored values unchanged, substitutes zero for NaN/Inf, and clears/advances the same cell. This suppresses invalid output during numerical overload without changing latency, support, or the ordinary sample path.
- No new buffers, dynamic allocation, synchronization or parameter/API changes. Existing trace, loading, Cholesky and weight-normalization thresholds/fallback policies are unchanged.

This policy does **not** reconstruct physically correct waveforms beyond the internal f32 FFT range. It prevents nonfinite public spectral output and permanent covariance poisoning. Representable large covariance is retained and decays at the original 0.95-per-hop rate; recovery is amplitude dependent and is not promised to be rapid.

## Red evidence

`target/audit-beamformer-covariance-red.log`: four of five initial new core tests failed before the fix. Failures independently demonstrate:

- A raw outer product overflows although its weighted candidate should be finite (`Inf` versus `2.5000007e37`).
- The coherent-power decision changes under overflow.
- Invalid covariance replaces previous state instead of preserving the whole bin.
- After 4096 subsequent ordinary updates the poisoned core remains at weights `[.5,.5]`, versus `[.004950495,.9950495]` in the healthy reference.

`target/audit-beamformer-spectral-red.log`: both public tests failed. Finite extreme input emitted nonfinite audio; the one-sided-interference fixture retained RMS **.176776658635** versus the required **<.002**, with maximum waveform error **.247524718754**.

## Independent coverage

New `src/mvdr/recovery_tests.rs` contains seven tests:

1. Exact preservation of an unrepresentable complete covariance bin, with updates to valid neighbors and correct dirty flag.
2. Independent real-arithmetic f64 complex outer-product/weighted-recurrence oracle for 2/4/8 microphones.
3. Independent f64 power-ratio classification across opposed, coherent and partially coherent inputs at four amplitude scales.
4. NaN/+Inf/−Inf FFT evidence rejects estimation without changing covariance, followed by successful finite learning.
5. Recovery of unequal adaptive weights after an unrepresentable overload without reset.
6. Exact original f32 covariance recurrence for microphone counts 2 through 8, five bins and 400 updates.
7. Analytic f64 `.95^hop` decay of retained representable overload through 2048 hops, relative error below 5e−6.

New public `tests/spectral_recovery.rs` verifies continuing ordinary learning and exact subsequent reference behavior without reset; finite MVDR/Superdirective output for 2/8 microphones under coherent/opposed finite extreme input; exact zero after the unchanged support; ordinary delayed waveform restoration; irregular callbacks; reset and reinitialization.

`tests/stream_boundaries.rs` adds fresh-thread allocation and deallocation checks reaching the first wide detector, recovered wide candidate, rejected candidate, invalid FFT estimation, invalid OLA output, later finite processing and reset. Both counts are **0**. Existing cold lifecycle and full tail/geometry/support regressions also pass unchanged.

## Recovery duration: measured, not inferred from finite output

The public fixture uses a deterministic opposed 1e20 burst, then silence and one-sided 1.5 kHz ordinary interference. Representable huge bins are deliberately retained. The final oracle keeps its original strict **RMS <.002** and **maximum waveform error <2e−6** requirements; learning runs long enough for the unchanged decay to remove their influence.

| Subsequent learning hops | Output RMS | Healthy RMS | Temporary rejection loss |
|---:|---:|---:|---:|
| 16 | .011168749900 | .002331174026 | 13.608597 dB |
| 256 | .009066341370 | .001750264679 | 14.286567 dB |
| 512 | .009066345750 | .001750263959 | 14.286575 dB |
| 1024 | .009066345695 | .001750263954 | 14.286575 dB |
| 1536 | .008931384579 | .001750263954 | 14.156305 dB |
| 1800 | .001766378017 | .001750263954 | .079602 dB |
| 2048 | .001750264144 | .001750263954 | approximately .000001 dB |
| 2304 | .001750263965 | .001750263954 | approximately 0 dB |

Maximum loss over 4096-frame windows through 2048 hops: **14.286575 dB**. At 48 kHz and hop256, 2048 hops are **10.923 s**, 2304 hops **12.288 s**. Final maximum waveform error: **9.313225746155e−10**. These are this fixture's measurements, not a universal settling deadline.

## Ordinary comparison and CPU

An isolated optimized binary compares the preserved pre-change source with the current core, in the same process and using the same dependency artifacts. Nine cases: 2/4/8 microphones × amplitudes 1e−4/.25/100, each 512 updates and 257 frequency bins. Noise decisions, every computed weight, and every beamformed complex output are **exactly equal**; maximum output difference **0**.

Artifacts:

- Original source: `/tmp/sotf-beamformer-mvdr-aud101-before.rs`
- Comparison source: `/tmp/sotf-beamformer-aud101-compare.rs`
- Binary: `target/audit-tmp/beamformer-aud101-compare`
- Results: `target/audit-beamformer-covariance-comparison.log`

Dense active-adaptation core timings, median of eight trials with alternating run order after one discarded warm trial; 512 analysis frames ×257 bins:

| Microphones | Before | Corrected | Delta |
|---:|---:|---:|---:|
| 2 | 11.922167 ms | 12.585969 ms | +5.568% |
| 8 | 85.275316 ms | 94.039575 ms | +10.278% |

These are local core measurements, not an audio-thread deadline guarantee. Whole-plugin release QA also passed: 8-mic MVDR 512-frame p50/p95/max **.041/.080/.087 ms**, versus **10.667 ms** callback duration. That existing QA signal is coherent and is not the dense covariance-update workload, which is why the separate core measurement is reported.

## Gates

- **90 tests passed**, zero failures/ignored: 57 unit +3 GSC reference +15 integration +2 earlier numerical +2 spectral recovery +8 stream boundary +3 tail metadata. Log `target/audit-beamformer-covariance-final.log`.
- All-target/all-feature Clippy with `-D warnings`: `target/audit-beamformer-covariance-clippy.log`.
- Existing release QA all pass: `target/audit-beamformer-covariance-qa.log`.
- Rustfmt for all five Rust files in this change and scoped `git diff --check`: clean.

## Files in this change

Under `crates/sotf-plugins/crates/sotf-plugin-beamformer/`:

- Modified `src/mvdr.rs`, `src/lib/beamformer_plugin.rs`, `tests/stream_boundaries.rs`, `README.md`, `CHANGELOG.md`.
- Added `src/mvdr/recovery_tests.rs`, `tests/spectral_recovery.rs`.

The approved proposal is `audit/proposals/beamformer-covariance-recovery.md`. Other earlier uncommitted Beamformer changes remain preserved. No Cargo manifest or lockfile changes.

## Root review

Root reviewed the ordinary/wide detector branches, staged covariance commits,
scratch ownership and finite OLA read boundary. The preserved ordinary arithmetic
and explicit overload suppression match the approved scope. No introduced blocker
was found. The slower decay of large representable covariance is explicitly
measured above; finite output alone is not used as proof of restored rejection.
This correction follows the thirteenth aggregate checkpoint.
