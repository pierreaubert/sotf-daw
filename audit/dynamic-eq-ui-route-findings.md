# AUD139: Dynamic EQ UI and controller route findings

Source inspection on 2026-09-29. These findings extend
`dynamic-eq-shape-gap.md`; they are not executed UI or controller tests.
The sibling `sotf` checkout is not an initialized TokenSave project, so its
current source was inspected with absolute file slices and scoped searches.
No sibling source changed during this inspection.

## Control identities and update path

All paths below are relative to `../sotf/crates/`.

- `app-gpui/components/plugins/ui_dynamic_eq.rs:21` assigns band controls
  `100 + 10 * band + local_index`. Existing local indices 0–6 represent
  frequency, Q, gain, threshold, ratio, active and solo. This is a separate
  index space from the sequential AU parameter addresses recorded in AUD139.
  A new control can use the unused local index 7 while preserving old UI IDs.
- `sotf-player/src/controllers/plugin/set.rs:194` and `adjust.rs:152` decode
  that same ten-slot band stride. They currently handle only local indices
  0–6. Both explicit values and incremental edits need the selected shape.
- `plugin_controller.rs:570` applies a setting and then calls
  `determine_update_effect`. The latter returns Structural when
  `param_index_to_engine_param` has no mapping. Dynamic EQ's band route has no
  scalar mapping; a regression already asserts this at
  `controllers/plugin_param_map.rs:360`.
- `app-gpui/ui/plugin.rs:249,291` submits Structural changes through the
  existing linear/graph `update_*_with_receipt` routes and stores
  `pending_ack`, including the submitted graph. Use the appropriate topology
  and preserve this acknowledgment flow when adding the shelf control.

The receipt completes when the underlying Player/Manager call returns
(`app-gpui/app/player_handle.rs:493–510`). It is stronger than actor admission,
but does not itself identify the active native instance or prove an applied
tuple. No-engine success and pre-/post-commit failures still need the explicit
handling in AUD135's accepted design. This observation is also relevant to the
native host worker; it does not authorize a new manager protocol.

## Source-confirmed navigation mismatch to reproduce

`controllers/plugin/misc.rs:8` returns `8 + 7 * num_bands` for Dynamic EQ.
`plugin_controller.rs:487–506` implements next/previous navigation as contiguous
indices modulo that count. For one to eight bands this is a range ending at
14–63, whereas every band control uses an ID of at least 100. Starting from a
global parameter, repeated next navigation therefore cannot select a band
control. `adjust_selected_param` forwards the resulting index directly to the
same setting handler; there is no translation in that method.

Reproduce this through a real configured `PluginController` before changing
navigation. A shelf implementation must not merely change the count from
seven to eight fields: it needs the actual selectable IDs, correct wraparound
in both directions, and coverage after changing the active band count. Keep
numeric UI IDs stable and verify the selected field really changes. This is a
source finding, not a claim that a mounted keyboard event was executed.

## Response display

`ui_dynamic_eq.rs:132` renders the static full-target response of audible bands.
Both combined and selected curves use the local `peaking_eq_response_db`
implementation at line 290. Live gain reduction only feeds a separate meter
at line 249; it is not used in either curve. The new shape must reach both
curves, including solo/active selection and the applied sample rate.

Keep a static target curve clearly identified. If displaying a live held-gain
curve, evaluate the chosen dynamic transfer; the existing dry/filtered blend
is not generally equal to a biquad redesigned at the current gain. Test
plotted values against the independent reference in AUD139, then exercise an
actual mounted shape selection, Structural request, preset/fresh-model restore
and applied processing. A changed settings object alone proves none of these
downstream steps.

## Inspected source hashes

| Source suffix | SHA-256 |
| --- | --- |
| `app-gpui/components/plugins/ui_dynamic_eq.rs` | `ac33018f26c5df95ae96050530eb9b2a814b671a35acbb3d229885ab5b714b75` |
| `sotf-player/src/controllers/plugin/set.rs` | `432c3be8433dee255be1323d6f98dd19f8190f0b251791613935a76a33926001` |
| `sotf-player/src/controllers/plugin/adjust.rs` | `e08e2ff8bc7645b6a139bea14c2cf8ff71f49c42b78f301296f11dcc402af758` |
| `sotf-player/src/controllers/plugin/misc.rs` | `b924102a1c4317384bb406e1e2e118fc2b8b321b5face6037d74f162703c1dd1` |
| `sotf-player/src/controllers/plugin/plugin_controller.rs` | `9bee04c978087e41bc7135d94484dd4b410414747f0ee2adf70cf9ebc623383e` |
| `app-gpui/ui/plugin.rs` | `d4e577f670753bbfdff71d0c07a717b2df0efec007b4d0f9a65dee6eba4616f1` |
| `app-gpui/app/player_handle.rs` | `fe45f25fc51dada5eca060cfe20565e0425b23be27c65cb953b3bc7787dfc2cc` |

## AU consumer inspection — 2026-09-30

The accepted DSP core and planned 80-entry C parameter table do not yet make
shelf selection usable in the Audio Unit. Bounded source inspection found
three additional consumer requirements; no macOS execution is claimed here.

1. **Address mapping:** `plugins-ffi/src/au_host.rs:200` renders the generic
   knob grid with absolute cache indices. Dynamic EQ uses this generic path
   (`:452`), but `PluginViewHost::set_plugin_param` and `reset_plugin_param`
   pass the index through `ui_to_au_param_index`, which adds the eight global
   parameters for band-based plugins. The current code therefore maps the
   first generic control to AU address 8 and the last controls beyond the
   table. Reproduce this through the callback path, then distinguish generic
   absolute indices from the custom EQ view's relative indices. Preserve
   existing C addresses; the 16 shelf fields append after all 64 legacy
   entries, so increasing the legacy per-band stride would rebind controls.
2. **Structural writes:** `GenericRustAudioUnit.swift:1087` builds all
   parameters as writable, infers realtime/ramp support from `steps == 0`,
   and supplies `valueStrings: nil`. Its control mailbox drain (`:730`) and
   render mailbox drain (`:1633`) both call `plugin_set_parameter` directly.
   A structural setter rejection is retired, and the render path returns
   `kAudioUnitErr_InvalidParameter`. Shape and slope need truthful update
   metadata and a prepared control-thread application path before these
   exposed controls can be claimed usable. Keep callback processing free of
   reconstruction and heap work. The current NativeSmoke ramp assertion
   compares flags with the same cached classification and does not establish
   that structural changes take effect safely.
3. **C declarations and labels:** `plugins-ffi/build.rs:28` returns before
   cbindgen or header synchronization when `SOTF_FFI_SKIP_HEADER_SYNC=1`.
   Tests using that setting do not validate declaration delivery for the new
   choice-label API. Generate and compare the FFI, AU Shared and SwiftPackage
   headers, then consume the labels in the native parameter tree. Native
   execution remains a separate platform gate from Rust C-entrypoint tests.

These are AUD139 route findings for the later native/UI stage, not new
production changes or a failure of the accepted numerical shelf core. The
Luna owner received the exact locations while implementing the FFI/engine
stage.

Inspected paths below are relative to `crates/sotf-plugins/crates/`:

| Source | SHA-256 |
| --- | --- |
| `plugins-ffi/src/au_host.rs` | `db8b63f44eb72046f1297adb3b99774b36fa37461dc34a9bac36ddc8e12849e4` |
| `plugins-ffi/build.rs` | `b3f5a33d5d903250f09044927c1adc33d45a95358db08b00ebb9cd5e77c9d354` |
| `plugins-au/GenericAU/GenericRustAudioUnit.swift` | `e9c6c8a69468a05571cbdf6b1b13c6f9c4c9a066a0e77549426194864be71160` |
| `plugins-au/DynamicEQAudioUnit/DynamicEQAudioUnit.swift` | `f50e0fe2a34c60215a0b2de3a0127d87aedde399621ccba085f59fec37207f44` |
| `plugins-au/NativeSmoke/main.swift` | `c1e6905eabd083611426cd5d587e5c3a608e0023435f54558ae9428d59a05eb2` |

## Header generation follow-up — 2026-09-30

Luna directly executed the existing FFI build-script binary against a temporary
minimal crate whose source and cbindgen configuration point to the current
checkout. Both generated headers (requested output and temporary SwiftPackage
copy) match both checked-in FFI/SwiftPackage headers byte for byte, SHA-256
`fe3e2512f366a2dc5aa742fe5474fe0e295669d07ed72b108a71c485ccc0d843`.
The temporary manifest has no dependencies or build script; this is header
generation/comparison evidence, separate from the actual package tests and
lint. No tracked header was written. The temporary AU sibling directory did
not exist, so its conditional copy was skipped. AU Shared header delivery and
native AU execution remain unverified.

## Native structural-control lifecycle — 2026-09-30

The remaining native control work needs a host lifecycle request as well as
parameter exposure. The following describes the source before the native
restart implementation, not executed live control tests:

- `plugins-nih/src/params.rs` hides non-realtime controls from the host but
  retains them in saved state. The native parameter callbacks mutate their
  atomic values; they do not themselves prepare replacement DSP.
- `plugins-nih/src/wrapper.rs:1135` detects a changed structural fingerprint,
  clears output and returns a processing error. Its comment mentions requesting
  reactivation, but that branch sends no host request. `initialize()` is the
  existing reconstruction boundary.
- Vendored NIH CLAP already has a bounded main-thread task path through
  `request_callback`. Its only current `request_restart` call is in the
  latency-change task. VST3 already has a `TriggerRestart` main-thread task.
  An explicit structural restart hook can reuse those mechanisms. A fake
  latency change would misrepresent the request.
- Reentrant restart calls must not run while holding the plugin mutex or on
  the render stack. A same-width Dynamic EQ change should retain the prepared
  DSP while its request is pending, coalesce repeated requests, and construct
  the replacement on the control thread. Validate before committing state.

The SOTF native consumer has additional gaps: `ClapBackend::process` currently
turns a restart request into an error and clears a main-thread callback request
without invoking it (`clap_backend.rs:910–918`). The inspected VST3 backend has
no component-handler/restart wiring. Consequently, exported-plugin restart
notification alone cannot prove working live controls through SOTF. Test an
actual native host request/acknowledgment cycle and the consuming SOTF control
route separately. Reuse existing control-thread candidate/state replacement
where possible; the excluded manager protocol rewrite is not authorized by
this finding.

The intermediate metadata/fingerprint gate now passes 2/2. Dynamic EQ's shape
and slope controls are visible, manual and marked for restart; their pending
changes can retain the prepared DSP while other structural changes still fail.
The additional fingerprint is evaluated on the process path only after the full
fingerprint differs and only for Dynamic EQ. Luna has now added deferred CLAP
dispatch through background and main-thread tasks, but its native lifecycle test
initially aborted under the callback allocation guard. The corrected focused
native checkpoint below now passes; consuming-host service and independent
acceptance remain open.

### Executed native callback allocation failure

The actual CLAP fixture reaches activation, then aborts with a 456-byte
allocation on its first ordinary Peak callback, before writing a shelf control.
Root reproduced this under GDB on the copied failing executable. The relevant
stack is:

```text
CLAP process → SotfDynamicEQ::process_with_api
→ DynamicParams::sync_to_plugin → DynamicEqPlugin::parametric_get_parameter
→ DynamicEqPlugin::current_values → BTreeMap::insert → AllocDisabler::alloc
```

The getter directly handled active-band controls but allocated a full snapshot
for globals, malformed IDs and dormant slots. Luna has replaced this fallback
with direct scalar reads and added a direct setter path for valid realtime
controls, avoiding the temporary parameter map for those updates. The corrected
focused gate passes 4/4. It exercises cold global/dormant/unknown reads, changing
global/per-band controls under the allocation guard, deferred native restart,
unchanged populated Peak audio before service, and full post-service audio
against both a fresh native shelf instance and a separately configured public
DynamicEqPlugin LowShelf reference. Shelf-versus-Peak sensitivity remains above
the original 1e-3 RMS threshold. A same-value echo after successful service does
not request another restart.

The executed command is `cargo test --offline --locked -p plugins-nih --features
dynamic-eq --lib dynamic_eq -- --nocapture`. Its log hashes to
`40fe7d740ba9bf8a07624924c0a1bffe1079c7ea3d8013af79fec532cc3f5f5b`
and is retained with selected source copies in
`artifacts/aud139-clap-allocation-fix-r3/`. This is a selected-source record,
not a complete build-source snapshot or Astra acceptance.

Root then found that DynamicEQ-specific assertions had been added unconditionally
to the shared scalar-getter fixture. Luna restricted them to that family; the
full module now passes 15/15. Root inspected the terminal log
`/tmp/sotf-aud139-scalar-getter-full-r1.log`, SHA-256
`16c47ac98a4e36953aa20f60a4ab41dfd402c2405ac725d78632c0e1fb4c3ce5`.
This later helper revision is distinct from the focused gate's source copy.
Ignored-host/same-value retry, failed preparation, VST3 reload, actual SOTF
consumption and broader package/lint gates remain open.

Evidence is frozen in `artifacts/aud139-native-getter-abort-r1/`; its six-file
index SHA-256 is
`fa196cbe427cbbdc5d0594ad9d3799e9293a40c97f2a2a1b6a8bded99b833d2f`.
The debugger exited successfully while the inferior received SIGABRT. Missing
split-DWARF files did not prevent symbol/source resolution of the relevant
frames. The executable was copied after the initial failure; the packet proves
the new debugger reproduction and records that it lacks an original build-bound
hash and complete historical build-source snapshot.

A bounded source inventory found 26 direct parametric implementations. The
remaining inherited getter in LinearPhaseEQ is insufficient evidence of the
same native failure: its NIH route uses `AsyncTimelinePlugin`, whose scalar
getter reads cached metadata values. This inventory is source analysis, not
an all-plugin native allocation gate. Owned-string control reads remain outside
the primitive realtime getter contract.

| Inspected source | SHA-256 |
| --- | --- |
| `plugins-nih/src/params.rs` | `30467bf2608e89c823a32ba06375eb4c624f290613580b9ed6eb597a6e876e66` |
| `plugins-nih/src/wrapper.rs` | `8268c2b26fa3b5c5a231f88eb45b7e0d45142cee242de03d4e52317b4e9969a8` |
| Vendored NIH `wrapper/clap/wrapper.rs` | `485f1449647f5c613b3e4fda622b5c8c57624861f6e1609a3bf686d0b1887592` |
| Vendored NIH `wrapper/vst3/inner.rs` | `6ea0ed02b5af1e7dbcd150c7aaf33f3040f565df25f3a69c72245cc6ca587f77` |
| `sotf-host/src/external_plugin/clap_backend.rs` | `efd3c504174f877cc48ed797a6a3427ad5ba91557efe28b8901b4e28fb6b8127` |
| `sotf-host/src/external_plugin/vst3_backend.rs` | `728921b41b53af8420ac16cdfd2bf5b455ed93629c5f084a2225af1c5e61215e` |

### Consuming-host parameter delivery follow-up

Root traced the actual SOTF parameter command as well as the restart flags:

1. `ClapBackend::query_parameters` skips hidden/read-only controls and builds
   each remaining control with `Parameter::new_float`, `new_int` or `new_bool`.
   `Vst3Backend::collect_parameters` does the same after its read-only filter.
   These constructors default to `UpdateMode::Realtime`; neither collector
   retains the native automation-permission flag or a restart requirement in
   its host parameter or binding.
2. `SetPluginParameterCommand::execute` sends `ProcessingCommand::SetParameter`.
   The processing-thread handler invokes `set_plugin_parameter_immediate` and
   acknowledges its return value and current geometry/latency.
3. The CLAP setter only enqueues a native parameter event, which reaches the
   plugin in a later process callback. The VST3 setter invokes the controller
   and also queues the processor parameter point. Neither path currently
   acknowledges a completed Dynamic EQ DSP reconstruction.

This source evidence explains why a changed value or engine acknowledgment will
be insufficient to prove that a shelf is audible through SOTF. The consuming
route must demonstrate actual applied audio, preserved metadata/state, and
control-thread lifecycle service after the native notification arrives. Keep
old prepared audio running while the request is pending. Exercise refusal and
retry as well as success, and preserve existing scalar automation behavior.
The native non-automatable flag alone is not a universal third-party rebuild
contract; a scoped route must identify the supported structural controls.
Reuse the existing candidate/update boundary and retain the exclusion of a
general manager protocol rewrite.

These are source findings, not an executed loaded-host or engine failure. The
next native integration fixture must cross the actual SOTF public setter and
audio path before claiming this gap closed. Root used bounded source reads
because TokenSave again returned `Transport closed`.

| Follow-up source | SHA-256 |
| --- | --- |
| `sotf-host/src/external_plugin/clap_backend.rs` | `efd3c504174f877cc48ed797a6a3427ad5ba91557efe28b8901b4e28fb6b8127` |
| `sotf-host/src/external_plugin/vst3_backend.rs` | `41b56cbcb96c240370273e06358f19ed29d0e674f1fbcf2a49b7abc8cab44b63` |
| `sotf-host/src/parameters.rs` | `3d429448a874d29b5115d342e3cbfc9da336fc63349e5a81e62818f0a5251cab` |
| `sotf-engine/src/engine/manager_thread/commands/set_plugin_parameter.rs` | `bf5adf299795bdf46364f8ee4ad4046e5894194101b0919cf4b0bfeba3594d69` |
| `sotf-engine/src/engine/processing_thread/processing_state.rs` | `396cf42451cfb90c864312471844b96f2e9dab731e69b0f5aa184bdab7185281` |
