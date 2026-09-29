# AUD118 — small-FFT seeded decorrelation geometry proposal

Read-only proposal following the frozen AUD115 checkpoint. No decorrelation source or tests changed. Reproduction details and the first real failure are preserved in `/tmp/sotf-upmixer-small-fft-panic.md` and `/tmp/sotf-spatial-autogain-private.log`.

## Confirmed failure and reachability

Public `UpmixerPlugin::from_params` with FFT64, 5.1, defaults for velvet decorrelation, then `initialize(44_100)` panics at `decorrelation.rs:377`: `seq_len.clamp(128, 32)` has inverted bounds. No audio was submitted. FFT128 has the same invalid bound by inspection (upper64); that case has not yet been executed.

`initialize` unconditionally calls `generate_per_channel_decorrelation_filters`, including when the separate LR decorrelation route is bypassed or uses LFO. The per-channel path assigns identity to front/LFE speakers and calls the seeded velvet generator for rear/height speakers. Thus selecting an output layout with those speakers exposes the failure; a stereo/front-only layout avoids that call. `setup.rs::change_speaker_config` also invokes the per-channel generator, so switching a valid small stereo instance to surround is another source-backed route requiring a permanent test.

The public constructor accepts power-of-two FFT sizes without a256 minimum. Existing neutral FFT64 stereo/bypassed fixtures work, so rejecting every size below256 would remove functioning configurations.

## Minimal recommended correction

Change only the seeded generator's sequence-length clamp:

```
let maximum = self.core.fft_size / 2;
let seq_len = seq_len.clamp(128.min(maximum), maximum);
```

The nominal minimum is reduced to the available half-window when N<256. For N>=256 this is exactly the existing clamp; RNG seed/advancement, pulse placement, fade, FFT, phase normalization, DC/Nyquist values and output copying are unchanged. The newly usable small seeded filter uses nominal sequence lengths32/64 at FFT64/128.

Do not change the separate LR velvet generator in this narrow fix. It currently uses `seq_len.min(N/2).max(128)`: at small N its nominal sequence length can exceed the stated half-window and even N, but pulse indices are explicitly clipped to N−1, fades iterate within the actual vector, and the measured public stereo64 route is valid. Its small-size phase distribution differs from the proposed seeded rule; no independent evidence currently proves that phase choice wrong. Preserving it avoids changing already functioning stereo64 waveforms merely to unify code. A future convention cleanup should state the audible compatibility change separately.

Do not change pulse clipping to `seq_len−1`: the current offset can place a pulse beyond nominal `seq_len` but within N. That is existing behavior for ordinary geometries and changing it would defeat the bit-identical256+ requirement. Consequently `seq_len<=N/2` is a nominal pulse-cursor/fade extent, **not** a claim that every generated time-domain coefficient is zero after N/2.

FFT1 also passes the constructor's power-of-two assertion while implying zero hop. This is a source-only preexisting boundary risk, not a verified working route; do not claim the clamp correction validates every possible power of two. A separate minimum-geometry contract can be considered if required. The reviewed implementation/test scope can target the concrete64/128 failure and preserve ordinary256+ behavior.

## Permanent red and independent tests

1. Before editing, add public initialization/process tests for FFT64 and128 with 5.1 and a height layout, rates44.1/48/96k, default velvet plus the bypass/LFO reachability variants. Capture the current panic through ordinary test failure. Do not add production panic catching.
2. Include the public layout-change route: prepare small stereo then switch to surround. Verify reported output width, full frame acceptance, finite output, reset/fresh equivalence, and unchanged latency N. Keep mode/layout changes on the control thread as before.
3. Retain meaningful negative controls: FFT64 stereo waveform and explicit renderer/decorrelation bypass waveform captured before the change; their old paths must remain exact. Capture raw enabled waveform/filter digests for256/512/1024/2048/4096 across layouts, then require identical post-change values.
4. For new small seeded filters, independently derive known pulse positions/signs from the fixed seed fixture, apply the documented fade and direct f64 DFT, and compare normalized complex response. Require finite unit magnitude for interior bins and exact real unity at DC/Nyquist. Include a very low pulse-density case where the short sequence is empty: existing identity fallback must remain safe. This checks the intended finite all-pass coefficients, beyond merely avoiding a panic.
5. Process impulses/dense deterministic stereo across1/17/137/8193-frame callbacks; check accepted/emitted counts and finite output, plus existing neutral delayed-identity oracle. Do not demand partition invariance for unrelated callback-sized raw smoothers under live parameter changes.
6. Cold first process and reset should remain0 allocations/frees; preparation remains allocation-permitted. Full Upmixer suite and strict all-target Clippy after the focused tests. No shared helper, host queues, engine or native changes.

The proposed source delta is one local geometry expression plus its explanatory comment. Ordinary256+ all-pass filters remain bit-identical by construction; small rear/height configurations become usable without removing established small stereo/bypass support.
