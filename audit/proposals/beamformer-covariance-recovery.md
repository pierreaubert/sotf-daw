# AUD-101 — Beamformer spectral overflow and adaptive recovery

2026-09-28. Read-only design and isolated public probes. No production numerical edits made. Native-tail metadata is independently proved/implemented and reported in `/tmp/sotf-beamformer-native-tail-verified.md`; it is not a claim of adaptive quality recovery.

## Confirmed failures

### Persistent covariance poisoning without nonfinite audio

The current MVDR gate can accept finite opposed microphone spectra whose look-direction projection cancels, while individual outer products overflow f32. `alpha * Inf + finite` remains Inf; NaN covariance also persists. The validated weight solver keeps output finite by using delay-and-sum, but adaptation cannot recover without reset.

Isolated direct public core (`covariance_recovery.rs`): one opposed ±1e20 spectral frame, followed by 4096 ordinary small-noise updates:

- Poisoned weights: `[0.5,0.5]` indefinitely.
- Healthy weights: `[0.004950495,0.9950495]`.

Full public plugin: finite opposed broadband 1e20 burst, 8192 silent frames, then 65536 frames of one-sided 1.5 kHz interference:

- No nonfinite output was needed to trigger the defect.
- Poisoned output RMS .1767766586; healthy RMS .00175026468.
- Lost rejection **40.0864 dB**.
- Poisoned output matches the independent delayed half-left waveform within **1.19209e-7**.
- Reset restores RMS .00175027614.

Probe/log: `/tmp/sotf-beamformer-tail-probe/covariance_recovery.rs` and `.log`.

### Transient spectral nonfinite output

The existing f32 FFT and inverse FFT can overflow from finite extreme input; finite weight validation alone cannot prevent it. The prior 192-case ordinary-support probe records 76638 MVDR and 76882 Superdirective nonfinite transient samples. Finite history and OLA clearing restore exact silence inside the 1024 bound, but public audio is nonfinite during the overload.

Probe/log: `/tmp/sotf-beamformer-tail-probe/ordinary_support.rs` and `.log`.

## Retained-state inventory

| State | Update / risk | Required handling |
|---|---|---|
| `noise_cov` | Recursive f32 complex covariance; outer products or mixed candidates overflow and poison all later updates | Never install a nonfinite matrix candidate; preserve complete prior frequency-bin covariance |
| `weights_buf` | Retained normalized MVDR coefficients | AUD-098 already installs only fully validated finite bins or finite steering fallback; preserve it |
| `target_presence_threshold` | Scalar construction default .65; public low-level control only | No automatic update, no new detector-history field |
| `frame_count`, `weights_dirty` | Integer / bool bookkeeping | Preserve accepted-noise-frame sequencing; mark dirty only when a covariance bin was committed |
| Total/look energy, projection, coherent fraction | Current-call local scalars, currently f32; overflow may change noise classification | Compute a wider fallback before classifying an overflowed current frame; do not retain Inf gate/floor state |
| `scratch_outer`, loaded/factor/solve scratch | Rewritten workspaces, not learned history | Use prepared scratch for validation; invalid scratch must not enter covariance or retained coefficients |
| STFT input/FFT buffers, OLA ring | Finite audio history; FFT results may be nonfinite during overload | Define public output policy explicitly; histories already clear inside support |
| Superdirective coefficients | Fixed prepared finite regularized solve | No learned state or asynchronous update; share the spectral output policy |
| GSC state | Separate f64 adaptive path, fixed by AUD-097 | Unchanged by this proposal |

There is no separate smoothed noise-floor, DTD, PSD or detector-history array in MVDR. The persistent floating-point estimator requiring protection is `noise_cov`. Covariance/solver conditioning and scalar floors are distinct from the native audio-support bound.

## Recommended narrow recovery patch, pending review

### 1. Range-safe current-frame noise decision

Keep the existing ordinary f32 path and comparison when all detector terms are finite. If total energy, look energy, any projection or the computed coherent ratio is nonfinite, recompute the current-frame powers/projections in f64 from the original complex f32 bins and finite steering components before applying the same .65 decision and existing 1e-20 floor.

For finite f32 bins and at most eight microphones / 257 bins, f64 products and sums have ample range. A current FFT frame containing NaN/Inf is unusable for estimating covariance: return no update for that frame and preserve prior finite state. This is not a reset and does not allocate.

Keeping a finite ordinary fast path preserves existing rounding and gate behavior away from overflow. The wide path should be explicit and tested at the original opposed-input case; do not reinterpret Inf/Inf as clean speech or as a valid stored floor.

### 2. Validate a whole covariance bin before publication

Use existing fixed-size `scratch_outer` storage as the candidate matrix. For each accepted-noise frequency bin:

1. Calculate the ordinary f32 outer product and recurrence in the existing order, writing the candidate to scratch while checking every component.
2. If every component is finite, copy the complete candidate bin into retained covariance. Ordinary arithmetic stays unchanged.
3. If any component is nonfinite, recompute the candidate bin in f64 from the previous finite covariance and original finite input bins. Apply the existing f32 alpha and one-minus-alpha values promoted exactly to f64. This recovers cases where an intermediate outer product overflows but the weighted final recurrence still fits f32.
4. Convert and validate the entire wide candidate. If it fits f32, commit the entire bin; otherwise discard that bin's update and retain the prior finite covariance. Never reset it to identity or partly install entries.

Skipping an unrepresentable update deliberately forgoes learning from that frequency bin during numerical overload. It preserves prior learned state and allows the next representable ordinary frame to resume adaptation. Other representable bins may continue learning. Keep the update dirty flag consistent with actual committed bins.

This is the smallest correction that preserves the existing covariance type, all time constants, support, gate semantics at ordinary levels, and reusable solver. It also avoids allocating a second full covariance matrix.

The existing f32 trace can overflow when several individually finite diagonal entries are large. It is temporary, so it cannot poison retained state, but unnecessarily forces fallback. A narrow overflow-only f64 trace/sigma calculation can avoid that spurious fallback while preserving the ordinary path. If a loaded matrix still cannot fit f32, retain the existing finite solver fallback. Do not silently change its existing diagonal/denominator thresholds in this patch; those conditioning choices are a separate accuracy policy.

### 3. Explicit finite public-output policy

Recommended smallest policy: at the existing OLA read-and-clear boundary, output the stored sample when finite and output zero when NaN/Inf. Always clear and advance the same cell. This applies identically to MVDR and Superdirective, preserves support and timing, and keeps valid ordinary samples bit-identical. It adds a bounded scalar check, with no error allocation or partial-block rollback.

This policy suppresses invalid spectral output samples; it does **not** preserve the physically correct amplitude of every finite extreme input that exceeds internal FFT range. Document that numerical-overload limitation. The covariance guards prevent permanent loss of subsequent ordinary adaptation, which the current implementation fails to do.

Do not claim that zeroing invalid output is equivalent to a full-range FFT solution. If preserving extreme waveform fidelity is required for AUD-101 closure, use the larger alternative below and retain the narrower change as an explicitly scoped recovery policy.

## Alternatives considered

| Alternative | Advantages | Costs / limitations |
|---|---|---|
| Validated covariance updates + finite output boundary (recommended narrow scope) | No covariance-layout change, preserves ordinary arithmetic, recovers learning after overload, bounded zero-allocation callback | Skips unrepresentable covariance updates; suppresses invalid spectral output rather than reconstructing its correct amplitude |
| Unconditionally widen covariance and solve | Better dynamic range and direct mathematical continuity | Doubles the large 257-bin covariance/work state, changes every solve/update and ordinary rounding; FFT overflow still remains |
| Per-bin scaled covariance with retained f64 scale | Keeps well-conditioned f32 normalized matrices and represents wide powers | New scale state and rescaling rules across smoothing/diagonal floors, larger algorithm change; needs a separate direct-matrix oracle and performance review |
| Emergency normalized FFT path with wider spectral/OLA reconstruction | Can preserve finite extreme input waveform up to the f32 output boundary | Additional prepared wide spectrum/OLA state, analysis and inverse scaling, coherent multi-mic scaling, and coefficient/covariance scale rules; changes ordinary OLA rounding unless a second exceptional path is maintained |
| Reject before mutation | Clear failure contract without emitting invalid samples | Requires a conservative amplitude limit derived from FFT, weights and OLA arithmetic, or a duplicate dry-run; rejects currently accepted finite input and can cause allocation/error paths in native audio callbacks |

The recommended policy is compatible with existing accepted finite input: it does not add a new process error or amplitude cap. It must be described as finite-output/adaptive-recovery behavior, not unrestricted spectral precision.

## Required independent tests

1. Turn the direct covariance and public delayed-half-left red probes into regressions. After unrepresentable finite input, ordinary one-sided interference learning must converge to the healthy reference without reset, recovering the lost rejection. Confirm updates are not merely disabled forever.
2. Test a representable weighted covariance whose raw f32 outer product overflows, against an independent f64 one-step recurrence. This distinguishes the wider retry from blanket large-bin rejection.
3. Snapshot prior covariance in private tests; an unrepresentable update preserves the entire bin exactly while valid neighboring bins update. All retained matrix and weight components stay finite.
4. Test the noise decision on exactly opposed, coherent and partially coherent finite bins at small/ordinary/huge levels. An overflowing projection must not silently become a different detector class. Include invalid FFT frames and subsequent clean learning.
5. Public spectral processing at finite extreme levels returns finite output for all callbacks and recovers an ordinary deterministic waveform without reset. Verify unchanged support, latency, arbitrary block partitioning, and reset/reinitialization. Do not encode a requirement that transient NaNs exist.
6. Compare ordinary output and covariance against the unchanged checkpoint across a deterministic range/geometry matrix. Preserve existing accuracy thresholds; quantify differences if the implementation cannot keep the finite path unchanged.
7. Reuse cold allocation/deallocation checks, explicitly reaching first wide-detector, wide-candidate, rejected-candidate and finite-output branches. Full Beamformer tests, strict Clippy and existing release QA; compare dense active-adaptation CPU cost against the frozen numerical checkpoint.

Production changes would be confined to `src/mvdr.rs`, the spectral output-read branch in `src/lib/beamformer_plugin.rs`, Beamformer tests and documentation. No metadata, GSC, host, engine, queue or native wrapper changes are required for the narrow policy.
