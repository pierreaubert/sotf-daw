# AUD112/AUD113: EQ and Crossfeed AutoGain measurement clocks

Public reproduction on 2026-09-28, after AUD110's local gain precision correction.
No production or permanent test changes for these clock issues yet.

## Public evidence

`/tmp/sotf-autogain-caller-clock-probe.rs` constructs both plugins through the
public bridge factory. It uses fixed EQ coefficients or full-wet Bauer
Crossfeed, 100 ms AutoGain smoothing, and identical stereo samples. Every call
must accept its complete declared frame count and emit finite audio.

Six seconds plus 17 frames are rendered using 137-frame callbacks, then 512
and 8,192-frame callbacks. Disabled AutoGain produces bit-identical audio for
all partitions in both plugins and rates. Enabled processing differs:

| Plugin | Rate | Maximum difference, 137 vs 512 | Maximum difference, 137 vs 8192 |
|---|---:|---:|---:|
| EQ | 48000 | 0.009181127 | 0.173689840 |
| EQ | 96000 | 0.001148585 | 0.174044330 |
| Crossfeed | 48000 | 0.003898196 | 0.048302963 |
| Crossfeed | 96000 | 0.002464555 | 0.034165464 |

A separate causality probe warms two identical instances for 109 callbacks of
4,096 frames. Their next 8,192-frame input blocks share the first 4,096 frames
exactly. Only the later half changes its channel balance. Comparing just the
common input prefix gives:

| Plugin | Rate | First different interleaved sample | Maximum earlier-output difference |
|---|---:|---:|---:|
| EQ | 48000 | 20 | 0.00014978088 |
| EQ | 96000 | 22 | 0.000035142526 |
| Crossfeed | 48000 | 4 | 0.00055161305 |
| Crossfeed | 96000 | 14 | 0.000015482306 |

Disabled prefix comparisons remain exact. No parameter automation or filter
change occurs during either comparison. This distinguishes AutoGain's target
measurement timing from the raw effects' processing and the shared gain
recurrence. The probe records existing defects; it is not a passing acceptance
test for a corrected measurement clock.

## Source explanation and proposed boundary

EQ measures only every tenth callback, skipping the intervening audio entirely.
It measures both sides of the selected callback before compensating that same
callback. Ordinary and compiled biquad routes share this policy.

Crossfeed ingests each complete input/output callback, refreshes its target, then
applies the target to the same callback. Its fixed-mode, fixed-mix raw DSP passes
the controls above. Its independent block-based mix ramp is outside this probe.

The proposed correction is continuous ingestion and a fixed sample clock for
derived measurements, with a refreshed target affecting only subsequent audio.
Before implementation, the plan must address EQ's native/compiled/oversampled
routes, reference delay alignment, finite drain and reset. Neither existing
callback policy should be described as fixed by AUD105 or AUD110. The precise
clock and implementation are still under review.

## Artifacts

- Complete measurements: `/tmp/sotf-autogain-caller-clock-probe.log`.
- Successful build: `/tmp/sotf-autogain-caller-clock-build.log`.
- Source, binary, dependency and production-source SHA256 values:
  `/tmp/sotf-autogain-caller-clock-probe-build.json`.
- Binary: `target/audit-tmp/sotf-autogain-caller-clock-probe`.

The first compile rejected mismatched old bridge/new host artifacts. The
successful compile used the matched AUD110 bridge and host from its recorded
dependency manifest; no source or dependency version was changed to obtain it.
