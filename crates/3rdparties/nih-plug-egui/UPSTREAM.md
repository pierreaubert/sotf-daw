# `nih_plug_egui` provenance

This adapter is copied from `https://github.com/robbert-vdh/nih-plug` at
commit `28b149ec4d62757d0b448809148a0c3ca6e09a95`, directory
`nih_plug_egui/`. The original ISC license is retained in `LICENSE`.

Local integration changes:

- `Cargo.toml` points `nih_plug` at this workspace's single vendored source
  (`crates/3rdparties/nih-plug`) and keeps `baseview` and `egui-baseview` at
  the upstream pinned Git revisions.
- The package is an excluded standalone workspace so Cargo does not discover
  a nested workspace inside the DAW workspace member tree.
- A `f32` suffix was added to one stroke-width literal for current compiler
  compatibility; adapter behavior is otherwise upstream.
