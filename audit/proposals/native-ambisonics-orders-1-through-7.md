# AUD-135 proposal: packaged native Ambisonics orders 1 through 7

Status: design accepted by Astra on 2026-09-29. The pre-edit route and default
native waveform baselines are captured; native production implementation is
starting. This proposal continues accepted AUD133; it does not claim complete
higher-order parity or close custom-layout/native-feature gaps.

## Confirmed gap and intended route

AUD133 extended the DSP, engine admission and app graph model to 64-channel
order 7. Its 64-channel AIFF route and engine output oracle are accepted, but
they do not prove that the packaged CLAP/VST3 wrappers, the C ABI, or SOTF's
external-plugin host can configure and deliver an ACN/SN3D stream. The
source-only audit in [`current-feature-route-coverage.md`](../current-feature-route-coverage.md)
records the present adapter limitations.

The proposed feature is a user-selectable native route for orders 1–7 with
every existing named decoder output layout on formats that can represent its
channel roles. These outputs are the eight `target_layout` choices `5.1`, `7.1`, `5.1.2`, `5.1.4`, `7.1.2`,
`7.1.4`, `9.1.4`, and `9.1.6`, with respectively 6, 8, 8, 10, 10, 12,
14, and 16 output channels, including the required 64-input to 16-output
route. It must preserve the existing CLAP plugin ID, VST3 class ID, legacy
first port-configuration identity, and serialized `order` / `target_layout`
parameter identifiers. New arbitrary/custom speaker layouts and unrelated NIH
wrapper protocols remain outside this batch.

The format availability is explicit: bridge, C ABI, VST3 and SOTF's own
speaker-layout route cover all 56 order/output tuples. Standard CLAP surround
metadata can faithfully map the six targets without wide speakers. It cannot
map `WideLeft` / `WideRight` in 9.1.4 or 9.1.6: CLAP's role list contains
front-left/right-of-center, which are different physical positions. Those two
outputs must not be advertised as standard CLAP surround configurations or
silently remapped. The CLAP adapter therefore exposes 42 order/output tuples
(seven orders × six representable outputs) and returns an explicit unsupported
configuration result for the two wide targets. The host UI filters choices by
the selected format and explains this limitation. Support for wide roles in
CLAP requires a separately reviewed standard or explicit SOTF-specific
channel-map contract.

The user selects input order and output target through the host's visible,
deactivated audio-configuration/setup controls. That typed per-instance setup
is authoritative for this route. A user selection updates the typed setup and
the plugin's serialized structural `order` / `target_layout` values together.
On restore, a saved typed setup and its saved structural parameter values must
agree; otherwise activation returns a clear error without truncating or
reinterpreting channels. The legacy order-1-to-5.1 configuration remains ID 0
and retains its behavior. Other supported order/output tuples get deterministic
appended IDs. CLAP has 42 advertised configurations because the two wide
outputs have no exact standard role map; VST3 negotiates all 56 tuples as exact
typed input/output arrangements.

## Whole-vector port contract

Each selected Ambisonics input is one ordered signal vector of exactly
`(order + 1)^2` channels: 4, 9, 16, 25, 36, 49, or 64 channels. The wrapper
must not describe a 16-channel main bus plus an unrelated 9–48-channel surplus
bus as one complete Ambisonics signal. A partial bus does not encode its ACN
offset in either standard's metadata.

NIH's current `BufferManager` requires the main input width not exceed the main
output width and its in-place `Buffer` contains the output-width slices. The
first implementation candidate is one full-width NIH auxiliary input bus and
a main output bus sized to the selected target. The auxiliary bus is an
internal storage detail: external hosts must see one primary/program
Ambisonics input bus, never a sidechain, with the complete ACN 0–(N−1) vector.
This candidate is acceptable only if all of the following hold: CLAP reports
the input as its main/program port (`has_main_input` and main-port flags
consistent with a program input), the CLAP Ambisonics extension returns
ACN/SN3D for that port, VST3 exposes the exact standard Ambisonics input
arrangement on a `kMain` program bus (not `kAux`), SOTF's host does not remap
it as a sidechain, and prepared callback buffers are alias-safe while
preserving every channel.
The process test must send nonzero, distinct signals on all input channels
through the full stream, not only check metadata. If NIH or an external
wrapper cannot expose the internal auxiliary storage with those public
semantics, stop using that candidate and implement the smallest NIH buffer
change that provides a full-width primary/program input while keeping
main-output-sized in-place storage safe. Do not split the HOA vector into
multiple buses or publish an incomplete ACN range.

## Format and host requirements

### CLAP

- Keep configuration ID 0 as the legacy order-1-to-5.1 tuple. Append the 41
  other standard-surround-compatible order/output combinations in a
  deterministic table. Equal-width outputs such as 7.1 versus 5.1.2 and 5.1.4
  versus 7.1.2 remain distinct named configurations; width alone must not
  select the target layout. Do not advertise 9.1.4 or 9.1.6 as CLAP surround:
  their exact SOTF wide roles are absent from the standard channel enum.
- Identify the complete primary input port as `ambisonic`. Implement the CLAP
  Ambisonics extension and report ACN ordering and SN3D normalization for the
  whole input port. Confirm that main/program flags, config summary and bus
  data all agree with the single full-vector semantics. Reject unsupported
  metadata configurations and invalid port indices.
- Implement `clap.surround/4` for output maps whose every role has an exact
  CLAP enum. Return the ordered channel map for the main output port at index
  0, and return `is_channel_mask_supported` only for the exact advertised
  masks. The six maps are frozen in
  [`native-speaker-role-mapping.md`](../native-speaker-role-mapping.md) and
  independently checked against physical-role fixtures. For 9.1.4 / 9.1.6,
  report no standard surround map and reject a request that asks the adapter
  to claim one. Never substitute FLC/FRC for SOTF's ±60° wide roles.
- Set the main/program output port to index 0 and `in_place_pair` to
  `CLAP_INVALID_ID` unless the adapter proves that its prepared input/output
  buffers safely support unequal-width aliasing for every advertised tuple.
  Do not infer the in-place contract from the common 4→6 case.
- Selecting a configuration remains a deactivated-only operation. Rebuild the
  decoder on the host's normal reactivation path and keep the persisted
  `order` value synchronized with the selected config. Preserve all existing
  IDs and old-state defaults.

The CLAP Ambisonics extension exposes ordering and normalization, with the
host notification restricted to deactivated state. CLAP's audio-port-config
extension explicitly presents configurations as user-menu choices and also
restricts selection to deactivated state. Its separate
[`clap.surround/4` header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/surround.h)
defines the standard physical-role map and contains no wide-left/right role.
See also the [official Ambisonics
header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/ambisonic.h)
and [audio-ports-config header](https://raw.githubusercontent.com/free-audio/clap/main/include/clap/ext/audio-ports-config.h).

SOTF's CLAP host must also select the chosen audio-port configuration and
validate the Ambisonics extension before activation. The current
`clap_backend.rs::initialize_instance` queries channel counts and activates
directly; it does not select a config or validate typed Ambisonics metadata.
The setup record chooses the stable CLAP config ID while deactivated, then the
host checks the selected port's exact ACN/SN3D metadata and widths before
activation.

### VST3

- Map the whole input bus to the official ACN/SN3D arrangement for its selected
  order and the output bus to the exact selected named speaker arrangement.
  Orders 1–4 and 5–7 use different input speaker-bit assignments; do not
  construct either input or output arrangements from popcounts or
  `(1 << channels) - 1`.
- Match the exact input and output arrangements during deactivated
  `set_bus_arrangements`, rather than accepting any same-width layout. Preserve
  all eight current target layouts through 9.1.6 and the order-1/5.1 default.
- Return only exact arrangements from `get_bus_arrangement`. Invalid bus
  direction, negative indices and out-of-range indices must fail rather than
  alias bus zero.

VST3 defines ACN/SN3D arrangements through seventh order. Its 3.7.8 notes that
orders 5–7 change speaker-bit compatibility relative to orders 1–4 and provide
conversion helpers; see [Speaker Arrangements](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__speakerArrangements.html)
and the [VST3 3.7.8 notes](https://steinbergmedia.github.io/vst3_dev_portal/pages/Versions/Version%2B3.7.8.html).

### SOTF's external-plugin host

The current CLAP host path also needs tuple selection: `clap_backend.rs`
initialization queries audio widths and proceeds to activation without
selecting a CLAP audio-port configuration or validating Ambisonics metadata.
It must consume a present per-instance `NativePluginAudioSetup` while
deactivated, select the matching stable configuration, verify exact widths,
ACN/SN3D input metadata and the six exact CLAP surround output maps, then
activate. For 9.1.4 / 9.1.6, filter the CLAP choice and return a clear
format-specific error if stale state requests it. Do not treat successful
width discovery or a configuration name as format negotiation.

The current `sotf-host/src/external_plugin/vst3_backend.rs` initialization asks
`speaker_arrangement(channels)` for an arrangement, then calls
`set_bus_arrangements` before activation. Its count-only helper maps 4 inputs
to quad and does not represent the 9/16/25/36/49/64-channel Ambisonics
arrangements. The typed per-instance request supplies exact input order and
target output; channel count alone cannot distinguish first-order Ambisonics
from quad. Do not special-case plugin names or reinterpret all four-channel
inputs as Ambisonics. The host must set exact arrangements before activation
and reject stale, unsupported or mismatched setup.

Typed negotiation also needs an explicit output-channel permutation for every
existing named target. The decoder's indices come from its canonical SOTF
speaker layout, CLAP surround maps encode exact roles for six targets, and
VST3 arrangements communicate speaker-bit sets; equal channel counts do not
imply equal speaker order. Maintain the mapping from each named target to its
VST3 arrangement and the permutation between VST3 channel order and SOTF
decoder output indices. For the two wide targets, VST3 has explicit W
arrangements while standard CLAP has no wide role. Persist the selected named
target in per-instance state; never infer 7.1 from eight channels when 5.1.2
is selected, or 5.1.4 from ten channels when 7.1.2 is selected.

Freeze this typed output map as a table rather than scattering count switches:

| Target layout | Channels | SOTF role order | CLAP surround map | VST3 arrangement / mask | VST3 bus index → SOTF index |
|---|---:|---|---|---|---|
| 5.1 | 6 | See reviewed mapping artifact | Exact standard map | Exact official 5.1 mask | Reviewed permutation |
| 7.1 | 8 | See reviewed mapping artifact | Exact standard map | Exact official 7.1 mask | Reviewed permutation |
| 5.1.2 | 8 | See reviewed mapping artifact | Exact standard map | Exact official 5.1.2 mask | Reviewed permutation |
| 5.1.4 | 10 | See reviewed mapping artifact | Exact standard map | Exact official 5.1.4 mask | Reviewed permutation |
| 7.1.2 | 10 | See reviewed mapping artifact | Exact standard map | Exact official 7.1.2 mask | Reviewed permutation |
| 7.1.4 | 12 | See reviewed mapping artifact | Exact standard map | Exact official 7.1.4 mask | Reviewed permutation |
| 9.1.4 | 14 | See reviewed mapping artifact | Unsupported: wide roles absent | Exact `k91_4_W` mask | Reviewed permutation |
| 9.1.6 | 16 | See reviewed mapping artifact | Unsupported: wide roles absent | Exact `k91_6_W` mask | Reviewed permutation |

The source/specification table with exact role arrays, CLAP maps, VST3 masks
and permutations is [`native-speaker-role-mapping.md`](../native-speaker-role-mapping.md).
It is the reviewed design input; implementation tests must compare it with
independent role/channel fixtures rather than copying its expected values.
Never infer or relabel a target from its channel count.

The setup control must be visible and explain that changing either order or
target output causes host deactivation, bus reconfiguration, and plugin
reactivation. It must be mounted in the SOTF external-plugin rack/settings
surface and drive the actual loaded instance, not only a hidden field or unit
fixture. Preserve existing graph/plugin configuration IDs and defaults. On
reactivation, the host passes the selected typed input/output tuple, creates
the matching decoder before processing, and reports a safe error for stale or
mismatched state.

Per-instance setup belongs to host-owned persisted plugin-instance state, not
the scanner-wide `PluginDescriptor`. Extend `ExternalPluginState` with a
serde-defaulted `audio_setup: Option<NativePluginAudioSetup>`; the setup enum
contains `Ambisonics { order, target_layout }`. `None` remains the default for
every generic third-party plugin and means no host override. Only the
recognized Ambisonics plugin, identified by exact format plus stable CLAP
plugin ID or VST3 class ID and a verified capability query, may accept this
setup. Never use display names, paths or fuzzy identity heuristics. The
Ambisonics adapter preserves the existing legacy no-setup default
order-1/5.1 tuple. A legacy state may be migrated only when its stable
identity and stored structural values establish a supported tuple; otherwise
keep the legacy default or return a specific incompatible-state error for an
unrecognized non-default value. Do not reinterpret an arbitrary external
plugin as Ambisonics.

`PluginSettings::External` already stores `ExternalPluginState`, and the
engine converter carries it into plugin configuration. The visible per-plugin
setup control writes `audio_setup` and the serialized structural `order` /
`target_layout` values in the same plugin configuration. For a user change,
the selected setup is authoritative: stage the structural parameter values
with the new setup and re-apply them after opaque state restoration, before
backend activation. For a persisted/imported record that already has both
fields, validate equality; reject inconsistencies rather than silently
overriding saved data. Opaque non-structural plugin state remains preserved.

Do not change scan-wide descriptor metadata to represent a particular
instance's selected tuple. `PluginSettings::External::required_input_channels`
currently reads `state.descriptor.audio_inputs`; the per-instance setup must
instead supply the effective `(order + 1)^2` graph input width, while retaining
descriptor counts and descriptor/state equality checks as scan facts. The
external-plugin factory/backend construction path must consume the same typed
setup for its effective input and output widths. The 64→16 test must therefore
pass through `PluginSettings` conversion and the mounted host graph, not just
construct a backend with manually supplied widths.

Use the actual existing replacement path instead of adding a new host API:
the setup control updates the selected rack/graph plugin configuration and
calls `Player::update_plugins` or `Player::update_plugin_graph`. For the linear
rack, `Player::update_plugins` delegates to
`AudioEngineManager::update_plugin_chain`, then the manager's
`UpdatePluginChainCommand` reaches `apply_plugin_update`. That path builds a
candidate host on a worker, prepares a `PreparedHostUpdate`, and commits it
through the processing-thread host-swap acknowledgment. `Player` may return
`Ok` when no engine is running, so that result alone is not evidence that the
selected setup reached an active instance. The UI must show the choice as
configured/pending until it observes the matching active instance and applied
tuple; it must not label it applied merely because the settings call returned
`Ok`. Graph selection uses the corresponding
`apply_plugin_graph_update` candidate path.

Keep commit stages distinct. A setup validation, negotiation, activation or
candidate-build failure before host swap leaves the old active host and old
applied setup in place; preserve the rejected desired setup as unapplied and
show its error. In `apply_plugin_update_once`, however, the host swap can be
committed before a later `playback.reconfigure` failure; that returns an error
with the new host committed and playback stopped. This batch does not add
rollback or broaden the manager protocol. Surface that post-commit failure as
the new host/configuration active with playback stopped, and keep the saved
rack/graph setup consistent with the actually committed instance. If the
existing result path cannot distinguish pre-commit rejection from a
post-commit reconfigure failure, show an explicit unknown/safe state and
reconcile from the active host before claiming either old or new setup. The
persisted desired setting and the active/applied tuple must never silently
diverge.

One narrow failure rule is required: the general linear builder can record
some plugin creation failures as warnings and skip those plugins. When the
changed instance is the selected Ambisonics native plugin, setup validation,
format negotiation or activation failure must be promoted to candidate
failure before host commit; it must never commit a chain that silently omits
the selected instance. Implement this as a target-specific build-diagnostic
policy in the existing candidate route, not as a broad manager/transport
rewrite. Report the new setup as applied only after observing the committed
active instance/tuple; report no-engine, pre-commit rejection, post-commit
playback failure, and unclassifiable outcomes distinctly.

The same optional setup travels through `IsolatedExternalPluginConfig` and the
external-plugin worker's initial state/reconstruction route. The mounted
control must select and verify both in-process and isolated paths before the
feature is called available; an in-process-only test is insufficient.

## Required implementation surface

The exact owner split is subject to root coordination after design acceptance.
Expected bounded files/areas are:

- `crates/sotf-plugins/crates/plugins-bridge/src/factory.rs`: verify widths
  4/9/16/25/36/49/64 and all eight existing named outputs through 9.1.6,
  including 64→16; reject mismatch while preserving config IDs/defaults.
- `crates/sotf-plugins/crates/plugins-ffi/src/plugin_factory.rs` and FFI state
  tests: create/process 64→16 and other current output widths, persist/restore
  order 7 / 9.1.6, retain order 1 / 5.1 defaults, and reject invalid tuples.
- `crates/sotf-plugins/crates/plugins-nih/src/wrapper.rs` and
  `params/configuration.rs`: order-to-layout construction, typed full-vector
  bus layout, state/layout agreement and host reactivation. Keep plugin IDs and
  parameter IDs unchanged.
- `crates/3rdparties/nih-plug/src/wrapper/clap/wrapper.rs`: only the bounded
  typed Ambisonics port/extension additions needed by this plugin; generic
  configs for other plugins remain unchanged.
- `crates/3rdparties/nih-plug/src/wrapper/vst3/wrapper.rs`: exact order maps,
  arrangements and bus-index validation without changing other plugins'
  accepted layouts.
- `crates/sotf-plugins/crates/sotf-host/src/external_plugin/vst3_backend.rs`,
  `clap_backend.rs`, `external_plugin_state.rs`, `external_plugin.rs`,
  `load.rs`, isolated worker setup, and the per-instance graph/preset owner:
  persist and pass typed input order plus output target, negotiate before
  activation, and avoid count-only layout inference.
- `crates/sotf-engine/src/plugins/plugin_settings.rs` and
  `plugin_config_converter.rs`: preserve the optional per-instance setup and
  derive effective graph input/output widths from it without mutating scanner
  descriptor counts or weakening descriptor/state consistency validation.
- The existing `Player::update_plugins` / `update_plugin_graph` route and
  engine candidate builder/swap: carry the user's updated config through the
  processing-thread acknowledgment; make a selected Ambisonics instance's
  setup/backend failure fatal to that candidate while retaining the prior
  active host and saved settings.
- The mounted external-plugin rack/settings component and its graph command
  route: expose choices supported by the selected plugin format, show applied
  setup/failure, and invoke the per-instance candidate rebuild for in-process
  and isolated loading. Keep every existing named output choice available in
  VST3 and the SOTF-managed path; CLAP hides and rejects the two wide layouts
  because its standard surround metadata cannot represent them.

No broad host transition/queue rewrite is authorized by this proposal.

## Acceptance evidence

1. Bridge tests cover every order 1–7 and each existing target layout, all
   56 input/output tuples, input widths 4/9/16/25/36/49/64, output widths
   6/8/10/12/14/16, and 64→16.
   Use distinct ordered signals on every input and compare complete outputs
   against the accepted AUD133 decoder oracle. Reject mismatched input/output
   tuples.
2. FFI tests cover all 56 tuples, create/process/save/restore order 7 / 9.1.6
   via the actual public C ABI, verify all 64 inputs reach all 16 output
   channels, retain legacy order-1 / 5.1 defaults, and reject malformed
   tuples.
3. CLAP tests enumerate the stable IDs/names for the 42 standard-surround
   order/output tuples, confirm unchanged ID-0 behavior, and verify each
   primary full input port reports exact width and ACN/SN3D. Assert main/program
   flags, config summary, input metadata, output channel maps and bus samples
   agree; preserve distinct named configurations for equal-width outputs.
   Reject 9.1.4 / 9.1.6 as standard surround, reject bad indices, and show
   selection takes effect only through deactivation/reactivation.
4. VST3 tests assert exact official ACN/SN3D arrangements for all seven input
   orders and exact output masks plus channel permutations for all eight
   targets. Cover order-1 versus quad, order-5–7 special masks, equal-width
   target disambiguation, exact arrangement matching, invalid directions,
   negative/out-of-range indices, and preserved defaults.
5. NIH process tests pass distinct, ordered input impulses on channels 0–63
   through the whole-vector bus for orders 1–7 and compare the rendered output
   against direct bridge/plugin processing. Include rejected partial-bus,
   missing-bus, reordered-bus, reset/reconfigure and state-restore controls.
   Guard the audio callback for allocations after buffers are prepared.
6. A mounted SOTF rack test loads actual packaged CLAP and VST3 instances,
   selects format-supported order/output setups, observes the per-instance
   request and matching active instance/tuple, then rebuilds/reactivates. It
   covers no-engine configured/pending status, pre-commit candidate rejection
   preserving the old active host/setup, and a post-commit playback error
   without claiming rollback; any outcome that cannot classify commit state
   remains visibly unknown until reconciled.
   CLAP visibly disables wide output choices; VST3 selects order 7 / 9.1.6.
   Process distinct signals across all 64 ACN channels to 16 speaker outputs;
   compare the full waveform and speaker order against an independent
   reference. Verify effective engine graph widths without changing the scan
   descriptor. Preserve saved/active consistency for a rejected candidate and
   reconcile post-commit playback failure to the committed setup. Save/reload
   order 7 / 9.1.6 and prove a pre-feature order-1 / 5.1 project still
   activates. Run both in-process and isolated loading.
7. Strict scoped Clippy, formatting and focused package tests pass. A
   coordinated MIDI/IAMF-excluded workspace gate is run once the shared source
   snapshot is stable; do not repeat it for Markdown-only changes. Record
   pre-edit source and actual audio baselines and compare accepted order 1–3
   outputs bit-for-bit.

The current host cannot supply every bit arrangement from channel count alone,
and neither wrapper currently proves a typed higher-order host route. This
proposal records these constraints without inferring behavior from plugin
names, saved hidden fields or raw bus width. Astra accepted this design; the
existing in-session authorization covers implementation.

## Pre-edit route and waveform baseline

Before changing native production code, the two public plugin routes and the
old NIH default were captured from source manifest
`/tmp/sotf-aud135-preedit-route-start.sha256`. The independently regenerated
end manifest is `/tmp/sotf-aud135-preedit-route-end.sha256`; each lists 49
files and hashes to aggregate SHA-256
`3f00a2911cf912462b8334a9baa726158cb7b965c06b4a759489737d9241ec4f`. The
DAW `Cargo.lock` entry is SHA-256
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.

- `cargo test --offline --locked -p plugins-bridge --lib ambisonics_bridge_processes_all_56_ordered_input_output_tuples -- --nocapture`
  passed 1/1. It processes 257 frames with distinct ordered values on all
  channels across each of the 56 order/output tuples, checking complete finite
  output and whole-call versus partitioned equivalence.
- `cargo test --offline --locked -p plugins-ffi --lib ambisonics_ffi_process_matches_bridge_for_all_56_ordered_tuples -- --nocapture`
  passed 1/1 through the public C ABI. The full result vectors match the bridge
  route bit-for-bit for all 56 tuples. The saved 605,040-byte artifact is
  `crates/sotf-plugins/target/audit-artifacts/aud135-preedit/ambisonics-ffi-all-56.f32le`,
  SHA-256 `52e9b4f5d7ffd02f28fca98d79dfa5c7d79115411e6932e47d695f697dd172c7`.
  Despite the suffix, it is a tagged container: each of 56 records has a
  `<BBHHI` header (order, output-layout id, input width, output width, frame
  count), followed by its 257-frame interleaved f32 output. The capture was
  parsed through the final byte; all vectors are finite and nonzero. The
  bridge parity is a route regression, not an independent decoder-accuracy
  oracle; that numerical evidence belongs to AUD133.
- `cargo test --offline --locked -p plugins-nih --no-default-features --features ambisonics capture_aud135_pre_edit_ambisonics_native_default_waveform -- --ignored --nocapture`
  passed 1/1. This captured the pre-edit NIH default wrapper at 4 inputs → 6
  outputs for 257 deterministic frames. Its 6,168-byte raw interleaved f32le
  output is
  `crates/sotf-plugins/target/audit-artifacts/aud135-preedit/ambisonics-nih-default-order1-5.1.f32le`,
  SHA-256 `6d8cab6f11e304014a35a9a9aa98506082ae504a86f028182d37250ae099e228`.

Logs are `/tmp/sotf-aud135-bridge-all56-preedit.log` (SHA-256
`15ad7ac7b058f5febe99d9cd8fa7dbbe0845b51837fd325ffd4771cfbf13371d`),
`/tmp/sotf-aud135-ffi-all56-preedit.log` (SHA-256
`4070ae0ea7162ae3ce02bf3ab6425a7b67a53000a911322e163de1a844ab755b`), and
`/tmp/sotf-aud135-nih-default-preedit-final.log` (SHA-256
`59892754313a19e2164dc10352df82157e7b282d5196a02c00c5aa565bfc512f`). An
earlier NIH invocation selected zero tests due to an incorrect module-path
filter; it is not counted as a pass and has been rerun with the matching test
name above.
