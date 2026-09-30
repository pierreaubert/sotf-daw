# AUD143 frozen native buffer and consuming-host review

Review with Astra medium after an implementation checkpoint releases a slot.
User authorization already covers this independent review; no approval question
is needed. Production source is under subsequent AUD135/AUD139 edits, so review
the frozen packet and its explicitly bounded historical results.

## Packet

`audit/artifacts/aud143-native-buffer-reuse-r1/` contains the archived callback,
buffer-manager and loaded-route source plus the selected-source manifest, logs
and README. Index SHA:
`33c0b4ec74c26ea9a71e2aa3bd5ad838dc33ebecdcb4c579585062f8839eb331`.
Root reverified all indexed entries when preparing this handoff.

## Acceptance questions

1. Does the actual VST3 absent→present→absent callback probe demonstrate that
   missing inactive storage clears stale sample counts/pointers, preserves
   declared channel slots, and never dereferences omitted storage? Check
   callback extents, canaries and complete provided-buffer silence.
2. Do the unchanged allocation-guarded prefix/malformed-width cases and the
   refreshed five-test BandSplit callback gate retain their existing contracts?
3. Does the five-test packaged CLAP/VST3 consuming-host gate prove all-band
   delivery for 2/3/4 bands, LR24/LR48 and both modes using full public-DSP and
   DawHost vectors, genuine state reload and late refusal/history/retry?
4. Are provenance and gate boundaries stated accurately? The packet records
   114 NIH tests, one ignored manual utility, strict all-target lint, and matching
   selected manifests for the loaded route. This is not a complete transitive
   build binding, native AU proof, or a current whole-worktree integration pass.

The workspace callback probe substitutes for the unexecuted standalone NIH
unit target, whose offline git dependency resolution failed. Its reuse case is
not allocation guarded; the unchanged prefix cases are. Keep those distinctions.
Previously accepted independent filter response, core lifecycle, migration,
UI and CPU evidence remains separate. Do not infer mathematical accuracy from
the native route's separately constructed production DSP reference.

Append scoped acceptance or actionable findings to `audit/reviews/AUD143-astra.md`.
Request Luna corrections if needed. No broad Cargo gate or production edits are
required merely to review this frozen checkpoint. Subsequent shared VST3/host
changes still need a coherent combined integration gate.
