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
