# AUD125: Maximum Momentary and Short-term Loudness

**Status:** core/API and sibling TUI/GPUI display stages are accepted by Astra
as of 2026-09-28. The full audit remains in progress. No EBU Mode or
certification claim is made. MIDI and IAMF remain excluded.

## Implementation

The host latches finite programme maximum Momentary and Short-term LUFS from
the monitor's completed, epoch-aligned 100 ms observation grid. Momentary is
eligible after `ceil(0.4 * sample_rate)` accepted source frames; Short-term is
eligible after `3 * sample_rate`. The values advance independently of snapshot
publication, reset with the Integrated measurement epoch, survive active
spatial-cache rebuilds, and are copied through `LoudnessData` constructors,
`Default`, `update_from`, serialization defaults, and prepared snapshots.

The current backend still uses `floor(sample_rate / 10)` frames per sub-block.
At rates not divisible by ten, this can make the backend's four/thirty-block
window width shorter than exact 400 ms/3 s; the latch does not become eligible
before the exact elapsed-frame threshold. This batch does not change that
inherited geometry. Values are maxima of the fixed observation grid, not
continuous-time maxima.

## Correctness and lifecycle evidence

- `/tmp/sotf-aud125-focused-final4.log`: 7 nested realtime tests and 9 maximum
  M/S tests pass. Coverage includes callback partition invariance, LRA on/off,
  independent test-owned BS.1770 weights, 5.1 and 7.1.4 routes, exact 48 kHz
  and 11,025 Hz eligibility boundaries, serialization/default compatibility,
  silence/cold queries, finish neutrality, reset, disable/enable, and cache
  rebuilds.
- The retained-reader regression first populates both maxima, holds all three
  snapshot generations, processes a louder signal while publication is blocked,
  releases the readers, and confirms both larger same-epoch maxima are
  published. The process calls run under the test's zero-allocation guard.
- `cargo clippy --locked --offline -p sotf-host --all-targets -- -D warnings`
  passes; log: `/tmp/sotf-aud125-clippy-final3.log`.
- The required offline workspace gate, including FFI and excluding MIDI/IAMF,
  passes: 6,020 tests passed, 11 skipped, 2 slow tests; 268.330 s. Log:
  `/tmp/sotf-aud125-workspace-nextest-final2.log`. The first full run had one
  convolution reset test failure outside AUD125 scope (no causal attribution to
  AUD125 was established); that isolated test passed on rerun, and the repeated
  full gate passed. The source manifest and AUD125/lockfile hashes
  matched before and after both full runs.

Commands:

```sh
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/tmp \
  cargo test --locked --offline -p sotf-host \
  --test maximum_ms_loudness --test loudness_nested_realtime -- --nocapture
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/tmp \
  cargo clippy --locked --offline -p sotf-host --all-targets -- -D warnings
CARGO_NET_OFFLINE=true \
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/tmp \
  cargo --locked --offline nextest run --workspace \
  --exclude sotf-midi --exclude sotf-iamf --no-fail-fast --status-level fail
```

## Matched CPU evidence

Criterion compares the saved pre-AUD125 production baseline with the candidate
using the same low-level `LoudnessMonitor::add_frames` fixtures. Each case is a
48 kHz, 480-frame callback after 301 prefill callbacks (3.01 seconds), with
20 samples, a one-second warm-up, and three seconds of measurement. The matrix
covers stereo, explicit 5.1, and explicit 7.1.4, each with LRA enabled and
disabled. It does not include plugin snapshot publication or represent a
worst-individual-callback bound.

| Layout / LRA | Pre-code estimate | Candidate estimate | Criterion change |
|---|---:|---:|---:|
| Stereo / on | 31.477 µs | 32.917 µs | +4.57%, detected |
| Stereo / off | 33.425 µs | 33.381 µs | −0.26%, no detected change |
| 5.1 / on | 83.559 µs | 76.547 µs | −8.33%, detected improvement |
| 5.1 / off | 77.781 µs | 76.720 µs | −1.20%, within noise threshold |
| 7.1.4 / on | 165.44 µs | 164.81 µs | −0.02%, no detected change |
| 7.1.4 / off | 154.73 µs | 164.06 µs | +5.96%, detected |

The candidate run reports Criterion estimate intervals, which summarize the
aggregate timing distribution rather than worst callback latency. Two cases
show statistically detected regressions: stereo/LRA-on by about 1.44 µs per
10 ms callback and 7.1.4/LRA-off by about 9.33 µs. These are approximately
0.014 and 0.093 percentage points of a 10 ms callback period, respectively;
they remain explicit costs of the current implementation. The other cases are
unchanged, within Criterion's noise threshold, or faster. Logs:

- Pre-code baseline saved with `--save-baseline aud125-pre-code`:
  `/tmp/sotf-aud125-ms-baseline-saved.log`.
- Candidate command:
  `cargo bench --locked --offline -p sotf-host --bench aud125_ms_cost -- --noplot --baseline aud125-pre-code`.
  Log: `/tmp/sotf-aud125-ms-candidate-final.log`.

The measured machine was an AMD Ryzen Threadripper PRO 3995WX (64 cores, 128
hardware threads), x86_64 Linux, with frequency boost enabled and frequency
scaling active. Bench profile is optimized. The baseline/candidate comparison
is a low-level monitor cost comparison, not a historical whole-monitor or
plugin-publication benchmark.

## Snapshot and lockfile disposition

The workspace source manifest (`*.rs`, `Cargo.toml`, and `build.rs` under
`crates/`) is recorded at `/tmp/sotf-aud125-workspace-source-start.sha256` and
`/tmp/sotf-aud125-workspace-source-end2.sha256`; they are byte-identical.
The final batch manifest is `/tmp/sotf-aud125-batch-end2.sha256`. Key hashes:

```text
Cargo.lock                                      07c81e7e1be09b1138d9f1c6590da939ec6ca409821bb077a28fa801c2087e95
analyzer_loudness_monitor.rs                    3efa5b3386535d27ab41859bfd95036cb36dee7798f3a71cbd5bc98544332106
analyzer.rs                                     46f2c45fbd0dfa7916a6df03d16bfca5f394055e1ed2a355a361a475c132399a
maximum_ms_loudness.rs                          f847b2f505aa4002bc307761a249ffa33657ef62e91659f3961ec6f435120345
loudness_nested_realtime.rs                     fd1f29cfe30fb33e2fcda96b1bd7c424ba0221dabba4c5f44cbee497ee93a5cf
aud125_ms_cost.rs                               659ca8781a538607b1d33edc90f8e8c9597966a02ee895f5a37ea2b123b76a27
```

The worktree `Cargo.lock` was already dirty before AUD125. A first non-locked
focused test invocation triggered a broader resolver rewrite; no exact
immediate pre-run lockfile snapshot exists, and the available older backups
are not authorized restoration targets. Astra's read-only review found the
current resolution consistent with the broader workspace manifests and
requested preserving it rather than guessing. Its hash stayed at
`07c81e7e…` through the locked focused, lint, benchmark, and workspace gates.
Details: `audit/reviews/AUD125-lockfile-disposition.md`.

## Sibling TUI/GPUI integration and validation

The maxima are displayed on the existing TUI level-meter surface and the
reachable Studio Loudness Monitor. Both are finite-only, use the host's prepared
snapshot fields, and do not depend on current M/S validity flags. TUI redraws
track the displayed 0.1 dB precision. GPUI keeps the LUFS unit in the section
heading, stacks localized values, and uses tighter row/group spacing so the
complete meter panel fits its tested compact and 720x900 viewports.

- `/tmp/sotf-aud125-tui-lufs-final4.log`: 3/3 finite/unsupported, height,
  localization, and border-preservation tests pass.
- `/tmp/sotf-aud125-tui-signature-final2.log`: 1/1 redraw-signature test passes.
- `/tmp/sotf-aud125-gpui-translations-final2.log`: 1/1 translation-completeness
  test passes.
- `/tmp/sotf-aud125-gpui-e2e-ms-final4.log`: 3/3 compact render, mounted Studio
  layout/retained-maximum/reset, and routed live-query tests pass. The route
  covers initial values, higher then lower intervals, reset/null, and nonfinite
  maxima mapping to JSON null for TP/M/S.
- `/tmp/sotf-aud125-gpui-all-targets-final2.log`: all-target `dev-api` check
  passes. `rustfmt --check`, pseudo-locale `--check`, design-token validation,
  and `git diff --check` also pass.

The GPUI gates used the temporary offline resolver lock SHA-256
`87f37029677509822f0164117da1a37fcf39d72a2523908b1b036f0b46bd554a`, which
provides the cached `mimalloc 0.1.52` stats API. The exact original sibling
lock SHA-256 `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`
was restored after the gates; no Cargo ran afterward. The tested SOTF source+
temporary-lock manifest SHA-256 is
`d9b5e6f5ddfad6d2d5ccf3439b5d91ed5b365dc62f17f2b7cb7b3aa5f178372c`; the
current source+restored-lock manifest SHA-256 is
`bb5e6cf1be2b8b280221d61dc27561dad525dbb7b081158463a940d66ca5af53`. These
gates are not claims against the restored lock.

## Remaining work

AUD126 pause/continue, AUD127 LRA stability, and AUD128 official corpus coverage
remain separate audit items.
