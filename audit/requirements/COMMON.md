# Common implementation contract

Applies with each [plugin assignment](README.md). Read [CHECKPOINT.md](CHECKPOINT.md) before starting and coordinate [shared files](SHARED.md).

## Current execution override

The user resumed implementation and replaced both Luna implementation and Astra review with **Muse `muse-spark-1.3-contributor`, effort `max`**. Use a separate Muse session for independent review and return its findings to the implementation owner. Older model names below and in historical handoffs are superseded. Muse currently uses file tools only because its embedded shell sandbox launcher fails; the coordinator executes and records Cargo gates through working tools. Keep sandbox enforcement enabled and never equate authored tests with passing tests.

## 1. Establish the current scope

- Read applicable AGENTS.md and skills. Use TokenSave first for source research and check index freshness; do not trust stale graph results over current source.
- Inspect current implementation and later review evidence before repeating historical fixes. Record a small feature table: implemented, confirmed gap, comparison dimension, validation gap, and evidence.
- Use the assignment's existing audit issue when applicable; create a scoped issue before planning newly discovered production changes. Do not invent missing features from comparison headings alone.
- For parity claims, cite current primary documentation and define the supported use cases. A plugin need not copy every feature of every competing product. Record an explicit disposition for each AUDIT item.
- Preserve MIDI/IAMF exclusions, existing Cargo minor versions, unrelated dirty files and external dependency pins. Do not broaden manager protocol or unequal-branch queue changes beyond the separately agreed scope.

## 2. Compatibility and signal contract

- Preserve legacy defaults, parameter identifiers/addresses and old saved-state/audio fixtures. Append new metadata where compatibility requires it; do not reorder old parameters. Define migration versions and reject malformed state transactionally.
- State supported rates, channel counts/roles, sample precision, latency, variable output-count behavior, overwrite semantics, sidechain layout and unsupported configurations.
- Define parameter ranges and units, automation/smoothing, bypass, reset, sample-rate/layout changes and end-of-stream behavior. Rejected changes retain the accepted configuration and populated history.
- Trace each setting through DSP, registry/schema, getters/setters, factory, engine conversion, bridge/FFI, supported native wrappers, UI and saved reload. Mark genuinely unsupported surfaces with a reason rather than silently omitting them.

## 3. Accuracy and complete audio chains

- Use an independent equation, implementation or frozen external reference. Specify numerical tolerances before inspecting candidate errors, using published contracts and reference precision. Report maximum error and the case producing it.
- Do not fit gain, timing or phase after the fact to hide errors. Only apply alignment explicitly required by the documented latency contract. Do not weaken tolerances or regenerate frozen expected output from the implementation under test.
- Cover applicable rates (typically 44.1/48/96/192 kHz), supported mono/stereo/multichannel layouts, silence, nonzero impulses/tones/noise, parameter boundaries and odd/partial/large blocks. A deliberately 48 kHz-only model needs explicit admission/resampling tests rather than invented support.
- Check long-stream and final-tail sample accounting, channel identity, stable finite output, latency and reset/reconfiguration history. Exercise randomized block partitioning against equivalent continuous processing.
- Run the assignment's actual full chain with nonzero audio, parameter changes, save/reload and rejection/recovery. Direct DSP or metadata tests alone do not prove application or host integration.
- For native changes, load freshly built CLAP/VST3 binaries and test real host callbacks/audio; macro expansion and helper tests are preliminary evidence. Test native feature selections separately where exports conflict. AU/macOS/iOS and packaged-runtime claims require their actual platform lane.
- Missing corpus/model/platform access stays an open limitation. Skipped tests and absent fixtures are not passing accuracy evidence. Preserve data provenance and applicable fixture permissions.

## 4. Realtime and lifecycle

- Measure cold and warmed process, drain and reset paths for allocation/free, locks, I/O and bounded work. Include first use, automation, queued changes, destruction/reclamation and failed restores where relevant.
- Document which thread prepares resources and commits state. Moving allocation outside ordinary process does not prove state restore is safe on an audio thread.
- Benchmark representative rates/layouts/block sizes against a recorded baseline with machine/build details. Explain CPU/memory tradeoffs; avoid an unqualified speed claim from one short run.

## 5. Execution and review

1. Claim one assignment and its exclusive files. Use a separate worktree/target for independent work; transfer the dirty checkpoint deliberately, because a fresh worktree from HEAD will not include uncommitted fixes. Do not reset or clean the shared checkout.
2. Use **Luna xhigh** for implementation. Coordinate shared changes with their designated owner; send concrete patches and adapter requirements.
3. Run focused package tests and necessary QA scenarios first, then affected adapter/engine tests. Consult manifest features and CLI arguments rather than treating a compiled QA binary as an executed diagnostic.
4. Use **Astra medium** for independent source, compatibility, numerical and integration review. Return findings to Luna, fix them and rerun affected gates until requirements pass. Record unresolved external/platform gates explicitly.
5. Update the assignment checkboxes with source links, exact commands, raw logs, measured errors, review results and remaining work. The integrator runs combined gates after shared patches land.

For Cargo commands in the current shared checkout, use the established lock and environment (replace the test arguments for the assigned package):

```bash
flock /tmp/sotf-daw-audit-cargo.lock env \
  TMPDIR=/tmp CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target \
  cargo test --offline --locked -p sotf-plugin-gain --lib --tests
```

Record exit codes and full raw output. Identify exact source/artifact hashes where results depend on a frozen candidate; a post-run hash is not a pre/post source seal. Avoid simultaneous edits to files being compiled. Keep evidence summaries honest about coverage and source freshness.
