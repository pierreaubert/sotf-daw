# Gate mode external wiring checkpoint

## Implemented scope

The engine, facade, bridge, C API and generated NIH wrapper now preserve Gate's Downward, Upward and Duck modes for internal-key processing. The DSP implementation is owned by the spectral agent. The engine exposes the typed `GateMode`, appends structural mode at parameter index 15 and realtime `max_boost_db` at index 16, and preserves every existing index. Old JSON presets without these fields decode as Downward with a 12 dB maximum boost. The facade exports `GateMode`; the Gate fuzzer explores all three modes and boost bounds.

NIH and C roundtrip fixtures load modes through their state lifecycle, verify runtime scalar types/indices, and measure independent signed plateau gains. They check restored structural mode changes using reinitialization or the existing lifecycle rejection contract. Native callback processing has the allocator guard enabled. The C test exercises both plain state and preset-document restoration APIs. Bridge original/restored waveforms match exactly over irregular blocks. Independent plateau gain expectations use f64 powers and allow 0.02 dB, covering the existing Gate fast-math approximation; this is not a looser restored-waveform equality test.

## Verification

`target/audit-gate-external-wiring.log`: focused command exited 0. Fourteen deterministic tests passed: bridge 1, C 2, NIH 4, engine 7. Eight are Gate-specific (the other six engine tests matched the textual filter). The additional hardware loopback fixture returned early because no virtual audio device was available; no hardware loopback claim follows. Spectral independently reports 87 Gate crate tests and all-target/all-feature Clippy passing.

Focused Clippy found the mechanical `chunks_exact(2)` to `as_chunks::<2>().0` fixture lint in the bridge and C tests. Equivalent iteration changes were applied after root's aggregate snapshot. The bridge rerun passed 1/1 (`target/audit-gate-bridge-final.log`) and C rerun passed 2/2 (`target/audit-gate-ffi-final.log`). Final all-target Clippy for `sotf-engine`, `plugins-bridge`, `plugins-nih`, `plugins-ffi`, and `sotf-plugins` exited 0 with warnings denied for these packages (`target/audit-gate-external-clippy.log`). The vendored NIH dependency still emits its pre-existing cfg-specific unused-import warning; this pass did not alter vendor source. `git diff --check` passes. Source and tests are frozen.

## Explicit route limits

- AUD-068: the actual generated NIH Gate layout advertises 2 main inputs, 2 outputs, and no auxiliary inputs. Internal-key upward and duck work. Restoring `sidechain_external=true` needs 4 inputs for 2 program outputs and is explicitly rejected during activation. Native external-key processing is not implemented by this checkpoint.
- AUD-070: the Rust bridge can construct an external-key Gate that correctly advertises 4 inputs and 2 outputs, but both host in-place adapter classes validate an input-width output buffer and use it as scratch. Therefore a correctly sized stereo output slice fails before DSP. Native f64 has the same shape defect. The raw Gate in-place core requires 4 interleaved lanes, with 2 program and 2 key lanes; that core convention does not change the public out-of-place output contract.
- AUD-070 C constructor follow-up: the current C factory passes total input width 4 as Gate's constructor program width, producing 8 inputs/4 outputs and rejecting the requested 4/2 shape. The C API currently supports internal-key Gate modes only.

Exact external-key bridge red fixture and failing log are preserved in `/tmp/sotf-gate-external-key-review/gate_modes_with_external_red.rs` and `red.log`. The correctly sized stereo output assertion was not changed to accept a false 4/4 public contract. This red fixture is outside the live suite, with no ignore added. No adapter, C constructor, or native auxiliary-bus production changes were made.
