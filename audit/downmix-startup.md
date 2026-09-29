# AUD081 Downmix negative-origin window — verified

2026-09-28. Source frozen. No drain, LFE recursion, public latency, schema, host, native wrapper, dependency, MIDI or IAMF changes.

## Minimal correction

One `discard_synthesis_prefix` boolean identifies the first negative-origin spectral window. Shared `clear_stream_state()` now starts spectral modes with H=1024 zero input history and read positionH, while preserving the existing N=2048 startup-delay counter. Constructors, from_params and setup mode changes use that shared state initializer; initialize/reset already did so. Simple mode keeps zero prefix/read/delay.

After synthesizing the first window, clear the finalized negative hop from the accumulator and omit that hop from available output. Its nonnegative overlap remains intact. Later windows retain existing output-fill/next-add behavior. The skipped cells are cleared before any circular-buffer wrap can reveal stale samples.

Independent source comparison against the saved pre-edit file confirms the entire per-sample `process` scheduler and all `process_fft_block` signal/phase/LtRt/FFT/OLA arithmetic before the scheduling epilogue are unchanged after whitespace normalization. One existing reset-state assertion now expects the intended1024-frame synthetic input prefix.

The audible change restores lost/attenuated initial programme samples at the existing2048-frame delay. Matrix routing, recursive LFE filtering, coefficient update formulas and parameter semantics are preserved. Explicit EOS still returns the previous default completion; ordinary zero continuation is required to expose stored spectral output, and recursive LFE response remains a separate duration-policy scope under AUD073.

## Red → green evidence

`/tmp/sotf-downmix-startup-red.log`: all three initial independent tests fail; the first expected0.25 marker emits0, dense first programme samples emit0, and constructor startup has the same defect.

`/tmp/sotf-downmix-startup-green.log`: those three pass after the reviewed prefix correction.

Final six new functions in `tests/startup.rs` cover:

- 480 independent dense waveform fixtures: all12 named channel layouts × two spectral modes × four rates44.1/48/96/192k × five callback patterns (1,17/137,N,4096/137,8193). The source extends beyond a4N output ring wrap. The oracle independently specifies direct front L/R routing or configured mono-center attenuation, then prepends2048 zeros. Fixed absolute FFT reconstruction tolerance1.5e-6; declared startup silence is exact.
- 2048 initial-hop impulse fixtures: every0..1023 position in both spectral modes. Each marker is also the final real programme sample. Only zero continuation follows, and the entire output waveform is checked against the independent delayed marker.
- Constructor processing, explicit parameter construction, pre-initialize mode changes, reset and sample-rate reinitialization establish equivalent timelines. Tests compare whole waveforms and external output canaries.
- All12 simple named layouts retain exact immediate front routing.
- 72 prepared cold-thread configurations: all12 layouts × Simple/Phase/LtRt ×44.1/192k. Measure first large callback, reset and137-frame continuation with explicit allocation AND deallocation counters. All record0 allocations and0 frees; output remains finite.
- Existing independent LtRt decoder, surround quadrature/polarity, phase magnitude/image, coefficients, mode constraints and numerical WOLA tests remain green at their unchanged tolerances.

## Commands

```text
cargo test -p sotf-plugin-downmix
61 passed; 0 failed; 0 ignored
/tmp/sotf-downmix-startup-full.log

cargo clippy -p sotf-plugin-downmix --all-targets -- -D warnings
Passed
/tmp/sotf-downmix-startup-clippy.log
```

Scoped rustfmt --check and git diff --check passed. No assertion tolerance was weakened.

Current wave files: `src/lib/downmix_plugin.rs`, one reset expectation in `src/lib/tests.rs`, new `tests/startup.rs`, README and CHANGELOG. Other prior dirty work remains preserved.
