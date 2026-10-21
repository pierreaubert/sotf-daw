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
- Share the contralateral transfer/LF blend between both geometry paths and
  diagnostics. Pinna shaping follows the blend; geometric delay owns ITD once.
- Keep mirror, near-center, independent arithmetic, rendered two-column routing,
  callback partition, reset, latency and zero-heap regressions. A finite pose
  change legitimately changes coefficients: distinguish that first-order response
  from the symmetric/full-branch limit and retain the raw differences.
- Fixed speaker Cartesian coordinates now subtract listener X/Z before deriving
  head-center range and incidence. Positive Z points toward speakers; positive yaw
  turns right and must not change center range. Keep effective symmetric depth
  incidence shared by pinna shaping and diagnostics.
- The legacy diffraction model still uses center ranges as path references and
  fixed left/right ipsilateral assignments. Large-yaw ear reclassification,
  ear-surface paths and measured calibration remain open. Cartesian arithmetic and
  routing tests do not resolve those physical-model limitations.
