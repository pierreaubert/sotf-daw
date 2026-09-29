# AUD-124 — Programme Maximum True Peak

Status: confirmed API/measurement gap; Astra accepted the staged core/API
implementation on 2026-09-28 after focused validation and the offline workspace
gate. EBU display remains open under
`programme-maximum-true-peak-ui.md`, and AUD-124 is not closed.

## Requirement

EBU Tech 3341 v4.0 (2023) lists **Maximum True Peak Level** as one of the three
minimum EBU Mode measures a meter must measure and display (§2.8). Its §2.6
requires the true-peak result to meet the specified tolerances, including the
filters' ripple and under-read. [Official Tech 3341 PDF](https://tech.ebu.ch/docs/tech/tech3341.pdf),
[official change log](https://tech.ebu.ch/publications/tech3341/changelog).

This proposal treats the requirement as a target for EBU Mode feature parity.
The current SOTF meter does not advertise EBU Mode or external programme
certification.

## Confirmed current behavior

- `Bs1770TruePeakMeter` accumulates per-channel peaks between data queries.
  `take_interval_peak()` uses `mem::take`, so a successful `update_loudness_data`
  returns peaks for that query interval and clears those interval accumulators.
- `LoudnessData` exposes only `true_peaks_dbtp: Arc<Vec<f64>>`; it has no scalar
  true-peak maximum or programme-epoch maximum. `LoudnessMonitorPlugin` queries
  after accepted callback input. A larger early peak can therefore disappear
  from later snapshots after a lower-level interval.
- Final publication merges the immediately preceding snapshot with the final
  interpolation suffix. That handles EOS publication, but does not recover a
  maximum from earlier, already-published intervals.
- The TUI displays the maximum of the current per-channel vector in its
  “True Peak” heading. GPUI uses the vector for per-channel bars and peak-spread
  display. Both read the current snapshot; neither retains a maximum across the
  complete measurement epoch. The app therefore has no programme-latched
  maximum to display.
- The host tests already aggregate per-channel vectors in test code across
  callbacks to prove partition invariance, which is evidence the test oracle
  must be lifted into the production measurement contract.

Source pointers:

- [`LoudnessData` fields and snapshot updates](../../crates/sotf-plugins/crates/sotf-host/src/analyzer.rs#L431)
- [interval peak consumption and `update_loudness_data`](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs#L592)
- [per-callback query and publication path](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs#L1667)
- [EOS-only merge with previous snapshot](../../crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs#L1316)
- [existing test-side per-channel maximum across partitions](../../crates/sotf-plugins/crates/sotf-host/tests/test_analyzer_plugins.rs#L545)
- [TUI current-vector maximum](../../../sotf/crates/app-tui/ui/draw_meters/draw.rs#L72)
- [GPUI per-channel true-peak bars](../../../sotf/crates/app-gpui/components/plugins/level_meters/render.rs#L293)

## Proposed contract

Keep `true_peaks_dbtp` as the existing per-channel query-interval result. Add a
separate optional scalar, tentatively named `maximum_true_peak_dbtp`, containing
the highest finite supported true-peak estimate across every channel and every
query interval since the current measurement reset epoch. `Some(value)` is
always finite dBTP. `None` means no finite non-silent result has been observed
in the epoch; this includes cold state, silence, and unsupported rates. The
existing support/enabled fields continue to describe those separate states.

- The scalar is `None` before a finite true-peak observation and for silence;
  it never uses negative infinity as a public value. When available, it equals
  the maximum of the per-channel maxima accumulated across the epoch.
- `LoudnessMonitor::reset`, plugin reset, reinitialization, and the existing
  destructive disable transition clear the value. A future AUD-126 pause must
  not implicitly clear it; whether paused audio continues metering is for
  AUD-126 to define. Pause/continue state changes are outside this batch.
- Successful and skipped snapshot publications, arbitrary callback partition,
  query frequency, and finalization cannot lower or lose the value. The final
  zero-continuation true-peak suffix contributes to it before EOS publication.
- Initialize and update the field in `LoudnessData::new`, `Default`,
  `update_from`, reset paths, manual snapshot construction, and monitor query
  publication. Retained readers of older generations must remain unchanged;
  resetting the epoch must publish a cold value to the new generation.
- The snapshot field uses `serde(default)` and omits `None` when serialized, so
  silence is JSON-safe and older serialized `LoudnessData` remains readable.
  Preserve all current per-channel and sample-peak fields and their meanings.
- The true-peak maximum is a scalar reduction over the whole programme, not
  the largest latest-interval channel value. The TUI/GPUI should display the
  programme-latched maximum separately from their existing channel bars.

## Implemented core/API shape

Keep a finite-only `Option<f64>` latch inside `LoudnessMonitor`, alongside the
prepared `Bs1770TruePeakMeter`. After a query takes each interval maximum,
ignore non-finite values and update the programme latch with the greatest
finite per-channel interval result. Copy the scalar into `LoudnessData` as part
of the existing complete snapshot write. Reset the latch with the underlying
measurement reset and explicitly clear it in `reset_loudness_data`. This keeps
allocation and locking off the realtime path and preserves the existing
nested-Arc publication readiness checks.

The cache constructors seed all three prepared snapshots from the monitor's
current epoch value when a control-time option rebuilds the cache. Reset and
reinitialization start with `None`. The UI renderers are unchanged in this
stage.

This core/API batch is one stage of AUD-124; it does not satisfy EBU Tech
3341's display requirement. TUI/GPUI programme-maximum display remains an open
follow-up because those consumers live in the sibling `sotf` checkout outside
this writable DAW root. The `LoudnessData` field itself is public and
re-exported through the plugin/player facades, so downstream struct literals
and serialization must be checked in the host workspace.

## Acceptance evidence

1. Add a red public regression with a high true-peak event in one channel and
   an early query, followed by lower later intervals in other channels. Verify
   that `true_peaks_dbtp` remains interval-scoped while
   `maximum_true_peak_dbtp` retains the early programme maximum.
2. Compare the scalar against an independent per-channel full-response
   convolution/reference over irregular callback partitions and query
   intervals, including 48/96 kHz published paths and a custom high-ratio rate.
3. Verify unqueried intervals and retained strong/Weak readers do not lose
   peaks when publication is skipped; verify the final FIR suffix updates the
   scalar exactly once and repeated drain is stable.
4. Verify reset, reinitialization and disable/enable start a fresh maximum
   epoch, even while old outer and nested-Arc generations remain retained.
   Rejected callbacks and queries/finish without accepted input must not invent
   a peak. Check empty, silence, positive result, unsupported rate, old finite
   serialized payload defaults, and new-field round trips explicitly. Existing
   unrelated `-inf` loudness fields mean this batch will not rewrite
   whole-snapshot cold serialization behavior.
5. Add a fresh-thread realtime allocation check at representative 1/2/6/24
   channel widths. The added scalar state and updates must allocate and free
   nothing on processing, query, publication and drain.
6. Keep TUI/GPUI display as an open AUD-124 acceptance item and implement it in
   a sibling-repository batch after independent review and writable-boundary
   approval. The concrete display proposal is
   `programme-maximum-true-peak-ui.md`. Add a multi-interval UI
   fixture proving a lower later snapshot cannot replace the programme
   maximum. Do not claim EBU Mode until display, the full EBU test set, and the
   remaining AUD-125/126/127 requirements are reviewed.

## Core/API implementation evidence

### Final gates after the EOS-scalar sensitivity fix

- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo test -p sotf-host --test programme_true_peak`: **6 passed**. The final-only largest impulse increases the programme maximum after drain at 48/96 kHz against direct published-table convolution and at 8 kHz against a separate radius-64 Lanczos reconstruction. Strong and nested-Weak readers retain all three prepared generations during the blocked publication; release and retry recovers the maximum, and another drain leaves it stable. Log: `/tmp/sotf-aud124-eos-focused.log`.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo clippy -p sotf-host --all-targets -- -D warnings`: passed. Log: `/tmp/sotf-aud124-eos-clippy.log`.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" TMPDIR="$PWD/crates/sotf-plugins/target/audit-tmp" cargo nextest run --offline --workspace --exclude sotf-midi --exclude sotf-iamf --no-fail-fast --status-level fail`: **6,003 passed, 11 skipped, 355 binaries, 347.721 s**, including FFI. Log: `/tmp/sotf-aud124-workspace-nextest-final.log`.
- `rustfmt --edition 2024 --check crates/sotf-plugins/crates/sotf-host/tests/programme_true_peak.rs` and `git diff --check`: passed after the last test edit.

The gate used the nested workspace target and inherited offline mode. The
AUD-124-owned files were unchanged while it ran; a parallel Upmixer audit used
different crate paths. No pre-run source-hash manifest was captured, so these
end-state hashes identify the AUD-124 files after the gate rather than proving
a frozen whole-worktree snapshot:

```text
3f3a4f89271a54b9ea0e115b116677dfdda772523ae00c4190e15a289993d20a  crates/sotf-plugins/crates/sotf-host/src/analyzer.rs
78de07ceb22697adce4acf715412d36358fc878ad20cdf1854d33bfc6f083cf1  crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs
fa7a0b9cc5927fd718b998442f77428838c399f656570b333afcef7add12d0fd  crates/sotf-plugins/crates/sotf-host/tests/programme_true_peak.rs
646bd4b6cf1a1e693b9057c566a09c0f7adf523905998c6bfdb4d1376cf04363  crates/sotf-plugins/crates/sotf-host/tests/true_peak_calibration_heap.rs
ea79bd2483404c29f235b7a96a11a4da6bd40f8f67025a5efe6466521b488e13  crates/sotf-plugins/crates/sotf-host/tests/loudness_nested_realtime.rs
1181c3bcd270106413a581eb375fe0d2ec77b818388bd5e9561016962e75fdba  crates/sotf-plugins/crates/sotf-host/tests/true_peak_finite_stream.rs
```

- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo test -p sotf-host --test programme_true_peak`: 5 passed. The 48/96 kHz programme result matches the independent published-table convolution within `2e-12`; custom 8/12/44.1/48/96 kHz reductions, unsupported 7,999 Hz, rejected input, empty drain, retained generations, reset/reinitialize/disable, serialization defaults, and silence all pass. Log: `/tmp/sotf-aud124-focused-2.log`.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo test -p sotf-host`: exit 0; 549 host library tests passed, all host integration and helper-binary suites passed, and 1 doctest passed (8 ignored). The existing full-history LRA allocator test took 308.31 seconds. Log: `/tmp/sotf-aud124-host-full.log`.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/crates/sotf-plugins/target" cargo clippy -p sotf-host --all-targets -- -D warnings`: passed. Log: `/tmp/sotf-aud124-host-clippy.log`.
- Realtime allocation checks passed with the new scalar updated across supported rates, drain, reset, and retained-reader paths. Logs: `/tmp/sotf-aud124-heap-rate.log`, `/tmp/sotf-aud124-heap-drain.log`, and `/tmp/sotf-aud124-nested-realtime.log`.
- `rustfmt --edition 2024 --check` on the six touched Rust files and `git diff --check`: passed.

The scalar adds a bounded finite check and maximum comparison per channel when
query intervals are consumed. No separate CPU microbenchmark was run. This
batch adds no callback allocations. The TUI/GPUI display and official EBU corpus
remain outside the accepted core/API scope; no EBU Mode or certification claim
is made.

The official EBU Loudness Test Set v5.0 page lists 70 test files for Tech 3341
and Tech 3342. The corpus is not present in the DAW/app source trees, so corpus
acquisition and full execution remain tracked separately as AUD-128:
[EBU Loudness test set](https://tech.ebu.ch/publications/ebu_loudness_test_set).
