# AUD-135 native Ambisonics route: implementation report

Status: wrapper/framework corrections for Astra's three findings are applied;
the NIH library suite, strict all-target Clippy, refreshed copied BufferManager
harness, production cdylib rebuild, and direct SOTF external-plugin CLAP/VST3
load/restore tests pass. Astra accepted this corrected wrapper plus loaded-host
checkpoint; the remaining native routes below are still open. The test
uses `ExternalPlugin` directly; it does not establish the consuming engine,
isolated worker, mounted setup/reactivation, or EOF routes. The bounded design
covers orders 1–7; CLAP wide-role interoperability, arbitrary custom layouts,
and broader native plugin parity remain full-audit work.

## Accepted scope

Astra accepted the design in
[`proposals/native-ambisonics-orders-1-through-7.md`](proposals/native-ambisonics-orders-1-through-7.md)
at SHA-256
`290e4c198a74747391a3c6adf15025551a90b78933308c88b1eea1f60c16b428`.
The batch extends the packaged native plugin route over all existing named
layouts through 9.1.6, whole ordered ACN/SN3D input widths 4–64, and the
64→16 tuple. Bridge, FFI, VST3, and SOTF-managed paths cover 56 order/output
tuples. Standard CLAP metadata covers 42 because it has no truthful wide-left
or wide-right surround roles; wide CLAP interoperability stays open. The
reviewed channel-role, mask, and permutation source is
[`native-speaker-role-mapping.md`](native-speaker-role-mapping.md), whose
contents were accepted by Astra.

The persisted setting is per plugin instance and optional for generic
external plugins. The selected order/output target must flow through the
visible setup, saved plugin state and isolated worker, and the active host
candidate before the plugin can claim it is applied. The proposal documents
no-engine, rejected-candidate, post-commit playback failure, and uncertain
commit states without widening the existing engine transition protocol.

## Pre-edit route checks and captured outputs

Before native production edits, the bridge and public C ABI were exercised
over all 56 tuples with 257 deterministic frames and distinct ordered input
values. The public NIH wrapper's old default 4→6 output was separately
captured for byte comparison. Those route tests do not re-establish DSP
accuracy; the independent spherical-harmonics and decoder checks are in
accepted AUD133.

| Check | Result | Evidence |
|---|---:|---|
| `plugins-bridge` all-56 route test | 1 passed | `/tmp/sotf-aud135-bridge-all56-preedit.log`, SHA-256 `15ad7ac7b058f5febe99d9cd8fa7dbbe0845b51837fd325ffd4771cfbf13371d` |
| `plugins-ffi` all-56 public C ABI parity/capture test | 1 passed | `/tmp/sotf-aud135-ffi-all56-preedit.log`, SHA-256 `4070ae0ea7162ae3ce02bf3ab6425a7b67a53000a911322e163de1a844ab755b` |
| NIH pre-edit default wrapper waveform capture | 1 passed | `/tmp/sotf-aud135-nih-default-preedit-final.log`, SHA-256 `59892754313a19e2164dc10352df82157e7b282d5196a02c00c5aa565bfc512f` |

The FFI output artifact is
`crates/sotf-plugins/target/audit-artifacts/aud135-preedit/ambisonics-ffi-all-56.f32le`,
605,040 bytes, SHA-256
`52e9b4f5d7ffd02f28fca98d79dfa5c7d79115411e6932e47d695f697dd172c7`. The
suffix is misleading: this is a tagged container with 56 `<BBHHI>` tuple
headers followed by 257 frames of interleaved f32 output per record. Each
record has order, output-layout id, input width, output width, and frame count.
The full artifact was parsed through exact EOF; each expected tuple is present
and all vectors are finite and nonzero. Maximum observed tuple peak was
0.2680885. The output vectors match the bridge route bit-for-bit.

The original NIH wrapper output is
`crates/sotf-plugins/target/audit-artifacts/aud135-preedit/ambisonics-nih-default-order1-5.1.f32le`,
6,168 bytes, SHA-256
`6d8cab6f11e304014a35a9a9aa98506082ae504a86f028182d37250ae099e228`. It is
the old default 4-channel input to 6-channel 5.1 output for 257 deterministic
frames.

The scoped source manifest before and after the baseline tests is recorded in
`/tmp/sotf-aud135-preedit-route-start.sha256` and
`/tmp/sotf-aud135-preedit-route-end.sha256`. Each lists the same 49 Rust and
lock files and has aggregate SHA-256
`3f00a2911cf912462b8334a9baa726158cb7b965c06b4a759489737d9241ec4f`. The lock
file is `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
The captured output files live under
`crates/sotf-plugins/target/audit-artifacts/aud135-preedit/`. Root also records
this route baseline in
[`current-feature-route-coverage.md`](current-feature-route-coverage.md).

The NIH capture test's first invocation used a nonmatching module prefix and
selected zero tests. That invocation is not counted as evidence. The corrected
filter selected and passed exactly one test:
`wrapper::process_tests::capture_aud135_pre_edit_ambisonics_native_default_waveform`.

## Wrapper/framework implementation checkpoint

The wrapper-stage changes add whole-vector input handling to the copied NIH
buffer manager, typed order/output-layout choices in the NIH wrapper, truthful
CLAP ACN/SN3D input and standard surround output metadata, and VST3 arrangement
masks/permutations from the reviewed role table. The exercised ABI cases are
order-7 CLAP 64→12 (7.1.4) and VST3 64→16 (9.1.6). CLAP's standard surround
metadata has no truthful wide-left/right roles, so its two wide layouts remain
unadvertised. Both wrappers advertise/process f32 only; their f64 requests are
rejected before touching sample storage.

The test invokes NIH's actual CLAP C callback and VST3 COM
`IAudioProcessor::process` callback on in-process wrapper instances. It checks
negotiated metadata, full ordered input/output waveforms against direct wrapper
processing, insufficient primary-input rejection, prepared-silence behavior
for absent/short optional Gate key input, f64 rejection, and input/output
frame sentinels. The callback paths run with NIH's process-allocation guard.
This is not an external host loading the built shared library, nor a test of
per-instance persisted host setup or host reactivation. The tests exercise the
maximum CLAP and VST3 tuples, not all 56 ABI tuples. The preserved bridge/FFI
all-56 artifacts above are pre-edit route baselines, not post-edit ABI
coverage.

The copied NIH BufferManager tests separately cover 64-channel primary input,
prepared scratch at varying and maximum callback lengths, shorter/missing
input clearing, and input-only layouts. The copied `buffer.rs` has the same
SHA-256 as the checked-in source (`4dd4e6cde774718aff43c6b89f5b652ec5826c918736ca5eec11fb2faf1b90ac`).
That four-test harness disables optional dependencies to work around an
offline `baseview` resolution failure; it is ordinary `cargo test`, not Miri,
and does not claim whole-wrapper allocation freedom. The NIH assertion guards
process callbacks only, not initialization, transport, setup, or host message
handling.

### Verification

The corrected full NIH library suite passes 100 tests with one manual utility
ignored. Its byte-identical 17-entry source-plus-lock manifests are
`/tmp/sotf-aud135-nih-full-final2-start.sha256` and
`/tmp/sotf-aud135-nih-full-final2-end.sha256`; each file's SHA-256 is
`c1396fcbf21c27513c2b99a42d8b77fef362d9d8ff4ba96882235e0b3d5e3bda`, and the
lock entry is `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`.
The earlier 98-pass/2-fail run is retained as a real compatibility finding:
exact geometry checks initially rejected Gate's optional key-sidechain bus.
The wrappers now require exact primary Ambisonics input and output geometry,
allow absent/short optional key inputs to use prepared silence, and reject
surplus buses. The Gate CLAP and VST3 regressions pass 2/2.

| Gate | Result | Log (SHA-256) |
|---|---:|---|
| `cargo test --offline --locked -p plugins-nih --features ambisonics --lib` | 100 passed, 0 failed, 1 ignored | `/tmp/sotf-aud135-nih-full-final2.log` (`0c0e258c32c7793a3fc791741c4d4eb7ef11b1318e967419e5e5c84f93c44079`) |
| Focused native callback/canary tests | 5 passed | `/tmp/sotf-aud135-callbacks-final3.log` (`27b6de0d49cc137eed272a1ce6df3e04106e1146286485aba07ee6d463835e47`) |
| Gate optional auxiliary input regressions (CLAP and VST3) | 2 passed | `/tmp/sotf-aud135-gate-sidechain-optional-aux.log` (`4aec57cceed91f2eeed36211e52df98f2c8b1c4f86f2602e0149b1ceadb6b18b`) |
| `cargo clippy --offline --locked -p plugins-nih --features ambisonics --all-targets -- -D warnings` | passed; one warning from vendored `nih_plug` (`unsafe_clap_call` unused import) | `/tmp/sotf-aud135-clippy-post-auxfix.log` (`aeabe8c2aa7953b26d760d6fdc0bf7d4fc3530d64db88a25dcad3e65755feb58`) |
| Copied-source BufferManager focused harness | 10 tests passed, including the four focused geometry/lifecycle cases | `/tmp/sotf-aud135-buffer-manager-current-source.log` (`a89440e08644c9b0bbe8310f36165c1874bafa337d7e8be3aa40079cdff46df8`) |
| `cargo build --offline --locked -p plugins-nih --features ambisonics --lib` | passed; produced `target/debug/libplugins_nih.so` (47,520,768 bytes, SHA-256 `990c7f1eec99d508ef223ce957af852e0a709a625e6c0d522225513c86a852c7`) | `/tmp/sotf-aud135-cdylib-post-fix.log` (`5586f4a48aa42cc42d52eb3abe5143e823041f130634c56705a140de55b6d57c`) |

The focused callback regression negotiates and processes CLAP order-7 64→12
and VST3 64→16, with full primary geometry checks before buffer access,
unsupported f64/oversize rejection, canary preservation, and process-allocation
guards. A state-sensitive dual-band twin test proves rejected malformed input
does not advance DSP, while an extra-valid-block positive control confirms
that the signal detects such advancement. Active CLAP configuration selection
is rejected through active-before-start, processing, and stopped-before-
deactivate states, and is permitted after deactivation. Unequal-width CLAP
layouts no longer claim in-place pairing.

The strict-lint source-plus-lock manifest is byte-identical at
`/tmp/sotf-aud135-final-clippy-start.sha256` and
`/tmp/sotf-aud135-final-clippy-end.sha256`; both have SHA-256
`06914de76a0a53f480381ae056552378d0791322e3a0c6df2e727fa97b70c2cb`. It
contains 22 relevant source and manifest entries, including the current
shared host and HAL edits; the host file SHA-256 is
`9b7bccab1760b536cb609b83709c648c995872f29ae804a472d0a52d1033bed8`. The
full NIH test manifest predates those unrelated host-file changes, so the two
gate snapshots are reported separately rather than represented as one source
snapshot. The final build's 23-entry source-plus-lock manifest adds
`plugins-nih/src/lib.rs`; start/end files
`/tmp/sotf-aud135-final-cdylib-start.sha256` and
`/tmp/sotf-aud135-final-cdylib-end.sha256` match at SHA-256
`653f83e852b9716de72fc1f5ca01e747247de532d71666590f4441c83e7fb78c`.

The copied BufferManager harness was refreshed byte-for-byte from the current
source (`09822efb24535d609f5357917769efa22a318b631f0dfbabe837e38e117f8402`).
Its start/end harness manifests match at SHA-256
`22c30c770b7d8b3f0940cffddee029fc7ae46cca04ad6c66f9d27445552461d2`. This
is ordinary `cargo test`, not Miri; optional dependencies were disabled and a
local `baseview` shim was used for offline compilation. NIH allocation guards
cover process callbacks, not initialization or host transport/setup.

## Exported CLAP/VST3 load and typed-state restore checkpoint

After the earlier production artifact was preserved, the corrected library was
rebuilt with `cargo build --offline --locked -p plugins-nih --features
ambisonics`. The preserved earlier artifact is
`crates/sotf-plugins/target/audit-artifacts/aud135-native-host-pre-restore-fix/libplugins_nih.so`,
SHA-256 `990c7f1eec99d508ef223ce957af852e0a709a625e6c0d522225513c86a852c7`.
The rebuilt test artifact is
`crates/sotf-plugins/target/audit-artifacts/aud135-native-host-restore-fix/libplugins_nih.so`,
SHA-256 `54c29a62f65c93ee65aa257b521089c8510d385388229413d4669602a00de20a`;
the `.clap` and `.vst3` aliases used by the loader tests are symlinks to that
same byte-identical file. The build log is
`/tmp/sotf-aud135-native-restore-cdylib.log`.

The loaded-plugin tests use `ExternalPlugin::from_placeholder_state` and real
CLAP/VST3 loader callbacks, not direct NIH wrapper calls. They negotiate CLAP
order 7 / 7.1.4 (64→12) and VST3 order 7 / 9.1.6 (64→16), render two successive
blocks including high ACN inputs, and compare the complete waveforms against a
separately configured Ambisonics decoder instance. That comparison verifies
native transport and layout wiring; the independent mathematical accuracy
checks remain the accepted AUD133 oracle. Both routes retain the original
4→6 discovery descriptor while reporting their selected effective instance
widths.

Each route also restores a serialized nondefault `max_re_weighting=false`
control with the typed order-7 selection, proves the control changes the
rendered waveform relative to a default-value control, and confirms the
serialized value survives matching restore. The test rejects order-1 opaque
state paired with typed order 7, including same-instance `load_opaque_state`
and preset `deserialize`; after rejection, the active tuple and subsequent
output still match an untouched twin. The NIH restore marker is consumed on
both match and mismatch. This proves marker cleanup; the separate deliberate
host reconfiguration path remains required below. Mismatch cases intentionally cause NIH's wrapper to log that plugin
reinitialization returned false; the host tests assert that the candidate is
rejected and the current instance is retained.

The restore marker is set when NIH deserializes a parameter set containing
the hidden structural order/layout pair. Ambisonics wrapper initialization
checks that pair before applying the negotiated tuple; fresh construction has
no marker and accepts the explicitly selected setup. Structural controls
remain hidden from generic automation. The VST3 structural readback metadata
uses plain order range 1–7 and layout range 0–7.

| Gate | Result | Evidence |
|---|---:|---|
| Updated `plugins-nih` library suite | 101 passed, 0 failed, 1 manual utility ignored | `/tmp/sotf-aud135-nih-restore-unit.log`, SHA-256 `4d4d0b313795cdc0ea62702c1b5017b0c5575b4b354b0bb31b79ff5037c7cbc5` |
| Updated `plugins-nih` strict all-target Clippy | passed | `/tmp/sotf-aud135-nih-restore-clippy.log`, SHA-256 `6e33de22a931b9aa310052ea91a46fdc3da05c9c0a27d47b0bd91beaeb0c6b14` |
| Exported CLAP/VST3 typed restore and audio test | 2 passed, 0 failed | `/tmp/sotf-aud135-native-restore-nondefault.log`, SHA-256 `df4c884e279ed2b51ee535534bda1db4b75408e8e59ab2418a5151e623adae63` |

The NIH unit and lint start/end manifests each contain 30 selected inputs and
match byte-for-byte at aggregate SHA-256
`25d4a747598097436d4429489d61e9a5210f4e692540cf738dbf6d0f472cd3b9`;
receipts and manifests use the `/tmp/sotf-aud135-nih-restore-{unit,clippy}-*`
prefixes. The separate exported-loader test used `Cargo.lock`
`c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5` and these
source hashes: `params.rs`
`902e9425c9cf6c86e49f5caf1f501afd5c42b82764d269869b96620f8263a198`,
`wrapper.rs`
`3d1c93591adcee5a75753b39a271fb8c944b3c6ebc7c6457d0774d41c447b3ea`,
`vst3_backend.rs`
`df91a2cc93e853bb22edc4388617688034ed4bf5c07559746786dabacc97f668`, and
`native_ambisonics_host_order7.rs`
`3d6a06655942431bbf200707fb97f5635b183277c4bf9aab4c5f5d5bc9a4c718`.
Those source files were unchanged during the respective commands; the unit/
lint manifest and exported-loader source snapshot are deliberately reported
separately because they exercised different target inputs.

## Post-restore compatibility replay

After Astra accepted the loaded-host restore checkpoint, root ran the existing
bridge, public FFI, default NIH and default loaded CLAP tests against the
current source. Each selected test passed. The CLAP loader used the preserved
rebuilt `54c29a62…` artifact. Each command has matching start/end selected
69-file manifests with aggregate SHA-256
`eac3e7864d9e3e4a1786b5e399b5d2460ae725fd0b6b4d41787036dbdf6139d1`.
These cover selected native/bridge/FFI/DSP sources; they exclude unrelated HAL
test edits and are not a whole-workspace snapshot.

| Gate | Result | Log SHA-256 |
|---|---|---|
| Bridge all 56 tuples | 1 passed | `e10a75e51378e87f435699cea4d10c071eafb33e4788d523706c982dd22f4811` |
| Public FFI all 56 tuples | 1 passed | `618542938565781e2c2e3817444ef49f53c5aa830a0272b810c5bc4583709163` |
| Default NIH waveform capture | 1 passed | `e2c742832b7327c2b3bc2e51c3aec1d596a38149f91d705781bd3efb0ddfcf36` |
| Default CLAP host waveform/setup | 1 passed | `ce24da5189edea98af62ff5f9f39e3db1cd589879165172f7f13303b0be3fb95` |

Logs, exact commands and manifest receipts use
`/tmp/sotf-aud135-postrestore-{bridge56,ffi56,nihdefault,clapdefault}` prefixes.
New waveform files are in
`crates/sotf-plugins/target/audit-artifacts/aud135-postrestore-replay/`.
Root compared their complete bytes with the untouched original artifacts and
verified the original hashes before comparison:

- FFI: all 56 tagged records match exactly, 605,040 bytes, SHA-256
  `52e9b4f5d7ffd02f28fca98d79dfa5c7d79115411e6932e47d695f697dd172c7`.
  Every order/layout/width/frame-count header was checked, all samples were
  finite, every record was nonzero, and parsing ended at the final byte.
- NIH default: 257 frames × six channels, 6,168 bytes, SHA-256
  `6d8cab6f11e304014a35a9a9aa98506082ae504a86f028182d37250ae099e228`.
- Loaded CLAP default: 1,024 frames × six channels, 24,576 bytes, SHA-256
  `b23e5c6ed504a94c9ac661d8b949e0f5cf55e4ef8ad925a61c6e05b57cdbafa5`.

The raw default arrays are finite and nonzero. The comparison receipt is
`/tmp/sotf-aud135-postrestore-byte-replay.json`. This closes the stated
post-edit bridge/FFI compatibility replay and saved default-waveform checks
at this source checkpoint. It does not extend mathematical accuracy beyond
AUD133 or establish all native tuple, isolated-worker, engine/UI or EOF routes.
There is no pre-edit VST3 host waveform comparison: the old count-only loader
failed negotiation before audio, as previously documented.

## Remaining implementation and acceptance gates

The wrapper/framework plus direct loaded-host restore checkpoint is accepted
by Astra. The remaining accepted-design work must establish, with
independent tests:

- support deliberate order/layout changes through an explicit host route that
  preserves unrelated controls and produces consistent new typed/opaque state;
  consumed restore markers alone do not prove this reconfiguration path;
- carry a typed optional setup through plugin settings/presets and the isolated
  worker, and through effective graph widths and actual engine candidate
  activation without mutating global discovery descriptors; the direct
  `ExternalPlugin` serialization/restore path above does not cover these
  additional consumers;
- exercise a packaged plugin through SOTF's consuming host, including exact
  VST3 tuple selection, host bus ordering, saved/reloaded setup, candidate
  rejection, and the 64→16 EOF receipt path;
- provide a reachable setup UI whose state distinguishes configured/pending
  from applied and preserves the documented safe/error states.

The exported-plugin tests prove the SOTF external-plugin loader can load the
shared library with an explicitly selected typed tuple; they are not an
independent third-party application host. This report closes no mounted UI,
engine candidate-application, isolated-worker, all-tuple native execution,
or EOF claim. Wide CLAP roles, custom layouts, and platform runtime behavior
remain open.
