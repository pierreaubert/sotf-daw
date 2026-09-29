# AUD111 — LoudnessMonitor nested snapshot ownership

Implemented 2026-09-28. Reviewed scope:
[proposal](proposals/loudness-nested-realtime.md).

## Correction

`LoudnessMonitorPlugin` now uses the existing `RealTimeCache::update_if` to
verify authoritative `Arc::get_mut` access and prepared lengths for **all three**
nested arrays before writing any candidate. A held sample-peak, true-peak or
correlation array, including a Weak-only reader, prevents that candidate from
being changed or published. An available fallback can publish instead. If both
spares are unavailable, the existing complete snapshot and its outer Arc
identity remain unchanged while enabled meter accumulation continues.

Reset, disable and reenable all publish complete cleared candidates through the
same readiness check. A single pending-clear flag survives rejected attempts;
disabled callbacks retry one complete clear without feeding the meter. Enabled
callbacks can supersede a pending clear with measurements from the current reset
epoch. A same-value enabled setter remains a no-op. The three existing bounded
control/reset attempts are retained.

The spatial builder now uses the control-thread preparation route both before
and after initialization. Fresh independent cache payloads preserve the owning
enabled state on spatial, integrated-policy and sample-rate preparation.

Audio samples, routing, meter arithmetic, numerical tolerances, parameter IDs,
cache `update`, and generic `LoudnessData` writers are unchanged. Only
`src/analyzer_loudness_monitor.rs` has production changes. Tests are in the new
`tests/loudness_nested_realtime.rs`.

## Before correction

The public probe covered widths 2, 7, 32 and 40 at 48 kHz, both spatial modes,
each nested field, and strong/Weak readers that released the outer snapshot.

- All 48 retained-inner callback scenarios allocated when rotating back to the
  retained candidate: empty matrix 1 allocation/0 frees; nonempty strong reader
  2/0; nonempty Weak reader 2/1.
- All 48 reset calls measured 0/0, but 40 final snapshots retained an old
  nonempty array beside reset scalar fields and invalid flags. The remaining
  eight matrices were already empty. This was telemetry incoherence; the meter
  itself reset correctly.
- Eight explicit spatial-toggle histories already prepared callback storage
  correctly. All four post-initialization `with_spatial()` cases allocated 2/1
  in their first callback.

Artifacts: `/tmp/sotf-loudness-nested-realtime-probe.rs` and
`/tmp/sotf-loudness-nested-realtime-probe.log`.

All five permanent tests failed on the original implementation with the
expected allocation, uncleared-array and disabled-preparation observations:
`/tmp/sotf-loudness-nested-realtime-red.log`.

## Verified regression matrix

| Test group | Fresh callback threads | Evidence |
|---|---:|---|
| Individual retained nested strong/Weak readers | 48 | Both processing routes, first callback and repeated rotations; retained contents unchanged and Weak owners remain upgradeable. |
| Warm reset with one retained nested array | 48 | Every published reset field cleared coherently; retained historical payload unchanged; new frame count begins at reset. |
| All generations retained, release and resume | 64 | All three nested strong/Weak arrays or outer strong/Weak snapshots; exact skipped identity; repeated reset; disable; reenable before/after release; pending disabled clear; current-epoch fresh-twin equality. |
| Spatial builder before/after initialization | 8 | First and reset callbacks use prepared matrix geometry. |
| Disabled structural preparation | 8 | Cold reset, spatial off/on/restore, integrated policy rebuild, old-reader preservation, enabled flag, and same-value setter identity/history. |

Total: **176 fresh callback threads**, **912 measured process/tap calls**
(848 primary calls plus 64 fresh-twin calls), **192 resets**, and **152 valid
prepared-ID setters**, each requiring **0 allocations and 0 deallocations**.
Ordinary process calls also assert exact passthrough. Reset/current snapshots
compare all relevant scalar validity, loudness, policy, peak and correlation
fields and nested arrays, without changing numerical tolerances.

The five focused tests pass in 0.03 seconds:
`/tmp/sotf-loudness-nested-realtime-green-final.log`.

The first post-fix run passed three tests and exposed a fixture ownership error
in the two setter histories: the fixture moved its last newly prepared
`ParameterId(Arc<str>)` owner into the measured setter, counting its destruction
as one free. The fixture now retains the prepared owner and passes an Arc clone.
This does not assert that consuming a caller's last ID allocation is free-free;
that public ownership behavior is unchanged. No production workaround or
allocation threshold relaxation was introduced. The first log remains
`/tmp/sotf-loudness-nested-realtime-green.log`.

## Full verification

Focused tests, rustfmt and scoped `git diff --check` passed. The combined host
and XTC gate, run by the SOFA implementation agent with root coordination,
passed **667 host tests across 22 suites**, with eight preexisting ignored doc
examples. XTC also passed 212 tests, with one ignored doc example. The five new
AUD111 tests passed in that full gate. Strict all-target host and XTC Clippy
passed with warnings denied (2.68 seconds).

```text
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-tmp cargo test -p sotf-host -p sotf-plugin-xtc
TMPDIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-tmp cargo clippy -p sotf-host -p sotf-plugin-xtc --all-targets -- -D warnings
```

Logs: `/tmp/sotf-sofa-host-xtc-full.log` and
`/tmp/sotf-sofa-host-xtc-clippy.log`.

Independent read-only review by `/root/plugin_chain` found no blocker. It checked
all-three authoritative readiness and Weak handling, complete control/reset
writes, pending-clear lifetime and disabled retries, enabled current-epoch
replacement, dirty-spare safety, structural builder preparation, and the public
Strong/Weak/outer exhaustion and recovery fixtures. The reviewer made no edits
and ran no duplicate builds.

## Runtime cost and limits

One boolean is added per plugin. Readiness checks at most three nested arrays
per candidate, with at most two candidates per attempt. No new snapshot storage
is required; the existing independent triplet is reused. Reset/control paths
retain their existing three-attempt bound. A disabled callback performs one
additional attempt only while a clear remains pending.

Readers retaining all prepared candidates can delay telemetry indefinitely.
Control state and audio passthrough are immediate; old telemetry remains an
immutable, internally complete snapshot until a candidate is released. There is
no asynchronous publication guarantee without a callback or control operation.

True-peak queries consume interval state only when publication succeeds, so the
interval extends across skipped publication just as under existing outer-cache
contention. No new unified peak-window convention is claimed. Generic
`LoudnessData` writers outside this prepared plugin can still allocate. Spatial
and integrated-policy structural preparation remains a control-thread operation.
The valid setter allocation result assumes a retained prepared parameter ID.
This work does not redesign other meter backends or initialization error policy.
