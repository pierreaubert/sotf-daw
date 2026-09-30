# AUD142 Crossover pre-edit baselines

Status: pre-edit audio, NIH/FFI metadata and optimized CPU baselines are
captured. This file is scoped support for
`audit/proposals/crossover-iir-families.md` and
`audit/proposals/crossover-aud142-route-design.md`.

## Frozen source and retained artifacts

Root preserved the selected current Crossover and `math-iir-fir` package
sources plus workspace manifests/locks before family implementation. The
durable copy is `audit/artifacts/aud142-preedit/selected-source.tar.gz`
(SHA-256
`4970c3ea910a2dd38e5bd72ccaac5aeb7043dcbeca3c45c8831fd51550ee9b1d`). Its
selected-file manifest is `selected-source.sha256` (manifest SHA-256
`90bd75c0ec2964d7c4be42df792b0391082c98161b5280354bc25190dd0a4d66`) and the
source receipt is `source-receipt.json`. The receipt records 105 files, no
changes during capture, and explicitly limits the archive to source
preservation: it is not a full transitive dependency snapshot or an executed
baseline. The lockfile captured there has SHA-256
`db9133949f6efa67544b407207feea40db05b4b2dd35f9964930415e5cfb1f18`.

Before capture, the live `crossover_plugin.rs` hash matched the archive entry:
`0c4357938dcf6b74141eee65416592b5c6db0b3a61a0efebcf9e8a742df28053`. The
workspace remains broadly modified by parallel audit work, so these runs
establish byte-exact replay in this preserved source state, not a clean-checkout
reconstruction of all workspace dependencies.

The four AUD141 control vectors are copied from the old target-only directory
to `audit/artifacts/aud142-preedit/aud141/`. Their outputs remain unchanged:

| Case | Output SHA-256 |
| --- | --- |
| `two-way-lr24-stereo` | `d09256fe721d0e3c872ada6b2e301c897cfcc34def707a4ed9aa32fdd5cc71dc` |
| `per-channel-lr24-stereo` | `b708f735e0a77786db98505420358d3c7ecdfbc9e06d2099eddf7f9767e11c15` |
| `four-way-fir-stereo` | `4c4c6d11cbc47a77a2137c4c0337a4941a3bb9c789b17a17d034fc916ca90c6b` |
| `four-way-lr24-final-high-stereo` | `88f989fa5649f6d17132b4dc92b4f5705fd997b9da27a2e257231a8f194d12bb` |

The default AUD141 replay path now resolves to this durable directory;
`SOTF_AUDIT_BASELINE_DIR` can still select another directory. The ignored
capture test requires that variable explicitly, so a normal test run cannot
overwrite the preserved samples.

## Executed Crossover audio vectors

`crates/sotf-plugins/crates/sotf-plugin-crossover/tests/aud142_preedit_baselines.rs`
captures seven current public-route cases using the AUD141 48 kHz, 12,288-frame
stereo multitone input:

| Case | Output width | Captured vectors |
| --- | ---: | --- |
| `two-way-lr24-both-stereo` | 4 | process |
| `three-way-lr24-both-stereo` | 6 | process, every band |
| `four-way-lr24-both-stereo` | 8 | process, every band |
| `four-way-lr24-low-stereo` | 2 | process, selected low output |
| `four-way-lr24-high-stereo` | 2 | process, selected high output |
| `per-channel-lr24-stereo` | 2 | process, one route per input channel |
| `four-way-fir-both-eof-stereo` | 8 | process and complete FIR EOF tail |

The FIR case uses 1,025 taps and three split stages. Its finite support is
3,072 frames; process output is 393,216 bytes and the drained tail is 98,304
bytes. Every vector checks exact frame/output lengths, finite samples and
non-silent active lanes. Runtime parameter metadata and current state values
are saved with each case. Capture is ignored and requires an explicit
`SOTF_AUD142_CAPTURE_DIR`; replay defaults to
`audit/artifacts/aud142-preedit/crossover/` and compares f32 values bit-exactly.

The original AUD141 fixture replay and all seven AUD142 captures were verified
against the retained files before DSP edits. Execution logs are in
`audit/artifacts/aud142-preedit/logs/`.

The Criterion release benchmark compiled successfully with the current
pre-implementation source. The build log is
`logs/crossover-bench-no-run-r1.log` (SHA-256
`6156fbdc3709d1bedac354c824894a459508823199ce2ae37c12a440f9fa611c`). It
contains the single release executable
`target/release/deps/crossover_block_benchmark-0f503e773e194c2e` (SHA-256
`06eafe84b566e4a10b9123c1a14e83fd16c6d20fef183a8e61e05dec26566e87`). The
benchmark source hash is `bccca5d341a43cba59d9629c0d0fee24e4feceea83d4ecfec0eb9ede066c4220`;
the Crossover production source still matches the preserved pre-edit source
hash `0c4357938dcf6b74141eee65416592b5c6db0b3a61a0efebcf9e8a742df28053`.
The executable is also retained as
`audit/artifacts/aud142-preedit/crossover-benchmark-r3.tar.gz`; its unpacked
binary SHA-256 matches the release executable above.

## Optimized CPU baseline

The successful release run executed the prebuilt benchmark from a fresh
`/tmp` working directory, so Criterion's output directory was real and its
raw sample and estimate files were saved. The terminal log is
`logs/crossover-bench-timing-r3.log`; source data is retained under
`criterion-r3/`, with one 30-sample `new/sample.json` and `new/estimates.json`
for each of the 31 cases. The complete medians, 95% median intervals,
sample minima/maxima and per-case sample CVs are in
`audit/artifacts/aud142-preedit/crossover-bench-summary-r3.csv`.
`crossover-bench-r3-receipt.json` records exact commands, hashes and the
timing environment. The run exited 0 without Criterion persistence errors.

Selected median setup times are LR24 2-channel 2-band 1.258 µs, LR24
2-channel 4-band 11.586 µs, LR24 8-channel 4-band 37.405 µs, LR24 8-channel
per-channel 9.550 µs, and FIR 4-band at 63/511/1,025 taps 24.909/168.953/
332.432 µs. These iterations construct and initialize the plugin and then
drop it.

Selected median processing times per call (32/512/2,048 frames) are LR24
2-channel 2-band 0.474/7.324/28.999 µs; LR24 2-channel 4-band
8.587/137.844/542.615 µs; LR24 8-channel 4-band 27.363/446.813/1,780.575 µs;
and LR24 8-channel mixed per-channel 1.507/23.785/95.275 µs. FIR 2-channel
4-band 63-tap times are 10.498/168.159/678.556 µs; at 511 taps they are
75.437/1,214.802/5,080.122 µs. Processing reuses the initialized instance;
the timer covers one `process` call, not setup.

These are throughput measurements from this host, not worst-case execution
time guarantees. The CPU has 128 logical CPUs and the benchmark process was
allowed CPUs 0–127. A local process/load snapshot at 06:09:15 UTC, 3 minutes
before the run began, recorded load averages 12.70/22.83/28.44; the visible
top process was the Codex process at 25% CPU and no Cargo process was present.
BandSplit and DynamicEQ owners held their builds and CPU-heavy tests during
the timed window, though container-visible load cannot account for every
host-level workload. Most sample CVs were small, but high variation appeared
for FIR 2-band 63 taps at 2,048 frames (19.45%), FIR 4-band 511 taps at 512
frames (12.13%), FIR 2-band 63 taps at 512 frames (8.69%), LR24 8-channel
4-band setup (5.53%) and FIR 2-band 511 taps at 32 frames (5.59%). Keep those
cases qualified; use the raw sample data and the preserved binary for any
later paired comparison. No timing claims are made from the earlier smoke or
failed-output attempts r1/r2.

## Current host parameter enumerations

### NIH native wrapper

The current wrapper has no static Crossover `ParamSpec` arm. Its fallback
enumerates the default plugin runtime IDs `[type, frequency, mode]` and omits
string parameters when converting metadata to NIH types. The actual published
native parameter/state-ID list is therefore only `[frequency]`.

The captured `frequency` parameter is Float, `20..=20000 Hz`, default 1000,
`logarithmic=false`, `realtime=true`. NIH constructs a linear `FloatRange` for
this parameter. Its normalized probes are:

| Normalized | Plain Hz |
| ---: | ---: |
| 0.00 | 20 |
| 0.25 | 5,015 |
| 0.50 | 10,010 |
| 0.75 | 15,005 |
| 1.00 | 20,000 |

The default/current normalized value is `0.049049050`; the saved parameter
field ID is `frequency`. No native `type` selector exists before this route is
implemented. Executed evidence is the 2/2 test log
`logs/aud142-nih-native-baseline-r2.log`; r1 is retained as history but its
printed range label was corrected in r2.

### FFI parameter map

The executed FFI `ParameterMap::from_plugin` fallback exposes the Crossover
runtime list directly. Captured order is:

| Instance | Runtime/FFI ID order |
| --- | --- |
| LR24 two-way | `type`, `frequency`, `mode` |
| LR24 four-way | `type`, `frequency`, `mode`, `frequency_2`, `frequency_3` |
| FIR four-way | `type`, `frequency`, `mode`, `frequency_2`, `frequency_3`, `fir_taps` |
| LR24 per-channel stereo | `type`, `channel_frequency_0`, `channel_mode_0`, `channel_frequency_1`, `channel_mode_1` |

Frequency and integer values normalize linearly in this current fallback;
runtime strings remain enumerated with fallback numeric metadata but do not
produce a normalized read. In the FIR instance, raw `type` reads as
`LinearPhase`, the canonical current runtime string, even though construction
used the legacy alias `FIR`. `frequency`, `frequency_2`, and `frequency_3`
retain their per-instance positions; `fir_taps` follows them only for FIR.
Executed evidence is `logs/aud142-ffi-parameter-map-baseline-r1.log` (1/1).

These observations match current source: NIH fallback is data-type-filtered,
while FFI uses the configuration-dependent runtime list because its static
Crossover spec arm is empty. New controls must be appended without rebinding
these old runtime prefixes; app JSON choice indices 0/1 remain LR24 and
LinearPhase as accepted in the route design.

## Commands and receipts

All Cargo invocations used the shared lock
`/tmp/sotf-daw-audit-cargo.lock`, `TMPDIR=/tmp`, the warm target
`crates/sotf-plugins/target`, `--offline`, and `--locked`.

```text
cargo test --offline --locked -p sotf-plugin-crossover --test aud142_preedit_baselines capture_aud142_pre_edit_audio_and_runtime_metadata -- --ignored --exact --nocapture
cargo test --offline --locked -p sotf-plugin-crossover --test aud141_baselines --test aud142_preedit_baselines
cargo test --offline --locked -p plugins-nih --no-default-features --features crossover --lib aud142_native_baseline -- --nocapture
cargo test --offline --locked -p plugins-ffi --lib aud142_crossover_baseline -- --nocapture
cargo bench --offline --locked -p sotf-plugin-crossover --bench crossover-block-benchmark --no-run
/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/release/deps/crossover_block_benchmark-0f503e773e194c2e --bench --noplot --sample-size 30 --warm-up-time 1 --measurement-time 2
```

Captured logs identify each actual command and terminal result. The timing
command ran from `/tmp/aud142-crossover-criterion-r3`; it writes Criterion
data relative to that working directory. The retained binary can be extracted
from `audit/artifacts/aud142-preedit/crossover-benchmark-r3.tar.gz` and run
with the same arguments from a fresh directory containing `target/criterion/`.
An earlier direct run without `--bench` only smoke-tested the cases and is
retained as r1. The first real-mode attempt, r2, was interrupted after the
workspace `target` symlink caused Criterion to fail writing output; neither
attempt is included in the reported baseline.
