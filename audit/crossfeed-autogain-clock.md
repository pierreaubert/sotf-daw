# AUD113: Crossfeed causal AutoGain measurement clock

## Result and scope

Crossfeed now measures every accepted frame while AutoGain is enabled and
publishes statistics after each complete `max(sample_rate / 10, 1)`-frame
interval. The newly published target first affects the following frame. At
44.1, 48, 96 and 192 kHz this is exactly 10 Hz.

The constructor prepares a stereo reference buffer at the existing
`max_block_frames` capacity. Processing copies sanitized original input into
that buffer, runs the complete existing raw callback, then walks measurement
interval boundaries through the reference and uncompensated output. It applies
the previously published target before refreshing the completed interval.
No new public parameter, callback limit, host API or DSP algorithm is added.

AutoGain-disabled processing retains the existing policy of freezing its
meters; the new interval phase freezes with them. Reset, successful
initialization and the existing whole-plugin Off/disabled reset route restart
the phase. Empty callbacks do not advance or publish AutoGain measurements.
Scratch is overwritten before use, so resetting it requires no clear or new
allocation. Added prepared storage is `8 * max_block_frames` bytes: 128 KiB at
the existing default capacity of 16,384 frames.

The raw deinterleave, mode processing, yaw/ITD, mix ramp and interleave region is
unchanged from the preserved pre-AUD113 source. Dynamic mix automation retains
its separate callback-dependent ramp. This change makes no new finite-tail or
arbitrary dynamic-filter partition-invariance claim.

## Meaningful red evidence

`target/audit-crossfeed-clock-red.log` captured the new public tests against the
unchanged implementation before production edits:

| Independent check | Original maximum sample difference |
| --- | ---: |
| Same fixed-mode source, 137 versus 8,192-frame callbacks | 0.048302963 |
| Identical callback prefix with a different later suffix | 0.00055161305 |
| Explicit completed-interval gain reference | 0.008204285 |

The disabled-AutoGain control and dynamic-mix negative control passed before
the correction. The three positive regressions then passed with exact waveform
equality after the correction; no numerical tolerance was relaxed.

## Independent timing and lifecycle evidence

`tests/autogain_clock.rs` contains six tests:

- Public partition and future-suffix causality checks at 48/96 kHz.
- A separate AutoGain instance driven by an independently rendered, uncompensated
  effect. The reference applies scalar gain once per frame and refreshes only
  after a complete interval. It covers Bauer/44.1 kHz, Meier/48 kHz,
  Multiband/96 kHz and HRTF/192 kHz, two correction directions and callback
  patterns `[1]`, `[512]`, and `[17, 137, 8193]` over six-second signals. The
  helper's numerical law is separately verified by AUD110; this test verifies
  the caller clock and measurement alignment.
- A one-frame reference with timestamped AutoGain disable/enable, target,
  smoothing, Off and active-mode changes. The reference counts only active
  frames and independently resets at Off. Irregular callback output matches
  it exactly.
- All four active modes across reset, sample-rate reinitialization, empty
  callbacks, invalid dimensions/rate/capacity and failed initialization. Rejected
  callbacks leave subsequent valid output identical to an untouched twin.
- An explicit dynamic-mix negative control preserving the old ramp behavior.

`tests/autogain_clock_heap.rs` exercises the first callback on fresh threads,
actual warm meter publication, controls, reset and empty processing across
four rates and four modes. Both allocations and frees are **zero**. Parameter
IDs and final ownership are retained outside the counted region; there is no
warmup that could hide lazy allocation.

The [independent source/oracle review](crossfeed-autogain-clock-independent-review.md)
found no introduced blocker.

## Matched before/current audio and CPU evidence

The isolated harness compiles preserved and current Crossfeed implementation
modules into one optimized executable against identical current dependencies,
including the AUD110 AutoGain helper. Live source is never swapped. Input is a
six-second stepped two-tone stereo signal. Setup is outside the timed region;
each case uses seven repeats with alternating implementation order.

Artifacts:

- `target/audit-tmp/crossfeed-aud113/manifest.json`: source hashes, dependency
  paths and compiler command.
- `target/audit-tmp/crossfeed-aud113/matched-cpu.csv`: all 48 timed cases.
- `/tmp/sotf-crossfeed-aud113-before.rs`: immutable pre-change source.
- `/tmp/sotf-crossfeed-aud113-cpu.rs` and `-cpu-build.py`: harness and build script.

The matrix covers four active modes, 48/96 kHz, callbacks 137/512/8193, and
AutoGain off/on. All **24 disabled** outputs are bit exact to the old source.
An additional **24 changing-mix** disabled renders are bit exact. Enabled output
intentionally changes as measurement targets stop depending on callback size;
the largest observed before/current sample difference is 0.052892789.

| AutoGain | Median current/previous time | Observed case range |
| --- | ---: | ---: |
| Disabled | 0.9957 | 0.9833–1.0738 |
| Enabled | 0.9912 | 0.9628–1.1829 |

The slowest relative enabled case was 48 kHz/Multiband/512:
74.10 → 87.65 ms for six seconds of audio. These are local elapsed-time
measurements, including differing intended gain trajectories and normal machine
noise; they establish neither a universal speedup nor a realtime deadline.

## Validation and changed files

- `cargo test -p sotf-plugin-crossfeed`: **92 passed**, no failures or skips
  (55 unit tests plus 37 integration tests).
- `cargo clippy -p sotf-plugin-crossfeed --all-targets -- -D warnings`: passed.
- Scoped rustfmt and `git diff --check`: passed.
- Logs: `target/audit-crossfeed-clock-{red,green,focused,full,clippy}.log` and
  `target/audit-crossfeed-clock-cpu-build.log`.

Production changes are confined to `src/lib/crossfeed_plugin.rs`; new regression
files are `tests/autogain_clock.rs` and `tests/autogain_clock_heap.rs`. README and
CHANGELOG document the measurement contract. The existing scalar getter and
`tests/auto_gain_smoothing.rs` belong to earlier audit fixes and were preserved.

AUD112 EQ remains a separate pending implementation. Shared AutoGain arithmetic,
host scheduling, engine transition protocols, MIDI and IAMF are unchanged by
AUD113.
