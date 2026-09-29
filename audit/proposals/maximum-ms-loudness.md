# AUD125: Maximum Momentary and Short-term Loudness

**Status:** design proposal; no production edits in this batch. The local issue
ledger is `AUDIT.md`. MIDI and IAMF remain excluded.

## Requirement and source comparison

The current `LoudnessData` exposes the current Momentary and Short-term
readings, but has no latched maxima. The monitor already divides input into
100 ms sub-blocks. It queries Momentary at those boundaries for integrated
history and, when Loudness Range is enabled, queries Short-term there for LRA
observations. The plugin publishes only once after each host callback, and
callbacks can cross multiple 100 ms boundaries. Therefore computing a maximum
only in `update_loudness_data` would miss an earlier louder window in a large
callback. The backend's current M/S readings change on that same sub-block
grid, so maxima should reduce that stable observation series rather than
depend on host callback boundaries.

[EBU Tech 3341 v4.0 (2023), §2.1](https://tech.ebu.ch/files/live/sites/tech/files/shared/tech/tech3341v4_0.pdf)
requires the meter to be able to display the maximum Momentary and maximum
Short-term values, and says both maxima reset when Integrated Loudness is
reset. Section 2.2 defines ungated 400 ms Momentary and 3 s Short-term windows;
Short-term updates for a live meter must occur at least 10 Hz. Individual
channel M/S measurements are explicitly outside the EBU Mode specification
(§2.10), so this proposal adds programme-summed maxima only. For explicit
wide-layout coverage, the independent reference follows the channel weights
and LFE exclusion in [ITU-R BS.1770-5 Annex 1](https://www.itu.int/dms_pubrec/itu-r/rec/bs/R-REC-BS.1770-5-202311-I%21%21PDF-E.pdf).
This is a feature comparison, not a claim that the host or application
implements EBU Mode.

## Proposed API and behavior

Add these finite-only fields to `LoudnessData`:

```rust
pub maximum_momentary_lufs: Option<f64>,
pub maximum_shortterm_lufs: Option<f64>,
```

Use `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward
compatibility. `Some` always contains a finite LUFS reading. `None` means no
finite completed window has been observed in the current Integrated
measurement epoch; cold and silent windows therefore remain `None` instead of
serializing non-finite values. Current M/S fields and their validity flags
retain their existing meanings.

Maintain the two maxima inside `LoudnessMonitor`, independent of snapshot
publication. They are maxima over the monitor's fixed, epoch-aligned 100 ms
sub-block observation grid, not continuous-time maxima. This is the grid on
which the current M/S readings themselves change; host callbacks and snapshot
queries may occur more often or span several grid points.

- At each 100 ms sub-block boundary, update the Momentary maximum from a finite
  400 ms window reading once at least `ceil(0.4 * sample_rate)` source frames
  have elapsed in this epoch.
- At each 100 ms sub-block boundary, update the Short-term maximum from a finite
  3 s window reading once at least `3 * sample_rate` source frames have elapsed
  in this epoch. Derive elapsed frames at that boundary from the completed
  sub-block count and the actual integer `sub_block_frames`; do not use an
  unqualified `completed_sub_blocks >= 4/30` eligibility check.
- Reuse the existing Momentary/Short-term queries where already performed for
  integrated history or LRA. When LRA is disabled, query Short-term at the same
  100 ms cadence so callback size and snapshot-query timing cannot hide a peak.
- Treat the latches as observations of the EBU meter's current-window readings,
  not a second continuous sample-level loudness algorithm. At sample rates not
  divisible by ten, the existing backend uses `floor(sample_rate / 10)` frames
  per sub-block and four/thirty such blocks for its M/S rings. For example, at
  11,025 Hz those windows are 4,408 and 33,060 frames, respectively, versus
  4,410 and 33,075 frames for exact 400 ms/3 s windows. The eligibility guard
  must not latch before the exact elapsed-sample threshold; the inherited
  window-width rounding remains a documented backend limitation of this
  batch. Do not claim sample-rate-independent exact window geometry.
- Publish these scalar latches in `update_loudness_data`; a skipped cache
  publication or a retained snapshot must not stop accumulation.
- Clear both maxima whenever Integrated measurement resets. The existing
  `LoudnessMonitor::reset`, analyzer reset, destructive enabled transition and
  reinitialization already start a fresh measurement epoch. Do not introduce a
  separate M/S reset or any pause/continue promise; coupled stand-by behavior is
  AUD126.
- Query errors and non-finite results do not update either maximum. Preserve
  the existing current-query error/validity behavior; an additional Short-term
  query made while LRA is disabled must not turn an error into a finite value.
- `finish_true_peak` advances only the interpolation detector. It must not
  advance the M/S observation grid or invent a louder M/S window.
- If `set_spatial_enabled` rebuilds the publication cache inside an active
  measurement epoch, copy the current maxima into the new prepared snapshots,
  as is already done for the true-peak maximum. Do not clear a latched result
  merely because spatial display storage changed.
- Do not allocate or add per-channel state in the callback. These are two
  scalars. Preserve all existing current-value readings and integrated/LRA
  behavior.

`maximum_momentary_lufs` and `maximum_shortterm_lufs` use full names so that
the `M` and `S` fields cannot be confused with mid/side channel notation.

## Acceptance evidence

1. A high-then-low stereo calibration programme: 1 kHz, 48 kHz, 4 s at
   -18 dBFS peak followed by 3 s at -30 dBFS peak. The end-of-programme current
   Momentary/Short-term readings must be below the earlier maxima. Both maxima
   should be within 0.2 LU of the -18.0 LUFS calibration value specified by
   Tech 3341 §2.9. This checks that the output is a historical maximum, not the
   final current reading.
2. Compare maxima against an independent reference pass that reads current M/S
   on the same 100 ms grid and reduces only finite valid windows. Use the same
   audio with 1-frame, irregular, 100 ms, and multi-second callback partitions.
   Maxima and final current readings must be partition-invariant within 0.01 LU.
   The reference reduction must not consume the new maximum fields.
3. Check full-window eligibility with actual integer frame counts at 48,000 Hz
   and 11,025 Hz. At 48 kHz, no M maximum is allowed before 19,200 frames and
   no S maximum before 144,000 frames. At 11,025 Hz, do not latch at 4,408 or
   33,060 frames; the first eligible observation is the next fixed-grid
   boundary after 4,410 or 33,075 frames. Record the existing 4/30-block
   backend window-width rounding described above. Test silence/cold windows and
   later finite audio so `None` transitions to a finite value without a floor.
4. Repeat the reference programme with LRA enabled and disabled; M/S maxima
   must match within 0.01 LU. Add a public explicit 5.1-layout fixture that
   checks the independent BS.1770 energy reference: equal 997 Hz, -18 dBFS-peak
   tones in L/R give -18.0 LUFS; equal tones in L/R plus both surrounds add
   `10*log10((2 + 2*1.41)/2)` LU; an LFE-only tone produces no finite maximum.
   This verifies the published 1.41 surround weights, LFE exclusion, and that a
   full-range signal does create a maximum. Also cover an explicit 7.1.4
   layout, which takes the separate per-channel mono-meter aggregation path:
   compare its M/S maxima against independent mono channel readings combined
   with the published role weights and LFE exclusion. Include that wide layout
   in LRA-on/off parity and the disabled-LRA CPU baseline.
5. Reset the public analyzer after a high epoch, hold old strong/Weak snapshots,
   and verify old snapshots remain immutable while the cleared/new generation
   has no old maximum. Feed a lower signal and verify new maxima contain only
   the new epoch. Exercise rejected malformed input and query-without-input so
   they do not create maxima. A true-peak-only finish must also leave M/S
   maxima unchanged. Include the `set_spatial_enabled` cache rebuild while
   active.
6. Test constructor, `Default`, `update_from`, prepared-cache initialization,
   `reset_loudness_data`, disable/enable, serialization and deserialization.
   Old finite payloads missing both new fields must deserialize to `None`;
   finite maxima round-trip, and no new field emits `NaN` or infinity.
7. Run fresh-thread allocation checks over input, sub-block boundaries, query,
   reset and retained-reader publication. Report callback CPU for default LRA
   enabled and LRA disabled, because the latter gains a Short-term query at the
   required 10 Hz cadence. Compare before/after on matched 48 kHz stereo and
   explicit 5.1 layouts for both LRA policies; capture the baseline before
   source edits.
8. Run focused host tests and strict host Clippy, then the documented offline
   workspace nextest gate with MIDI and IAMF excluded and FFI included. Record
   a source snapshot manifest around the workspace gate if the Upmixer worker
   has concurrent edits.

## Staging and remaining work

This batch is the public host data/API and measurement-epoch behavior. EBU
Tech 3341 also requires displaying both maxima; application TUI/GPUI display
remains a separate required UI stage, coordinated with the pending AUD124 UI
integration boundary. Keep AUD125 open until the user-facing display and its
localized narrow-layout tests pass. AUD126 pause/continue, AUD127 LRA stability,
and AUD128 authentic corpus coverage remain separate. No external issue
publication or sibling-repository source edit is authorized by this proposal.
