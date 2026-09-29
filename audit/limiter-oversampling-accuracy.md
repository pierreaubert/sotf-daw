# Limiter oversampling: independent public accuracy tests

## Scope and ownership

Added only `crates/sotf-plugins/crates/sotf-plugin-limiter/tests/oversampling_accuracy.rs`.
No production, host, manifest, or shared test files changed. The production owner
is `/root/spectral_accuracy`; its separate tests cover automation, finite drain,
allocation/deallocation, and telemetry. This report does not claim those gates.

All fixtures instantiate the public `LimiterPlugin::new`, set the structural
`oversampling` integer choice before initialization, and call the public
`ParametricInPlacePlugin` processing interface. They do not reconstruct the new
implementation from a generic oversampling wrapper and a separate limiter.

## Verified numerical contracts

Six tests passed in 3.59 seconds:

1. Independent f64 Hann-sinc reconstruction detects a >1.1 peak in a sequence
   whose samples have magnitude at most 1; zero padding leaves the result exact.
   The finite reconstruction kernel is generated from its mathematical formula,
   never the production detector. This is a finite reconstruction oracle, not
   an assertion about all possible continuous-time reconstruction filters.
2. **840 burst configurations:** rates 44.1/48/96/192 kHz × sample/ISP modes ×
   1x/2x/4x × lengths 1/2/3/5/13/31/127 × five deterministic burst patterns.
   Final emitted sample peaks remain below −12 dBFS + **0.00001 dB**, and ISP
   reconstructed peaks remain below −12 dBTP + **0.1 dB**. No tolerance changed.
3. **24 coherent tone renders:** sample/ISP × 7/11/17/21 kHz × 1x/2x/4x at
   48 kHz. A one-second coherent window after one second of settling measures
   the third-harmonic folded bin and fundamental independently in f64. All
   fundamentals stay within 0.25 dB of the −12 dB ceiling, precluding blanket
   attenuation as an alias improvement. Final sample/ISP peak checks also apply.
4. **48 nonlinear waveform configurations:** four rates × 1/2/6 channels ×
   2x/4x × sample/ISP. Whole 12051-frame calls, 1/127/257-frame calls, and mixed
   callbacks containing 9217 frames produce **bit-identical complete output**.
   The same prepared instance is reset between renders, testing history reset.
5. **108 impulse configurations**, each repeated after reset with two partitions:
   four rates × 1/2/6 channels × 1x/2x/4x × sample 0 ms/sample 0.37 ms/ISP
   0.37 ms. Both frame-zero and frame-2047 impulse peaks occur at the independent
   declared delay. Oversampled delay is 512 + floor(Fs*lookahead_ms/1000) plus
   4D for ISP (D=6 below 96 kHz, 12 below 192 kHz, otherwise zero). Native delay
   retains lookahead +3D. Peak magnitude checks prevent a silent-output pass.
6. **24 final-mix configurations:** four rates × 1/2/6 channels × 2x/4x.
   Dry output equals the original interleaved sequence delayed by the complete
   declared latency, sample for sample. Half-wet output agrees within 1e−7 with
   one independent aligned blend of that dry stream and a separate fully wet
   render. These tests intentionally do not impose a ceiling on dry/mixed audio.

## Measured alias evidence

Third-harmonic folded-bin amplitude relative to the fundamental, in dBc:

| Mode | Frequency | 1x | 2x | 4x |
|---|---:|---:|---:|---:|
| Sample | 7 kHz | −56.727 | −58.138 | −58.304 |
| Sample | 11 kHz | −53.857 | −67.248 | −83.112 |
| Sample | 17 kHz | −56.951 | −94.802 | −83.396 |
| Sample | 21 kHz | −50.762 | −57.297 | −62.057 |
| ISP | 7 kHz | −127.546 | −102.794 | −96.017 |
| ISP | 11 kHz | −87.934 | −95.675 | −99.181 |
| ISP | 17 kHz | −121.117 | −116.430 | −100.501 |
| ISP | 21 kHz | −78.419 | −85.452 | −92.632 |

The regression retains >10 dB improvement at 2x and >20 dB at 4x for sample-mode
11/17 kHz only, based on the earlier isolated prototype. It deliberately makes
no universal ISP advantage or monotonic factor claim: the table contradicts
both. The production ISP path differs from the earlier sample-core prototype,
so its new measured values are reported directly, not substituted silently.

## Verification and limitations

Commands used the existing Cargo target with the approved spacious TMPDIR:

```text
cargo test -p sotf-plugin-limiter --test oversampling_accuracy -- --nocapture
cargo clippy -p sotf-plugin-limiter --test oversampling_accuracy -- -D warnings
```

Results: **6 passed, 0 failed, 0 ignored**; strict Clippy passed. Rustfmt and
scoped `git diff --check` passed. Logs:

- `/tmp/sotf-limiter-oversampling-accuracy-first.log`
- `/tmp/sotf-limiter-oversampling-accuracy-clippy.log`

The initial compile exposed only a test-fixture `ProcessContext` field-path
mistake, corrected to `context.transport.sample_position`. No production
failure was found and no numerical assertion was weakened. Finite-stream
draining, live automation ordering, cold heap ownership, and native/factory
integration are separate owner gates; these tests use explicit zero continuation
when observing delayed program output.
