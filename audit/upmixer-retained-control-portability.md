# AUD132 retained controls across x86 and ARM64

## Attribution

The six historical output digests in
[the AUD132 timing report](upmixer-above512-hr-timing.md) are unchanged.
Gitea [run 938](http://192.168.1.32:3001/pierre/all_of_sotf/actions/runs/938)
reproduced every digest exactly on Linux x86 with Rust 1.99.0, DAW revision
`ce7b549feee25c7a312c9026470960bcea686ee7`, and the canonical stereo input.
Its captured full outputs are now retained as test fixtures. Each fixture is
checked against its original FNV-1a digest before it is used as a reference.
`tests/data/aud132-preedit/retained-controls.json` records SHA256 values,
source and CI provenance, frame counts, and the input hash.

The same source and input produced byte-identical Mac and Linux ARM64 outputs.
Five ARM64 output hashes differed from x86; the N=2 HR-off vector was identical.
Every emitted frame count matched. Full-vector comparison measured:

| FFT size | HR | Maximum absolute delta | RMS delta |
|---:|:---:|---:|---:|
| 2 | off | 0 | 0 |
| 2 | on | 1.490116119e-8 | 3.691182114e-10 |
| 256 | off | 5.960464478e-8 | 7.513239687e-9 |
| 256 | on | 8.940696716e-8 | 1.008456283e-8 |
| 512 | off | 5.960464478e-8 | 8.550652488e-9 |
| 512 | on | 1.192092896e-7 | 1.382454327e-8 |

The unchanged controls passing exactly on x86, with ARM64 differences at f32
roundoff scale, attributes the reported failures to cross-architecture
floating-point arithmetic for this fixture. It does not identify a particular
FFT instruction or establish general upmixer quality.

## Portable regression contract

The test still renders the actual plugin stream at 48 kHz, stereo identity
routing, callback partitions 17/137/256, and the existing 512-frame EOS drain.
Production DSP, scheduling, latency, parameters, and gain fixtures are unchanged.

All targets compare every output sample against the recovered x86 vector.
The maximum absolute difference is limited to one unit-scale `f32::EPSILON`
(`1.1920928955078125e-7`); RMS difference is limited to one eighth epsilon
(`1.4901161193847656e-8`). These fixed precision-based limits bound both a
single-sample deviation and accumulated error. There is no relative tolerance,
rescaling, retiming, sample exclusion, or reference regeneration. Sample count,
finite values, and the existing latency assertion remain mandatory.

x86_64 additionally retains the original bit-exact output digest assertion.
The fixture digest guard applies on every architecture, so changing fixture
bytes cannot silently change the historical reference.

A negative control requires rejection of a one-frame timing shift, a truncated
tail, a one-part-per-million gain change, and sustained sub-epsilon bias that
exceeds the RMS limit.

## Verification

The unchanged six-cell x86 test and all four scoped compilation checks passed
in run 938. Native Mac qualification with Rust 1.99.0 and the ONNX feature passed:

- Focused `aud132_` tests: 9 passed, zero failed or ignored. The retained
  controls report the maximum and RMS differences shown above.
- Full Upmixer package, including integration tests: 199 passed, zero failed,
  three unchanged intentionally ignored rejected-candidate tests.
- Strict all-target Clippy (`-- -D warnings`), rustfmt, and `git diff --check`:
  passed.

The updated source still requires x86 CI qualification before release acceptance.
