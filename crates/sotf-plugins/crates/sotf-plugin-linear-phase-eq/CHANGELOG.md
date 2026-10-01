# Unreleased

- Fixed validation-r5 test setups without weakening bounds: stronger Mid cut
  for the inactive-slot guard, 2 kHz opposite-gain peaks for the
  order-noncommutation guard, and twin retired-route reclamation in the
  retry-after-refusal test. The 96 kHz / 1024-tap / 500 Hz multiband gap is
  analyzed as a short-FIR resolution limit (see lane fix-r5-result.md); the
  0.05 dB bound and matrix are unchanged.
- Added the realtime commit entrypoint `try_commit_prepared_update` with a
  caller-owned `Option` slot and an allocation-free typed `CommitRefusal`
  error: success and every refusal perform zero allocations and zero frees,
  and refusals retain the prepared update for correction or retry. All
  validations run before priming or live-state mutation, so refusals are
  fully transactional. `commit_prepared_update` is now the control-thread
  compatibility wrapper (it allocates its `String` error and frees the
  prepared update on refusal) and must not be called on the audio thread.
- Fixed the realtime commit to perform zero frees: the prepared base snapshot
  is stashed into the target banks for off-thread reclamation with the retired
  route instead of being dropped on the commit thread.
- Fixed validation-r3 test setups without weakening bounds: stream beyond
  cascade latency for the inactive-slot difference guard, use same-frequency
  opposite-gain peaks for the order-noncommutation guard, cap the 192 kHz
  analytic band at the 20 kHz validation ceiling, and gate multiband magnitude
  asserts to probes above the FIR resolution limit (phase asserts unchanged).
- Fixed the remaining validation-r2 compile/lint failures without behavior
  changes: `unwrap_err` on the non-`Debug` plugin type, `too_many_arguments`
  on the `EqBand::update` mirror, `assign_op_pattern` in cascade-response
  math, and range-loop/`chunks_exact`/type-complexity lints in tests.
- Fixed the ordered-route channel response to use single-channel (delta)
  excitation, reporting the diagonal transfer for Mid/Side topologies instead
  of the correlated-input response; documented the chart semantic.
- Fixed dynamic-update priming to cover the full cascade response support
  (`stages * (N - 1 + 32)`) and sized the history ring to the worst case;
  removed the dead legacy-spectrum FFT from the commit path and documented
  its staleness contract.
- Documented that controls/charts report the committed target while audio
  blends, and clarified the 512-step / 513-frame crossfade naming.
- Added per-band channel routing (`placement`: stereo/left/right/mid/side) with
  explicit disjoint `stereo_pairs`, an ordered per-band FIR cascade route, and
  channel-aware complex response / group-delay APIs. Legacy configs without
  placements keep the single-FIR path bit-identically.
- Added dynamic band updates: off-thread `prepare_band_update` plus bounded
  allocation-free `commit_prepared_update` with a fixed exact-0/1 output
  crossfade. Phase mode and latency never change across an update; topology,
  counts, placements, FIR length, phase mode and auto gain stay structural.
- Extended accuracy coverage: all four tap counts in analytic complex-response
  and group-delay tests, exact ordered-cascade references, dynamic blend
  references, refusal atomicity, and legacy preset/audio compatibility tests.

## 0.5.6 — 2026-08-12

- Replaced callback-sized full-FIR FFT convolution with 32-sample-head
  non-uniform partitioned convolution.
- Made every FIR-response control structural so FIR design/planning and state
  replacement never run from a warmed callback or splice filter generations.
- Corrected even-tap and partition latency, per-sample mix smoothing, and stable
  Auto Gain compile metadata.
- Added impulse-latency, block-invariant automation, structural-control,
  malformed-state, zero-allocation, and channels/taps/block deadline coverage.

## 0.5.5

- Align the dry mix branch with the reported linear-phase FIR group delay using
  preallocated per-channel delay storage; reset clears the delay history.
- Update passthrough regressions to assert the latency contract rather than an
  impossible immediate dry signal.

## Fixed

- Validate serialized FIR-EQ parameters (including finite values and the
  sample-rate-dependent Nyquist limit) before constructing biquads or FIRs.
- Restrict FIR design and host parameter exposure to the configured active
  band count; align the shared band keys with the DSP (`type`, `freq`, `q`,
  `gain`, `active`) and expose exactly ten bands through FFI.
- Reject malformed audio buffers without panicking, preserve oversized-buffer
  tails, reset mix smoothing with convolution state, and activate the nested
  DSP regression module.

## Changed

- Added selectable linear and minimum-phase FIR design modes.
- Consolidated the former FIR Designer implementation; legacy `FirDesigner` presets and the `fir_designer` factory name now migrate to FIR EQ.

# 0.5.5

## Fixes

- Simplified overlap-add tail management by sizing per-channel overlap buffers
  to exactly `fir_len - 1` instead of `fft_size`. Added
  `test_overlap_buffers_match_fir_tail_length`.
- Reused FIR-design frequency and magnitude scratch vectors across rebuilds
  instead of allocating fresh vectors each time. Added
  `test_rebuild_fir_reuses_design_scratch_vectors`.
- Documented that Auto Gain normalizes DC gain to unity; it is a stable
  reference-point correction, not perceptual loudness matching.

# 0.5.4

## Fixes

- Corrected the overlap-add chunking guard to use `fft_size - (fir_len - 1)` as the maximum
  valid block size. Blocks that fit below `fft_size` but exceed the convolution-safe length are
  now chunked, preventing circular-convolution tail wrap.

# 0.5.3

## Fixes

- DC magnitude was hardcoded to 0 dB** (`src/lib.rs:348`).
  The `rebuild_fir` DC point was `magnitudes_db.push(0.0)` regardless of filter
  shape. Lowshelf cuts and highpass filters now correctly attenuate the DC region
  because the DC gain is computed by summing `band.biquad.log_result(1.0)` over
  all active bands.
- Lowpass/Highpass bands were silently skipped** (`src/lib.rs:361`).
  The `band.gain_db.abs() > 1e-6` guard was applied to every filter type.
  Since lowpass/highpass always have `gain_db == 0`, every LP/HP band was treated
  as flat and omitted from the FIR design, producing an all-pass FIR. Fixed by
  matching on `BiquadFilterType::Lowpass | BiquadFilterType::Highpass` to always
  include those types; the gain guard is retained only for Peak, Shelf, etc.
- Insufficient frequency sampling for long FIR lengths** (`src/lib.rs:342`).
  `num_points` was a fixed 4096 regardless of FIR tap count. For an 8192-tap FIR
  this left high-Q narrow peaks undersampled. `num_points` now scales as
  `MAG_RESPONSE_POINTS.max(fir_length * 2).next_power_of_two()`.

## Tests added

- `test_highpass_attenuates_below_cutoff` — regression for bug #2.
- `test_lowshelf_cut_attenuates_low_frequencies` — regression for bug #1.

## Deferred

- **#4 (🟠): Overlap-add buffer sized to `fft_size` instead of `fir_len - 1`.**
  The current logic is correct (no out-of-bounds access was observed) but complex.
  Refactoring requires careful regression testing of the overlap-add path and is
  deferred to avoid scope creep. Ticket recommended before next major release.

- **#5 (🟡): Auto-gain normalizes by DC gain, not perceived loudness.**
  This is documented behavior: auto-gain targets DC unity. A treble boost with
  auto-gain enabled will reduce DC gain to 0 dB, which may partially counteract
  the boost at high frequencies. Documented in code comments; the trade-off is
  intentional (predictable loudness reference point).

- **#6 (🟡): FFT per channel without SIMD batching.**
  Acceptable for stereo; deferred for multi-channel optimization work.

- **#7 (🟡): `rebuild_fir` allocates `freqs`/`magnitudes_db` vectors.**
  Only called on parameter change, not in the audio thread. Deferred.

# 0.5.2

## Fixes

- Process blocks larger than the FFT size by chunking them through the overlap-add path.
- Avoid silently passing oversized blocks through dry while still reporting FIR latency.
- Add regression coverage that verifies large blocks are processed.
