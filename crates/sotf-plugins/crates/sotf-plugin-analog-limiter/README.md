# sotf-plugin-analog-limiter

Analog limiter — a mastering limiter with an analog coloration stage.

## What It Does

The proven limiter core (ceiling, release, lookahead, soft knee, true peak,
mix) runs first; the output then passes through one of six `math-analog`
coloration models. With color at 0% it behaves exactly like the clean
limiter for signals below the ceiling (hot signals agree within the
dB-conversion epsilon; see contract); raising drive and color adds
tape/console-style finishing character staged at 0 VU = −18 dBFS.

## Features

- **Full limiter core**: no fork — composition over `sotf-plugin-limiter`
- **Opinionated subset**: threshold, release, lookahead, soft knee, true
  peak, mix; advanced detector options stay at core defaults
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Core lookahead and true-peak protection latency reported**; the color stage adds none

## Consumer notes

- `analog_model` is structural: apply a model change by reconstructing the
  plugin or restoring a saved snapshot. Live `set_parameter` on an
  initialized instance refuses and keeps prior state and history.
- FFI callers enumerate the six models by 0-based index (`analog_model`
  scalar range 0–5); the family exposes no per-model label strings.

## Output ceiling contract

`threshold` is the final emitted sample-peak ceiling when `mix` is fully wet
(100%): after the limiter core and the analog color stage, a zero-latency
per-channel clamp limits emitted samples to the threshold. Analog drive, color,
character, and trim cannot push fully wet output past the ceiling.

- A dry blend (`mix` below 100%) can exceed the ceiling, exactly like the clean
  limiter core.
- `true_peak` enables ITU-R BS.1770-compatible inter-sample peak detection in
  the core detector. It is detection only: with no output ISP correction
  stage, no strict output true-peak guarantee is claimed before or after
  color.
- Signals already below the ceiling pass the guard bit-exactly (it never
  writes in-range samples, so it cannot blanket-attenuate). With color at 0%,
  below-ceiling output is bit-identical to the clean limiter; hot signals
  agree within the dB-conversion epsilon (2e-6 relative), the only divergence
  between the precise guard ceiling and the core's fast clamp ceiling.
- Automation: the guard tracks the threshold/mix targets immediately, while
  the core detector threshold and blend smooth one-pole over 5 ms. A downward
  threshold step therefore flat-tops at the new target from the first sample
  (stricter than the core-only transient, which the core automation contract
  allows to exceed it briefly), and a fade to fully wet clamps dry overshoot
  during the transition.
- Linking applies to the core detector gain (pinned at 100%, shared across
  channels). The color stage is per-channel, so colored lanes may diverge
  across channels by design while sharing one gain history.

## Finite streams

Call `initialize` before processing and repeatedly call `drain` after the last
input block. Drain writes only its returned output frames, up to 256 per call.
Invalid rate, incomplete channel frames or insufficient capacity for a pending
tail leave drain history untouched and can be retried. A supported nonempty EOS
freezes controls and new input until `reset` or reinitialization; identical
parameter snapshots remain accepted. Empty-stream drain is a no-op. Reset
retains parameter targets and clears EOS and audio history.

Exact finite drain is supported when analog color has remained zero throughout
the epoch established by initialization/reset. Core drain output passes through
the same prepared color stage, and the finite bound includes core lookahead plus active true-peak protection delay.
Any nonzero color target invalidates this proof until reset/reinitialization,
even if the target is changed back to zero without processing. A zero target
alone cannot prove that a previous color fade has settled. Replacing a model
preserves a proved zero-color epoch and the core audio history.

Color models contain recursive filters or nonlinear history. Outside a proved
zero-color epoch, the response is `TailLength::Infinite` and legacy zero-frame
COMPLETE drain behavior remains; automatic recursive completion is unresolved (AUD073).
The change does not recover recursive colored tails. Callers needing a chosen
tail duration can continue ordinary zero-input processing before EOS.

`drain_output_frames_max` is a per-call capacity; `tail_length` is a bound from
the last input and does not count down. Queries are allocation-free. The generic
in-place adapter can copy input to output before an inner `process` error;
processing errors do not promise destination preservation. Drain capacity
errors and direct/compiled EOS preflight retain their transactional guarantees.
