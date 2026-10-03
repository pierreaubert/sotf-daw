# Release stabilization checkpoint

User request 2026-10-03 supersedes completing every audit feature before integration: reach a stable sotf-daw merge point, defer remaining tasks so sibling integration and release can proceed.

Acceptance: preserve accepted functionality; remove or restore experimental changes causing existing regressions; workspace build/lint and meaningful broad tests green; focused host/engine/native/FFI gates appropriate to touched shared behavior; record platform/fixture exclusions explicitly. Prepare a reviewable commit and sibling integration notes. Do not include unrelated user edits or generated build/event payloads. No requirement silently marked complete by deferral.
