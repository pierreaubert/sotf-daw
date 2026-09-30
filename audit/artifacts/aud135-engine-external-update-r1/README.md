# AUD135 requested external update failure

This packet records the engine update-boundary fix that rejects an incomplete
explicit external-plugin candidate before sending `PreparedHostUpdate`, while
leaving the documented best-effort initial-build behavior unchanged.

## Evidence

- `logs/sotf-aud135-requested-external-red-r1.log`: pre-fix linear candidate
  test failed because the skipped external plugin still allowed a replacement
  command to be sent. The exact pre-fix source snapshot was not archived; this
  log is retained as the behavioral red receipt.
- `logs/sotf-aud135-requested-external-r2.log`: after the fix, the requested
  external no-send test passed (1/1), including a startup positive control
  showing best-effort startup still skips the malformed plugin, followed by
  the existing invalid BandSplit candidate test (1/1).
- `logs/sotf-aud135-requested-external-graph-r1.log`: graph route no-send test
  passed (1/1); this confirms the graph's existing all-or-nothing construction
  behavior remains intact.
- `source/apply.rs`: exact engine source at the green checkpoint.

These are command-probe unit tests. They establish preflight failure/no-send
and retained engine metadata, not a populated live host's audio continuation.
The separate loaded isolated-worker/native late-load test remains required.

## Commands

All commands ran from the DAW workspace with:

```text
CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target
TMPDIR=/tmp
CARGO_NET_OFFLINE=true
```

```text
cargo test --offline --locked -p sotf-engine --lib external_candidate_does_not_send_host_update -- --nocapture
cargo test --offline --locked -p sotf-engine --lib invalid_band_split_candidate_does_not_replace_working_host -- --nocapture
cargo test --offline --locked -p sotf-engine --lib failed_external_graph_candidate_does_not_send_host_update -- --nocapture
```

Each ran under `/tmp/sotf-daw-audit-cargo.lock`. The first two commands were
combined in one lock window. The pre-fix red used the same linear test name.
