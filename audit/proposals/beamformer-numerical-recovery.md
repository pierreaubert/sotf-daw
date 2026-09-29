# Beamformer ordinary-processing numerical blockers and repair plan

2026-09-28. Read-only investigation; no repository source/test edits and no Cargo invocation. Native tail metadata remains `Unknown` until the numerical fixes and ordinary-zero proof are reviewed.

## Reproductions

The standalone public-API probes link existing workspace rlibs. Both use `BeamformerPlugin::from_params`, `initialize` and `Plugin::process`; every processing call returns `Ok(frames)`.

### AUD-097: GSC finite input permanently poisons adaptive state

`main.rs` / `red.log`: four microphones, broadside, 48 kHz, GSC, one frame `[A,-A,-A,-A]`.

| A | First output | Nonfinite outputs in 4096 zero frames | Nonfinite outputs in following 257 small-signal frames |
|---|---|---|---|
| .25 | -.125 | 0 | 0 |
| 1e-20 | -5e-21 | 0 | 0 |
| .5*f32::MAX | -8.5070587e37 | 0 | 0 |
| .75*f32::MAX | NaN | 4096 | 257 |
| f32::MAX | NaN | 4096 | 257 |

Reset recovers all cases. Every input is finite. An independent f64 implementation of the four-microphone blocking projection and NLMS recurrence produces finite output and exact zeros after 31 continuation frames. The .25 and 1e-20 cases match its entire impulse-plus-silence output exactly.

Public input contract: `beamformer_plugin.rs:437` calls `plugins_spatial::validate_interleaved_io`, which checks buffer sizes only (`plugins-spatial/src/lib.rs:13`). No amplitude restriction or clipping is specified or enforced. The GSC path directly copies input samples. Thus these finite inputs are accepted; this is not a rejected NaN-input test.

Cause: `gsc.rs:174-180` accumulates the first blocking row `[.75,-.25,-.25,-.25]` into f32. Its exact reference is 1.5*A, which overflows for the final two cases. At `:194`, the initial zero weight multiplies infinity. The NaN error enters the unguarded NLMS update at `:210-220` and contaminates retained weights. Subsequent zero reference history still multiplies NaN weights. Signal history alone is finite, but retained coefficient state breaks the exact-zero support proof.

### AUD-098: MVDR generates NaN from continuous silence

`spectral.rs` / `spectral.log`: fresh initialized plugins, 1100 ordinary callbacks of 256 all-zero frames (281600 total), 48 kHz.

| Mics | MVDR nonfinite output count | First bad frame | Last bad frame | Superdirective bad frames |
|---|---|---|---|---|
| 2 | 10496 | 220160 | 230655 | 0 |
| 4 | 12288 | 218368 | 230655 | 0 |
| 8 | 14080 | 216576 | 230655 | 0 |

Cause: quiet covariance decays by .95 each hop. The loaded Cholesky solve remains above its 1e-20 diagonal floor and yields inverse coefficients around 1e19 or higher. `mvdr.rs:209` uses f32 complex division for normalization. The pinned `num-complex` `Div<Complex<T>>` computes both denominator norm-squared and numerator products without scaling; these overflow to infinity and produce Inf/Inf -> NaN. Later covariance crosses the existing diagonal floor and the delay-and-sum fallback restores finite weights. This occurs with no nonzero audio input at all.

## Proposed production scope, pending parent review

Only the Beamformer crate: `src/gsc.rs`, `src/mvdr.rs`, focused tests plus README/CHANGELOG if required. No host, metadata, parameter, geometry, scheduler, latency, drain-bound, control-policy or async changes.

### AUD-098 minimal correction

1. Preserve covariance updates, Cholesky solve, steering, thresholds, and fallback policy.
2. Normalize the f32 solved vector using f64 real/imaginary arithmetic: convert the denominator before squaring, perform numerator products/division in f64, then convert each candidate weight to f32.
3. Check the denominator and every converted candidate component for finiteness. If any candidate is invalid, install the existing steered delay-and-sum fallback for the whole frequency bin. Fixed-size existing storage suffices; no allocation or locks.
4. Do not replace failed results with zero beam weights: the fallback preserves the intended distortionless target route.

This addresses the actual overflow without changing covariance time constants or adding a silence threshold.

### AUD-097 preferred correction

Keep public f32 I/O, geometry/steering coefficients, delay ring and scheduling unchanged. Use a bounded private f64 adaptive kernel where range is required:

1. Convert delay-line samples before interpolation and accumulate aligned values, fixed-beam sum and blocking references in f64.
2. Store reference FIR history, reference/alignment scratch and adaptive weights in f64. Keeping only wider sums while narrowing the 1.5*A reference back to f32 would recreate the confirmed overflow; clipping references would change the modeled blocking signal.
3. Compute reference powers, target-presence comparison, error and NLMS update in f64, retaining the existing delta, mu, gate and update order. Commit only finite candidate weights; a failed candidate leaves its prior finite weight intact. This prevents retained NaN state even if intermediate arithmetic is ever invalid.
4. At the f32 output boundary, clamp finite out-of-range values to the representable f32 range; map a nonfinite intermediate result to zero. Ordinary representable output is converted normally. A mathematically finite output exceeding f32 range cannot otherwise satisfy a finite-output contract.
5. Update only private test helpers to install f32 test weights through conversion and snapshot the actual f64 adaptive state. Public constructor/process signatures remain unchanged. Prepared storage roughly doubles for the small adaptive vectors; callback allocation count remains zero and loop work remains bounded by microphones times the existing 32 taps.

This is more faithful than suppressing any reference that overflows f32, and avoids an arbitrary input-amplitude cap. It changes ordinary rounding, so verification must quantify equivalence instead of promising bit identity.

## Verification after approval

- Preserve the two public red probes as permanent regressions. GSC must recover without reset after finite extremes, after scale changes, and after very small references; MVDR must remain exact zero across and beyond the former bad interval for 2..8 microphones.
- Independent f64 direct GSC equation: broadband training plus small-reference recovery, all supported microphone counts, fractional steering delays, and ordinary-amplitude inputs. Compare existing output-error tolerances and measure maximum ordinary-amplitude difference from the unchanged baseline. Preserve all existing direct frozen-drain support/oracle thresholds.
- Independent MVDR distortionless-target oracle and delayed all-zero oracle. Cover covariance scales spanning the normalization overflow boundary, valid diagonal solves, low-energy fallback, and complex steering. Verify the new normalization yields the correct nonzero steering response, not just finite values.
- Exercise irregular callback partitions, reset/reinitialize, nonempty zero continuations, and existing frozen drain after adaptive training. Changes must not alter support, phase, declared latency or learned-state freeze semantics.
- Cold first public callbacks and first low-energy solve: zero allocations and zero deallocations; use the existing Beamformer allocation harness. Full Beamformer tests and strict all-target/all-feature Clippy only once focused regressions pass.

## Metadata conclusions, separate future scope

- GSC algebraic audio support is `ceil(max steering delay)+31`; it becomes a valid ordinary-processing bound only with finite retained coefficients and finite output handling.
- Spectral audio history has conservative support `2*512=1024` frames; changing coefficients cannot generate audio after all analysis input and OLA cells are zero, provided the active weights are finite. MVDR currently violates that precondition even during silence.
- All Beamformer configuration controls are structural and synchronous. There is no pending worker publication or runtime algorithm switching to extend retained audio. Native tail bounds should remain configuration bounds, not countdowns.
- Superdirective currently has no demonstrated ordinary-zero blocker. Its static coefficients and finite STFT history support the same 1024 bound for valid prepared configurations, but retain `Unknown` until the combined repair/proof checkpoint is approved.
- Parent owns the separate AEC proof and any AEC changes. AEC uses a P-block finite reference history plus overlap/save and output buffering, giving conservative `(P+2)*256` output frames; continuing adaptation and suppressor/mix ramps are multiplicative and do not inject new audio.

## Reproduction commands and provenance

Both probes were built with `rustc --edition=2024`, `-L dependency=target/debug/deps`, `-C link-arg=-fuse-ld=mold`, and these matching existing artifacts:

- `libsotf_plugin_beamformer-03f6a0299a96ad4a.rlib`, SHA256 `79a35b890fa9f936796a245a43684ff3aa0a004ba070ba440f2a0ab55365b389`
- `libsotf_host-69e1d53b0bc3c6f9.rlib`, SHA256 `18e9799a934e899fa235a58eaa4af062f5b960ae62c3520ee90ef719d5bf9cd9`

Source SHA256 at reproduction:

- `gsc.rs`: `96a032f1bedb2ae694cbac9c4b79ad812d82840dcec339ca112e456ffdd37db0`
- `mvdr.rs`: `434e7edb82a867a04349e82ba1a0295b4b549fe35ae447edcff465e4fd219dc5`
