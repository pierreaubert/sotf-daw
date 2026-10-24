# Local manifest/lock prerequisite repair

Locked candidate checks stopped before compilation. An isolated manifest tree
resolved a copy of the existing dirty lockfile without changing the original.
The resolver required exactly six already-local AutoEQ/RoomEQ package-version
alignments; `isolated-resolver.patch` preserves the complete change. Registry
packages and dependency lists did not change.

The six version values were then changed in the working lockfile only after
checking its original hash. All other existing dirty bytes and the staged
lockfile blob were preserved. `main-repair.json` records the hashes and exact
mapping. The user lockfile was not staged or committed as part of this repair.
The Linear-phase EQ core check and focused tests and the XTC candidate's locked
acceptance commands subsequently compiled; their DSP decisions remain separate.

The isolated full, unfiltered metadata command also tried to load an uncached
Linux accessibility crate under `--offline`. That full-metadata cache failure
is retained in `resolution.json`; it did not prevent the targeted macOS DSP
checks. It is not evidence that FFI or cross-platform builds are complete.
