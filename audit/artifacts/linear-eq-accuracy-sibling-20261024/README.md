# Sibling controller and UI propagation

Feature commit `ff4d38c000b5bc36f99e6affd1673ae4c8c10a2a` contains eleven Rust
paths in the sibling `sotf` repository. It adds the accuracy setting to desktop
parameter 105, TUI descriptors/value/adjustment paths, settings constructors,
counts and GPUI controls. Existing IDs and saved manual choices are retained.

The 85 controller/dev-api tests pass. Default-feature GPUI library compilation,
formatting and design-token checks pass. Raw logs and fixture-only manifest/lock
diffs are retained; temporary dependency-path and six local-version adjustments
were excluded from the feature commit.

The feature patch was also applied to the active sibling checkout without
staging or changing its branch. The captured proof preserves its four existing
user changes, active lockfile and staged blobs. `summary.json` independently
checks all eleven active source files against the feature commit.

GPUI has no live engine-to-view accuracy-report path. Its automatic summary
therefore avoids invented tap counts or manual-derived latency; it displays the
automatic/pending state. The DSP/host API reports resolved support, total latency
and added latency. This checkpoint does not implement a new telemetry bridge or
claim interactive native-host validation.

The original isolated worktree is
`/Volumes/home_worktrees/sotf-lineareq-accuracy-20261024`; the copied evidence is
self-contained for review. Private app issue 192 links the implementation to
DAW issue 9. No push was performed.
