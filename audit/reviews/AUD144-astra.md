# AUD144 Astra review — 2026-09-30

## Disposition

**Accepted: bounded FFI preset-envelope validation and supported legacy FletcherMunson → LoudnessCompensation migration.** This verdict applies to the sealed r3 packet's final `sources-r2/` sources and immutable library, not concurrently evolving AUD142 FFI code. No Rust edits or Cargo runs were made by the reviewer. TokenSave MCP status/context and authoritative sealed source reads were used.

## Source assessment

`plugin_import_preset_json` requires integer schema version 1, the exact preset type marker and a string plugin identity compatible with the target before extracting state or invoking replacement. Invalid documents therefore return InvalidConfig without constructing/replacing the active DSP instance. Existing document/state size bounds and byte validation remain intact. Descriptive names remain editable. Raw `plugin_load_state` retains its separate partial-update contract.

The identity helper canonicalizes supported factory aliases and adds an explicitly directional legacy migration: a LoudnessCompensation target accepts FletcherMunson state. Unrelated families remain refused. Reverse migration is intentionally outside this checkpoint; its rejection is not evidence that it was historically invalid. The separate DynamicEQ full-preset defaults code is not newly accepted under AUD144.

The tests exercise genuine exported documents, aliases, wrong/missing/typed identity fields, zero/future/float versions, and the unsupported spelling case. Refused imports preserve serialized state and exact complete continuation against a populated twin; a cold control demonstrates sensitivity to accidentally clearing recursive history. Successful imports compare with fresh prepared state/audio, rather than incorrectly requiring old history to survive successful replacement. The genuine legacy migration uses nondefault values and a default-output sensitivity control.

## Executed evidence and provenance

Independently verified every checksum entry in `audit/artifacts/aud144-preset-envelope-r3/SHA256SUMS`: index SHA `1087e2b392de89ef9192fc3c57e6daf66941d0b12c570a7e11687a0986cba3bc`, **76 entries**, all matching. The report currently says 75 entries; correct that bookkeeping count without rerunning tests.

- Immutable real shared library SHA `7311b8fa7044084096bbdb219f56656673d7feef86bd8c58943a4147c36e71a8`.
- Final focused log: 3 passed. Final full FFI library: 82 passed, 0 failed, one manual utility ignored. Final strict all-target Clippy and build logs terminate successfully.
- Unchanged root-authored ctypes envelope probe: nine invalid envelopes return -8, preserve state and match the populated twin's complete 1,272-sample continuation; two valid controls pass.
- Unchanged legacy ctypes probe: genuine exported FletcherMunson → LoudnessCompensation succeeds with exact state and complete 9,464-sample audio; nondefault sensitivity is approximately 0.27917.
- Root verification receipt SHA `da61a5a1092cec59079686ddd8c114f9921ea204067a3f2590695945f1190f6a` records complete-vector verification. The probes explicitly check finite outputs and use exported C functions against the actual shared library.

The initial 340-entry build manifests match. The final formatted-source build excludes only the named concurrently edited, uncompiled host integration test from its matching 339-entry manifests. This qualification is explicit and appropriate; neither manifest establishes the entire transitive build closure. Final formatting did not change the probed library bytes. Historical r1 invalid-envelope acceptance and r2 legacy migration refusal remain preserved failures, not relabeled successes.

## Limits

No whole-workspace green result, callback allocation guarantee, AU runtime execution, generalized malformed-state guarantee or broader feature completion follows from this checkpoint. Header synchronization was skipped. Existing transaction/DSP evidence is sufficient for the bounded correction; no redundant Cargo or probe rerun is needed. Current unrelated Crossover, DynamicEQ, MIDI and other work is outside this acceptance.
