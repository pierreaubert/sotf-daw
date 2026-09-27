# sotf-plugin-analog-compressor

Analog compressor — a single-band bus-style compressor with an analog
coloration stage.

## What It Does

Linked feed-forward detection (threshold, ratio, attack, release, knee),
static plus auto makeup, and parallel mix run first; the output then passes
through one of six `math-analog` coloration models. With color at 0% it is a
clean bus compressor; raising drive and color adds console-style glue
character staged at 0 VU = −18 dBFS.

## Features

- **Bus-style detection**: hottest channel drives one shared envelope
- **Full comp vocabulary**: threshold, ratio, attack, release, knee, static
  and auto makeup, parallel mix
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Zero latency**
