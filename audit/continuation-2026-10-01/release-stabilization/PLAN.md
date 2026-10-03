# Stable merge plan

1. Freeze new feature assignments. Current live Muse work gets a checkpoint handoff; no further feature expansion.
2. Restore Declick's last passing detector behavior; preserve rejected candidate evidence outside active regression gates. Keep original compatibility tests and actual release invariants green. Defer new accuracy enhancements where clean separation is possible.
3. Finish or park the current AB and Denoiser changes according to actual build/tests and compatibility. Favor the last validated implementation if new changes remain unstable. No new preservation mechanisms or native/platform expansion.
4. Run broad workspace check/clippy/tests plus focused FFI and affected plugin/engine QA. Fix actual regressions; document incomplete audit probes in the deferred backlog, not as completed features.
5. Prepare scoped commit(s), exact gate summary, remaining risks and sibling integration handoff. Preserve unrelated changes. No Git push until separately requested.

Current scope definition is this user-requested stable checkpoint. Historical original74feature IDs remain tracked in requirements; their unfinished items are deferred, not erased.
