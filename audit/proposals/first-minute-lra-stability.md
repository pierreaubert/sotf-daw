# AUD127: First-minute LRA stability indication

**Status:** AUD127 host/API and reachable TUI/Studio implementation accepted by
Astra on 2026-09-29. Evidence is recorded in
`audit/first-minute-lra-stability.md`. The broader audit remains open; MIDI and
IAMF remain excluded.

## Requirement and current behavior

EBU Tech 3341 (2023), section 2.4, requires a meter to indicate that a displayed
LRA value is not yet stable during the first 60 seconds of its LRA measurement
(for example after reset). The implementation may choose how to show that
indication. [Official EBU Tech 3341 PDF](https://tech.ebu.ch/docs/tech/tech3341.pdf).

The host currently exposes `LoudnessRangeStatus::Valid` as soon as its
percentile calculation produces a finite value. With one retained LRA
observation, that result is a valid numeric zero, so a constant signal can be
marked `Valid` after the initial three-second Short-term window. There is no
separate stability field. The TUI and GPUI source currently has no consumer of
`LoudnessData::loudness_range`, so neither surface displays the range or the
required instability indication.

The accepted AUD126 API already defines the relevant clock: Integrated and LRA
measurement run together, pause together, and reset together. Live M/S/TP keeps
running during an I/LRA pause. The host tracks `integrated_frames_seen`, which
counts accepted active samples and stops while I/LRA is Paused. The LRA
observation grid starts only after 30 completed 100 ms sub-blocks and is
approximate at sample rates not divisible by ten. Therefore neither wall time
nor `observed_windows` alone represents the first 60 seconds exactly.

## Proposed data semantics

1. Add `is_stable: bool` to public `LoudnessRangeData`, with `#[serde(default)]`
   so old serialized snapshots deserialize as unstable. Keep
   `LoudnessRangeStatus` unchanged: `Valid` continues to mean the numeric range
   is available; `is_stable` independently records whether 60 seconds of
   active I/LRA samples have elapsed since the current measurement reset.
   The displayed numeric unit is **LU**, not LUFS: LRA is a range of loudness,
   not an absolute loudness level.
2. Set stability from `integrated_frames_seen >= 60 * sample_rate` using checked
   or widened integer arithmetic. Count all active audio from reset, including
   the first three seconds before the first LRA observation. Paused samples do
   not advance it. This uses exact accepted frame time even where the inherited
   LRA observation grid is approximate.
   Only successfully accepted non-empty audio advances this counter. Rejected
   `add_frames` calls, empty process calls, and true-peak EOS draining must not
   advance the 60-second clock or create extra LRA observations. A query may
   publish a boundary reached by previously accepted samples, but it does not
   add elapsed measurement time.
3. Recompute the boolean on every data query even when no LRA history is dirty;
   the 60-second boundary can fall between percentile observations. Reset,
   explicit Start, reinitialization, and destructive enabled epochs clear it.
   Continue resumes the existing active-time count. Retained snapshots may
   continue to show the prior value until the next successful publication.
   Integrated-mode changes construct a new measurement epoch and clear the
   flag. Spatial snapshot rebuilds preserve the monitor's measurement epoch,
   so they must preserve or immediately re-derive the flag from its active
   sample count rather than resetting it. This follows the existing AUD126
   lifecycle: mode changes reset measurement history, while spatial storage
   changes do not.
4. Stability is elapsed measurement time, not an assertion that a value is
   numerically valid. The UI may render a numeric range and its stable/unstable
   indication only when status is `Valid` **and** `range_lu` is finite and
   nonnegative. Apply that guard even to malformed or nonfinite snapshots.
   For BelowGate, WarmingUp, CapacityExceeded, MeasurementError, missing, or
   malformed values, preserve the existing unavailable presentation and show
   no stability claim.

## User-visible behavior

Add one-decimal LRA display in LU to the reachable TUI LevelMeters and GPUI
Studio loudness surfaces. When a finite valid LRA is displayed before the 60-second
active-time boundary, show a translated “not stable” marker beside it. At and
after the boundary, keep the number and remove the warning. For absent or
non-valid range data, show the existing unavailable placeholder and no
stability claim. The status marker describes LRA readiness only; it does not
alter or delay the numeric percentile result.

The row must fit within the actual panel height and preserve its border. In a
constrained TUI layout, keep current M/S/I readings and their programme maxima
ahead of the new LRA row; draw the LRA value and marker only as a complete row
when space remains. Do not let the new row overwrite a border or displace
existing momentary, short-term, integrated, or maximum values. Cover both the
ordinary EBU meter route and the explicit per-channel aggregation route used
for layouts with more than six channels.

The indicator follows the same I/LRA lifecycle controls already implemented
for AUD126. Pause freezes its state; Continue resumes the active-time clock;
Reset while either Running or Paused clears range data and returns the marker to
the early-measurement state. No audio transport or live M/S/TP state changes.

## Acceptance evidence

- At exact 48 kHz and at a non-divisible sample rate such as 11,025 Hz, a
  constant non-silent signal has a valid numeric LRA after the first complete
  Short-term observation but remains unstable at 59.999 seconds of active
  samples and becomes stable at 60.000 seconds. The non-divisible case must keep
  `timebase_is_exact == false` while still crossing stability at the exact
  sample count.
- Assert LRA display units are LU. Valid finite nonnegative values may carry
  the unstable marker before 60 seconds and lose it at the boundary. Invalid
  status, NaN/infinity, negative values, and malformed snapshots must show no
  numeric value and no stable/unstable claim.
- Reaching the threshold must not depend on callback partition or query
  frequency. Cover bulk, irregular and repeated-query processing, and assert
  the exact one-frame-before / exact-boundary transition.
- Rejected malformed callbacks, accepted empty process calls, and repeated
  true-peak EOS drains leave the active frame clock unchanged and cannot create
  an LRA observation. An empty query may only publish the state implied by
  already accepted active samples.
- Pause after 30 seconds, feed additional audio while paused, and verify the
  stability state remains unchanged; after 30 seconds of resumed active input,
  it becomes stable. Reset while Paused clears the state and preserves Paused;
  Continue then starts a fresh 60-second active measurement.
- Verify reset, Start, reinitialize, enabled transitions, and retained
  strong/Weak snapshot publication. A retained old snapshot may remain visible
  but must not be mutated; the next successful generation must publish the
  current stability state. An integrated-mode rebuild clears the flag with
  its new epoch; a spatial cache rebuild preserves the existing active count
  and published/next-query stability state.
- Cover silent/BelowGate, cold/WarmingUp, query-error, optional-LRA-disabled,
  legacy serde default, new serde roundtrip, `LoudnessData::update_from`, and
  snapshot/cache constructors. Unavailable readings must not gain a visible
  stable claim.
- TUI and Studio rendering tests cover numeric unstable and stable states,
  unavailable and malformed states, translations, narrow/short layout, and
  redraw at the one-frame threshold. Include ordinary and >6-channel meter
  aggregation routes. The tests inspect visible text/state, not just a helper.
- Guard representative host process, pause/continue/reset, and published
  snapshot paths for zero allocations. The implementation adds only a bounded
  frame comparison and boolean field; report total memory/API impact without a
  worst-case CPU claim.
- Run focused host tests, strict host Clippy, TUI/GPUI tests and compile checks,
  localization generation checks, `git diff --check`, and the required offline
  workspace gate with MIDI/IAMF excluded. Record tested and restored sibling
  lock hashes separately.

The change adds an LRA display because neither current application surface
consumes the already-published range; displaying only an instability warning
without its associated LRA value would not make the EBU requirement meaningful.
No EBU Mode or certification claim follows from this batch.
