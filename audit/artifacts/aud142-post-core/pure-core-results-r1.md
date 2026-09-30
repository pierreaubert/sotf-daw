# AUD142 pure Crossover core result packet

Status: the pure DSP and public Crossover parameter stage is implemented and
passed the scoped numerical, lifecycle/replay, package and lint gates below.
This is a core checkpoint only; FFI, actual engine, NIH native and mounted UI
routes remain outside this result. It is not full AUD142 feature acceptance.

## Numerical and lifecycle evidence

- The independent analog response and public process suite passed 7/7 in
  [`sotf-aud142-iir-families-r4.log`](logs/sotf-aud142-iir-families-r4.log)
  (SHA-256
  `c21a4ab0a0c79661b4c8b2a9d77e2e7b4cbb413320a9c95f844f83dec0d7d90b`).
  It covers all new families, independent complex transfer probes below/at/
  above cutoff, two- and four-way products/sums, normalized RMS waveform
  residuals, distinct per-channel routes, rejection at a newly inadmissible
  sample rate, invalid controls, finite output and returned frame counts.
- The complete package run passed **113 tests; 2 capture utilities were
  ignored** in
  [`sotf-aud142-crossover-package-test-r4.log`](logs/sotf-aud142-crossover-package-test-r4.log)
  (SHA-256
  `c8e117711e807bb5ba1abba8a27764592a464e7de083a106742d0546a58d4961`).
  It includes the 7/7 AUD142 suite, bit-exact AUD141/AUD142 fixture replays,
  and the repeated four-family allocation/reset test. The one intentional
  metadata difference in legacy replay is that `mode` is now explicitly
  `Structural` because selecting `Both` changes output width; the saved
  pre-edit metadata remains immutable and every other old runtime value/order
  and legacy audio vector is exact.
- Strict all-target Crossover Clippy passed in
  [`sotf-aud142-crossover-clippy-r2.log`](logs/sotf-aud142-crossover-clippy-r2.log)
  (SHA-256
  `f328e62f5b3b381acde7dd50f2e4c3bedfe837d824aae453f90a185923815cb4`).
  The changed benchmark target also passed strict Clippy in
  [`sotf-aud142-crossover-bench-clippy-r1.log`](logs/sotf-aud142-crossover-bench-clippy-r1.log)
  (SHA-256
  `59d8e191bb0f2b7123e4dcf98a4c18dd8ce539b5b2e732b5508c900ca4221b1d`).
- Root independently checked the raw counts and median/CV arithmetic in
  [`AUD142 CPU verification receipt`](../aud142-cpu-root-check-r1/receipt.json)
  (SHA-256
  `e451e8eab91e57984ea77f08e7d235f3289086cd4106488e3a6e1862b05027d1`).
  That is an arithmetic check of this same run, not a second timing run.

The separate immutable fixture details and per-source hashes are in
[`audit/crossover-aud142-pre-edit-baselines.md`](../../crossover-aud142-pre-edit-baselines.md).
Current implementation source hashes:

| Source | SHA-256 |
| --- | --- |
| `src/crossover_plugin.rs` | `cb9adee45f0e8bfc04e8ef33c1345e80adbcaed9e36d990b4573387e222e5ac8` |
| `src/iir_family.rs` | `4c0c921fa89a8175e93bb494eb3f5b2946b280e5b43a2c3e08da2198a693271e` |
| `src/tests.rs` | `457109563bbe6e4c8f644397deeb87c606413a5d17e8b21097bdda1f5c64695e` |
| `tests/aud142_iir_families.rs` | `164e961c4b35f940c92e860fe0b4795999d894eea898586df1941fa93c09fbcd` |
| `tests/aud142_preedit_baselines.rs` | `b0c0c1650ef3a2d38e176601941bb781f4f49bba7865524dceb76a74aa230e7d` |
| `tests/aud141_realtime.rs` | `819f2d295115b97f3aec9af4a1f586cba0c023a5906f5002ebc05cdebc7aceda` |
| `tests/integration.rs` | `fb783c9d54dfb928b403ef621393e0a8907ca3082cb7f5e5d1daa66d0a00de0e` |

## Matched release throughput run

The pre-edit executable is retained at
[`crossover-benchmark-r3.tar.gz`](../aud142-preedit/crossover-benchmark-r3.tar.gz)
(SHA-256
`e7da1160ccc9e29b8327acccbd499ddcdf81522c1fc428042080fbc14a6cb6d6`);
its unpacked binary hash is
`06eafe84b566e4a10b9123c1a14e83fd16c6d20fef183a8e61e05dec26566e87`.
The post-core release executable is
[`crossover-benchmark-r1`](crossover-benchmark-r1) (SHA-256
`712be67ce14286a6b554f60df282eab51bc85c336f8d328dc55be66239760659`). Its
source adds new cases while retaining the exact 31 old case IDs and setup/
processing definitions.

Each invocation used `--bench --noplot --sample-size 30 --warm-up-time 1
--measurement-time 2`. The archived and current binaries were run without
builds, interleaved by legacy group, with separate `CRITERION_HOME` roots.
Eight per-command logs and their raw Criterion output are retained here:

| Variant | Filter | Log |
| --- | --- | --- |
| Current | `crossover_setup_and_initialize` | [`current setup`](logs/crossover-timing-r1-current-legacy-setup.log) |
| Archived | `crossover_setup_and_initialize` | [`archived setup`](logs/crossover-timing-r1-archived-legacy-setup.log) |
| Archived | `crossover_lr_interleaved_blocks` | [`archived LR24 process`](logs/crossover-timing-r1-archived-legacy-lr-process.log) |
| Current | `crossover_lr_interleaved_blocks` | [`current LR24 process`](logs/crossover-timing-r1-current-legacy-lr-process.log) |
| Archived | `crossover_fir_interleaved_blocks` | [`archived FIR process`](logs/crossover-timing-r1-archived-legacy-fir-process.log) |
| Current | `crossover_fir_interleaved_blocks` | [`current FIR process`](logs/crossover-timing-r1-current-legacy-fir-process.log) |
| Current | `crossover_new_iir_setup` | [`new-family setup`](logs/crossover-timing-r1-current-new-setup.log) |
| Current | `crossover_new_iir_interleaved_blocks` | [`new-family process`](logs/crossover-timing-r1-current-new-process.log) |

The raw roots contain 31 archived cases and 95 current cases, each with 30
samples and Criterion estimates. The 31 legacy case paths match byte-for-byte
by name across archived, current and original pre-edit Criterion data. The
current-only addition has 16 setup cases and 48 processing cases. The reusable
summary calculation is
[`summarize_crossover_timing.py`](summarize_crossover_timing.py).
Machine-readable summaries:
[`paired legacy CSV`](legacy-paired-summary-r1.csv),
[`new-family CSV`](new-family-summary-r1.csv),
[`summary JSON`](timing-summary-r1.json), and the exact
[`run receipt`](timing-r1-receipt.json).

### Legacy comparison

Current/archived median ratios are close to 1 for all three groups. The full
per-case rows and both CVs are in the paired CSV.

| Legacy group | Cases | Ratio min | Ratio median | Ratio max |
| --- | ---: | ---: | ---: | ---: |
| Setup and initialize | 7 | 0.969 | 1.000 | 1.011 |
| LR24 processing | 12 | 0.932 | 0.999 | 1.033 |
| FIR processing | 12 | 0.965 | 0.999 | 1.016 |
| All legacy cases | 31 | 0.932 | 1.000 | 1.033 |

The maximum per-case sample CV was 13.29% in the archived run and 6.83% in
the current run. The old and current measurements were paired during the same
no-build window, but the host load changed substantially. The initial
load-average snapshot was 59.96/25.78/19.80 (1/5/15 minutes); after the window
it was 4.23/7.48/12.89. TokenSave reported an index rebuild in progress in its
pre-window status. Root, BandSplit and Upmixer confirmed their CPU-heavy work
was held. This changing and partly unobservable shared-host load limits
small-ratio interpretation; this is not a performance acceptance claim.

### New-family absolute times

Setup is construct + initialize + drop; processing is one call on a reused
instance. Values below are medians in microseconds. `—` means that combination
was intentionally not included. These are throughput measurements, not
worst-case execution-time bounds.

**Setup medians (µs)**

| Family | 2-way Both | 4-way Both | Per-channel |
| --- | ---: | ---: | ---: |
| LR12 | 1.472 | 5.474 | 2.089 |
| LR48 | 1.824 | 7.644 | 2.475 |
| BW6 | 1.537 | — | — |
| BW42 | 1.785 | 3.049 | 2.450 |
| BW48 | 1.798 | 3.092 | 2.484 |
| Bessel12 | 1.519 | 2.246 | 1.949 |

**Two-way Both processing medians (µs/call)**

| Family | 32 frames | 512 frames | 2,048 frames |
| --- | ---: | ---: | ---: |
| LR12 | 0.840 | 13.173 | 52.334 |
| LR48 | 1.199 | 18.743 | 75.032 |
| BW6 | 0.682 | 10.567 | 42.288 |
| BW42 | 1.180 | 18.999 | 74.879 |
| BW48 | 1.189 | 18.905 | 74.973 |
| Bessel12 | 0.678 | 10.606 | 42.338 |

**Four-way Both processing medians (µs/call)**

| Family | 32 frames | 512 frames | 2,048 frames |
| --- | ---: | ---: | ---: |
| LR12 | 8.814 | 139.405 | 561.604 |
| LR48 | 13.429 | 216.132 | 865.614 |
| BW42 | 3.602 | 57.248 | 230.704 |
| BW48 | 3.643 | 58.446 | 234.319 |
| Bessel12 | 2.060 | 32.980 | 130.861 |

**Per-channel processing medians (µs/call)**

| Family | 32 frames | 512 frames | 2,048 frames |
| --- | ---: | ---: | ---: |
| LR12 | 0.823 | 12.942 | 51.808 |
| LR48 | 1.132 | 18.064 | 71.799 |
| BW42 | 1.132 | 17.902 | 71.834 |
| BW48 | 1.132 | 17.911 | 71.449 |
| Bessel12 | 0.656 | 10.259 | 40.923 |

The highest new-family per-case CV was 8.51%; the maximum setup CV was 1.03%.
The new-family values are absolute costs for the tested 2-channel layouts; the
archived binary has no corresponding family cases. Preserve the raw samples
and estimates when comparing another implementation.

## Reproduction and scope

The current executable was built with
`flock /tmp/sotf-daw-audit-cargo.lock env TMPDIR=/tmp
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target
cargo bench --offline --locked -p sotf-plugin-crossover --bench
crossover-block-benchmark --no-run`. The archived executable is directly
recoverable from its retained tar archive. Each command set
`CRITERION_HOME` to its own durable output root, avoiding the broken workspace
`target` symlink. The receipt includes exact paths, hashes, machine details,
and run order. Re-run summary generation with:

```text
python3 audit/artifacts/aud142-post-core/summarize_crossover_timing.py
```

The core package gates used the shared Cargo lock, offline locked mode,
`TMPDIR=/tmp`, and the warm plugin target. No source was edited during the
timing window. No worker ran a competing audit benchmark or build. No
worst-case or release-quality performance guarantee is inferred.
