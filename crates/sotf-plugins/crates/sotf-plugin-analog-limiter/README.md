# sotf-plugin-analog-limiter

Analog limiter — a mastering limiter with an analog coloration stage.

## What It Does

The proven limiter core (ceiling, release, lookahead, soft knee, true peak,
mix) runs first; the output then passes through one of six `math-analog`
coloration models. With color at 0% it behaves exactly like the clean
limiter; raising drive and color adds tape/console-style finishing character
staged at 0 VU = −18 dBFS.

## Features

- **Full limiter core**: no fork — composition over `sotf-plugin-limiter`
- **Opinionated subset**: threshold, release, lookahead, soft knee, true
  peak, mix; advanced detector options stay at core defaults
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Core lookahead and true-peak protection latency reported**; the color stage adds none

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
