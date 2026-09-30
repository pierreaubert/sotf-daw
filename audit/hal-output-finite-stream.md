# AUD138: HAL Output pending audio at EOF

Status: **direct plugin and bounded serial host process/drain, recovery and
preserving ring-size reprepare accepted by Astra on 2026-09-30**.
Consuming-engine admission and physical playback remain open. The full audit
is not complete.

The final test-only multi-chunk case closes the remaining producer-incomplete
condition; earlier host corrections were also traced and accepted. The final
verdict and exact source/log hashes are in
[`AUD138-astra.md`](reviews/AUD138-astra.md). Historical pending-review text
below describes prior checkpoints, not the current bounded verdict.

## Defect and change

A final short writer call leaves audio in HAL Output's pending queue. Before
this change, the inherited `Plugin::drain` immediately returned complete; only
a later ordinary input block could retry the retained samples. Two public
process/drain regressions reproduced this with a portable fake writer.

The plugin now drains retained samples into its writer, with at most two write
attempts per call. Prepared storage preserves frame order across wrapped queue
storage. A blocked transport leaves the queue incomplete; the total call bound
remains unknown while samples are pending. A sink emits zero output frames,
including when its writer accepts queued audio.

Control-thread transport recovery preserves the pending queue, including failed
format preparation. Explicit reinitialization can cancel queued frames after
successful preflight and counts them as dropped. Ordinary overflow accounting
is preserved. Completion certifies that the writer accepted retained samples
into its ring; it cannot recover earlier overflow drops or certify playback.

See the [bounded proposal](proposals/hal-output-eof.md),
[Astra review](reviews/AUD138-astra.md) and
[consuming-route audit](remaining-finite-stream-reconciliation.md).

## Executed evidence

Root inspected terminal logs and current file hashes on 2026-09-29. The two
original regression tests are included in the complete package run below.
The ready-writer case compares all accepted samples with the eight-sample
input, and observes no completion until both stereo suffix frames are handed
off. The blocked case verifies incomplete status and retained frames before a
later retry completes without new input.

| Gate | Result | Log SHA-256 |
| --- | --- | --- |
| `cargo test --offline --locked -p sotf-plugin-hal-output --lib` | 37 passed, 0 failed, 0 ignored | `44015810f4baf1e2514602e0c254470c9e36db74b32a4a79a0d4588a08ff46c2` |
| `cargo clippy --offline --locked -p sotf-plugin-hal-output --all-targets -- -D warnings` | Passed | `a53a4a993712e426e545fe12773ce2b5371c956e639b2e83aef2f2fe52e81530` |
| `cargo fmt --package sotf-plugin-hal-output -- --check` | Passed, reported by implementation owner | No separate captured log |
| `git diff --check` | Passed | No separate captured log |

Logs are `/tmp/sotf-aud138-hal-green-final.log` and
`/tmp/sotf-aud138-hal-clippy.log`. Commands use the absolute warm Cargo target
and offline environment recorded in [the implementation plan](IMPLEMENTATION_PLAN.md).

The package tests cover wrapped frame storage, short/zero writes, transport
gates and recovery, invalid contexts and destinations, writer errors, input
after incomplete/completed drain, explicit cancellation and prior overflow
telemetry. Heap guards cover initial empty, blocked, successful retry and
terminal drains using a prepared nonallocating writer. These portable tests
do not execute the macOS transport or its physical consumer.

### Frozen source and provenance

| Artifact | SHA-256 |
| --- | --- |
| `crates/sotf-plugins/crates/sotf-plugin-hal-output/src/lib.rs` | `4905e63a57c8c20a615dfd6cea5974440b04296ccb32da493a0df0a37a6bcb1c` |
| `audit/proposals/hal-output-eof.md` | `debdf2b4f228e665e97906d060dceff1cbffb613b4de55b429ca957845e1f3d7` |
| `Cargo.lock` | `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5` |
| Pre-edit source `/tmp/sotf-aud138-hal-output-pre-edit.rs` | `4b782ed45bbc5913b13c7e113b1f2c43c9391187e6c3b945af568c42cf242243` |
| Expected-red `/tmp/sotf-aud138-hal-red-clean.log` | `ed2a9c51949eaa66a710b416fdbf47208e7fe0ef751e99683eaac03bc00bc644` |

Root verified that the pre-edit source is byte-identical to this file at Git
commit `93027970f412ce47c0cd2b8e4b7b1a5b0e5f0261`. It can be recovered from
that revision if the temporary snapshot is removed.

The red run reports two expected failures, with 27 other tests filtered out.
An earlier failing test poisoned its observation mutex during assertion; that
harness issue was fixed before the preserved clean red run. No passing claim
uses the earlier poisoned run.

## Host implementation: first compiled capacity checkpoint

The host/HAL API batch now compiles. The focused command
`cargo test --offline --locked -p sotf-plugin-hal-output --lib public_daw_host_sink -- --nocapture`
passes one test:
`public_daw_host_sink_drain_preflights_tail_before_any_source_consumption`.
Root verified the terminal log
`/tmp/sotf-aud138-host-sink-focused-impl.log`, SHA-256
`1e6d25d46d1eb6ff32527053664ca5175756fb0a86b106bae69292b56c4b05e0`.

The inspected case checks capacity refusal before producer begin/drain calls,
unchanged observation state and exact accepted ordinary samples, rejection of
sink removal after stream freeze, and a separate adequate-capacity host that
hands over the exact complete delayed stream. It uses the new explicit sink
API. This is an intermediate single-test result while source work continues;
there is no final source manifest, package gate or implementation acceptance
for the host stage yet. Backpressure, recovery, failures, lifecycle/heap and
ordinary-route compatibility remain required.

## Host correction checkpoint — 2026-09-30

Luna implemented the five findings from the host review: finite-tail and
topology checks before source consumption, retention of an admitted block
across transport changes, typed drain errors without implicit graph builds,
and a public control-thread recovery path. Recovery preserves local pending
audio and the frozen EOF state. It returns `NeedsReprepare` for ring growth
or shrink, `Waiting` when the format is unavailable, and resumes delivery
when the original exact geometry is available again.

Root inspected the following terminal results. These supersede the earlier
single-test host checkpoint; independent Astra acceptance of the corrections
is still pending.

| Gate | Result | Log SHA-256 |
| --- | --- | --- |
| Focused correction and recovery tests | 7 passed | `088e5b558ffeb9515ef5804f27f14d86fd2264d6d3f99538b37a281d5fbfbbb1` |
| Full HAL library | 61 passed | `6ca88354b3ca4e98799f13dfd31ee185c932b7af79131dff11a7ef0cb102af8c` |
| Full host library | 551 passed, 1 ignored | `fa29db292bd38ce094ec5bdbca73f25e4ddce0a8e62e041bf0a2c5ba25b37907` |
| Strict host all-target Clippy | Passed | `cd01ba7c651b146a8ab290ac76e0d6ef7186f055666c42675f13c7e541be8d60` |
| Strict HAL all-target Clippy | Passed | `03a6f99f816e2bb17ee460aeede61e2df50dc5f675f7a85d3d33fce049e734f9` |

Logs use `/tmp/sotf-aud138-` prefixes:
`recovery-focused-r3.log`, `hal-final.log`, `host-lib-final.log`,
`host-clippy-final.log`, and `hal-clippy-r2.log`. The final selected-source
manifest is `/tmp/sotf-aud138-hal-clippy-r2-end.sha256`; its matching start
manifest covers the host plugin API, host implementation, public reexports,
HAL source and lockfile. The HAL library run predates two test-helper type
aliases added for strict lint; those edits do not change production behavior.
The host run includes ordinary processing panic isolation with passthrough.

Before persistent-geometry work started, root rechecked all four Rust files
and the lock against that final manifest and retained their exact bytes plus
the manifest in
`crates/sotf-plugins/target/audit-artifacts/aud138-exact-geometry-checkpoint/source.tar.gz`.
Archive SHA-256 is
`24954d3da6dd11b4e883fe276bb580529c861144f265f6c089f80bdd18a41af5`.
This local source archive preserves the source input for the queued
correction review; it is not a completed review or a whole-workspace snapshot.

The focused recovery test admits four pending frames, checks growth/shrink
refusal without changing local samples or flushing the ring, checks temporary
format unavailability, then restores the original format and verifies the
saved input plus finite Delay output is handed over once. An earlier run
aborted after a test assertion poisoned its observation mutex; that failed
run is not passing evidence. The corrected test compares the initialization
flush baseline and copies observations before assertions.

The current channel-changing facade regression now passes 13 tests, with three
manual capture/replay utilities ignored. Root executed
`cargo test --offline --locked -p sotf-plugins --test aud140_channel_changing_eof`;
log `/tmp/sotf-aud138-aud140-facade-r4.log` has SHA-256
`410f1e24cbba1739b63623231011f4699e6bd3e30d5ad6a8f7b8626804c21b77`.
Its 57-file selected source/artifact start/end manifests match at
`522f2daa68feff091b67fbc1f848522cf744d95126e85a7a7545291992d5a4da`.
Earlier attempts failed before tests on BandSplit engine integration errors;
those failures are retained in their separate logs. No physical playback or
consuming-engine admission claim is made by this checkpoint.

### Preserving ring-size reprepare — revision awaiting review

The control-thread reprepare path stages the pending queue and sink buffers
before committing a ring-size change. It preserves the drain cursor, source
completion state, queued samples, host latency, retained node outputs and
compensation-delay history; after a successful same-rate/channel resize it
recomputes cached latency. Failure before commit leaves the old geometry and
pending stream intact. The earlier exact-geometry source snapshot remains
preserved in the archive above.

The added tests cover a producer whose drain has already started before both
growth and shrink, plus a no-resize twin and full-vector comparison; shrinking
the physical ring from 8 to 4 frames while 6 frames remain queued, then
servicing that backlog and finite tail exactly under allocation/deallocation
guards; and a 16-channel route processing 8,193 input frames followed by an
8,192-frame tail at ring size 16,384, with the full accepted vector checked
under those guards. Sixteen channels is the maximum admitted width because
HAL supports at most 16 and the terminal serial route preserves every node's
width. The wide route does not claim support above that limit.

Two deterministic failure regressions exercise the transaction boundary. The
fake writer mutates format, configuration, connection, or key readiness on the
second format read, between initial plan capture and the host's pre-commit
recheck; each case refuses before flush/reload or Ready publication, preserves
local samples, geometry, latency, telemetry and producer state, then recovers
and delivers the exact stream once. A test-only staging seam fails the drain
buffer reservation after earlier staging succeeds and verifies the same local
state remains intact before successful retry. This is deterministic checked
failure coverage, not a claim about process behavior under real system OOM.

| Gate | Result | Log SHA-256 |
| --- | --- | --- |
| HAL library before the r2 test-only addition | 71 passed, 0 failed, 0 ignored | `a2446abaadf61d4a02113ce333ed3e14663322c2f7a7de15ac85aeee84de9deb` |
| Reprepare-focused HAL tests on final lint-clean source | 9 passed, 0 failed | `23335546657eb345d396c3f4f3164b0cb0211b1419389a9d57b6090df7fff8ad` |
| Strict HAL all-target Clippy | Passed | `44306e1b44e397885f61ed9424569aa6b74ed33c89464a2dca86a78a1c6dd6f7` |
| r2 unfinished multichunk producer regression | 1 passed, 0 failed | `bf9f83ec1a7aa0075c8cb554c8e20a47f81419df0ea98cc63151fca3b183c815` |
| Strict HAL all-target Clippy after r2 test addition | Passed | `8d34be7b97bb6470481527f39d69c55b6ccd89b7c883df0b68b511f1b7f2c71c` |

The r2 test starts a six-frame finite producer tail in three two-frame chunks.
It drains the first nonzero chunk, confirms four frames and two producer calls
remain, then grows and shrinks the sink ring in separate runs. It checks that
calls servicing the already queued backlog do not advance the producer, that
the next producer chunk resumes at the saved cursor with its remaining quota,
and that the final accepted waveform exactly matches both an unresized twin
and the finite-delay reference. The producer begins once and completes after
exactly three producer drain calls in each resize direction. The first two
focused attempts exposed a test assumption about queue servicing; they did not
identify a production failure. The corrected focused command was
`cargo test --offline --locked -p sotf-plugin-hal-output --lib aud138_reprepare_growth_and_shrink_preserve_unfinished_multichunk_tail -- --nocapture`;
the passing log is `/tmp/sotf-aud138-multichunk-reprepare-focused-r3.log`.
The strict lint command was
`cargo clippy --offline --locked -p sotf-plugin-hal-output --all-targets -- -D warnings`,
with passing log `/tmp/sotf-aud138-multichunk-reprepare-clippy-r1.log`.
Both commands used the shared Cargo flock, absolute warmed target and
`TMPDIR=/tmp`. This was a focused regression and lint run; the 71-test HAL
library suite above was not rerun after adding this test, so no 72-test full
suite result is claimed.

The pre-r2 HAL source SHA-256 was
`27d7a4a452b580b0250ccce2b1994bade1e7b7c64516e3371a847c004aa7d04f`; the
current HAL test source SHA-256 is
`b81d7042ce4766cb8252c477539c0318a079f92e481f266929da3f147d027346`. The
five-entry manifest at `/tmp/sotf-aud138-multichunk-source-manifest.sha256`
has SHA-256 `a8e77d733f79609cdc3d73d268cdfe827add0a2b0872fdfa1c26030d0619f79f`.
The host plugin API, host implementation, public host reexports, and lock remain
unchanged at `9f704ca257ae03394bcd304098c8ad9e2ad5eebef44e931ffa464a637ae59fcd`,
`b51f44b6515ce3e93e833be6c45fb2d1a5152cb5ac6944b61eb320436261463a`,
`d5a656f6f24aaf044857be05ba9ec843ae1adbe1914955afe4fc5a40e4fc959f`, and
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`,
respectively. `git diff --check` is clean. Earlier 551-test host library and
strict host Clippy results remain applicable because these dependencies were
not changed for this revision. Portable tests do not establish macOS HAL
playback or consuming-engine scheduling behavior.

## Remaining scope

- The accepted source above covers the direct plugin. Corrected public host
  regressions now reproduce the terminal process panic and stale pending
  samples after reset. The drain probe is refused by topology validation before
  sink capacity preflight; the source tail remains available. Evidence is in
  [the bounded host proposal](proposals/hal-output-host-drain.md). Astra accepted
  its final design at SHA-256
  `bedb455fd4ba7abf835d10b537d3a1e93da0e44612a2864a830a0d56f4b3bf9a`;
  Luna implemented the explicit exclusive-owner serial sink route and the
  host corrections recorded above. Complete acceptance of the preserving
  ring-size reprepare revision still requires independent review.
- The original generic `DawHost` drain rejects zero-output sinks and assigns an
  unknown drain a 4096-call fallback quota. The new explicit sink route must
  separate external backpressure from finite producer progress. Its consuming scheduler needs an
  explicit external-backpressure contract.
- At the accepted direct-plugin checkpoint, generic `Plugin::reset` was the
  inherited no-op for HAL Output; cancellation tests exercised reinitialization.
  The host stage
  must define and test pending-queue ownership at generic reset without
  confusing it with transport-ring cancellation.
- Normal engine configuration and prepared host updates reject zero-output
  hosts. The current systemwide daemon removes these legacy HAL graph nodes.
  A hypothetical processing-worker sink is not an admitted application route.
- Native transport execution and physical playback remain unverified. Ring
  flushing discards data and is never delivery evidence.
- No new workspace-wide gate covers all concurrent AUD135–143 edits.
