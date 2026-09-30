# AUD143 native BandSplit integration findings

Inspected 2026-09-30 during implementation. Astra accepted the scoped DSP,
UI, fixture, FFI restore, and NIH migration checkpoints in
`reviews/AUD143-astra.md`. The bounded packaged CLAP/VST3 and `DawHost` route
now passes its Linux loaded-host matrix. The refreshed NIH auxiliary-buffer
reuse callback probe and route gates are preserved in
`artifacts/aud143-native-buffer-reuse-r1/`; that new probe is not yet reviewed
by Astra. Shared NIH/FFI/engine edits are coordinated with the Dynamic EQ
owner.

## Current native acceptance boundary

The current feature-enabled NIH `--lib --tests` gate passes 111 unit tests and
three native auxiliary-output integration tests, with one manual test ignored;
strict all-target Clippy passes with warnings denied. The native BandSplit
callback filter passes 5/5. In addition, a freshly built, byte-identical
CLAP/VST3 artifact pair passes the loaded `ExternalPlugin` and `DawHost` matrix
for 2/3/4 bands, LR24/LR48, both recombination modes, saved-state reload, and
late candidate refusal/retry. This is Linux loaded-package evidence; it does
not establish AU behavior on Linux or a full cross-platform package gate.

The old-state policy is resolved: a complete NIH preset missing `num_bands`
means two bands, and missing recombination mode means LegacyCascade, even
when imported into an existing three/four-band instance. Incompatible host
geometry is refused by the direct NIH callback; the old preset succeeds after
the host negotiates its compatible two-band layout. The callback module passes
5/5 in
`/tmp/sotf-aud143-nih-old-state-module-r1.log`, SHA-256
`9dca5638ffa650f57fe22b03f647f83794cb08f572766b39c4af15a75ca00c51`.
The final full-library run also executes all five callbacks after the unused
hook cleanup. The exact old two-control preset migrates after actual
three/four-band audio processing; direct incompatible import and subsequent
processing return `kResultFalse`; packed two-band renegotiation/retry succeeds
with a full block matching the LegacyCascade reference. The sparse route test
checks distinct cases: a three-band host supplies only the main output bus;
the four-band host supplies buses 0 and 3 but null channel-pointer arrays for
buses 1 and 2. Those absent outputs retain canaries. A separate successful
three-band route provides inactive bus 3 storage and verifies it is zeroed.
This is in-process VST3 callback evidence, not detached-candidate preservation
or a packaged plugin result. Astra accepted the migration review. Packaged and
consuming-host routes remain pending. FFI partial state continues to preserve
omitted current values under its separate contract.

### Current consuming-host constraints

The following table records the current implementation checkpoint and its
remaining runtime proof. Source changes and compile success do not establish a
loaded all-band route.

| Route | Current implementation checkpoint | Remaining runtime proof |
|---|---|---|
| Typed setup | `NativePluginAudioSetup::BandSplit` carries 2/3/4-band count and CLAP/VST3 output representation; saved state is validated against structural readback | Loaded CLAP/VST3 matrix passed for every supported count, slope, and mode |
| CLAP | The recognized packed one-main-port route selects 2/3/4 bands and forwards setup through detached-candidate reconfiguration | Loaded CLAP state reload, per-band distinct stereo vectors, and `DawHost` delivery passed |
| VST3 admission | The recognized BandSplit route accepts four declared output slots, negotiates stereo arrangements, and activates selected buses | Loaded VST3 admission and old packed two-band state compatibility passed |
| VST3 process | Candidate code prepares per-slot buffers/descriptors and applies the documented band-major permutation | Loaded complete-vector routing, distinct stereo, and `DawHost` delivery passed for all counts; separate callback gates cover sparse, inactive, and omitted auxiliary buses |
| Replacement | `ExternalPlugin::reconfigure_audio_setup` reconstructs a disposable candidate for BandSplit setup changes | Valid-envelope late native restore refusal preserves live state/audio against a synchronized twin; cold sensitivity and successful retry passed |

The loaded gate compares complete finite vectors against a separately
constructed public BandSplit DSP composition. It enforces a maximum absolute
sample residual of `2e-5`; passing residual values are not printed. DSP
coefficient/complex-response acceptance remains separately covered by the
AUD143 core tests.

Do not infer all-band SOTF delivery from wrapper callbacks alone or disguise
bands as surround speaker positions to satisfy the one-bus host. The loaded
CLAP/VST3 and `DawHost` matrix now executes the two/three/four-band waveform,
save/reload, and failure-continuation cases. Preserve existing single-bus and
accepted Ambisonics behavior. General manager and unequal-branch queue rewrites
are outside this bounded route work.

## Parameter and constructor contracts

The expanded static schema exposes band count as choice indices 0–2, while
`BandSplitPluginParams` uses counts 2–4. Recombination mode is also a choice
index in the schema, while its constructor enum uses canonical snake-case
strings. The NIH structural configuration writer now translates these
BandSplit choices before construction and validates restored structure against
the selected layout. Constructor and callback tests cover defaults, saved
state, both modes and all supported counts; loaded package activation remains
open.

Preserve the existing NIH `crossover_type` ID. FFI/AU previously enumerated
BandSplit's two static specs, `frequency` at 0 and `type` at 1; appending specs
preserves those addresses. The runtime `Plugin::parameters()` list is a
different surface: its former active cutoff and band-gain indices must also
remain stable. Do not infer an AU address regression from that runtime list.
Inactive static cutoff slots need consistent successful set/get and restore
behavior even when no corresponding DSP smoother is active.

### Executed constructor/wrapper checkpoint

Root ran `cargo test --offline --locked -p plugins-nih --lib bandsplit --
--nocapture`. The first attempt failed to compile two new tests (an Arc return
type and equality on NIH's non-PartialEq state enum). Luna corrected both.
Rerun r2 passes all four selected tests: constructor choice translation and
selected widths, restore-marker lifecycle, legacy migration/conflicting layout
rejection, and packed CLAP wrapper channel delivery against direct DSP output.
These are constructor/wrapper tests, not exported CLAP/VST3 callback or
independent compensated-response acceptance.

Log `/tmp/sotf-aud143-nih-focused-r2.log` has SHA-256
`ca6fa2f64609c51189e0f530936a697889847dd0bf7e42cad1e070e7a65cb8e5`.
The selected 491-file run manifests differ only in `daw_host.rs`: its owner
removed `mut` from one local binding while the command ran. Root verified that
restoring that token exactly reproduces the complete start-file hash; receipt
`/tmp/sotf-aud143-nih-focused-r2-host-delta-verification.json`. Native,
BandSplit production and selected math sources remained unchanged. Preserve
this qualification instead of claiming an identical whole-source snapshot.
HAL is disabled in this gate. The earlier failing r1 log is retained.

## Native output representation

The original generic NIH BandSplit wrapper declares stereo input and a packed
four-channel main output. Its width checks assume all DSP output is on the
main bus. Adding packed six/eight-channel layouts through the generic
count-to-arrangement mapping would label frequency bands as speaker channels.

VST3 documents a speaker arrangement as a bitset of speaker roles. Its bus
interfaces allow multiple output buses, with main buses preceding auxiliary
buses; arrangement negotiation and reporting operate per bus. Bus information
and arrangements must agree. See the primary SDK documentation:
[speaker arrangements](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__speakerArrangements.html),
[buses](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__vstBus.html),
and [IAudioProcessor](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/classSteinberg_1_1Vst_1_1IAudioProcessor.html).

The design uses one stereo VST3 bus per frequency band, without assigning
frequency bands to surround speaker positions. The NIH wrapper advertises four
slots, tracks active buses, routes sparse outputs, and preserves the old
single four-channel packed arrangement. Its in-process tests exercise
discovery, legacy packed samples, sparse masks, populated three/four-band state,
and repeated activation. The freshly loaded VST3 package now passes its
2/3/4-band waveform matrix through the SOTF external host; the separate NIH
callbacks remain useful coverage for direct bus discovery and activation.

The first exported VST3 load red (session 62673, before the current VST3 host
route was implemented) recorded the old host's rejection of multiple output
buses. The later loaded VST3 success and its complete waveform matrix are
recorded in the refreshed route section below.
Default-active bus metadata remains a host preference: bus activation is
explicitly exercised by the callback tests under the
[SDK bus contract](https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__vstBus.html).

## Reachability and acceptance

- NIH's VST3 `get_bus_count` reads the current layout. Merely appending layouts
  with more buses does not prove a host can discover/select them from the old
  single-bus default. Specify and exercise the actual negotiation and saved
  state route, including the deliberate compatibility policy for that default.
- The SDK bus description requires reporting the supported maximum bus count
  and uses activation/deactivation for dynamic bus usage. `setBusArrangements`
  expects the host to supply the bus counts reported by `getBusCount` (primary
  references linked above). A synthetic test that requests previously
  undisclosed extra buses does not establish an ordinary host route. Exercise
  discovery, an accessible selection or supported restart, subsequent bus
  queries and activation before claiming reachability.
- Verify per-band bus names, stereo arrangements and channel counts, explicit
  bus activation, every sample's mapping, and behavior when an auxiliary bus
  is absent or inactive. No callback allocation or out-of-bounds access is
  permitted when hosts expose fewer buffers than the DSP's packed width.
- Use actual native ABI callbacks, distinct left/right audio and per-band
  waveform oracles for two, three and four bands. Metadata and constructor
  tests alone do not establish delivery to every native bus.
- Preserve old IDs and saved-state behavior, and test failed activation without
  silently producing default-layout audio. Keep CLAP, VST3, public DSP and
  application-route results separately scoped until each path is executed.

The bounded Linux VST3 route is now executed alongside CLAP. Windows/macOS
package loading and AU remain outside this Linux checkpoint.

## FFI saved-state reconstruction

Initial source inspection found a separate preset route that needed correction.
`plugins-ffi/src/lib/plugin.rs::replace_plugin_from_state` reconstructs from
the handle's original configuration, then replays current and incoming flat
parameter snapshots. Before this correction its special structural handling
covered Convolution and LinearPhaseEQ. `plugins-bridge/src/state.rs::load_state`
iterates the snapshot's entries and invokes individual setters; it documents
that structural changes require reconstruction by its caller.

BandSplit validates each cutoff against its neighboring current targets. A
valid complete four-band change from `[1000, 2000, 3000]` to
`[2000, 3000, 4000]` therefore reaches a conflicting intermediate state if
`frequency` is applied before the other cutoffs. Source inspection predicted
rejection; no executed pre-fix red is claimed for this path. The correction
merges the complete intended vector and structural settings into candidate
construction instead of relying on sequential scalar updates.
Partial state must retain omitted live values. Invalid state must preserve the
old populated instance and its subsequent waveform.

Changing band count also changes output width. The FFI handle's fixed input
and output geometry must remain authoritative: count changes require a
corresponding host reconstruction/negotiation path, or explicit refusal that
preserves the old route. Same-width mode, slope and cutoff restoration still
need to work. Luna BandSplit owns this bounded FFI follow-up; Luna Native
continues to own NIH integration.

### Executed FFI restore checkpoint

The two public C ABI tests now pass through both flat state and preset-document
imports. A four-band stereo handle restores coordinated cutoffs
`[1000, 2000, 3000]` → `[2000, 3000, 4000]`, LR24 → LR48 and
LegacyCascade → PhaseCompensated, comparing output to a separately configured
reference at the same initialized startup state. The first comparison uses a
peak-difference reduction bounded by 1e-6; Astra identified missing full-length
and finite checks in that reduction. A cutoff-only
partial restore preserves omitted cutoffs, mode, slope and band gain. Invalid
ordering and a count change preserve the plugin object, serialized state,
handle configuration and the complete subsequent populated audio waveform.

Initial fixtures incorrectly combined three cutoffs with a six-channel handle;
the actual public vector precedence correctly selected four bands/eight output
channels. Later comparisons mixed initialized gain targets with a reference
whose setter had just started a gain ramp. Corrected references use the public
reset after setting gain. The cutoff-only partial restore and invalid-state
continuation use exact vector equality; the coordinated case's weakened
diagnostic reduction must be corrected before acceptance. The startup fixture
failures themselves are not production restore defects.

Focused log `/tmp/sotf-aud143-ffi-bandsplit-r5.log`, SHA-256
`9fe5434e1027f0b6d9889001415ec251e010aa8b1ea9baf7a26d8635d344f89e`.
The full FFI library also passes 70/70: `/tmp/sotf-aud143-ffi-lib-r1.log`,
SHA-256 `82c4851b8e3ae9ee8fb695ac75368e88c8ff80c66666347000c2e3dd5299d9f9`.
Root verified the focused terminal log and these current FFI source hashes:
`plugin.rs` = `aa8d040d133dc446b4db01c6dfb3a073fd0e4b27d57f5678b1566cfac73552f6`;
`state_tests.rs` = `8b718012595f59cff2c26dccf0a2ff2f9e2af3f81e3d3c7b38282ba2ff336cdb`.
These are selected current-source checks, not a full run-bound manifest.
Strict FFI `--lib --tests` Clippy passes with warnings denied: log
`/tmp/sotf-aud143-ffi-clippy-r1.log`, SHA-256
`e9b44e61665073fa3dc667426e8c848a43f4bf60b67384d549cd620220b20ac6`.
Root inspected the full and lint terminal logs and preserved selected source
bytes plus all three logs under
`crates/sotf-plugins/target/audit-artifacts/aud143-ffi-restore-r1/`;
source archive SHA-256
`5aaa319f0e35c59ff1cad73a522a515948b836a9871d33ca54038330ef7f680b`.
The receipt distinguishes verified FFI source hashes from supporting source
captured at packaging time. Independent production review remains pending.
The tests call public C functions in the Rust test binary; they do not
establish dynamically loaded FFI or native AU/CLAP/VST3 behavior.

Astra's independent review requires two corrections before scoped FFI
acceptance. A handle created using the supported `crossover_type` configuration
alias retains that key in its original config; adding canonical `type` during
restore creates duplicate serde fields and rejects a valid candidate. Luna
must canonicalize the alias and execute raw-state/preset-document regressions.
The coordinated waveform case must also verify complete equal lengths and
finite samples before any reduction, with exact equality restored where
appropriate. See [`AUD143-astra.md`](reviews/AUD143-astra.md). The earlier
passing gates remain evidence for their covered cases, not for this alias.

Luna then executed the alias regression before the fix. It failed through the
public restore API with `duplicate field type`: log
`/tmp/sotf-aud143-ffi-alias-red-r1.log`, SHA-256
`76a3a23f41db3d527df2a927a88cf76243dec9e57ce1814d8e25626d21ec76c1`.
The production correction removes `crossover_type` from the detached candidate
configuration before inserting canonical `type`. Root compared it with the
preserved r1 source: this removal and its comment are the only production
changes. The successful waveform comparisons now require complete finite
vectors and exact equality. Astra accepted this scoped FFI correction after
checking both reviewed findings. At that review checkpoint the packaged
BandSplit route remained open; the later loaded route is recorded below.

All three focused BandSplit tests pass, including raw-state and preset-document
alias restoration: `/tmp/sotf-aud143-ffi-bandsplit-green-r1.log`, SHA-256
`9e80e7a219fd54c784091eedb15a674faa8e7c8543e10c638f6ec00c53b2c088`.
Strict FFI lint also passes:
`/tmp/sotf-aud143-ffi-clippy-alias-r1.log`, SHA-256
`21976a4cc0ae403f6b4d78caa5debc95dc79a4595947b16968573c8eb1a30939`.
Root verified current FFI `plugin.rs` SHA-256
`14bf3da073cf2b0b2b07aa25945c67b1da9dc7e077359f8886293f52eb8f6b3e`
and `state_tests.rs` SHA-256
`90e7967a5f49863161579fcbea140aa98ca892cfe5972de50ecf5e170ea81ef7`.
The prior full 70-test run remains historical; these focused cases and affected
lint cover the narrow reviewed correction.

Provenance clarification: root subsequently checked the preserved r1 helper.
`Handle::process` already initialized the complete output buffer with NaNs and
asserted all returned samples finite. The earlier local peak reduction lacked
explicit checks, but the complete test did not lack all finite-output guards.
The revised local length/finite assertions make its contract explicit, and
exact vector equality replaces the diagnostic tolerance. The alias rejection
was the executed production defect. This clarification does not change the
passing corrected checkpoint.

Root preserved this accepted correction in
`crates/sotf-plugins/target/audit-artifacts/aud143-ffi-restore-accepted-r2/`.
Its `source.tar.gz` SHA-256 is
`40b3242fb7d2945eacadcb37a71f09d37bdf9edecd03384fe75387a5d63930f6`.
The receipt records reviewed FFI source hashes, the unchanged DAW lock,
supporting sources at packaging time, Astra's review and the preserved
alias-red/focused-green/strict-lint logs. This is selected-source preservation,
not a complete transitive build snapshot or dynamically loaded ABI evidence.

## Executed NIH legacy-state migration checkpoint — 2026-09-30

The exact pre-AUD143 NIH BandSplit schema is a two-control state with
`frequency` and `crossover_type`; it has no `num_bands`, recombination mode,
secondary cutoffs or NIH gain entries. The migration helper fills missing
mode with LegacyCascade index 0 and missing band count with the two-band choice
index 0, then sets the restore marker so the structural values are checked
against the selected layout. Explicit modern values remain in the state and
are validated rather than replaced.

The preserved pre-fix red is
`/tmp/sotf-aud143-nih-old-state-red-r1.log` (SHA-256
`620621daa83647731f495209e383d93d498e49da08d66ec95e0a5d48d18336da`): its
callback test had initialized a wider layout but had not processed audio
before restoring the old state. It showed the old count was retained instead
of defaulting to two bands. Do not describe that red as a populated-history
test. The final callback test separately performs a nonzero block on both
three- and four-band layouts before importing the authentic old state.

For each populated layout, the direct NIH state callback refuses the old
two-band structural state with `kResultFalse` while the wider output buses are
selected. It exposes the migrated two-band/LegacyCascade values; subsequent
processing of that same instance is refused and its supplied output buffers
are silent. This confirms direct NIH failure semantics, not preservation of
the prior same-instance DSP history. The test then deactivates the plugin,
negotiates its compatible old packed two-band layout (one four-channel main
bus, auxiliary buses inactive), retries the same state successfully and
compares all packed samples in the callback block with a fresh LR48
LegacyCascade reference. The intermediate r6 diagnostic
`/tmp/sotf-aud143-nih-old-state-r6.log` (SHA-256
`47b71a29303d3921c5682835d44dfa19f6727e4f69e1a39ef6a343ab6be31c4d`) also
recorded that processing after the incompatible direct import returned
`kResultFalse`; its failed assertion was corrected to encode the supported
refusal contract.

Sparse output coverage is separate from silencing provided inactive buffers.
The three-band sparse callback passes only the main bus (`num_outputs = 1`),
so no auxiliary descriptor exists; its Band 1 output matches and unprovided
band channels remain canary-valued. The four-band sparse callback passes four
bus descriptors with output pointers only for buses 0 and 3; bus 1 and bus 2
have null channel-pointer arrays. It checks Band 1 and Band 4 against the
reference, while omitted middle-band storage stays untouched. Three ordinary
deactivate/reactivate cycles preserve the selected count. Separately, the
populated three-band wide callback provides valid buffers for all four buses
and verifies that the inactive fourth bus is silent. Its four-band case has
all four buses active. No missing-buffer case is generalized into a claim
that provided inactive buffers must be silent.

The unused NIH trait hook `vst3_output_bus_activation_selects_configuration`
was removed with its BandSplit override. Repository search and TokenSave
caller analysis found no call sites, so it never selected a structural
configuration. The actual activation mask remains in place for output routing,
silencing and cycle bookkeeping; it does not set the DSP band count.

The exact focused migration callback command passed 5/5 in
`/tmp/sotf-aud143-nih-old-state-module-r1.log` (SHA-256
`9dca5638ffa650f57fe22b03f647f83794cb08f572766b39c4af15a75ca00c51`). The
qualified shared constructor regression passed 1/1 in
`/tmp/sotf-aud143-nih-dynamiceq-shape-r4.log` (SHA-256
`e1e6eef38c7f70d7160063eda7475ffbc668c0f26fb6a210728634eb3a1c492e`). The
final full NIH library passed 111 tests, with one ignored, in
`/tmp/sotf-aud143-nih-full-lib-migration-r5.log` (SHA-256
`de3d5b40e4a94eb3fa6f1608e40b8e6c7b755bc73ee66c9205ebd3cd71d90e6a`). Strict
all-target Clippy with warnings denied passed in
`/tmp/sotf-aud143-nih-clippy-migration-r5.log` (SHA-256
`1513748dc13e3efebf8fb09e71bd7cf7bfe5e03803540261eb7335435ea064ca`). The
selected start/test/lint source manifests match SHA-256
`277aa180d203d7d6697f3900032a0bb7d1b6fc65e8c57e75798635d29bd62448`.

Selected sources, logs and manifests are preserved in
`crates/sotf-plugins/target/audit-artifacts/aud143-nih-old-state-r2/`.
`selected-source.tar.gz` SHA-256 is
`c004865197256c5b8ac1235b53fbb9a7a00985cac9040d0d779dd5fc2474aa5e`. This is
selected-source preservation, not a full transitive build archive. These
results exercise NIH callbacks in the test executable; loaded packages and the
SOTF external-plugin all-band host route remain open.

## Native short output-prefix safety — 2026-09-30

The fixed-snapshot broad gate exposed two regressions in
`native_aux_output`: CLAP returned `CLAP_PROCESS_ERROR` and VST3 returned
`kResultFalse` when hosts supplied the main output plus a shorter auxiliary
suffix. The separate limiter timing failure passed its isolated rerun. The
wrapper correction now requires a declared main output, validates every bus
that is supplied, and lets an omitted auxiliary suffix reach the existing
empty-slice path that skips DSP. VST3 still requires the last active bus and
exact geometry for active supplied buses. Surplus buses, wrong widths, null
channel pointers, and f64 buffers retain their preflight rejection checks.

Both format tests retain complete and short-prefix callbacks, output canaries,
input-to-main copy checks and the allocation guard. They also supply a
wrong-width auxiliary bus backed by valid pointers, expect the format's error
status, check bounded safe silence, and verify the surplus pointer canary.
`native_aux_output` passes 2/2; the BandSplit lib filter passes 9/9, including
sparse activation masks and untouched inactive-slot canaries; NIH `--lib --tests`
passes 111 unit tests plus both integration tests with one ignored;
strict all-target Clippy passes. Logs: `/tmp/sotf-aud143-native-aux-prefix-r2.log`
(SHA-256 `6e7cd81fa2ef15806bd93c22b2b202bd410062c50dca140c4ee47e9e453a4489`),
`/tmp/sotf-aud143-native-bandsplit-mask-r1.log` (SHA-256
`9f1faf5605249b40574deb8b625a3e6014271e4ef4fd43c5191817da23381037`),
`/tmp/sotf-aud143-native-nih-tests-r1.log` (SHA-256
`a59ff23ceef63efbee2929d2272712e7d67ad5ad601e21c76f36a5dd59d057e3`), and
`/tmp/sotf-aud143-native-nih-clippy-r1.log` (SHA-256
`03608de1b94bf576e0b51cd9be6e14419ab89abf14b8cb77dad9a3ed74d44969`).

The first short-prefix pass, before adding the malformed-width subcase, was
session 28042 and had no saved log. Exact command and terminal output:

```text
flock -x /tmp/sotf-daw-audit-cargo.lock env CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/target TMPDIR=/tmp CARGO_NET_OFFLINE=true cargo test --offline --locked -p plugins-nih --test native_aux_output -- --nocapture
Compiling nih_plug v0.0.0 (/home/pierre/src/all_of_sotf/sotf-daw/crates/3rdparties/nih-plug)
Compiling plugins-nih v0.5.4 (/home/pierre/src/all_of_sotf/sotf-daw/crates/sotf-plugins/crates/plugins-nih)
Finished `test` profile [optimized + debuginfo] target(s) in 2.61s
Running tests/native_aux_output.rs (crates/sotf-plugins/target/debug/deps/native_aux_output-706638cda9594fec)
running 2 tests
test vst3_missing_auxiliary_output_never_accesses_hidden_valid_bus ... ok
test clap_missing_auxiliary_output_never_accesses_hidden_valid_bus ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The saved r2 log includes the later malformed-width assertions. Selected source
and lock hashes are in `/tmp/sotf-aud143-native-prefix-source-r1.sha256`
(manifest SHA-256 `a5652eb5b1f9a9695b48f94d572629f5b4212bfc1448abbb5d146a65f345c9c9`).
Root preserved the three source files, before/after snapshots, correction diff
and four gate logs in
`audit/artifacts/aud143-native-prefix-root-r1/SHA256SUMS` (SHA-256
`e0b5102a01f3793188607309e66bc3ea44cd712259bba1a41d820c53e1adbd3d`). This is
selected-source evidence, not a full dependency start/end manifest. At this
historical prefix checkpoint the direct callback tests did not establish
packaged CLAP/VST3 loading or the SOTF consuming-host all-band route; the later
loaded evidence is recorded below.

## Refreshed loaded route and inactive-buffer reuse — 2026-09-30

This section supersedes the earlier open-route statements above. It records a
fresh packaged CLAP/VST3 route run and the later NIH auxiliary-buffer reuse
probe. The new buffer probe is not yet independently reviewed by Astra.

### Loaded CLAP/VST3 and `DawHost`

The `plugins-nih --features band-split` build produced the CLAP and VST3 copies
under
`crates/sotf-plugins/target/audit-artifacts/aud143-bandsplit-native-route-r3/`.
Both copies are byte-identical and have SHA-256
`6f1803cbad160f59f589569e600e10ffc7dc1a5cf5986b68e4eff123a5ef838b`. The
loaded test uses the `.clap` file and the VST3 bundle path ending in
`BandSplit.vst3`; it does not load a bare shared-object path.

`cargo test --offline --locked -p sotf-plugins
--features external-plugin-clap,external-plugin-vst3
--test native_bandsplit_external_host -- --nocapture` passes 5/5 in
`/tmp/sotf-aud143-loaded-band-route-r6.log`, SHA-256
`bcdeeb9189f518b9378161164c74dd157fed8069e832ea895f1c46c310ab2aee`. Its
selected start/end manifests are preserved under
`artifacts/aud143-native-buffer-reuse-r1/logs/` and match; this is selected
source provenance, not a transitive build manifest.

The matrix loads each packaged format for 2/3/4 bands, LR24/LR48, and both
LegacyCascade and PhaseCompensated modes. It compares the complete finite
waveform against an independently configured public BandSplit DSP composition
with a maximum-error threshold of `2e-5`; passing residual values are not
printed. Distinct left/right inputs and per-band output checks run across
partitioned callbacks, including the short final block. Saved-state reload and
the actual `DawHost` chain are both checked at each valid layout. DSP formula
accuracy remains separately established by the independent AUD143 core tests;
the loaded comparison demonstrates routing and composition consistency.

The failure case first validates the outer `ExternalPluginState` envelope,
then reaches the native state-load/readback refusal for a conflicting bus
geometry. CLAP reports rejected persisted state; VST3 reports a component
state restore failure. The test compares the complete live state, continues
processing against a synchronized populated twin, verifies a cold-start
sensitivity control, and retries a valid candidate against a fresh reference.
This demonstrates detached-candidate preservation after native-stage refusal;
it does not claim that candidate audio was processed before commit.

### Auxiliary output buffer reuse through VST3

After the prefix checkpoint above, the `BufferManager` missing-auxiliary-output
branch was narrowed to set the buffer sample count to zero and replace every
declared channel slice with an empty slice. A supplied inactive output bus
continues to receive full callback-length slices, which are cleared before
`Plugin::process`.

The new workspace test
`vst3_inactive_auxiliary_output_reuse_tracks_absent_present_absent_buffers`
uses one native VST3 wrapper for absent→present→absent inactive mono and stereo
auxiliary buffers at 17, 33, and 9 frames. The plugin records sample count,
channel count, slice-length consistency, and pre-process zero state in
preallocated atomics; assertions happen after callbacks. Host-side checks
verify that supplied inactive storage is cleared only over the callback range
and that omitted storage retains its canary. The existing CLAP/VST3 prefix and
malformed-width tests keep their allocation guards unchanged. This additional
probe is not wrapped in an allocation guard because a debug assertion during
buffer preparation on a regression could be hidden by allocator-guard panic
formatting; the existing callback gates continue to cover no-allocation
behavior.

The focused prefix/reuse gate passes 3/3 in
`/tmp/sotf-aud143-inactive-output-buffer-r1.log` (SHA-256
`7c357773401874cfccc7c057d00a0bf0c4a313d1435c40da4804dab94fe29490`). The
BandSplit VST3 callback/mask filter passes 5/5 in
`/tmp/sotf-aud143-native-bandsplit-mask-r2.log` (SHA-256
`daec77cf3d855308490bb462912859fa3fba2386ca0ef0e2a76a8ff7ebdbe634`). The
feature-enabled full NIH `--lib --tests` run passes 111 unit tests and all 3
native auxiliary-output tests, with 1 manual utility ignored: 114 passed, 1
ignored. Strict all-target Clippy with warnings denied passes. Logs:
`/tmp/sotf-aud143-nih-full-band-split-r1.log` (SHA-256
`6be92fda2743e16d6b559f2ee53cca306ffc4344714e47097898fbdff5416467`) and
`/tmp/sotf-aud143-nih-band-split-clippy-r1.log` (SHA-256
`f1e48d8d163d8a3dfd21b0929b98249b77c8de0b7b2d80951ea0f2beeaa815a7`).

The direct vendored `BufferManager` unit test for the same transition remains
unexecuted because standalone offline resolution of its nested `nih_plug`
manifest requires an uncached `baseview` git revision. No repeated nested
resolution attempt was made. The workspace VST3 probe exercises the production
buffer manager through native callbacks without exposing its private module.

Selected current source hashes and the preserved logs/source copies are in
`artifacts/aud143-native-buffer-reuse-r1/`. Its `SHA256SUMS` index SHA-256 is
`33c0b4ec74c26ea9a71e2aa3bd5ad838dc33ebecdcb4c579585062f8839eb331`;
`selected-source.sha256` SHA-256 is
`e1e7779a5ce054adb4b71c3783635b3f2a46d8993509e87a37f263b3be910eb3`. The
archive includes the current callback test and vendored buffer source as well
as the loaded route test and both loaded-run manifests. It is scoped evidence,
not a full workspace gate, an AU result, or an Astra acceptance claim.
