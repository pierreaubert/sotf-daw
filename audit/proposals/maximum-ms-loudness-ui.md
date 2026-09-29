# AUD125: Maximum Momentary and Short-term Loudness UI

**Status:** sibling TUI/GPUI implementation and focused gates accepted by Astra
on 2026-09-28. The AUD125 host fields and this display stage are complete. The
user’s existing authorization for the sibling TUI/GPUI metering display applies
to this related display stage; no external publication is in scope. MIDI and
IAMF remain excluded.

## Current application surfaces

- The TUI loudness box is `crates/app-tui/ui/draw_meters/draw.rs::draw_lufs_box`.
  It already renders the current Momentary (`M`), Short-term (`S`), and
  Integrated (`I`) bars and the AUD124 Max TP summary. The meter is reachable
  in the existing TUI level-meter screen.
- The mounted GPUI surface is the Loudness Monitor custom plugin on
  `Screen::Studio`, rendered through
  `components/plugins/ui_loudness.rs` and
  `components/plugins/level_meters/render.rs::render_lufs_with_true_peak`.
  The actual Studio route is covered by the AUD124 mounted-plugin E2E. The old
  queue-side meter panel is not mounted in the current flat Queue screen and is
  outside this proposal.
- `LoudnessData` now carries finite-only
  `maximum_momentary_lufs: Option<f64>` and
  `maximum_shortterm_lufs: Option<f64>`. The host updates these over its fixed,
  epoch-aligned 100 ms observation grid, makes them eligible after the 400 ms
  and 3 s windows, copies them through snapshots, and clears them on Integrated
  measurement reset. These are maxima over that observation grid, not
  continuous-time maxima. The UI must read the prepared snapshot fields; it
  must not recompute maxima from the current M/S readings.

## Proposed display behavior

Add two distinct numeric summary entries near the existing current M/S bars:

| Metric | English label | Value format |
|---|---|---|
| Maximum Momentary | `Max momentary` | `−18.0` |
| Maximum Short-term | `Max short-term` | `−17.2` |

The LUFS section heading supplies the unit. GPUI uses full localized labels,
so the display cannot be mistaken for Mid/Side channel measurements. TUI uses
compact M/S labels in the narrower localized layouts and retains the LUFS
heading. Copy for the full GPUI labels:

| Locale | Maximum Momentary | Maximum Short-term |
|---|---|---|
| English | `Max momentary` | `Max short-term` |
| French | `Max. momentané` | `Max. court terme` |
| German | `Max. Momentan` | `Max. Kurzzeit` |
| Spanish | `Máx. momentáneo` | `Máx. corto plazo` |
| Pseudo | generated from the English source strings | generated from the English source strings |

The TUI keeps the full English labels (they fit the default 22-column
interior) and uses compact localized labels at that width: `M max` / `S max`
in French, `Max M` / `Max S` in German, and `Máx M` / `Máx S` in Spanish.
Values use one decimal place,
matching the current M/S bars and the existing Max TP summary precision. A finite maximum is authoritative even when
the current `momentary_valid` or `shortterm_valid` flag is false: active cache
rebuilds and reset/publication transitions can make current readings invalid
while preserving an already observed finite maximum. Do not gate either maximum
on the current-reading validity flags. The entry remains present when the
`Option` is `None` and displays `— LUFS`. For these two maxima, `None` means that no
finite window has completed in the current epoch, including cold startup,
silence, reset, and rejected/error-only input; there is no separate
unsupported-rate flag. Do not show the true-peak `Unavailable` text for this
state. If no loudness snapshot exists, keep the current no-meter-data view.
Sanitize a non-finite `Some` defensively to the same placeholder, but do not
change core serialization semantics.

In TUI, render the two values as separate summary lines beside the LUFS section
and stack label/value when a combined row would exceed the inner width. Preserve
both entries at the 24-column German regression width without clipping or
overwriting current bars. Added rows must not displace the current M/S bars as
the box becomes shorter: suppress the scale first, then per-channel True Peak
bars, while preserving the Max TP summary, both M/S maxima and current M/S/I
bars. In GPUI, add a compact summary group adjacent to the M/S bars; each value
must remain independently visible at the mounted plugin width and at the
compact component width. Retain the existing current M/S bars, true-peak bars,
and TP summary behavior.

The TUI redraw signature must include both values at the same 0.1 LUFS
precision as their rendered text, distinguish `None` from a finite value, and
continue to include current meter readings. GPUI is reactive to the published
snapshot; its live Studio test must prove a maximum-only update changes the
tracked rendered values even when the current M/S readings stay constant.

## Routed queries

Expose the snapshot values as nullable dev-API queries:

- `meters.maximum_momentary_lufs`
- `meters.maximum_shortterm_lufs`

The query catalog and `read_path` return the scalar fields directly, so JSON
`null` represents `None`. A live route test should query both values before and
after a higher interval, then after a lower interval while the programme
maxima remain latched; an Integrated reset must make both queries return null.
Do not derive results from channel bars or a separate query-time reduction.

## Acceptance evidence

1. A TUI TestBackend fixture renders finite values for both maxima while the
   current M/S readings are lower, including finite latches with
   `momentary_valid` and `shortterm_valid` false; then render reset/empty
   placeholders. At 24 columns in German, assert all labels and values remain
   inside the box without overlap with current bars. Add short-height fixtures
   (24×12 and 24×10) to prove height pressure suppresses optional scale/TP
   channel bars before it displaces maximum values or current M/S/I bars.
2. Extend the TUI redraw-signature test: a visible 0.1 LUFS maximum change and
   `Some` to `None` each change the signature; changes below the displayed
   precision do not; a lower current M/S interval does not replace the maxima.
3. Extend the GPUI helper/translation completeness test for both labels across
   every supported language, including pseudo text generated by
   `scripts/generate_pseudo_locale.py --check`.
4. Render a compact GPUI fixture and exercise the mounted Studio Loudness
   Monitor. Assert the painted localized labels and values, finite maxima while
   current M/S validity flags are false, high-to-lower interval retention, a
   maximum-only update, and reset-to-placeholder. Verify the summary stays
   inside the panel and does not overlap the bars at compact width; recheck
   layout after the reset state.
5. Exercise the two real dev-API query paths with finite values, higher and
   lower interval snapshots, and reset to `null`.
6. Run focused TUI/GPUI tests, `cargo check --all-targets -p sotf-gpui
   --features dev-api`, rustfmt, design-token, `git diff --check`, and the
   pseudo-locale generator check with the reviewed temporary offline resolver
   lock. Restore and verify the sibling's original lock afterward, recording
   the tested/restored hashes separately. No DSP benchmark or host callback
   gate is needed for this display-only stage.

This is an application display requirement from EBU Tech 3341 §2.1, but passing
these UI tests is not a claim of full EBU Mode compliance or certification.
AUD125 remains open until Astra independently accepts the application display
and routed-query evidence.
