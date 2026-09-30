# AUD144 public preset-envelope refusal reproduction

This is the pre-correction red checkpoint from 2026-09-30. Build exit code is
0; the public probe exits 1 because all nine invalid envelope cases are accepted.
The two valid-import controls pass. See `observations/result.json` for exact
outcomes and `../../ffi-preset-envelope-validation.md` for the interpretation.

The command was:

```sh
python3 audit/tools/aud144_preset_envelope_probe.py \
  crates/sotf-plugins/target/audit-artifacts/aud144-preset-envelope-r1/libplugins_ffi.so \
  audit/artifacts/aud144-preset-envelope-r1/observations
```

The real library and its hash are in `build-receipt.json`. Build manifests cover
339 selected source/header/config files; all match across the actual build.
`sources/` preserves the exact FFI import/factory/header bytes and executed
probe, separately checked against that manifest. The library is preserved under
the target directory, not duplicated in this packet. The original build uses
`SOTF_FFI_SKIP_HEADER_SYNC=1`; this packet does not verify header generation.

`observations/` retains complete exported/mutated documents and complete finite
stereo live/twin/cold continuation vectors. A saved-state equality check alone
would miss the rejected-import history loss. Valid import deliberately resets
the prepared audio; invalid envelopes must instead leave the live instance
untouched. This is a control-thread API/continuation test, not a realtime heap,
standalone DSP accuracy or native AU test.
