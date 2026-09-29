# AUD126: Coupled Integrated Loudness and LRA Pause/Continue

**Core/API status:** accepted by Astra on 2026-09-29.
**AUD126 status:** core/API and reachable sibling Start/Pause/Continue/Reset UI
stages accepted by Astra on 2026-09-29. The UI evidence and sibling lock
qualification are recorded in [the UI report](coupled-integrated-lra-pause-ui.md).
AUD127, AUD128, and the broader audit remain open.
MIDI and IAMF tests remain excluded from workspace gates.

## Implemented behavior

The host exposes `integrated_measurement_running` in `LoudnessData` (old
snapshots default to `true`), matching direct monitor/plugin controls, and the
`integrated_running` Boolean plugin parameter. The plugin getter and live
control state update immediately. Published snapshots are best-effort and may
show the prior state while every writer slot is retained; the next successful
publication carries the new flag without consuming an audio query or interval
peak.

The monitor keeps a prepared active-time I/LRA lane separate from live
Momentary/Short-term, sample peak, true peak and correlation. Paused input
continues through the live lane and exact analyzer pass-through. I/LRA filters,
partial sub-block and histories are held while paused, so the resumed result
matches the concatenated pre-pause and post-resume programme. Pause/continue
never clear programme or live maxima.

The active lane's completed backend sub-block count controls I and LRA
admission: four blocks for Integrated Loudness and thirty blocks for the first
Short-term/LRA observation. This preserves the backend's existing
`floor(sample_rate / 10)` grid at rates not divisible by ten. Live M/S validity
continues to use the existing sample-count windows. Reset clears measurement
history while preserving Running/Paused. Explicit start and reinitialization
enter Running. A transition of the destructive `enabled` switch clears history
and enters Running whether disabling or re-enabling; repeating a value is a
no-op. Spatial snapshot rebuilds preserve the current state. Changing
integrated mode starts a fresh history while preserving that state.

## Regression coverage

The two new host integration suites cover:

- A/B/C programme exclusion against an A+C control, including 4-second single
  callback input, irregular and 100 ms callback partitions, live telemetry
  changes during pause, exact pass-through, and retained maxima.
- One-frame pause/resume during a partial sub-block, with post-resume I/LRA
  equal to a concatenated control.
- 11,025 Hz never-paused admission boundaries: no I/LRA observation before
  four/thirty completed backend sub-blocks, finite Integrated Loudness after
  block four, and the first LRA observation at block thirty. The paused
  11,025 Hz case also asserts observations at active blocks thirty and
  thirty-one.
- Whole-program and rolling integrated modes; LRA on/off; stereo, explicit
  5.1, and explicit 7.1.4; reset/start/reinitialize/enable transitions;
  serde defaults and `update_from`; retained strong/nested-Weak cache pressure
  and full-snapshot preservation.
- Allocation-free running, paused, resumed, pause/continue/reset/start control
  paths and explicit wide-layout processing.

The final focused command passed 9 pause/lifecycle tests and 3 realtime/storage
tests:

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo test --locked --offline -p sotf-host \
  --test loudness_pause_continue --test loudness_pause_realtime
```

Log: `/tmp/sotf-aud126-focused7.log`. The memory probe was run with
`-- --nocapture`; its output is `/tmp/sotf-aud126-memory-realtime-final.log`.

## Prepared monitor storage

A warmed test allocator measured the net requested heap bytes retained by
constructing `LoudnessMonitor` at 48 kHz with whole-program integrated mode.
It sums allocation layout sizes and subtracts deallocations during
construction, after one unmeasured construction of each route. It excludes
allocator metadata, plugin snapshot caches and process RSS; these are total
prepared monitor bytes, not incremental bytes relative to an archived
pre-AUD126 meter.

| Layout | Channels | LRA off | LRA on |
|---|---:|---:|---:|
| Stereo | 2 | 579,528 B | 581,576 B |
| Explicit 5.1 | 6 | 588,256 B | 590,304 B |
| Explicit 7.1.4 | 12 | 317,768 B | 319,816 B |

The wide explicit route uses the host's prepared per-channel mono meter set;
its storage layout differs from the ordinary EBU R128 path.

## CPU evidence

The matched pre-edit baseline is the AUD125 monitor-core Criterion benchmark
captured before AUD126 Rust source edits. It uses 480-frame/10 ms callbacks at
48 kHz, 301 callbacks of prefill, 20 samples, 1 s warm-up and 3 s measurement
per case. The final command kept the original six Running benchmark IDs for
comparison and used `--baseline-lenient` for six candidate-only Paused IDs:

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo bench --locked --offline -p sotf-host --bench aud125_ms_cost -- \
  --noplot --baseline-lenient aud126-pre-pause
```

Pre-edit log: `/tmp/sotf-aud126-cpu-pre-pause.log`. Final source measurement:
`/tmp/sotf-aud126-cpu-final2.log`. The machine was an AMD Ryzen Threadripper
PRO 3995WX, x86_64 Linux, with frequency scaling active. Criterion point
estimates and confidence intervals are aggregate timing estimates, not worst
individual callback bounds. These monitor-core `add_frames` measurements
exclude plugin snapshot publication.

| Route / LRA | Pre-edit estimate | Running estimate | Criterion change | Paused estimate |
|---|---:|---:|---:|---:|
| Stereo / on | 32.943 µs | 36.894 µs | +11.7% | 30.948 µs |
| Stereo / off | 32.986 µs | 36.739 µs | +11.4% | 32.899 µs |
| 5.1 / on | 81.584 µs | 94.879 µs | +16.3% | 81.901 µs |
| 5.1 / off | 81.470 µs | 89.872 µs | +10.2% | 75.804 µs |
| 7.1.4 / on | 150.26 µs | 191.55 µs | +27.5% | 160.84 µs |
| 7.1.4 / off | 150.33 µs | 194.56 µs | +29.3% | 161.80 µs |

Frequency scaling caused visible run-to-run movement: an earlier candidate run
on the same fixture measured stereo/LRA-on at 34.57 µs, 5.1/LRA-on at 88.18
µs and 7.1.4/LRA-off at 178.71 µs. The table reports the later final2 run;
these timings should be treated as profile evidence rather than a hard cost
bound. The measured 7.1.4 Running core cost remains below 0.2 ms of a 10 ms
callback in this profile.

## Final gates and source snapshot

Strict host all-target Clippy passed:

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo clippy --locked --offline -p sotf-host --all-targets -- -D warnings
```

Log: `/tmp/sotf-aud126-clippy-final2.log`.

The final offline workspace gate, with FFI included and MIDI/IAMF excluded,
passed 6,043 tests across 358 binaries; 13 were skipped. It took 268.544 s.
The only emitted warning was the existing `nih_plug` unused import.

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo nextest run --offline --locked --workspace \
  --exclude sotf-midi --exclude sotf-iamf --no-fail-fast --status-level fail
```

Log: `/tmp/sotf-aud126-workspace-final2.log`. The source/Cargo manifest lists
captured immediately before and after the gate match at SHA-256
`fc8ef98a8323141d4837a7eb92da01b74ab251f9af7235f21a32ad242459e12c`:
`/tmp/sotf-aud126-workspace-final2-start.sha256` and
`/tmp/sotf-aud126-workspace-final2-end.sha256`. `Cargo.lock` remained at SHA-256
`6fd8186e6c6ef66ac2a18c243fd3320bfd98f07fa54b230041b72cab1e3ed088`.

## Open work

AUD126 is complete across the host core/API and reachable sibling UI stages.
AUD127 first-minute LRA indication and AUD128 complete EBU corpus coverage
remain open. This report records host core behavior and evidence; it does not
claim EBU certification or completion of the broader metering audit.
