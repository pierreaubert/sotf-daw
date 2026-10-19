# sotf-plugin-analog-compressor

Analog compressor — a single-band bus-style compressor with an analog
coloration stage.

## What It Does

Linked feed-forward detection (threshold, ratio, attack, release, knee),
static plus auto makeup, and parallel mix run first; the output then passes
through one of six `math-analog` coloration models. With color and trim at zero
it is a clean bus compressor. Raising drive and color adds the selected model's
nonlinear character. Choosing a color model does not change the detector topology
or make the compressor a measured VCA, opto, or FET circuit emulation.

## Features

- **Bus-style detection**: hottest channel drives one shared envelope
- **Full comp vocabulary**: threshold, ratio, attack, release, knee, static
  and auto makeup, parallel mix
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Zero latency**

## Mix and detector semantics

The signal flow is **linked peak detector → gain/makeup → compression Mix →
analog color/trim**. Mix blends the uncompressed input with the compressed,
made-up signal before coloration. **Mix 0% still applies analog color and trim.**
To obtain a dry identity, also set color and trim to zero or bypass the plugin.
Detector, hold and automatic-makeup state continue to advance at zero Mix.

The maximum absolute sample across all channels drives one shared linear-amplitude
envelope. Attack/release use `exp(-1/(time_seconds*sample_rate))`; they specify
the envelope's exponential time constant, rather than the time for output gain
to settle. The soft knee operates in dB after detection. Hold freezes a falling
detector for the rounded frame count; an equal/higher input rearms it. Range
limits gain reduction before makeup and parallel mixing; 120 dB disables that cap.
Auto makeup follows reduction in dB with 10 ms attack and 500 ms release, capped
at +24 dB. Its state is retained while disabled and resumes when enabled.

The compressor has no lookahead. A cold attack can pass an onset above threshold;
this is not a peak-ceiling limiter. Dynamics control changes apply at the next
frame, preserving detector state during timing edits. The color stage owns its
own smoothing. Successful initialization clears detector, hold, auto-makeup and
color histories for a fresh stream.

## Realtime controls and validation

Primitive scalar validation/setters reuse prepared metadata and require no heap
allocation or free. Valid bulk updates and canonical same-model snapshots require
no new allocations, but consuming the caller-owned map can free its storage.
Actual host callback tests apply queued numeric/Boolean controls without heap
activity; immediate setters and event producers remain control-thread APIs.
Model replacement requires reconstruction after initialization.

```bash
cargo test -p sotf-plugin-analog-compressor
cargo test -p sotf-plugins --test audit_analog_compressor_host
```

Independent f64 physical-time references cover complete attack/release, hold,
range, makeup, Mix and hottest-channel control sweeps at 44.1/48/96/192 kHz,
including first-sample startup. The fixed normalized waveform error bound is
3e-4; coefficients/envelopes in the production host primitive remain f32.
Measured soft-knee edge tests check value and first-derivative continuity.
Every color model verifies compression/Mix-before-color ordering, including the
colored zero-Mix case. Host preset reload and queued controls compare with native
audio across callback partitions.

`tests/audit_profile.rs` records 512-frame block timings at 48/192 kHz, 2/8/32
channels and all six models, with color-off baselines and color at 70%. A local
diagnostic receipt is stored at
`audit/artifacts/plugin-audit-20261018/analog-compressor-profile.json` in the
workspace. These optimized test-build samples depend on hardware and system load;
they are not a deadline guarantee, a portable benchmark, or evidence of hardware
emulation accuracy. No inferred color-model performance ranking replaces measurement.

The receipt retains an earlier loaded run with high-channel scheduling/deadline
excursions alongside the final baseline/color matrix. Inspect both observations
and profile the intended deployment before relying on 32-channel high-rate operation.
