# AUD118 — small surround FFT initialization

The seeded per-channel velvet filter clamped its nominal sequence length between
128 and half the FFT size. Public surround initialization therefore panicked for
FFT64 and FFT128, including with LR decorrelation bypassed or set to LFO. The
lower bound now shrinks to the available half-window. The separate LR generator,
pulse placement, fades and phase normalization are unchanged.

## Evidence

- Three public tests failed before the correction and pass afterward: 72
  configurations across FFT64/128, 44.1/48/96 kHz, 5.1/7.1.4, velvet/bypass/LFO
  and HR-direct on/off. Irregular callbacks accept all frames, produce finite
  audio, report latency N, and reproduce fresh output after reset. Four public
  stereo-to-surround changes also match fresh instances.
- Two independent filter tests failed before and pass afterward. Fixed pulse
  fixtures and a direct f64 DFT check complex phase, unit magnitude, DC/Nyquist
  and empty-sequence identity. Nominal length does not bound every pulse offset.
- Eight fresh-thread small surround configurations have zero allocations and
  frees in first processing and processing after reset.
- All 157 Upmixer tests pass, with no ignored tests; strict all-target Clippy
  passes. Logs: `/tmp/sotf-upmixer-small-fft-{red,filter-red,green,full,clippy}.log`.
- Before/after captures match exactly for 34 cases: FFT64/128 stereo and
  FFT256/512/1024/2048/4096 across stereo/5.1/7.1.4, LR bypass on/off, at 48 kHz
  with HR-direct and AutoGain off. All 2,658,768 audio samples and latency headers
  are identical. The 10,635,344-byte artifacts have SHA-256
  `44c43b96b5cff43797d2c6b262e82df9ff5bedc1717d842b8dffdb6e4e2a5a46`.

The captures, manifest and temporary probe source are preserved under
`crates/sotf-plugins/target/audit-tmp/upmixer-small-fft-*`. The former external
Cargo target disappeared before this check; these artifacts use the new ignored
workspace-owned target. No temporary example remains in the source tree.

The subsequent checkpoint 17 passes 5,903 workspace and 66 FFI tests, with ten
skipped and MIDI/IAMF package tests excluded. Its host/private meter optimization
also retains the existing Upmixer causal-clock and reference-delay regressions.

This verifies the concrete 64/128 failure and ordinary-size compatibility. It
does not validate every accepted power of two: FFT1's zero-hop geometry remains
a separate source-only finding. Preparation and layout changes still allocate
on the control thread.
