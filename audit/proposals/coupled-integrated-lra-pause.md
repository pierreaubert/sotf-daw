# AUD126: Coupled Integrated Loudness and LRA Pause/Continue

**Status:** host core/API implementation accepted by Astra on 2026-09-29.
AUD126 remains open until the sibling Start/Pause/Continue/Reset UI is accepted.
Implementation evidence is recorded in `audit/coupled-integrated-lra-pause.md`.
MIDI and IAMF remain excluded.

## Requirement and current behavior

[EBU Tech 3341 v4.0 (2023), §2.2](https://tech.ebu.ch/files/live/sites/tech/files/shared/tech/tech3341v4_0.pdf)
requires a meter to start, pause, and continue Integrated Loudness and Loudness
Range together. It also requires resetting both while the meter is running or
in stand-by. Section 2.1 requires maximum Momentary and Short-term readings to
reset whenever Integrated Loudness is reset. This is a feature comparison, not
a claim that SOTF implements or is certified for EBU Mode.

`LoudnessMonitor::add_frames` currently feeds one EBU R128 instance for live
Momentary/Short-term and Integrated Loudness. At each fixed 100 ms boundary it
also derives the host's optional whole-program/explicit integrated history and
LRA observations. The same call advances correlation, sample peaks, true
peaks, and AUD125's live M/S maxima. `LoudnessMonitorPlugin::enabled` is a
different, destructive control: disabling resets all monitor state and
re-enabling starts a clean measurement. Merely skipping integrated/LRA queries
while paused would be incorrect: EBU R128 would keep accumulating the paused
audio internally, and its M/S windows would bridge the pause into integrated
results on continue.

## Recommended control contract

Add an explicit integrated-measurement state, separate from the existing
plugin `enabled` state. A newly constructed or reinitialized monitor starts
in `Running` with an empty I/LRA epoch.

- `pause_integrated_measurement()` changes `Running` to `Paused` and is
  idempotent. It preserves the accumulated Integrated Loudness and LRA state.
- `continue_integrated_measurement()` changes `Paused` to `Running` and is
  idempotent. It resumes that same epoch; it does not reset history.
- `start_integrated_measurement()` performs the existing full monitor reset
  and enters a fresh `Running` epoch. The meter is initially in this state;
  `start` is the explicit way to restart after a pause.
- `LoudnessMonitor::reset()` and `Plugin::reset()` clear I/LRA and live
  measurements as they do today, but preserve the current Running/Paused
  state. Reset therefore works in either state; reset while Paused leaves
  I/LRA stopped until continue. M/S maxima restart only after their full live
  window duration. Reset is idempotent.
- A change to the existing destructive `enabled` switch clears all measurement
  history and enters a fresh Running epoch, both when disabling and when
  re-enabling. Repeating the same value is a no-op. While disabled, explicit
  pause/continue calls may update the reported state but accept no samples;
  enabling again clears that empty epoch and starts Running.

Expose `integrated_measurement_running: bool` in `LoudnessData`, defaulting to
`true` when deserializing an older snapshot. Provide matching public methods on
`LoudnessMonitor` and `LoudnessMonitorPlugin`; the plugin's existing parameter
surface also gains an `integrated_running` Boolean for hosts that control
plugins only through `set_parameter`. `false` pauses and `true` continues;
this toggle never resets. Its getter and live control state agree immediately;
the serialized snapshot field catches up on the next successful publication,
so retained readers may temporarily observe an older state. Repeating the
current value is a no-op. A reset through the existing Plugin trait preserves
the run state; `start_integrated_measurement()` resets and enters Running. The UI must
describe the distinction between Integrated/LRA pause
and the existing destructive `enabled` switch. This proposal covers the host
core/API; the required Start/Pause/Continue/Reset UI and routed controls remain
an explicit follow-up stage before AUD126 is closed.

## Signal and clock behavior

Pause controls the I/LRA measurement clock, not audio transport or pass-through.
Every input callback continues to copy its input unchanged to output. While
Paused, current Momentary/Short-term readings, sample peaks, true peaks,
correlation and their live observation clocks continue to process the supplied
audio. The AUD125 maximum M/S latches continue to observe that live series
unless the existing full reset clears them. True-peak interval state and the
AUD124 programme maximum are never cleared or frozen by pause or continue.
They continue to measure the audio stream.

Integrated Loudness and LRA consume only samples accepted while Running. The
pause duration is removed from their measurement-time axis: the final running
samples before a pause and the first samples after continue are adjacent in the
I/LRA windows, including their K-weighting filter histories. Compare pause
behavior to a control fed the exact concatenation A+C with no reset at the join;
the first samples after resume can therefore include the I/LRA filter tail from
the last samples before pause. Their 100 ms observation cadence follows
accumulated running audio frames, not host callback count or wall time. A paused
interval creates no integrated blocks and no LRA observations. Integrated
validity, LRA warming/count/status, and whole-program capacity all use this
active-lane frame/history only, never the live frame counters. Therefore those
results remain held while Paused, including when pause begins before the first
400 ms or 3 s window and a long paused interval follows. Use the active lane's
own completed sub-blocks for eligibility, preserving the backend's existing
floor(`sample_rate / 10`) observation-grid rounding at rates not divisible by
ten. The M/S live meter keeps its existing fixed-grid clock.

Playback stop is not inferred from the absence or content of audio. If the
engine makes no callbacks, both clocks naturally hold. If callbacks continue
with silence, live M/S/peak meters observe that silence while I/LRA behavior is
determined solely by the explicit Running/Paused control. Existing analyzer
disable semantics are not reinterpreted as pause.

## Implementation shape and realtime constraints

Split the meter into a live M/S/peak lane and a prepared I/LRA lane. The live
lane always receives input while the analyzer is enabled. The I/LRA lane is
fed only while Running. This is needed because the backend's single EBU R128
instance accumulates Integrated Loudness as it receives frames; omitting only
its integrated query cannot freeze the internal history while preserving live
M/S.

For ordinary layouts, prepare an EBU R128 I/LRA instance at construction or
initialization. Keep the live instance in `M|S` mode (plus sample peak where
currently needed), without allocating an unused integrated history. For
explicit layouts wider than six channels, prepare a second role-weighted
mono-meter set alongside the live set; these mono meters provide the M/S block
levels needed by the host's existing integrated accumulators. Both lanes use
the same channel mapping, LFE exclusion, role weights and sample rate. The
I/LRA lane supplies rolling integrated queries, whole-program block energies,
and LRA Short-term observations. It has independent active-frame and 100 ms
sub-block counters, so pausing mid-sub-block preserves the partial running
interval and continuing completes it from the next accepted samples. The live
lane's sub-block counter remains independent. Report measured/derived prepared
bytes per route and channel count; do not estimate from only the new Vec lengths.

All extra meters, vectors and scratch storage are allocated during construction
or reinitialization. Pause/continue/reset only change prepared state and clear
existing buffers. The callback must remain allocation-free, lock-free, and
bounded; a paused callback still executes the live metering path and bypasses
only the I/LRA lane. Retained strong/Weak snapshots keep the existing best-effort
publication behavior: measurement state changes immediately, and the current
state/scalars publish on the next writable snapshot without losing accumulated
I/LRA history.

The second lane increases DSP and memory cost. A pre-edit baseline was captured
before any Rust production edits using the current AUD125 monitor benchmark.
The command was:

```text
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target cargo bench --locked --offline -p sotf-host --bench aud125_ms_cost -- --noplot --save-baseline aud126-pre-pause
```

It measures 480-frame/10 ms callbacks at 48 kHz, after 301 callbacks of
prefill, with 20 Criterion samples, 1 s warm-up, and 3 s measurement per case.
The existing benchmark exercises `LoudnessMonitor::add_frames` and excludes
plugin snapshot publication. Criterion console estimates and confidence
intervals (not worst callback bounds) were:

| Layout / LRA | Pre-edit estimate interval | Estimate center |
|---|---:|---:|
| Stereo / on | 32.878–33.099 µs | 32.943 µs |
| Stereo / off | 32.969–33.001 µs | 32.986 µs |
| 5.1 / on | 81.543–81.616 µs | 81.584 µs |
| 5.1 / off | 81.308–81.664 µs | 81.470 µs |
| 7.1.4 / on | 150.03–150.50 µs | 150.26 µs |
| 7.1.4 / off | 150.13–150.50 µs | 150.33 µs |

The run used optimized bench profile on an AMD Ryzen Threadripper PRO 3995WX
(64 cores/128 threads), x86_64 Linux, with frequency scaling active. Its log
is `/tmp/sotf-aud126-cpu-pre-pause.log`; Criterion's saved baseline is named
`aud126-pre-pause`. Source plus lock manifests captured before and after the
run are `/tmp/sotf-aud126-pre-edit-start-files.sha256` and
`/tmp/sotf-aud126-pre-edit-end-files.sha256`. Their aggregate SHA-256 matches
at `e328254f500ad9bd2a65a15b6e08758cd15980d710621c9b53581ca6637eccd0`.
`Cargo.lock` itself is
`6fd8186e6c6ef66ac2a18c243fd3320bfd98f07fa54b230041b72cab1e3ed088`.

After implementation, rerun the same benchmark IDs against this saved
baseline, and add candidate-only Paused cases. Report Running and Paused
callback costs separately, plus measured prepared bytes. Exact M/S,
integrated, LRA and peak controls must pass before interpreting any cost
change. These are monitor-core costs, not whole-plugin publication costs.

## Acceptance evidence

1. **Pause exclusion and continue.** Feed a calibration programme A while
   Running, then a materially louder/different programme B while Paused, then
   programme C with a deliberate level/frequency discontinuity after continue.
   Compare Integrated Loudness and LRA with an independent control that receives
   concatenated A+C only, with filter state preserved at the join. During B, I/LRA
   value/status/window counts remain unchanged while current M/S, M/S maxima,
   sample peak, true peak and correlation reflect B. Assert exact pass-through
   during all three intervals.
2. **Partition and clock invariance.** Repeat the same A/pause-B/C schedule
   with one-frame, irregular, 100 ms, and multi-second callbacks. Apply each
   state transition at the same source-frame boundary. Integrated/LRA results
   and active-time observation counts match within the established host meter
   tolerances. A partial 100 ms integrated sub-block survives pause and is
   completed after continue; no observation is created by pause or a query.
   Repeat eligibility at 48 kHz and 11,025 Hz using the active lane's actual
   four/thirty completed-sub-block geometry and document the inherited
   floor-to-100ms rounding.
3. **Reset state matrix.** Reset while Running and while Paused. In both cases,
   I/LRA histories, LRA observed/retained counts and maximum M/S latches clear;
   Running/Paused state is preserved. The next eligible M/S maxima wait for a
   complete 400 ms/3 s live window after reset. Reset is idempotent. Exercise
   the same transitions through direct methods and the `integrated_running`
   setter/getter, including while disabled. Reinitialization and either
   destructive `enabled` transition begin in Running; reconfiguring spatial
   snapshot storage preserves state. Changing integrated mode starts a fresh
   history but preserves Running/Paused state.
4. **Layout, mode and publication lifecycle.** Exercise rolling and
   whole-program integrated modes, LRA on/off, stereo, explicit 5.1 and
   explicit 7.1.4. Pause before 400 ms and before 3 s, then feed more than 3 s
   while paused; Integrated validity, LRA counts/status, and whole-program
   capacity must remain cold. Verify identical policy across both lanes,
   deserialization of old snapshots defaults to Running, and `update_from`,
   cache reconstruction and retained strong/Weak readers preserve the state
   without resuming stale samples. A retained snapshot may show its pre-change
   state, then must publish the new state on the first writable update. Query
   and drain while Paused must not
   advance I/LRA or invent an observation. A structural spatial-cache rebuild
   preserves the current state; changing integrated mode starts a fresh epoch
   but preserves the current Running/Paused state. Reinitialization explicitly
   enters Running.
5. **Realtime and cost.** Count allocations while processing Running, Paused,
   and across pause/continue/reset transitions; require zero in every callback.
   Run strict host Clippy and focused host tests, then the prescribed offline
   workspace nextest gate excluding MIDI and IAMF. Capture source/lock manifests
   around the gate and serialize Cargo use with the Upmixer track. Matched CPU
   results report cost only; they do not replace correctness checks.

## Open acceptance boundary

The explicit live-M/S-during-pause policy is recommended here because §2.2
couples only Integrated Loudness and LRA, while the live M/S meter remains
useful feedback. True peak is also explicitly kept live and is not reset by
pause. If independent review interprets pause as freezing M/S or true peak,
resolve that before code changes; do not silently change this contract in
implementation. The sibling Start/Pause/Continue/Reset display and control
integration remains required after core/API acceptance. AUD127's first-60-
second LRA instability indication is a separate issue and remains open.
