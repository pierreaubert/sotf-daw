# AUD127: First-minute LRA stability indication

**Status:** accepted by Astra on 2026-09-29 for the AUD127 host/API and
reachable TUI/Studio stages. The broader metering audit remains open. MIDI and
IAMF remain excluded.

## Behavior

`LoudnessRangeData` now carries a serde-defaulted `is_stable` bit, separate from
numeric availability. The host derives it from the accepted active I/LRA frame
count at `sample_rate * 60`, including the first three seconds before an LRA
observation exists. It recomputes the bit on every snapshot query so a
non-divisible-rate threshold can be published between 100 ms LRA observations.
Only successfully accepted nonempty samples advance the clock. Pause freezes it;
Continue resumes it; reset and integrated-mode rebuild start a new unstable
epoch; spatial snapshot rebuild preserves the active clock. Empty/rejected input
and TP-only drain do not advance it.

Both application surfaces show LRA in LU. A stability marker appears only when
the range status is `Valid` and the value is finite and nonnegative. It is
removed at the first stable snapshot. Missing, nonvalid, negative, NaN, or
infinite values show the localized unavailable value with no stability claim.
The TUI signature now includes the visible LRA status, one-decimal LU value,
and stability bit, so the marker transition redraws even when the number stays
the same. The TUI places its complete LRA row after M/S/I and their maxima and
keeps the panel border intact under height pressure. GPUI renders the same
states on the mounted Studio Loudness Monitor surface.

## Changed source

Host/API:

- `crates/sotf-plugins/crates/sotf-host/src/analyzer.rs`
- `crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor.rs`
- `crates/sotf-plugins/crates/sotf-host/src/analyzer_loudness_monitor/loudness_range.rs`
- `crates/sotf-plugins/crates/sotf-host/tests/loudness_range.rs`

Reachable UI and tests in sibling `../sotf`:

- `crates/app-tui/ui/draw_meters/draw.rs`
- `crates/app-tui/main/misc.rs`
- `crates/app-tui/i18n.rs`
- `crates/app-gpui/components/plugins/level_meters/render.rs`
- `crates/app-gpui/app/i18n/translations.rs`
- `crates/app-gpui/app/i18n/translations_pseudo_generated.rs`
- `crates/app-gpui/tests/component_tests/tests.rs`
- `crates/app-gpui/tests/e2e/scenarios/plugin_rack/mute_solo_level_meter.rs`

The sibling files already contain accepted AUD124–126 work and other user
changes. AUD127 additions were applied in place; no unrelated files were
reverted.

## Evidence

Host focused regression:

```sh
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
CARGO_NET_OFFLINE=true cargo test --locked --offline -p sotf-host --test loudness_range
```

Result: **10 passed**. It checks the exact one-frame-before and exact 60-second
boundary at 48 kHz and 11,025 Hz, bulk and irregular callback/query patterns,
empty and rejected input, repeated TP drain, pause/resume and paused reset, a
12-channel explicit 7.1.4 meter route, spatial and integrated-mode rebuilds,
serde legacy defaults/roundtrip, retained snapshots, and `LoudnessData::update_from`.
Stable `true` also survives snapshot serialization/copy and is cleared by Reset,
Start, reinitialize, and disable/enable transitions. At 11,025 Hz the exact
stability frame changes the bit without increasing `observed_windows`, proving
the non-dirty-query path. A retained valid false snapshot and nested Weak owners
block publication across the threshold; a later update publishes stable true
without mutating the retained snapshot. At 48 kHz the exact threshold coincides
with an LRA observation boundary. The 11,025 Hz case remains
`timebase_is_exact == false` while stability follows the exact active sample
count.

Strict host Clippy passed:

```sh
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target/audit-tmp \
CARGO_NET_OFFLINE=true cargo clippy --locked --offline -p sotf-host --all-targets -- -D warnings
```

The required offline workspace nextest gate passed **6,055 tests across 359
binaries; 13 skipped; 0 failed** in 268.335 seconds. Test selection excluded
`sotf-midi` and `sotf-iamf`; FFI remained included. The build compiled some
excluded crates as dependencies, but their test packages were not selected.

Sibling UI checks used the warmed SOTF target and the reviewed temporary
resolver lock:

- TUI library: **367 passed** (`cargo test --locked --offline -p sotf-tui --lib`).
- TUI binary redraw signature: **1 passed** for
  `loudness_signature_tracks_lra_display_and_first_minute_transition`.
- GPUI compact render: **1 passed** at 420×720.
- Mounted Studio Loudness Monitor render: **1 passed**, including valid zero LU,
  unstable/stable marker transitions, BelowGate and nonfinite fallback, and
  summary/bar bounds.
- GPUI translation-completeness test: **1 passed** across supported languages.
- `cargo check --locked --offline -p sotf-gpui --all-targets --features dev-api`
  passed.
- Pseudo-locale `--check`, Rust formatting check, and `git diff --check` passed.

Exact gate logs are under `/tmp/sotf-aud127-*`. The final TUI library run is
`/tmp/sotf-aud127-tui-final.log`; the separate binary redraw test is
`/tmp/sotf-aud127-tui-redraw-signature.log`; GPUI logs are
`/tmp/sotf-aud127-gpui-mounted2.log`,
`/tmp/sotf-aud127-gpui-compact-e2e.log`,
`/tmp/sotf-aud127-gpui-translations.log`, and
`/tmp/sotf-aud127-gpui-all-targets-final.log`. The host focused and Clippy logs
are `/tmp/sotf-aud127-host-focused-final3.log` and
`/tmp/sotf-aud127-host-clippy-final2.log`; the workspace result is
`/tmp/sotf-aud127-workspace-nextest-final2.log`.

## Snapshot and lock provenance

The DAW workspace source manifest covers Rust files and Cargo manifests outside
`.git` and target directories. Start/end manifests for the final gates match at
`cc5cce1410755e67e88844715890761c287998ac9562c9cd626e7bca0c1a1bb8`
(1,779 entries). Only the intended host integration test changed relative to
the earlier accepted snapshot. The DAW `Cargo.lock` stayed at
`6fd8186e6c6ef66ac2a18c243fd3320bfd98f07fa54b230041b72cab1e3ed088`.

The sibling UI source manifest covers Rust files in `app-tui` and `app-gpui`;
start/end match at
`76723a81628b9af2c48ef752c72bc7e0de7149e65c8484c49c1d5c66cf7ba760`
(741 entries; `/tmp/sotf-aud127-ui-source-start-final.sha256` and
`/tmp/sotf-aud127-ui-source-end-final.sha256`). UI gates used temporary resolver lock
`7a03b9f561ee929aa189ee70881d84563ab1b1d0036b3a76078d50c033a5af3a`.
Afterward, the exact original sibling lock was restored and verified at
`2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f`.
The UI checks are not claims against that restored lock.

## Limits

The existing realtime guard suite exercises cache publication while the
stability bit is recomputed as part of the same update; no separate
stability-only allocation probe was added. The mounted Studio test uses a
720×1,100 QA window so the complete existing
plugin card, including its status and controls, is in the viewport. The
standalone component test remains at 420×720. This verifies bounds and
non-overlap for the meter surface; it does not claim the whole Studio rack fits
inside a 900-pixel viewport without scrolling. No CPU benchmark or worst-case
callback-time claim was made for the boolean check. No EBU Mode or programme
certification claim follows. AUD128 official EBU corpus evidence and the broader
audit remain open.
