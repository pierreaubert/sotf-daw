# AUD-124 — Programme Maximum True Peak Display

Status: concrete sibling UI proposal and draft patch; not applied. The AUD-124
core/API stage is accepted, while this display stage and the complete EBU
feature remain open. The DAW workspace is writable here; the sibling `sotf`
checkout is outside the writable roots, so the review artifact is
`programme-maximum-true-peak-ui.patch`. Its hunks pass `git apply --check`
against the current sibling snapshot, and the listed sibling source paths
remain unchanged.

## Scope and data contract

Display the accepted `LoudnessData::maximum_true_peak_dbtp` snapshot field in
both the TUI and GPUI loudness meters. This is the programme-epoch maximum and
must be read directly from the snapshot. Keep `true_peaks_dbtp` as the current
query-interval values used by the existing per-channel bars. Do not derive the
programme maximum from the latest channel vector, and do not change the bar
values or their interval semantics.

Use these UI states:

| Snapshot state | Max TP display |
|---|---|
| No loudness snapshot | Keep the existing no-meter-data state |
| Finite `Some(value)` | Always format the scalar in dBTP, for example `−1.2 dBTP`; the finite programme result takes precedence over status flags |
| Non-finite `Some` from malformed external data | Treat as unavailable defensively |
| `None` with `true_peak_is_compliant == true` | An em dash with dBTP context; `None` covers cold, silent, and reset epochs, which the public field intentionally does not distinguish |
| `None` with `true_peak_is_compliant == false` | Localized unavailable marker (`N/A`); the flag describes host rate support, not certification |

Do not gate the programme value on `true_peak_valid`: that flag describes the
current query interval, while the maximum can remain valid from an earlier
interval. Do not gate a finite value on `true_peak_is_compliant` either: a
control-time snapshot rebuild can retain the latched scalar before the next
query refreshes its status flags. Do not show negative infinity for the
empty/silent state. The existing `measurement_enabled` flag may be used for
accessibility copy, but it must not clear or override a finite latched maximum.

## Display changes

- **TUI:** In `crates/app-tui/ui/draw_meters/draw.rs`, use the existing
  one-line heading slot for `Max TP: <value> dBTP`, replacing its redundant
  current-interval aggregate. Keep channel bars bound to
  `true_peaks_dbtp`; they remain the per-channel interval display. Format the
  value with one decimal to match the 0.1 dB redraw quantization, and reuse the
  existing stack buffer. When the localized unavailable text plus heading is
  wider than the inner terminal width, render the heading and unavailable
  status on separate rows instead of clipping the status.
- **TUI redraw:** In `crates/app-tui/main/misc.rs`, add the programme scalar and
  support state to `loudness_redraw_signature`. Also hash `true_peaks_dbtp`,
  because it drives the visible interval bars. A rising maximum, a reset to
  `None`, a support-state change, or new interval bars must trigger a redraw.
  The current signature quantizes meter levels to 0.1 dB, so Max TP must use
  one decimal unless signature precision is changed in the same patch.
- **GPUI:** In
  `crates/app-gpui/components/plugins/level_meters/render.rs`, add a distinct
  `Max TP` summary line below the existing `True Peak`/sample-`Peak` heading
  and above the bars. Stack the localized label and value vertically so a
  narrow panel or long translation cannot clip one against the other. The
  existing bars and peak-spread view continue to use the interval vector.
- **Translations:** Add `Max TP` and unavailable-state copy to all shipped TUI
  languages in `crates/app-tui/i18n.rs` and all GPUI languages in
  `crates/app-gpui/app/i18n/translations.rs`. Update the GPUI translation
  completeness test, then regenerate and check the generated pseudo locale
  with `scripts/generate_pseudo_locale.py`; do not hand-edit its generated
  output.

## Snapshot propagation and manual endpoints

The app meter consumers hold `LoudnessData` directly (TUI clones it from the
cached plugin snapshot; GPUI downcasts the same re-exported type). There is no
separate UI data model to extend. Still search all `LoudnessData` struct
literals in the sibling checkout and add the new field where needed, especially
the GPUI meter fixture in `crates/app-gpui/app/dev_api/server/dispatch.rs`.
Extend `crates/app-gpui/app/dev_api/queries.rs` with a direct query for
`meters.maximum_true_peak_dbtp` and register it in
`crates/app-gpui/app/dev_api/server/parse.rs`'s query capability allowlist.
Verify it reads the latest snapshot rather than recomputing a value from the
interval vector. Preserve the host field's serde default behavior and test old
finite serialized payloads through the existing host tests; do not add a
second serializer or custom conversion.

## Acceptance tests

1. TUI render fixture at 24 columns: assert the compact heading retains the
   programme scalar while the channel bars show lower interval values.
2. TUI redraw signature: assert it changes for a maximum rise, maximum reset,
   support-state change, and interval-vector change; include a maximum change
   across a visible 0.1 dB rounding boundary. Assert unrelated stable snapshots
   produce the same signature.
3. GPUI rendered-screen fixture: use the same high-then-lower snapshots and
   assert the Max TP summary stays latched while interval bars update. Test
   finite maximum with false support/interval-valid flags, supported `None`,
   unsupported, and non-finite defensive formatting. **Pending:** the current
   patch has value-formatting helper tests, not a rendered-screen test.
4. GPUI component-test completeness: require the new translation fields in
   English, French, German, Spanish, and the generated pseudo locale.
5. GPUI routed dev API test: set a finite programme maximum in the manual meter
   fixture, query it through the registered endpoint, then set a lower interval
   peak without lowering the scalar and verify the endpoint still returns the
   programme maximum. **Pending:** the current patch has a query helper test,
   not a routed endpoint integration test.
6. Check both layouts at the narrowest supported meter width with German
   unavailable copy. The TUI fixture uses a 24-column terminal (22-column
   inner box) and verifies that `Max TP` and `Nicht verfügbar` both remain
   visible on separate rows. Add a GPUI rendered-screen fixture at the narrow
   player-panel width and verify the stacked German label/value remain visible
   without clipping bars or neighboring controls. **Pending:** the current
   patch does not contain a GPUI layout render fixture.
7. Compile the TUI and GPUI targets after searching all sibling `LoudnessData`
   struct literals, so the added public field is integrated across every
   downstream construction path.

## Current artifact limits

The patch is a reviewable application draft, not a passing UI implementation.
It contains the TUI narrow localized render regression, translation-completeness
checks, scalar-formatting tests, and a dev-query helper test. It does not yet
contain a GPUI rendered-screen test or a routed dev-API endpoint integration
test. Add those tests, including a lower later interval with an unchanged
programme maximum, before treating acceptance items 3, 5, and the GPUI half of
6 as complete. No sibling Cargo compile or test has been run; the present
evidence is limited to formatting and read-only patch applicability checks.

## Review and verification sequence

After Astra reviews this proposal and patch artifact, a sibling-source
implementation requires an approved writable boundary for
`/home/pierre/src/all_of_sotf/sotf`. If that boundary is granted, keep the
patch limited to the UI, localization, manual fixture/query, and related tests
described above. Capture sibling worktree status before editing and preserve
pre-existing changes.

Planned checks from the sibling repository root, after serializing Cargo use
with other workers:

```sh
rg -n 'LoudnessData\s*\{' crates
git apply --check /home/pierre/src/all_of_sotf/sotf-daw/audit/proposals/programme-maximum-true-peak-ui.patch
python3 scripts/generate_pseudo_locale.py
python3 scripts/generate_pseudo_locale.py --check
python3 scripts/check-design-tokens.py
cargo test -p app-tui --lib
cargo check -p app-tui
cargo check -p sotf-gpui --all-targets
cargo test -p sotf-gpui --test component_tests level_meter_copy_is_complete_and_only_technical_literals_remain
cargo test -p sotf-gpui --test e2e maximum_true_peak
```

The generated pseudo-locale diff must be limited to the intended new strings.
The UI test establishes display propagation, not the accuracy of the host
measurement. The host numerical tests remain the oracle for the dBTP value.
Even after this display stage passes, AUD-124 does not establish EBU Mode or
external certification: AUD-125 through AUD-128 and full EBU corpus coverage
remain open.
