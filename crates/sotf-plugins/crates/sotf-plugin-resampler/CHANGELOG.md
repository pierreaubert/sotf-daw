# Unreleased

- Add opt-in `cutoff_smoothing` (bool, default off, Realtime): upward
  cutoff widening slews one prepared table per backend chunk while
  downward narrowing jumps immediately, preserving alias protection,
  clocks, counts, drain, and partition invariance. Default-off audio is
  bit-exact with prior releases.
- Quantify per-preset transition width, passband ripple, and stopband
  quality against an independent analytic DTFT, with measured-vs-analytic
  agreement on an integer-step sweep and an independent direct-sinc
  end-to-end reference at 44.1/48 kHz; publish per-quality CPU lines in QA.
- Add allocation-free typed dynamic controls (`try_set_ratio`,
  `try_set_ratio_relative`, `try_set_cutoff_smoothing` returning the
  `Copy` `ResamplerControlError`); the `String` API delegates with
  identical messages and the valid path still allocates nothing.
- Add slew artifact bounds (analytic HF derivation, 0.1 dB LF, -50 dB
  coherent residual), full-trajectory checks, nominal-0.5/2.0 bank units,
  nominal-not-1 / instant-upward / Fast-Medium downward / 8 ch / 96 kHz /
  preemption / drain-mid-slew / per-block-count / latency coverage, and an
  asserted 96->24 near-cutoff sweep. Unify QA Test 7 to the 9..13 window.
- Correct the slew oracles with independent derivations (same bounds):
  true 2x2 least-squares coherent fit for the non-coherent transition
  window (same -50 dB bound) and a floor-aware trajectory check with a
  linear floor from the established 2e-6 alias bound (same 1 dB
  above-floor monotonic, same per-block coverage, plus a 20 dB
  above-floor measurability assert). Add cumulative-clock uniformity,
  fixed-2.0 control-leg, and injected-spur regression coverage
  distinguishing true artifacts from intended rate/chirp modulation.

# 0.6.0

- Prepare anti-alias cutoff tables for dynamic ratio changes; select a safe cutoff
  across each ramp without allocation or resetting audio history. Reset restores
  the nominal filter. Enable the previously failing dynamic alias regression.
- Use a private Rubato fork with exact inverse-ratio ramp sizing, derived output
  capacity and sufficient history for deferred tiny blocks at extreme ratios.
- Rebase the private Rubato fork onto upstream 5.0.0; it is now the single
  Rubato in the workspace (all users) with a single audioadapter-buffers 5.x.
  Bank, ramp, history, and EOF behavior are unchanged and re-pinned by the
  ported fork suite plus the production cutoff/drain regressions.

- Reject equal-rate dynamic-mode transitions after accepted input until reset,
  preserving buffered audio, chronological order, and active latency. Fresh
  setup transitions clear dormant backend ratio ramps; unchanged flags remain
  idempotent. Runtime metadata is Structural only for equal-rate instances.
- Preserve live unequal-rate backend history when disabling dynamic updates and
  propagate ratio-reset failures before changing public state.
- Validate drain capacity and callback clocks before latching EOF; reject ratio
  and control mutation after finalization. Empty valid drains still finalize.
- Add exact public history/retry/reset regressions and cold allocation/deallocation
  checks.
- Replace accepted-input times requested-ratio EOF estimates with the actual emitted
  interpolation trajectory and exact integer submitted-input origins. Buffered
  overwritten targets no longer truncate or extend equivalent fixed streams.
- Preserve fixed-rate frame counts; variable trajectories drain through the first
  source-clock boundary crossing, with less than one output interval of overshoot.
  Keep zero-output drain steps unfinished until their endpoint is reached.
- Add independent clock, full zero-continuation, final-marker, output-canary,
  reset/retry, tiny/low-ratio and cold allocation/deallocation regressions.
  Signal-delay and realtime latency/PDC declarations are unchanged.

# 0.5.27

## Improvements

- Removed the redundant planar full-chunk copy: the preallocated residual planes now feed rubato directly in both processing and drain paths.
- Added complete-stream rate, spectral-rejection, and callback-partition invariance evidence.
- Added allocation regression coverage for dynamic-ratio automation through the
  `Plugin::set_parameter` trait path with buffered residual input.
- Added warmed callback timing distributions (p50, p95, p99, and max after 128 warm-up calls
  and 256 measured calls per case) across the quality/rate/channel/callback QA matrix, with
  per-case p99 and max deadline gates.
- Documented why fixed-size input callbacks cannot imply the same fixed output frame count across different sample-rate clock domains; produced frame counts remain authoritative.

# 0.5.26

## Fixes

- Added object-safe allocation-free plugin/host draining and engine EOF propagation, including
  causal draining through downstream stateful plugins.
- Replaced one-chunk discard estimation with rubato's cumulative complete-stream procedure so
  leading delay and final sinc ringing are preserved exactly.
- Negotiated sample rates per host node after rate-changing plugins and reset pending state on stop.
- Added exhaustive residual-boundary/extreme-ratio/quality count tests, transactional multi-chunk
  retry, bridge choice roundtrips, unity non-finite policy, estimator channel matrix, and realtime
  ratio-automation allocation coverage.
- Clarified capacity, availability, latency-domain, unity, dynamic-ratio, quality, and drain APIs.

# 0.5.25

## Fixes

- Preserve the actual zero/burst output frame count of rate-changing plugins in `DawHost`; sub-chunk Resampler calls are no longer padded with input-rate silence.
- Reject host sample rates that disagree with the configured Resampler input clock.
- Validate process and flush destination capacity before consuming residual input or advancing rubato, making buffer-error retries transactional.
- Make equal-rate, non-dynamic conversion a bit-exact, zero-latency passthrough for arbitrary block partitions.
- Express latency entirely in output frames by converting the chunk priming interval through the active ratio.
- Expose quality as the canonical integer choice used by ParamBridge and reject live post-activation quality rebuilds that allocate and discard filter/residual state.
- Remove cached-parameter reconstruction from runtime ratio updates and always report `Some(0)` for known zero-frame output.
- Split immediately available output (`available_output_frames`) from conservative destination capacity and add an explicit `flush_output_frames_max` contract.
- Correct the crate guide/API documentation and expand QA/test coverage for these contracts.

## Remaining limitations

- The object-safe `Plugin`/engine EOF drain contract and exact complete sinc-tail trimming require a coordinated host/engine design; the concrete `flush()` helper still processes one padded partial chunk.
- The planar residual-to-rubato copy and scalar interleave/deinterleave loops remain candidates for profile-guided optimization.

# 0.5.24

## Fixes

- **QA binary:** The performance benchmark now appends each block's output into `bench_output`
  instead of reusing `bench_output[..rt_max_out * channels]` for every block. This keeps the QA
  benchmark's output layout correct if it is extended to validate rendered samples.

# 0.5.23

> Historical note: the concrete one-step flush and latency limitations below
> were superseded by the complete-stream drain and output-domain latency
> contracts in 0.5.25-0.5.26.

## Fixes

- **Critical – latency:** `latency_samples()` now returns `resampler.output_delay()` (rubato's
  exact FIR group delay including ring-buffer and polyphase offsets) instead of the stale
  `sinc_len / 2` heuristic. For a 44.1 → 48 kHz Medium-quality resampler the old value was 64;
  the correct value from rubato is higher. Hosts that use `latency_samples()` for delay
  compensation were placing resampled audio out of phase. (`src/lib.rs:599`)

- **Critical – data loss:** Added `ResamplerPlugin::flush(&mut self, output: &mut [f32]) -> Result<usize, String>`.
  Without it, any input frames buffered in the residual (i.e. the last `0 … chunk_size-1` frames
  of a stream) were silently discarded. Offline rendering pipelines and streaming deactivations
  must call `flush()` after the last `process()` call. (`src/lib.rs:316`)

- **High – stale residual after reset:** `reset()` now zeroes `residual_input` in addition to
  setting `residual_frames = 0`. Prevents future refactors from accidentally reading old audio
  data through unbounded slice access. (`src/lib.rs:514`)

- **High – RT allocation avoidance in `rebuild_resampler()`:** Quality-preset changes
  (`set_parameter("quality", …)`) no longer reallocate `output_buffer` or `residual_input`.
  `output_frames_max()` depends only on chunk_size and ratio (not sinc length), so the existing
  buffers remain correctly sized across quality transitions. The only remaining allocation is
  rubato's internal sinc-table build, which is inherent and documented. (`src/lib.rs:244`)

- **QA binary:** `initialize()` was called with `output_sr` (48000) instead of `input_sr`
  (44100), triggering a spurious warning on every QA run. (`bin/qa_resampler.rs:18`)

## Tests added

- `test_latency_uses_rubato_output_delay` — asserts the returned latency differs from the old
  `sinc_len/2` heuristic and is > 0.
- `test_flush_empty_residual` — flush on a clean resampler returns 0 frames.
- `test_flush_recovers_trailing_frames` — a sub-chunk block is buffered (0 output), then flush
  recovers it.
- `test_variable_block_size_small` — four 256-frame blocks filling a 1024-frame chunk.
- `test_variable_block_size_non_multiple` — 1500-frame block produces output for 1 full chunk,
  flush recovers the 476-frame residual.
- `test_zero_frame_block` — zero-frame `process()` succeeds and returns 0.
- `test_cumulative_frame_count` — 10 s of 44.1 → 48 kHz resampling stays within ±2 frames/chunk
  of the theoretical output count.

## Deferred

- **3.2 Double copy of input data** (`src/lib.rs:488–505`): `residual_input` is copied into
  `input_buffer` before each rubato call. Eliminating this copy requires passing `residual_input`
  directly as the rubato adapter, which needs mutable borrow restructuring. Deferred to avoid
  scope creep; documented with a TODO comment.
- **3.3 SequentialSliceOfVecs per-chunk construction**: The adapters cannot easily be stored as
  struct members (require `&mut` on construction). Deferred.
- **3.4 `planar_to_interleaved` loop order**: Acceptable for current channel counts (≤ 8 in
  practice). Deferred.
- **1.2 Chunking latency not reported**: The `chunk_size - 1` buffering latency is inherent to
  the chunked architecture and is now documented in `flush()` and `process()`. Adding it to
  `latency_samples()` would require a cross-crate API change to report both algorithmic and
  buffering latency separately. Deferred.

# 0.5.22

## Fixes

- Account for multi-chunk input in `output_frames_for_input()` so hosts allocate enough output space.
- Include pending residual frames when estimating output capacity.
- Add regression coverage for multi-chunk resampling estimates.
- **CRITICAL** Fix `latency_samples()` to use rubato's `output_delay()` instead of the `sinc_len/2` heuristic.
- **CRITICAL** Add `flush()` API to drain residual buffered frames and prevent silent loss of trailing audio.
- **CRITICAL** Zero `residual_input` buffers in `reset()` to prevent stale audio leakage.
- **MAJOR** Eliminate allocations in `rebuild_resampler()` by reusing pre-allocated buffers (real-time safety).
