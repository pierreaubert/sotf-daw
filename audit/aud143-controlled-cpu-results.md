# AUD143 reconstructed BandSplit CPU comparison

Status: **controlled comparison executed; evidence ready for review.** This is a reconstructed comparison from preserved pre-edit DSP sources, not a reproduction of the earlier unbound QA run.

## Result

In the release-profile processing measurements, current `LegacyCascade` medians ranged from **0.982× to 1.028×** the recovered legacy medians across stereo and 12-channel workloads, both steady and automated, at all three callback sizes. The measurements show no consistent broad change in the old processing route. The simple 12-channel/four-band/LR48 automated 512-frame case was 17.431 ms recovered versus 17.275 ms current legacy for 32,768 measured frames.

`PhaseCompensated` increased processing medians by **1.048× to 2.240×** over current legacy, with cost growing with band and channel count. The 12-channel, four-band, LR48 automated 512-frame case measured 38.200 ms per 32,768-frame batch versus 17.275 ms for current legacy. Divided over its 66 process calls, that is 578.8 µs/call versus 261.7 µs/call. These are measured averages and medians, not worst-case bounds.

Construction/initialization is reported separately. For the stereo two-band LR24 setup, current legacy was 2.371 µs per instance against 1.388 µs recovered, a 70.8% median increase. That setup case is noisy: the 18-sample coefficients of variation were 51.3%, 58.5% and 57.4% for recovered legacy, current legacy and phase-compensated respectively. Treat the absolute setup values and this percentage as weak evidence, not a stable cost estimate.

### Processing cost by workload

Each processing range covers six conditions: steady and automated cutoff paths at maximum block lengths of 32, 512 and 2,048 frames. Each ratio is computed from the median across 18 samples (two nine-sample passes).

| Workload | Current legacy / recovered | Phase-compensated / current legacy |
|---|---:|---:|
| Stereo, 2 bands, LR24 | 1.003–1.011× (+0.3% to +1.1%) | 1.098–1.113× (+9.8% to +11.3%) |
| Stereo, 4 bands, LR24 | 0.982–1.004× (-1.8% to +0.4%) | 1.629–1.657× (+62.9% to +65.7%) |
| Stereo, 2 bands, LR48 | 1.014–1.028× (+1.4% to +2.8%) | 1.048–1.057× (+4.8% to +5.7%) |
| Stereo, 4 bands, LR48 | 0.995–1.008× (-0.5% to +0.8%) | 1.726–1.746× (+72.6% to +74.6%) |
| 12 channels, 4 bands, LR48 | 0.991–1.007× (-0.9% to +0.7%) | 2.179–2.240× (+117.9% to +124.0%) |

### Setup/initialization

Medians in microseconds per instance; each trial constructs and initializes 64 plugins, then divides elapsed time by 64. Setup had no separate warmup. Each cell is the median of 18 samples.

| Workload | Recovered legacy | Current legacy | Current phase | Current legacy vs recovered | Phase vs current legacy |
|---|---:|---:|---:|---:|---:|
| Stereo, 2 bands, LR24 | 1.387 | 2.370 | 2.817 | +70.8% | +18.8% |
| Stereo, 4 bands, LR24 | 3.491 | 3.434 | 6.418 | -1.6% | +86.9% |
| Stereo, 2 bands, LR48 | 1.825 | 2.441 | 3.151 | +33.7% | +29.1% |
| Stereo, 4 bands, LR48 | 4.652 | 4.645 | 9.396 | -0.2% | +102.3% |
| 12 channels, 4 bands, LR48 | 16.698 | 16.698 | 39.572 | +0.0% | +137.0% |

### Prior 12-channel automated case

| Variant | Median batch for 32,768 frames | Median batch / 66 process calls | Share of measured audio duration* |
|---|---:|---:|---:|
| Recovered legacy | 17.431 ms | 264.1 µs | 2.5533% |
| Current legacy | 17.275 ms | 261.7 µs | 2.5306% |
| Current phase | 38.200 ms | 578.8 µs | 5.5957% |

*The percentages divide each measured 32,768-frame batch by its 0.682667 s audio duration at 48 kHz. The 66-call averages are informational: automation splits two calls at event frames, so those calls include short partial blocks and do not represent a full 512-frame callback. These shares are not a deadline guarantee or an individual-call worst case.

## Four-band PhaseCompensated lifecycle evidence

The final focused fixture [aud143_compensated_lifecycle.rs](../crates/sotf-plugins/crates/sotf-plugin-band-split/tests/aud143_compensated_lifecycle.rs), SHA-256 `89fb0692e6fcc58c3fc8267da503d3a56a3fbea7af1a2d8859d0994a59ab5499`, exercises two-channel, four-band PhaseCompensated LR24 and LR48 instances. It changes all three cutoffs and four gains, processes full-band deterministic noise to populate each split and compensation path, then performs two populated reset/process cycles per slope. Each full output is finite, has the expected length, has nontrivial audio in every band, and matches a freshly initialized instance bit for bit at the selected cutoffs and gains.

The lifecycle test preallocates its test inputs, outputs, contexts and fresh reference plugins before arming its counting allocator. Both reset calls and both process calls in each repeated epoch are inside the measured window; it records zero allocations and zero deallocations for LR24 and LR48. This is distinct from the earlier four-band heap test, which checked process and reset allocation behavior without comparing repeated reset epochs against full reference waveforms.

On populated instances, `initialize(0)` and `initialize(8_000)` are rejected; the latter makes the selected 8,640 Hz third cutoff exceed the positive sample-rate admission limit. After each rejection, the complete next 8,192-frame output matches an untouched twin. A second-cutoff request below the first cutoff is likewise rejected without changing its reported target or subsequent audio. A valid second-cutoff retry remains identical to the twin, and a successful 48-to-96 kHz reinitialize matches a fresh 96 kHz instance over the complete output vector. All cases check output length, finiteness, distinct input channels and nontrivial output in all bands.

The final focused test passed 1/1, and strict Clippy passed for this test target. Exact commands, final logs, and their hashes are in [aud143-lifecycle artifacts](artifacts/aud143-lifecycle/). The preserved r1 logs are historical and predate the counting-allocator and 8 kHz refusal additions; r2 is the final result. No DSP accuracy tolerance or existing gate was changed, and this lifecycle fixture did not require a production source change.

## Method and reconstruction

- Recovered legacy DSP source came from the preserved pre-edit archive. It contains nine BandSplit package paths and 59 `math-iir-fir` paths; all 66 non-manifest files were hash-verified against the archive receipt. Current and recovered snapshots used the same current `Cargo.toml` files and `Cargo.lock` (SHA-256 `db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`). The two selected-source archives, hashes and path-by-path verification are in the artifact directory.
- Both variants first passed the existing full-vector legacy fixture test: one exact byte comparison against the checked-in 48 kHz stereo gzip fixture (SHA-256 `e45166957b2ec671fcb3b5d98987b1c2ca3d9ccf8e200c1666d8e6edadbb53ff`). The current and recovered replay logs are identical (SHA-256 `8a5564df25fc6e6274769bba63ea83a85910026593376044f0fce22b9db25144`).
- The release harness covers stereo 2/4-band LR24/LR48 and 12-channel/four-band LR48, with 32/512/2,048 maximum block lengths. It precomputes deterministic multitone input, warms each processing instance for 8,192 frames, and times 32,768 frames. It separately times construction plus initialization in 64-instance batches.
- Steady processing holds cutoffs fixed. Automation applies the same two frequency targets at measured offsets 701 and 9,113 for every variant of a workload. The harness splits process calls at those sample boundaries and sets the scalar frequency parameter before the following call. The timed automation route is this sample-boundary adapter; it is not an end-to-end native-host automation benchmark.
- It checks the full output buffer outside the timed region for finite, nontrivial audio and records a checksum. Each variant ran nine samples per condition twice; pass B reversed pass A’s variant order. The raw CSV retains 1,890 individual measurements; the summary JSON contains 105 groups with 18 samples each.
- Final ELF binaries were copied to variant-specific paths and hashed before direct invocation, despite the shared Cargo build cache. Build logs and binary hashes are in `build-receipt-durable.json`.
- Warning-denied Clippy passes for the new `aud143_cpu` test target. Package-wide `--all-targets --release` Clippy stops at the existing `tests/realtime_parameters.rs` imports of `sotf_host::{CountingAlloc, assert_no_allocs}`; those exports are cfg-gated and unavailable to that release test target without the `qa` feature. The failed and scoped-success logs are both preserved.

## Environment and limits

Rust `1.98.1` / Cargo `1.98.1`, release profile (`thin` LTO, 14 codegen units), x86_64 Linux kernel `6.8.0-142-generic`. CPU: AMD Ryzen Threadripper PRO 3995WX, 64 cores / 128 hardware threads; the process could run on CPUs 0–127 and was not pinned. Governor was `schedutil`. The recorded one-minute load average ranged from 1.93 to 2.10 during the 17-second timing window, on the 128-thread host. Compiler/CPU details and per-run load/process snapshots are preserved in the artifacts.

Setup measurements for stereo two-band LR24 were especially variable because each setup trial times only 64 short constructions and has no separate warmup. No stable setup-cost claim is made from that row. Processing results are medians of finite repeated runs under this host load; they are not WCET or a realtime certification. The older CPU log is dev-optimized and has no run-bound manifest; it is intentionally not used to form ratios against these release measurements.

## Durable artifacts

- [Benchmark helper](../crates/sotf-plugins/crates/sotf-plugin-band-split/tests/aud143_cpu.rs) (SHA-256 `c69637034798e1604ef7b8956acc71f8a0225ae2fb38fafee483eaa524c35637`)
- [Raw 1,890-sample CSV](artifacts/aud143-cpu-timing/raw-samples.csv)
- [Per-condition summary metrics](artifacts/aud143-cpu-timing/summary-metrics.json)
- [Timing session and machine snapshots](artifacts/aud143-cpu-timing/session-start.json), [run order/load/executable receipt](artifacts/aud143-cpu-timing/session-summary.json)
- [Pass A recovered legacy](artifacts/aud143-cpu-timing/pass-a-recovered-legacy.log), [current legacy](artifacts/aud143-cpu-timing/pass-a-current-legacy.log), [current phase](artifacts/aud143-cpu-timing/pass-a-current-phase.log)
- [Pass B current phase](artifacts/aud143-cpu-timing/pass-b-current-phase.log), [current legacy](artifacts/aud143-cpu-timing/pass-b-current-legacy.log), [recovered legacy](artifacts/aud143-cpu-timing/pass-b-recovered-legacy.log)
- [Recovered selected source archive](artifacts/aud143-cpu-timing/recovered-selected-source.tar.gz), [current selected source archive](artifacts/aud143-cpu-timing/current-selected-source.tar.gz), [source receipt](artifacts/aud143-cpu-timing/source-snapshot-receipt-durable.json)
- [Durable build and replay receipt](artifacts/aud143-cpu-timing/build-receipt-durable.json), [artifact SHA-256 list](artifacts/aud143-cpu-timing/SHA256SUMS)
- [Harness-only Clippy pass](artifacts/aud143-cpu-timing/aud143-cpu-helper-clippy.log), [package-wide release Clippy result](artifacts/aud143-cpu-timing/bandsplit-clippy.log)
- [Clippy commands and results](artifacts/aud143-cpu-timing/clippy-receipt.json)
- [Toolchain and CPU information](artifacts/aud143-cpu-timing/toolchain-and-cpu.txt)

No production DSP behavior changed for this comparison. The separate overall AUD143 acceptance remains pending review of DSP, application and native routes.
