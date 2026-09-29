# Beamformer numerical recovery — AUD-097 / AUD-098

2026-09-28. Production and tests frozen. No native tail metadata, host, engine, parameter schema, channel layout, geometry or dependency changes. Parent owns AUDIT.md. Proposal copied to `audit/proposals/beamformer-numerical-recovery.md`.

## Confirmed failures

AUD-097: public four-microphone broadside GSC accepts one finite frame `[A,-A,-A,-A]` for A=.75*f32::MAX or f32::MAX and returns NaN. All 4096 following zero frames and 257 small-signal frames remain NaN until reset. The blocking reference is mathematically 1.5*A; f32 overflow creates Inf, zero-weight*Inf becomes NaN, and the update retains NaN coefficients. Public processing validates dimensions/rate but has no finite amplitude ceiling. Half-MAX, .25 and1e-20 do not reproduce the failure.

AUD-098: initialized MVDR fed only zeros produces NaN after approximately4.5seconds of ordinary 48kHz processing. In281600zero frames, two/four/eight microphones produce10496/12288/14080NaN outputs respectively. Quiet covariance decays until the inverse coefficients are near1e19; f32 complex normalization squares the denominator and multiplies numerators to infinity. Later the existing Cholesky diagonal floor restores the fallback. Superdirective remains exactly zero.

Permanent red tests: `target/audit-beamformer-numerical-red.log`. Public standalone probes, original source snapshot, independent reference, artifact hashes and exact frame ranges: `/tmp/sotf-beamformer-tail-probe/`.

## Production changes

`src/gsc.rs`:

- Keep the f32 public API, steering/geometry coefficients, delay-ring samples, mu/delta values, update ordering and target-presence gate.
- Interpolate/accumulate in f64 and retain adaptive weights, reference FIR history and aligned/reference scratch in f64. Do not narrow a valid reference such as1.5*f32::MAX back into f32 storage.
- Commit finite candidate coefficients only. Invalid arithmetic cannot permanently poison retained weights; once finite reference history clears, zero input again produces zero.
- Convert at the final public sample boundary: saturate finite out-of-range output to f32 extrema, map invalid intermediate output to zero. Do not clip the internal adaptive reference.
- Adapt private weight injection/snapshot helpers and the existing leakage-test accumulator type to the actual wider state. Existing assertion thresholds remain unchanged.

Prepared additional scalar storage is268bytes for two microphones or1852bytes for eight, using the existing32taps; vector counts and allocation lifetime stay unchanged.

`src/mvdr.rs`:

- Keep covariance estimation, Cholesky solve and existing thresholds.
- Promote the solved denominator and numerator to f64 before complex division.
- Validate every converted f32 candidate in a fixed eight-element stack array before installing the frequency bin. Invalid candidates retain the existing steered delay-and-sum fallback for the entire bin.

Native `tail_length()` remains `Unknown`. Finite metadata requires a separate ordinary-processing review; these fixes do not change the frozen native-drain protocol.

## Exact files for this implementation

Modified:

- `crates/sotf-plugins/crates/sotf-plugin-beamformer/src/gsc.rs`
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/src/mvdr.rs`
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/tests/stream_boundaries.rs` (one additional cold numerical regression)
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/README.md`
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/CHANGELOG.md`

Added:

- `crates/sotf-plugins/crates/sotf-plugin-beamformer/src/mvdr/numerical_tests.rs`
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/tests/gsc_reference.rs`
- `crates/sotf-plugins/crates/sotf-plugin-beamformer/tests/numerical_recovery.rs`
- `audit/proposals/beamformer-numerical-recovery.md`

Other existing Beamformer worktree differences are the earlier verified startup/drain changes, preserved untouched by this task.

## Numerical evidence

Eight new tests:

1. Public finite-extreme GSC input and small-signal recovery without reset, with independent first-output mean and exact zero after finite history.
2. Public MVDR prolonged silence across the former NaN interval, two epochs, two/four/eight microphones, repeated17/997/256/73callbacks.
3. Direct f64 time-indexed GSC oracle:42configurations (2..8microphones, integer/fractional delays, .25/1e-20/.75*MAX scale), dense training, silent suffix and exact reset repeat. Independent projection uses aligned input minus its mathematical mean; no production matrix, rings, steering or coefficient helper.
4. Extreme GSC burst followed by small references compared to the same independent recurrence without reset.
5. Learned GSC cancellation deliberately exceeding f32 output range; verify the independent clipped finite result and subsequent recovery.
6. MVDR diagonal-covariance oracle:63configurations (2..8microphones ×9scales from1e-30 to1e18), complex steering, explicit diagonal inverse/normalization in f64, distortionless response and the existing low/high-scale fallback boundaries.
7. Invalid covariance replaces the complete MVDR bin with the existing steered fallback.
8. Fresh-thread first GSC extreme callback, first MVDR low-energy normalization, silence, reset and repeat all measure **zero allocations and zero deallocations** for four/eight microphones.

Measured independent-oracle errors:

- GSC maximum absolute error divided by input amplitude: **4.7457538315809344e-8**, bound2e-6.
- MVDR maximum complex-weight absolute error: **7.757480358958225e-8**, bound2e-6.
- All existing frozen-drain window/FIR/learned-state oracles retain their original thresholds and pass.

Ordinary-amplitude difference from unchanged production GSC, separately measured in a standalone binary compiling old and new kernels together:

- Fourteen cases:2..8microphones ×integer/fractional delays,32768dense samples each, input peak below.45.
- Maximum absolute difference: **3.725290298461914e-7**.
- Minimum SNR of old output to old-minus-new difference: **124.659dB**.
- This quantifies changed rounding; it is not a claim of bit equivalence or a target-rejection metric.
- Source/measurements: `/tmp/sotf-beamformer-tail-probe/compare.rs`, `gsc_baseline.rs`, `comparison.log`; binary in `target/audit-tmp/beamformer-compare`.

## CPU and memory implications

The direct kernel comparison used optimized code, prepared inputs, black-boxed samples/results and seven timing runs. Median time for32768dense samples with fractional delays:

| Mics | Original | Corrected | Ratio |
|---|---|---|---|
| 2 | 2.693ms | 3.010ms | 1.117 |
| 8 | 16.230ms | 17.766ms | 1.095 |

Existing release QA passes before and after. Eight-microphone GSC512-frame callbacks at48kHz:

- Original p50/p95/max: .162/.167/.167ms.
- Corrected p50/p95/max: .165/.169/.178ms.
- Deadline:10.667ms.
- Standard GSC QA estimated CPU: .27% -> .28% (5seconds audio in13.38ms ->14.07ms).

These local timings indicate increased kernel cost and substantial deadline margin on this machine; they are not cross-platform worst-case guarantees. The QA maximum-layout input is coherent and suppresses adaptation, while the standalone dense fixture exercises active adaptation.

## Verification gates

Root reviewed both production changes and the public/direct-reference tests;
a separate agent reviewed finite-state handling, normalization thresholds,
whole-bin fallback, oracle independence and cold checks. Neither found a
remaining blocker in AUD-097/098. Review notes:
`/tmp/sotf-beamformer-numerical-independent-review.md`. General spectral
FFT/OLA overflow under extreme input is a separate question for any future
finite-tail declaration; this report does not claim it was fixed.

- `cargo test -p sotf-plugin-beamformer --all-features -- --nocapture`: **76pass**, zero failed/ignored (49unit +3GSC reference +15existing integration +2numerical recovery +7stream boundaries). Log `target/audit-beamformer-numerical-full.log`.
- Parent review corrected the silence fixture's block-selection index from sample position to a separate iteration counter. Final focused two-test rerun: **2pass**, `target/audit-beamformer-irregular-final.log`. This change is included in strict Clippy below.
- `cargo clippy -p sotf-plugin-beamformer --all-targets --all-features -- -D warnings`: clean, `target/audit-beamformer-numerical-clippy.log`.
- Existing release QA: **ALL PASS**, `target/audit-beamformer-qa-baseline.log`, `target/audit-beamformer-qa-corrected.log`.
- Six changed Rust files pass rustfmt check; scoped git diff check clean.
- No native tail metadata changes. No new ongoing builds or source edits planned.
