# AUD140: channel-changing serial host EOF

Status: **bounded implementation accepted by Astra; focused tests and strict
lint pass.** Recorded 2026-09-29. See `reviews/AUD140-astra.md` for the frozen
implementation review and its scope qualifications.

## Pre-edit finding

Before AUD140, `DawHost::drain` called `is_topologically_linear_chain`. That helper
requires every node's input and output channel counts to match. The same helper
also gates `can_process_f32_linear_chain`, so removing its width check globally
would change ordinary processing fast-path eligibility. The scoped fix must add
a separate drain admission policy and leave that ordinary path unchanged.

The existing ordinary-processing helpers
`is_topologically_linear_chain` and `can_process_f32_linear_chain` are byte-for-byte
unchanged from the pre-edit source (1,713 bytes; span SHA-256
`90af47503cc9a14ae70c840b2da692d2fba5c3569177267f1d083c57ef96d2ca`). The
comparison receipt is `/tmp/sotf-aud140-root-fastpath-check.json`.

The restriction is reproduced through the public host API on a nonzero,
same-rate order-7 Ambisonics route (`64 -> 16`, `9.1.6`). The public test
processes audio successfully, then `drain` returns:

```text
end-of-stream drain currently requires a linear plugin graph
```

The same rejection occurs on a finite `64 -> 64 -> 16` serial route after a
finite FIR producer has supplied its last process block. The failure is at EOF,
not ordinary processing. Exact red output is in
`/tmp/sotf-aud140-public-red-final3.log` (SHA-256
`4f8f8356d2f7c515f4eb5e9c209efbb7c3722d28222f0d0a8aed5f83b016e9a8`): one
unrelated bypass-rejection test passed, and these two public EOF tests failed at
the current linear-graph guard as expected. The red command exited 101; it is
preserved as regression evidence, not a passing gate.

## Pre-edit audio and source evidence

Before any AUD140 production edit, I preserved the tested source snapshot and
audio arrays under
`crates/sotf-plugins/target/audit-baselines/aud140-pre-edit-final/`. The
`source/` subdirectory includes `Cargo.toml`, `Cargo.lock`, the test source,
and copies of the host drain, plugin capability, and Ambisonics decoder source.
The selected 223-file source manifest is
`source/source-manifest.sha256`; start and end manifests match exactly, with
aggregate SHA-256
`7abb95aafb26fcd2c306371a21e8546e2f35806d55327b54f70b261ba2aed7ca`.
The lockfile hash is
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
Manifest copies are `/tmp/sotf-aud140-final3-source-start.sha256` and
`/tmp/sotf-aud140-final3-source-end.sha256`.

The captured ordinary host outputs each contain 97 frames and are replayed
byte-for-byte by the ignored replay utility:

| Route | Captured artifact | SHA-256 |
| --- | --- | --- |
| Order 1, 4 -> 6 | `order1_4_to_6.f32le` | `4397bca0fdbf185123220f37514eec1c4a4b5be688e9f2273f28ea7e5c241a58` |
| Order 3, 16 -> 12 | `order3_16_to_12.f32le` | `47c9057b9f234b2323b46e5df930675821d9bf306173b656de4fff284f859c2b` |
| Order 7, 64 -> 16 | `order7_64_to_16.f32le` | `bfb7d4d76feb142ae08c4986804b1e056dcdf9c03fffe95b04afb0c7bc86ea63` |
| Finite FIR ordinary process, 64 -> 16 | `finite_fir_64_to_16_process.f32le` | `f96b65ac9e9394c901bf336b5cfba9af1962b97f5601463747b56993da1955d4` |

These ordinary captures are wrapper-versus-direct-plugin compatibility
controls, not independent Ambisonics accuracy measurements. The finite FIR
ordinary signal is also checked against the independent f64 equation
`y[n] = x[n] + 0.5*x[n-1] + 0.25*x[n-2]`.

The refusal capture records the exact error and proves no producer drain state
was advanced. After publicly removing the downstream decoder and rebuilding,
the same producer is drained normally and returns its full two-frame tail,
matching the complete independent f64 FIR reference. The recovered tail is
`aud140-pre-edit-recovered-fir-tail.f32le` (SHA-256
`b50a7edb067aef98173244ec616fa0694e8bcb27ab07a692670ac01f25e314f8`); refusal
metadata is `aud140-pre-edit-drain-refusal.txt`. It records zero begin/drain
calls at refusal, then one begin call and two one-frame drain calls after
rewiring, with `recovered_tail_frames=2`.
An independent decode of the saved tail reports exact equality to the f64 FIR
reference (maximum absolute error 0); the final marker is `0.1875` on ACN W,
with the other final-frame channels zero. The verification receipt is
`/tmp/sotf-aud140-root-tail-check.json`.

The finite-route reference uses independent f64 FIR arithmetic followed by a
separately constructed production Ambisonics decoder. That is a chain
composition oracle; it is not an independent Ambisonics implementation. The
decoder's separate matrix accuracy evidence is recorded under AUD133.

## Commands and evidence

The public red, ordinary capture, refusal/recovery capture, and ordinary replay
used the same selected source snapshot and the shared Cargo lock. The commands
were run with the offline locked `sotf-plugins` test target and filter
`aud140_channel_changing_eof`:

- `/tmp/sotf-aud140-public-red-final3.log`, exit 101 as described above,
  SHA-256 `4f8f8356d2f7c515f4eb5e9c209efbb7c3722d28222f0d0a8aed5f83b016e9a8`.
- `/tmp/sotf-aud140-ordinary-capture-final3.log`, exit 0, one ignored manual
  capture utility passed, SHA-256
  `737db0240ef15f9fe38d33dfd1ea68fc3a05778956209c173354a168978ba25f`.
- `/tmp/sotf-aud140-refusal-capture-final3.log`, exit 0, one ignored manual
  refusal/recovery utility passed, SHA-256
  `365219f9f0cdbe418d437334eb14335867155e0b46df94f073fc0f1e180e7cf2`.
- `/tmp/sotf-aud140-ordinary-replay-final3.log`, exit 0, one ignored manual
  replay utility passed, SHA-256
  `4c5cd8cf7fffe4ed685b0ee82e44152b7b901785000509b6a2fc6d4d54e546c2`.

The ordinary capture command was
`SOTF_AUDIT_BASELINE_DIR=<artifact-dir> cargo test --offline --locked -p sotf-plugins --test aud140_channel_changing_eof capture_aud140_pre_edit_ordinary_host_vectors -- --ignored --exact --nocapture`.
The refusal capture and replay used the same command with their respective test
names shown in the log descriptions above. All Cargo commands were run under
the process-held `/tmp/sotf-daw-audit-cargo.lock` with the workspace's absolute
warm target and `CARGO_NET_OFFLINE=true`.

All three manual utilities require `SOTF_AUDIT_BASELINE_DIR` set to
`crates/sotf-plugins/target/audit-baselines/aud140-pre-edit-final`; routine
test runs ignore them by default. The six binary artifact hashes are in
`artifacts.sha256` (aggregate SHA-256
`7cd6289a110102c19f12b15c11316ce6630ebee2287298b53f7380de03af9c3c`).

## Accepted bounded design

The proposed drain-only admission and composition design is in
[`proposals/channel-changing-host-eof.md`](proposals/channel-changing-host-eof.md).
It keeps `is_topologically_linear_chain` and ordinary fast-path behavior
unchanged, and admits only a strictly serial, same-rate path whose active nodes
explicitly guarantee identity frame geometry and finite tails. Unknown or
recursive tails, including Ambisonics dual-band mode, remain excluded from this
new width-changing admission policy. That exclusion does not redefine their
tail metadata or claim general finite support.

The existing frame-geometry capability defaults to false and is separate
from channel-width changes. Ambisonics must explicitly opt in only after each
supported process mode and rate path is source-verified; sampled
`output_frames_for_input` probes are not evidence for that capability.
The actual implementation keeps `is_topologically_linear_chain` unchanged.
`DawHost::drain` uses a separate strict serial validator only when the legacy
equal-width route check fails. It checks the single input/output chain and
adjacent audio edges, contiguous channel widths, width-preserving bypasses,
same host sample rate, explicit identity-frame capability, finite tail
metadata, and at least one active width-changing node. Branches, sidechains,
maps, and mismatched channel widths are refused before native drain calls.

Before taking the host scratch buffers or calling any native `begin_drain` or
`drain`, the new route checks caller frame alignment and declared output
capacity, then computes checked source, input, and output sample extents for
every uncompleted tail stage. The checked extents must fit the buffers prepared
by `build()`; oversized expansion and contraction cases return an error before
the producer begins. The accepted route preserves the existing causal drain
walk, so an upstream finite tail is processed through downstream nodes before
the next producer is drained.

`AmbisonicsDecoderPlugin` now opts into identity frame geometry. Its source
returns exactly the input frame count in both successful single- and dual-band
process branches and preserves the input rate. This capability covers frame
geometry only. Dual-band continues to report `TailLength::Unknown` and the new
channel-changing EOF route rejects it before processing state advances.

## Implementation evidence

The final AUD140 integration gate passes 13 tests with 3 ignored manual
utilities. It covers:

- zero-tail Ambisonics routes at orders 1, 3, and 7 (`4 -> 6`, `16 -> 12`,
  `64 -> 16`) with an empty output slice and an explicit zero-frame completion
  receipt;
- independent f64 FIR tail composition through actual order-7 Ambisonics,
  including the final nonzero marker and exact completion receipt;
- two finite producers separated by the width-changing stage, proving the
  first producer's frames advance the second producer before its own drain
  begins;
- a successfully bypassed width-preserving node, plus width-changing bypass
  refusal followed by unchanged ordinary processing and recovery of the full
  pending tail;
- preflight refusal for oversized 4-to-6 output scratch and 64-channel source
  scratch, with unchanged caller sentinels and zero begin/drain calls;
- undersized and misaligned caller buffers followed by successful exact-
  capacity retry, with the complete tail preserved;
- explicit rejection before begin/drain for missing identity capability,
  unequal negotiated rates, invalid adjacent widths, and dual-band's Unknown
  tail. The identity and rate refusals are followed by ordinary blocks compared
  against uninterrupted host twins.

The 4-to-6 and 64-to-16 oversized fixtures cross the current prepared graph
scratch bound of 8192 frames by 32 channels. The regression asserts rejection
before producer state changes; the regular admitted cases remain within the
prebuilt scratch. This is scoped preflight evidence, not a general allocator
or real-time performance claim.

The route oracle uses independent f64 FIR arithmetic followed by a separately
constructed production Ambisonics decoder. It is a full-vector chain
composition oracle, not independent Ambisonics mathematics. AUD133 records the
decoder's independent matrix-accuracy evidence. Existing equal-width drain
behavior is preserved by the unchanged legacy admission branch.

## Final commands and provenance

The tested source set is `daw_host.rs` SHA-256
`9b7bccab1760b536cb609b83709c648c995872f29ae804a472d0a52d1033bed8`,
`ambisonics_decoder_plugin.rs` SHA-256
`7a1f8c48dea0e2940da3c0fa65434107e1b8604c4161dbe1ffd11c0f17e4a64c`, the
integration test SHA-256
`6bd9ba00c0878281933235ceaf2493dbcc70038c2ee2b5aa65baf187b235b97c`, and
`Cargo.lock` SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
Final focused start/end manifests are
`/tmp/sotf-aud140-final-focused-start.sha256` and
`/tmp/sotf-aud140-final-focused-end.sha256`; both have SHA-256
`002b6a4e6f8a52c02780cc3f9a351ffc532c7b3d787f30de332f1d4773d1b244` and
match exactly. The host strict-lint, facade all-target strict-lint, and
Ambisonics all-target strict-lint manifests also match exactly.

The final facade strict-lint run includes two serialization-helper fixes in
`tests/aud140_channel_changing_eof.rs`: `write_baseline_vector` now uses
`size_of_val` instead of manual slice-size multiplication, and
`read_baseline_vector` uses `as_chunks` instead of constant-size
`chunks_exact`. The earlier host-clippy snapshot recorded test SHA-256
`20702f765c9181eed9b9d531731e14db02756379f4aec33a0c20cf9ac87c3d66`; the
final facade-clippy and focused manifests include the corrected test at SHA-256
`6bd9ba00c0878281933235ceaf2493dbcc70038c2ee2b5aa65baf187b235b97c`. These
are test-helper-only changes; no production behavior changed between those
test snapshots.

The exact four tested files were preserved for the next shared-host work under
`crates/sotf-plugins/target/audit-baselines/aud140-implemented-final/source/`.
The accompanying manifest at
`crates/sotf-plugins/target/audit-baselines/aud140-implemented-final/source-manifest.sha256`
has aggregate SHA-256
`002b6a4e6f8a52c02780cc3f9a351ffc532c7b3d787f30de332f1d4773d1b244`, matching
the focused start/end manifests listed above.

- `/tmp/sotf-aud140-final-package-test.log`, exit 0, 13 passed / 0 failed / 3
  ignored; SHA-256
  `00ea4ea42af3fee234432a77dee5c3d1c3205597d09e19e56d00325b5c1d5601`.
- `/tmp/sotf-aud140-final-replay.log`, exit 0, one saved ordinary-vector replay
  passed with `SOTF_AUDIT_BASELINE_DIR` set; SHA-256
  `1375b759f755b52c25b88c82cbd3b8f8ec0dc0d0ba678f2c95d6b32eb6f88b5e`.
- `/tmp/sotf-aud140-host-tests.log`, exit 0, 551 passed / 0 failed / 1
  ignored; SHA-256
  `01a37e7c3522feb5799ffdc1568f7399a9d58b1e673c14ede517ba1fdf2be924`.
- `/tmp/sotf-aud140-ambisonics-tests.log`, exit 0, 59 passed / 0 failed;
  SHA-256 `c0b1b7b89e6ca9cd6504865de9e2621b92ab8782c7159cbe986c7809ecfd25d6`.
- `/tmp/sotf-aud140-host-clippy.log`, exit 0:
  `cargo clippy --offline --locked -p sotf-host --all-targets -- -D warnings`;
  SHA-256 `e213fa6b9b6c69194a832c7cda0a9db2f0b9bb773cfb46f0fc7e3614d50a473f`.
- `/tmp/sotf-aud140-plugin-clippy-final.log`, exit 0:
  `cargo clippy --offline --locked -p sotf-plugins --all-targets -- -D warnings`;
  SHA-256 `a50d047397674cdf53b758152b7139ffddcb95a105123db5c0ea3ef25dd628b2`.
- `/tmp/sotf-aud140-ambi-clippy.log`, exit 0:
  `cargo clippy --offline --locked -p sotf-plugin-ambisonics --all-targets -- -D warnings`;
  SHA-256 `f99f53cb4f9cbe366dd16d33771e466f151c7fd7e97af89525af72087a5fd358`.

All Cargo commands used the process-held `/tmp/sotf-daw-audit-cargo.lock`,
offline mode, and the workspace's absolute warm target. The existing pre-edit
source copies and audio vectors remain under
`crates/sotf-plugins/target/audit-baselines/aud140-pre-edit-final/`; the saved
refusal capture and ordinary replay are still documented above.

This batch does not admit unknown or recursive tails, unequal-rate paths,
branching graphs, sidechains, channel maps, terminal sinks, or manager queues.
The AUD138 sink owner has a separate in-progress contract amendment; this
report does not claim its work is accepted. No full workspace gate is claimed
for this scoped snapshot.

## Consuming engine follow-up inspection — 2026-09-30

Current source inspection confirms a narrower next verification step than a
manager protocol change. `run_processing_thread` handles decoder EOF in
`processing_state.rs:1016`: each drain iteration obtains the current host's
output width/rate, sizes its output slice from `drain_output_frames_max`, calls
`DawHost::drain`, then creates an `AudioFrame` using those output dimensions.
It sends all produced frames before `ProcessingMessage::EndOfStream` and
checks for commands while a send is blocked. These are source observations,
not executed channel-changing engine evidence.

The existing worker fixture in `processing_thread/tests/eos.rs` prepares a
one-input/one-output probe which doubles the sample rate. Its ordinary EOF,
interruption and host replacement cases exercise the real worker, but that
probe does not establish channel expansion/contraction delivery. The missing
AUD140 engine gate should extend that existing worker harness with the already
accepted finite, same-rate identity-frame route: distinct channel markers,
both an expansion and contraction, complete ordinary-plus-tail samples,
correct frame metadata and exactly one EOF after the final marker. Include
the real 64→16 Ambisonics route and preserved output under receiver
backpressure; use the existing command handling without adding a queue or
manager protocol. A production correction is warranted only if the public
worker regression exposes one.

Zero-output terminal sinks remain a separate AUD138 contract:
`PreparedHostUpdate::prepare` in `engine/types.rs:155` rejects them, and the
ordinary worker calls `drain`, not `drain_to_sink`. This inspection does not
authorize removing those admission guards or treating writer acceptance as
device playback.

Inspected SHA-256 values:

- `processing_state.rs`:
  `396cf42451cfb90c864312471844b96f2e9dab731e69b0f5aa184bdab7185281`
- `engine/types.rs`:
  `c4f4657bab66fa8652471b81ce44e8ad59090025c7f67a83f772887d4c548bf8`
- `processing_thread/tests/eos.rs`:
  `92f05de6cd6c2851c8ccaa3342d29bea86942ace2c1e31c405f94773beaa0ae0`

No new engine test or production edit was made during this inspection.
