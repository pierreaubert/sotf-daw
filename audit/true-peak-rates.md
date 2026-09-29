# AUD123 — true-peak rate coverage

## Scope and policy

The host's private `Bs1770TruePeakMeter` now reports true peak from **8,000 Hz
through 2,822,400 Hz**, inclusive. Rates below or above that interval retain
explicit unavailable status or are rejected by the host constructor. The
prepared power-of-two factor is chosen so the interpolated rate reaches at
least 192 kHz, and is at least 2x, capped at 32x:

| Input rate | Factor | Kernel |
| --- | ---: | --- |
| 8,000–11,999 Hz | 32x | Prepared 64-tap Blackman-sinc |
| 12,000–23,999 Hz | 16x | Prepared 64-tap Blackman-sinc |
| 24,000–47,999 Hz | 8x | Prepared 64-tap Blackman-sinc |
| 48,000–95,999 Hz | 4x | Published 12-tap phases |
| 96,000–2,822,400 Hz | 2x | Published 12-tap phases |

The existing 48 kHz and 96 kHz arithmetic is preserved exactly; 88.2 kHz uses
four published phases. The custom phase tables are generated and normalized at
construction, outside audio callbacks. Per-channel interpolation history and
peak accumulators remain prepared storage. Finalization advances 11 intervals
for the published FIR or 63 for the 64-tap kernel, drains interpolation state
without emitting audio, and does not advance loudness clocks.

The prepared custom coefficient bank uses `512 × factor` bytes: 4, 8 or 16 KiB
for 8x, 16x or 32x. Per-channel history uses 512 bytes with the 64-tap kernel
or 96 bytes with the 12-tap kernel, plus 8 bytes for that channel's interval
peak. This storage is allocated when constructing the meter.

The public status documentation describes this as the host's supported
measurement policy. It does not claim external programme certification. The
separate vendored math-dsp EBU detector remains at its explicit 48 kHz reference
scope.

## Accuracy and lifecycle evidence

- EBU Tech 3341 synthetic test cases 15–19 run with independently generated
  tones, 10 ms fades, and irregular 274-sample callback blocks at twelve rates
  from 8 kHz through 384 kHz. All 60 two-channel cases pass the standard
  expectations with the documented +0.2/−0.4 dB tolerance.
- Rate-boundary tests cover 5,999/7,999 Hz unavailable, every transition at
  11,999/12,000, 23,999/24,000, 47,999/48,000 and 95,999/96,000 Hz, the
  accepted 2,822,400 Hz maximum, and rejection at 2,822,401 Hz.
- A separate offline Lanczos reconstruction uses radius-64 support and a 64x
  dense time grid over the complete finite stream and both tails. Its window and
  evaluation are independent of the production Blackman phase bank. The
  measured 8/12/44.1 kHz high-ratio results match it within 0.03 dB.
- A direct index-based convolution test checks a dense input and every final
  impulse position 0–63. It verifies interval maxima and the complete custom
  kernel suffix independently of stateful production history. Published
  48/88.2/96 kHz results retain direct coefficient-table oracles.
- Public lifecycle tests cover 8/12/44.1/48 kHz, irregular callbacks, reset,
  reinitialization, disable/enable, retained strong and nested Weak readers,
  and final peak recovery against an ordinary zero-continuation control. The
  48 kHz output matches the published oracle within 2e-12 dB; exact equality is
  checked separately in the matched kernel control.
- Finish-only checks at 8/12/44.1/48/88.2/96 kHz compare unrelated telemetry
  fields with an unfinished control. Custom peaks match ordinary 63-zero
  continuation; the published cases match direct legacy oracles.
- Fresh-thread allocation tests cover supported rate families through the
  accepted maximum and 1/2/6/24 channels. Two repetitions of processing,
  finalization, interval queries and reset report zero allocations and frees.

## Verification

The final focused run passes **549 host library tests** (one ignored), plus 4
calibration, 3 heap, 11 finite-stream and 4 true-peak-rate integration tests.
Strict all-target host Clippy passes. The final offline workspace gate passes
**5,997 tests**, with 11 skipped; MIDI and IAMF tests are excluded, FFI is
included. The existing loudness-history heap stress test takes 267 seconds in
this run; the next slowest test takes 77 seconds. A preceding full `sotf-host`
package run also passed 548 library tests and every host integration test. Logs:

- Initial public regression failures: `/tmp/sotf-aud123-public-red.log`.
- Final focused tests: `/tmp/sotf-aud123-focused-final2.log`.
- Strict host Clippy: `/tmp/sotf-aud123-clippy-final.log`.
- Full host package tests: `/tmp/sotf-aud123-host.log`.
- Workspace nextest gate:
  `/tmp/sotf-aud123-workspace-nextest-final.log`.

Reproducible commands, run from the workspace root:

```sh
CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo test --offline -p sotf-host --lib --test true_peak_finite_stream --test true_peak_rates --test true_peak_calibration --test true_peak_calibration_heap
CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo clippy --offline -p sotf-host --all-targets -- -D warnings
CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo bench --offline -p sotf-host --bench true_peak_rate_benchmark
CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo test --offline -p sotf-host --lib pre_aud123_published_kernel_cpu_control -- --ignored --nocapture
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo nextest run --workspace --exclude sotf-midi --exclude sotf-iamf
```

## CPU cost

Criterion ran optimized builds on an AMD Ryzen Threadripper PRO 3995WX
(64 cores, SMT enabled), Linux x86_64, rustc 1.98.1. Each callback fixture is
128 frames, with one or 24 channels. Callback timing covers
`LoudnessMonitor::add_frames`; reset and the snapshot query are outside that
timer. Finish timing covers `finish_true_peak` plus snapshot update after input
was prepared outside the timer. Criterion used ten samples, 200 ms warmup and
500 ms measurement per case. Values below show 24-channel point estimates and
95% confidence intervals for aggregate timings in milliseconds. These are not
worst individual callback bounds:

| Rate | Factor | 128-frame callback | Finish + publication | Callback budget used |
| ---: | ---: | ---: | ---: | ---: |
| 8 kHz | 32x | 3.701 [3.695, 3.712] | 1.817 [1.816, 1.818] | 23.1% |
| 12 kHz | 16x | 1.893 [1.887, 1.900] | 0.924 [0.920, 0.930] | 17.7% |
| 24 kHz | 8x | 0.970 [0.969, 0.971] | 0.477 [0.477, 0.478] | 18.2% |
| 44.1 kHz | 8x | 0.969 [0.968, 0.970] | 0.479 [0.476, 0.481] | 33.4% |
| 48 kHz | 4x | 0.0748 [0.0747, 0.0749] | 0.00613 [0.00612, 0.00613] | 2.8% |
| 96 kHz | 2x | 0.0601 [0.0601, 0.0602] | 0.00429 [0.00429, 0.00430] | 4.5% |
| 192 kHz | 2x | 0.0649 [0.0649, 0.0650] | 0.00430 [0.00429, 0.00431] | 9.7% |

Callback budget use is the measured callback time divided by the duration of
128 frames at that sample rate. It is a single-thread estimate for this host
method, not a device-level realtime guarantee. The accepted 2,822,400 Hz rate
is covered by accuracy and allocation tests but was not CPU-profiled.

An additional ignored unit test compares a reconstructed pre-AUD123 12-tap
kernel and current published kernel with the same coefficients. It asserts
exact per-interval and final-tail output before timing, and alternates legacy
and candidate order for seven trials. Median process ratios (candidate/legacy)
are 1.054 and 0.978 at 48 kHz for 1/24 channels, and 1.031 and 1.017 at 96 kHz.
Finish ratios are 0.957/0.967 at 48 kHz and 0.975/0.959 at 96 kHz. The run
includes visible scheduling outliers, so these ratios are only a kernel-level
control. It is not a historical whole-monitor comparison. Log:
`/tmp/sotf-aud123-kernel-cpu-control-final.log`.

The complete absolute Criterion output is `/tmp/sotf-aud123-cpu.log`. The
reconstructed-kernel control shows callback medians from 2.2% faster to 5.4%
slower, with scheduling outliers; absolute differences are below 1.4 us per
128-frame interval, and drain medians are 2.5–4.1% faster. This is only a
kernel-level comparison. High-ratio low-rate processing and drain costs are
substantially larger; hosts using 8 kHz with 24 channels should account for the
measured callback budget.

## Limits

The 64-tap high-ratio kernel is an explicitly tested host policy; the tests do
not certify arbitrary programme material or every possible audio rate against
an external meter. Synthetic EBU cases and independent reconstruction fixtures
cover selected signals and tolerances. The maximum accepted rate is not
CPU-profiled. Broader programme maxima, maximum M/S, measurement pause/continue,
and full external corpus comparisons remain in the metering audit. This batch
does not change the vendored math-dsp detector.
