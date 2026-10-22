# sotf-plugin-xtc

Crosstalk cancellation plugin for binaural-like playback over stereo speakers, using frequency-domain inverse filtering with physical head/speaker modeling.

## Architecture

```
src/
  lib.rs         -- XtcPlugin (Plugin), XtcData, main processing with overlap-add
  config.rs      -- XtcPluginParams, geometry and algorithm configuration
  filters.rs     -- HrtfTransferFunctions, XtcFilters, geometry cache, inverse filter computation
  reflections.rs -- Room reflection modeling (image source method and IR-based)
  validation.rs  -- Filter validation utilities
  params.rs      -- Centralized parameter specs
  tests.rs       -- Integration tests
```

**Algorithm pipeline:**
1. Stereo input deinterleaved and windowed (Hann, 75% overlap)
2. Forward FFT to frequency domain
3. Compute/apply XTC inverse filter matrix: models ipsilateral (direct) and contralateral (crosstalk) speaker-to-ear paths
4. Regularized inversion with frequency-dependent smoothing to avoid excessive boost
5. Optional room reflections (image source or IR-based)
6. Auto-gain normalization
7. Built-in limiter envelope to prevent clipping
8. Inverse FFT + overlap-add
9. Output as stereo (2 channels)

**Physical model:** Speaker geometry (distance, angle), head radius, head shadowing filter. Computes ipsilateral and contralateral path lengths and time delays for the inverse filter.

**Key types:**

- `XtcPlugin` -- Main plugin implementing `Plugin`. 2 input -> 2 output channels. Decomposed into cohesive sub-structs aligned with processing stages: `XtcFftConfig`, `XtcInputBuffers`, `XtcWorkBuffers`, `XtcFilterState`, `XtcOutputBuffers`, `XtcDynamics`, and `XtcDiagnostics`.
- `XtcPluginParams` -- Config: speaker distance/angle, head radius, regularization, HRTF options, room reflections.
- `XtcData` -- Monitoring data: auto-gain info, limiter envelope.
- `XtcFilters` -- Pre-computed frequency-domain XTC filter matrix, with an audio-owned current snapshot and bounded worker publication/retirement exchange.

## Key Public API

- `XtcPlugin::new(params) -> Self` (`lib.rs`)
- `XtcPlugin::from_params(params) -> Self` (`lib.rs`)
- Exposes `XtcData` via `analyzer_data()` for UI monitoring
- Implements `Plugin` trait: 2 input -> 2 output channels

**Parameters:** `distance_m`, `angle_deg`, `head_radius_m`, `regularization`, `low_shelf_gain`, `high_shelf_gain`, auto-gain settings, room reflection settings.

## Testing

```bash
cargo test -p sotf-plugin-xtc
```

Benchmarks:
```bash
cargo bench -p sotf-plugin-xtc --bench xtc-validation-benchmark
```

## Important Notes

- XTC filters are recomputed when geometry parameters change. A background worker publishes into a prepared bounded exchange. The callback uses `try_lock`, keeps its current filters under contention, and postpones adoption when retirement capacity is full. Filter and auxiliary-data destruction stays off the audio thread.
- Regularization prevents excessive filter boost at frequencies where the inverse is ill-conditioned. Higher values = safer but less cancellation.
- The built-in limiter envelope prevents output clipping that can occur with aggressive cancellation settings.
- The finite-attack safety ceiling must use a shared gain over every output
  channel. Independent clipping changes the speaker-drive ratios needed by XTC.
  The final scalar clamp is only a floating-point rounding guard.
- SIMD operations: `complex_mul_simd`, `complex_mul_add_simd`, `deinterleave_stereo`, `window_mul_simd`.
- Head shadowing is modeled as a frequency-dependent lowpass filter based on head radius and angle.
- Analytical frequency-region joins must preserve magnitude continuity; test
  both sides of ka=0.5 and ka=2 across head radii and ear-incidence angles.
- Independent inverse/mismatch tests must retain signed crosstalk degradation.
  A shared design-model diagnostic is not independent acoustic validation.
- Room reflection support via two methods: image source (synthetic, from geometry) or IR-based (from measured impulse responses loaded via Symphonia).
- Uses a geometry cache (`compute_geometry_cache`) to avoid redundant trigonometric calculations when parameters haven't changed.
- FFT size 2048 by default, 75% overlap (hop size 512) for overlap-add processing.

## Cancellation diagnostics

- Keep `predict_analytic_cancellation` labeled as shared-model filter prediction. Evaluate the plant at the selected FFT bin and configured FFT size; preserve both input cues and asymmetric matrix columns.
- Use `evaluate_cancellation` for supplied independent plant/filter matrices. Rows are ears/speakers for the plant and speakers/intended inputs for the filter; entries are `[real, imaginary]`. Preserve negative/infinite/undefined values instead of clipping them to a display range.
- Always retain intended gain and normalized separation alongside raw attenuation. Muting is not successful separation. No measured provenance is implied by accepting an external matrix.
- Resource-dependent HRTF/RoomEQ/room configurations need an actual resource snapshot, not a silent analytical fallback. Default 8 kHz ILD mismatch remains open; report-structure/stability tests do not certify it.
- `tests/audit_cancellation.rs` measures actual Rust plugin speaker outputs against controlled point-source/echo fields. It does not certify native ABI, measured human HRTFs, device deadlines or listener quality.

- Prediction helpers allocate filter arrays; keep them on the control/analysis thread. The pure supplied-matrix evaluator is separate from the rendering hot path.

## Analytical pose invariants

- Any nonzero lateral offset or yaw selects the asymmetric matrix. Positive X
  points right, matching the room-reflection coordinates. Exact centered/yaw-zero
  geometry retains the optimized symmetric path.
- Share the continuous four-path analytical plant between symmetric/full design
  and diagnostics. Source projection selects ear incidence and the extra far-ear
  arc; matrix indices identify ears/speakers without assuming fixed near ears.
- Normalize both paths of each source by its near-ear shadow/pinna response.
  Use the mean source-center range as a common smooth amplitude/time reference;
  taking the minimum creates a cusp at the centered pose. The explicit LF mode
  blends geometric/shadow magnitude toward unity before pinna shaping.
- Keep mirror, near-center, independent arithmetic, rendered two-column routing,
  callback partition, reset, latency and zero-heap regressions. A finite pose
  change legitimately changes coefficients: distinguish that first-order response
  from the symmetric/full-branch limit and retain the raw differences.
- Fixed speaker Cartesian coordinates now subtract listener X/Z before deriving
  head-center range and incidence. Positive Z points toward speakers; positive yaw
  turns right and must not change center range. Keep effective symmetric depth
  incidence shared by pinna shaping and diagnostics.
- Center-range plus spherical extra-arc paths remain an approximation, not exact
  finite-distance ear-surface acoustics or measured human HRTFs. Per-source
  near-ear normalization intentionally changes centered/default coefficients.
  Preserve historical default and continuous-path fixtures beside the current
  coordinated-limit fixture.
- Prepare geometry and inverse arithmetic in f64 off the callback. Compute the
  small singular value through the determinant/product relation rather than
  subtracting nearly equal eigenvalue roots. Preserve mirror/scale invariance,
  relative singularity checks and the existing gain limits.
- Retain quarter-turn equal-column and front/rear median-crossing regressions.
  An antipodal sphere without pinna cues must give equal source columns at a
  quarter turn; this rank-deficient geometry cannot provide independent ear cues.
- Keep prior mismatch/report failures as historical evidence. Shared-model
  predictions and spherical symmetry tests do not establish measured robustness,
  reconstructed true peaks, native deadlines or listening quality.

## Prepared inverse gain invariants

- Limit inverse and normalized filter columns with a shared real gain over both
  speaker coefficients. Keep the half-ceiling tanh knee and coefficient ceiling;
  independent coefficient knees rotate a cancellation steering vector.
- Apply the final per-bin effort budget with one common matrix gain from the
  larger speaker-row power. Preserve both input columns and every row budget;
  do not independently scale speaker rows. Unused quieter-row headroom is valid.
- Widen row-power and scaling arithmetic so finite f32 extremes cannot overflow
  past the budget. Preserve quiet-column identity, the legacy peak-knee curve,
  independent Cholesky steering comparisons and asymmetric/extreme effort tests.
- A gain cap does not prove measured cancellation or true-peak safety. The
  intentional identity blend can change steering; retain its nominal failures
  and every negative mismatch row without changing thresholds to clear a report.
