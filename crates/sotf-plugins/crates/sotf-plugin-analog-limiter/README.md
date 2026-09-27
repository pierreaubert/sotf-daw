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
- **Core lookahead latency reported**; the color stage adds none
