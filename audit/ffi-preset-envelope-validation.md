# AUD144: FFI preset envelope validation

Status: **r3 passes both unchanged public probes: invalid envelopes preserve
state/audio, and genuine legacy FletcherMunson migration is restored**. Focused
tests, the full FFI library and strict lint pass. Recorded 2026-09-30; Astra
review accepts this bounded checkpoint; see `reviews/AUD144-astra.md`.

## Original behavior

`plugins-ffi/src/lib/plugin.rs::plugin_export_preset_json` emits a complete
document containing `schema_version: 1`, the SOTF preset `ut_type`, a
`plugin_type`, descriptive metadata and a `state` byte array.

`plugin_import_preset_json` parses the JSON, bounds the document and state
sizes, verifies that state entries are bytes, then passes those bytes to
`replace_plugin_from_state`. It does not inspect the document's schema version,
type marker or plugin identity. A valid payload can therefore reach replacement
even if its envelope names a different plugin or an unsupported format version.
Transactional candidate construction protects a failed state replacement but
does not validate the ignored envelope fields.

The original inspected source SHA-256 was
`340248a846d96ca42df1386754ff0497cae96a546a0fe07122741f2c75ad2513`.
TokenSave status/context returned `Transport closed`; bounded current-source
reads were used. The executed checkpoint below supersedes the original
source-only status; root did not change Rust production code.

## Executed public reproduction

Root built the real `plugins-ffi` cdylib with offline locked Cargo under the
shared Cargo flock. All 339 selected FFI/bridge/host/DynamicEQ/Crossover source
and workspace-manifest hashes match before and after the build. This is a
selected-source receipt, not a complete transitive dependency snapshot. Header
synchronization was skipped; separate AUD139 header evidence remains separate.
The immutable library SHA-256 is
`8954f6abe82bd6685b8f4d3142b06dbb2e1f89f7cdb615b3692497d40587c529`.

`tools/aud144_preset_envelope_probe.py` calls only exported C functions through
Python ctypes. It creates synchronized DynamicEQ instances, warms both through
three 257-frame blocks, exports a genuine full preset, then changes one envelope
field while leaving all state bytes intact. The unchanged document and a renamed
preset import successfully and match fresh prepared output. A public Gain handle
also constructs successfully, establishing a real different-family identity.

All nine invalid cases incorrectly return success:

- Different, missing, or non-string `plugin_type`.
- Future version 2, missing, or string-valued `schema_version`.
- Different, missing, or non-string `ut_type`.

Saved parameter state remains identical in every case, but the complete stereo
continuation differs from the synchronized untouched twin: maximum absolute
sample residual is `0.029786840081214905`. A cold instance establishes sensitivity
to the lost audio history. Each continuation comprises 127 then 509 frames
(1,272 interleaved samples), all finite. Complete vectors and their hashes are
preserved, along with the exact documents, probe, source and build receipts,
under `artifacts/aud144-preset-envelope-r1/`. The probe terminates with exit 1
because all nine required refusal cases fail; this is deliberate red evidence.
No allocation, AU-runtime, or independent numerical DSP acceptance is claimed.

## Executed correction and compatibility follow-up

The r2 validator checks integer schema version 1, the exact preset type marker,
and supported plugin-family identity before extracting state. Focused tests pass
2/2; the full FFI library passes 81 tests with one manual utility ignored; strict
all-target Clippy passes. A rebuilt cdylib has matching 340-file selected start
and end source manifests, with header synchronization skipped. Its SHA-256 is
`17ada1bd75f5174e8510d75131ba2ca880538302619eda9c633bc16710caa4f2`.

Root ran the unchanged external refusal probe against that library: all nine
invalid cases now return -8, preserve saved state and produce the exact full
1,272-sample continuation of the populated twin. Both valid controls still pass.
The cold-instance sensitivity remains 0.029786840081214905. Root verified all
55 files in `artifacts/aud144-preset-envelope-r2/SHA256SUMS`, whose index hashes
to `8adc1cc21d9cb9434e700bf75de4f7aab9d2138e0435458afad6e4702f6f5e94`.

Independent compatibility probing then found a regression. The bridge explicitly
routes legacy FletcherMunson construction to LoudnessCompensation, and engine
defaults migrate that legacy type. A genuine exported FletcherMunson preset
imports successfully into LoudnessCompensation with the pre-validator library:
full state and all 9,464 output samples match the fresh FletcherMunson source
exactly. The r2 validator instead returns -8 for a family mismatch. This is an
unmodified legacy document, not the intentionally forged different-family case.
Its nondefault audio differs from default LoudnessCompensation by up to
0.27916882932186127, so the fixture detects a missing import.

The unchanged migration probe is `tools/aud144_fletcher_munson_compat_probe.py`.
Its pre-validator run exits 0 and r2 run exits 1. Complete documents, states,
waveforms and command receipts are retained under
`artifacts/aud144-fletcher-munson-compat-r1/`; the 15-file checksum index is
`8412ef3bdbf97e9739bdc5ec4fb9d22fa8010ef872ffa5b48ed73e77651b42f4`.
Reverse migration is not covered by this probe. Luna must preserve the supported
legacy migration while retaining the invalid-envelope protections. Both public
probes must pass against the next immutable library before this is ready for
Astra review. Existing r1/r2 evidence remains unchanged.

## r3 compatibility correction and verification

Luna made preset acceptance directional: current LoudnessCompensation handles
accept saved FletcherMunson documents, including the factory aliases. Ordinary
same-family aliases remain accepted; unrelated families remain refused. Reverse
LoudnessCompensation-to-FletcherMunson import remains outside this selected
legacy migration contract; no claim is made that reverse import previously
failed. The helper's name and argument names now express target versus saved
identity.

Focused public-C-API tests pass 3/3. The new migration test uses genuine exported
state, nondefault values, full-state equality, exact audio and a default-output
sensitivity control. Full FFI library tests pass 82 with one manual utility
ignored; strict all-target Clippy and the shared-library build pass. All commands
ran offline and locked under the shared Cargo flock; header synchronization was
skipped. The 340 selected build-start and build-end source hashes match exactly,
manifest SHA-256
`4d25271308fe05ee76c18004bfaa880b98b6c80de6f15afff52a320147879aba`.
This remains a selected-source receipt, not a full transitive snapshot.

The immutable r3 library SHA-256 is
`7311b8fa7044084096bbdb219f56656673d7feef86bd8c58943a4147c36e71a8`.
Both unchanged root-authored external probes terminate with exit 0 against it:

- All nine invalid envelopes return -8, with unchanged saved state and exact
  complete continuation against the populated twin. The two valid controls
  succeed and match freshly prepared audio, as required for successful import.
- Genuine legacy migration returns 0, with exact state and all 9,464 output
  samples matching the source. Default-output sensitivity remains
  0.27916882932186127.

Root independently verified both result files, library and probe identities,
matching selected manifests, every stored waveform hash and length, and full
byte equality against the relevant controls. The root verification receipt is
`artifacts/aud144-preset-envelope-r3/root-probe-verification.json`, SHA-256
`da61a5a1092cec59079686ddd8c114f9921ea204067a3f2590695945f1190f6a`.
The packet includes copies of the validator, identity helper and regression
test sources. This closes the executed regression and preserves the original
refusal behavior. Astra medium independently accepted the sealed r3 checkpoint
after checking the validator, migration contract, full vectors and terminal
evidence. AU runtime, callback allocation and full-workspace integration
are not established by these C-ABI probes.

A final test-only rustfmt adjustment was followed by separate `*-r2.log`
focused/full/lint/build gates, all passing with the same test counts. The rebuilt
library is byte-identical to the probed immutable r3 library. The broad final
340-file manifests differed on the concurrently edited host integration test
`sotf-host/tests/native_vst3_host.rs`, which is not compiled by this FFI library
build. Both broad manifests are retained. Excluding that named test gives
matching 339-file final selected manifests, SHA-256
`3bbd068c7489de6725dd1c7f6b510f1c0189e8c6217921c863633aa5f27c614a`.
Root verified that it is the only omitted/differing path and checked the final
identity helper source copy. Original and formatted source copies are retained
separately under `sources/` and `sources-r2/`; the earlier 340-file equality
claim applies to the first r3 build only.

Root verified the final sealed r3 checksum index; Astra rechecked all 76 entries
and corrected the earlier bookkeeping count of 75. Index SHA-256 is
`1087e2b392de89ef9192fc3c57e6daf66941d0b12c570a7e11687a0986cba3bc`.
The final execution receipt hashes to
`ad5d1a003a330a62773d603385601aca8c4ceb43734252df7d96ca79741622e6`.

## Required public regression and correction

1. Export an actual populated handle through the C API and prove that its
   unmodified document imports successfully into a compatible fresh handle.
2. Change only `plugin_type` to a different real plugin family while retaining
   the valid state bytes. Require rejection before replacement, unchanged
   saved state and continuation audio matching an untouched populated twin.
3. Exercise unsupported schema versions, missing or wrongly typed required
   identity fields, and a wrong preset type marker. Keep descriptive names
   editable. Preserve existing supported same-family aliases and tracked
   pre-edit version-1 preset fixtures.
4. Validate the envelope at the preset-import boundary before applying its
   state. Preserve the separate raw `plugin_load_state` partial-update contract
   and the existing size limits and transactional replacement behavior.
5. Run focused regressions, full FFI library and strict lint; obtain Astra
   medium review of Luna's correction. A matching preset label alone is not
   evidence that the running plugin retained its audio history on rejection.

This issue is distinct from AUD139's missing-shape legacy migration and band
key validation. It does not claim that malformed state can bypass the existing
state validators or that any memory-safety defect has been demonstrated.
