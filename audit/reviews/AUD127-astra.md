# AUD127 independent design review

Status: scoped host/API and TUI/Studio implementation accepted. Validator: Astra medium.

Verified official EBU Tech 3341 (November 2023) section 2.4: during the first
60 seconds of LRA measurement, displayed LRA must carry a not-yet-stable
indication. Source: https://tech.ebu.ch/docs/tech/tech3341.pdf . Active accepted
I/LRA frame time is consistent with the coupled pause/reset contract. Keep
numeric availability separate from elapsed-time stability; 60 seconds is not
a statistical convergence guarantee or a certification claim.

The proposal correctly avoids using approximate observation cadence, counts
initial warmup time, excludes paused input, and recomputes the field across
query-cache boundaries. Serde default, publication retention, exact one-frame
boundary and 11025 Hz geometry are required and included.

Requested explicit refinements:

- Display range in LU, guarded by both Valid status and finite numeric value.
- TP-only drain and rejected/empty input do not advance the stability clock.
- Structural integrated-mode/spatial rebuilds follow existing measurement
  epoch reset/preservation rules; cover ordinary and wide aggregation routes.
- Define the added TUI row priority and guard complete rows at small heights;
  retain current meters/maxima and border integrity under locale expansion.

No architectural blocker. Implementation owner should record these details
and return the stable candidate with current-source evidence for validation.

Revised proposal explicitly incorporates all requested refinements. Design
accepted; no production acceptance implied. Preserve numeric zero as valid and
assert exact integer one-frame thresholds in the implementation tests.

## First stable candidate review

Production clock uses widened `u64(sample_rate) * 60` and overrides cached LRA
stability on each query. Source review finds the accepted active-time contract
intact. Both UIs guard Valid/finite/nonnegative numeric values, use LU, and hide
stability claims for unavailable data. TUI guards the complete added row set
and includes stability in redraw signatures.

Evidence revisions required: current serde/update_from/retained/reset fixture
uses a four-second false flag throughout. It cannot detect loss of true state,
failure to clear a stable flag, or a blocked false-to-true publication. Requested
stable=true serde/update_from and reset/Start/reinitialize/enabled transitions,
plus strong/nested-Weak retained generations across threshold and recovery.
Also assert unchanged observation count across the 11025 Hz one-frame boundary
to prove cache-independent stability. No production defect identified and no
validator Cargo run; implementation owner to revise evidence and revalidate.


## Final acceptance

Evidence findings closed: stable=true serde/update_from and reset/Start/
reinitialize/enabled transitions are now nonvacuous. Nested Weak references pin
cache generations at a valid false reading; crossing the threshold preserves
the old snapshot, releasing readers permits true publication. At 11025 Hz the
exact threshold frame changes stability without changing observed-window count.

Verified final workspace 6055 passed/13 skipped, strict host lint, and current
DAW files against equal start/end manifest
`cc5cce1410755e67e88844715890761c287998ac9562c9cd626e7bca0c1a1bb8`.
Current sibling source verifies against
`/tmp/sotf-aud127-ui-source-end-final.sha256`, matching start at
`76723a81628b9af2c48ef752c72bc7e0de7149e65c8484c49c1d5c66cf7ba760`.
The similarly named final-gates manifests are older; report should name exact
paths. TUI367, mounted/compact and redraw/localization evidence reviewed.
Original sibling lock restored to `2c87468c…531f`; UI gates used temporary
`7a03b9f5…af3a`, not that restored lock.

Report accurately limits realtime evidence to existing guarded publication
paths and limits mounted bounds to720x1100 plus compact420x720. No statistical
convergence, worst-case CPU or EBU certification claim. Scoped AUD127 accepted.
AUD128 official corpus inventory/evidence and the broader audit remain open.
