# sotf-plugin-aae

Active Acoustic Enhancement (AAE) plugin — LARES-inspired multichannel reverb. Takes stereo input and outputs multichannel audio (5.0–9.1.6) with synthesized early reflections and late reverberation distributed across speakers via VBAP.

## Architecture

- `lib.rs` — main plugin struct implementing `Plugin` (variable in/out channel counts).
- `params.rs` — parameter definitions (`AaePluginParams`, `build_parameters`).
- `delay_line.rs` — pre-delay and tap delays.
- `early_reflections.rs` — tapped-delay early-reflection generator with per-tap VBAP routing.
- `fdn.rs` — 8-line Hadamard feedback delay network for late reverb (time-variant).
- `hadamard.rs` — Hadamard mixing matrix.
- `tone_filter.rs` — global tone shaping for the reverb tail.

## Signal Flow

```
Stereo input → mono downmix → pre-delay → input diffusion →
  early reflections (tapped delay, per-tap VBAP)
+ FDN late reverb (8-line, Hadamard, time-variant)
→ multichannel speaker routing → output mixed with dry signal
```

## Features

- `qa` — enables `sotf-host/qa` and the `qa-aae` benchmark binary.

## Testing

```bash
cargo check -p sotf-plugin-aae && cargo clippy -p sotf-plugin-aae
cargo test -p sotf-plugin-aae
cargo run -p sotf-plugin-aae --features qa --bin qa-aae
```

## Important Notes

- Implements `Plugin` (not `ParametricInPlacePlugin`): output channel count differs from input (stereo → 5.0–9.1.6).
- VBAP routing depends on the active speaker layout from `sotf-host::speaker_config`.
- Pre-allocate FDN/delay-line buffers in `build()`; never resize on the audio path.
- Delay-tap crossfades must use constant-sum weights, including inside FDN
  feedback. Correlated taps must not acquire the equal-power midpoint boost.
- AAE is zero-latency through its immediate direct path. Room preset and output
  layout are structural; rebuild rather than mutate them live.
- The LFE effects send is LR4-low-passed at 120 Hz and is not complementary
  bass management. Spatial rows must never route ER/FDN energy into LFE.

- Feedback limiting must be odd, continuous, monotone and bounded by its ceiling.
  Preserve unity below half the ceiling and a matching unit slope at that knee;
  do not switch directly from identity to tanh at the ceiling.
- `audit_fdn_limiter` checks threshold-adjacent recursive responses across four
  rates, both extreme room sizes and three modulation depths. It does not
  certify arbitrary modulated RT60, recursive finite tails or listening quality.

- An active room fade owns its two read heads until completion. Coalesce room
  retargets in one fixed latest-target slot; do not restart active fades or grow
  head count. A request for the active destination cancels queued changes.
- Keep pending damping gains current in setters, including independent RT60
  changes. Admission must use prepared gains with no power calculation/heap work.
- Reset installs the latest requested room/decay endpoint and clears coefficient
  ramps as well as recursive state, so output matches a fresh endpoint network.
- `audit_room_retarget` compares sample-exact staged twins at multiple fade
  phases, rates and modulation depths; it does not establish perceptual acceptance.

- Predelay uses one latest pending target and preserves the active two-head
  fade, matching room retarget semantics. Returning to the active destination
  cancels pending work. Reset/initialize must install the latest requested delay
  and clear its pending slot, including zero-delay targets.
- Public predelay request values may lead the audible fade by up to the remaining
  fade plus one 10 ms fade. Keep direct graph latency and delay capacity unchanged.

- `qa-aae-decay` is an explicit offline 208-row corner diagnostic, separate from
  callback timing and existing acoustic QA. Preserve the full matrix, raw failed
  rows, 10% neutral target and r² > 0.98 checks; do not trim sparse failing cases.
  Band fits are frequency-region observations, not endpoint-ratio guarantees.

- Reject invalid/overflowing buffer geometry, non-finite input and wrong clocks
  before output, recursive DSP or excitation metadata changes. Preserve established
  AAE input/output size diagnostics.
- Empty/reset/reinitialized tail is Finite(0); accepted excitation conservatively
  makes it Infinite until reset/reinitialize. Bypass/muted sends/silence cannot
  establish finite support. Active EOF must reject without consuming state;
  never inherit default drain completion or invent support from RT60.
- AAE preserves frame count despite changing channels; identity frame geometry
  and the output envelope must agree with every successful process call.

- FDN reads before push, while `DelayLine::read(0)` returns the most recent push.
  A tap index `m` therefore traverses `m+1` process clocks. Include that clock in
  constructor, active/pending prepared damping gains and reset; do not change tap
  arrival timing to conceal an attenuation-clock mismatch.
- Preserve `audit_neutral_decay`'s independent f64 rings/dense matrix and neutral
  exponential-weighting checks. It excludes modulation/limiting and does not
  override the 208-row fit gate: an exactly weighted sparse response can fail r².

- Colored FDN shelves must account for filter memory: solve `H(r_dc)=g_dc` and
  `H(-r_ny)=g_ny` at `r=g^(1/M)` for the full loop length, not only `H(+/-1)`.
  Preserve equal-gain scalar attenuation and public standalone unit-circle APIs.
- Keep prepared coefficient targets (not only gains) for queued rooms. Admission
  may copy targets/start ramps but must not run radial powers/design or allocate.
  Setters/reset/construction own design work. Preserve f64 coefficients/state so
  near-canceling fast-endpoint numerators do not lose relative accuracy.
- Keep independent radial constraints, the exact neutral lossless reference,
  lifecycle/heap tests and the unweakened 208-row fit gate. Endpoint correction
  does not make a sparse impulse response exponentially dense or establish
  measured/listening quality, deadline certification or finite tail support.

- Do not retain f64-subnormal tone-filter output state across silent samples: a
  near-unit pole can round it back to itself forever. Clear only that state,
  preserving normal f64 values and current f32 output. Keep f32-subnormal audio
  intact; it is normal in f64. No global floating-point-mode changes are needed.
- Keep long-silence and seeded-state regressions plus the unflushed recurrence
  comparison. Preserve nonzero output bits/numerical zeros; signed silent zero
  bits may differ. This cleanup must not change excited Infinite tail metadata.

- Keep `qa-aae-timing` separate from the legacy wall/load gate. Preserve all paired
  callback samples and the fixed five-second full-stream/spatial-prelude matrix.
  Missing CPU clock data is unknown, not zero; it must produce an incomplete result.
- CPU spans bracket wall intervals and include sampling work. Preserve negative
  wall-minus-CPU values; do not subtract calibration to obtain passing deadlines.
  CPU excludes other-thread work and does not establish a causal waiting source.
- Preserve both allocated-only and explicitly touched timing cohorts with two
  repetitions each in alternating ABBA/BAAB order (64 cases). Retain every wall
  failure and verify identical audio hashes; preparation is not memory pinning.
- Resource snapshots describe the whole process, including instrumentation and
  other threads. Keep unsupported/decreasing data explicit; do not use aggregate
  faults/context switches to attribute a particular callback's elapsed gap.

- Keep offline export duration/fade policy distinct from native support. The
  reference caller feeds stereo silence through DawHost, counts tail frames from
  source EOF, applies one common output-only half-cosine fade and reports duration
  truncation. Do not stop on quiet callback gaps or write the fade into feedback.
- Preserve `audit_offline_tail`'s independent unfaded continuation, partition,
  silent-predelay, short-budget, zero-endpoint and continued-state checks. Its
  finite after-cap energy is observed omitted energy, not an infinite remainder
  bound or listening acceptance. Excited metadata stays Infinite.
