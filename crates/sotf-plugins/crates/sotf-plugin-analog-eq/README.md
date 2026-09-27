# sotf-plugin-analog-eq

Analog EQ — a 4-band parametric EQ with an analog coloration stage.

## What It Does

Four fixed bands (low-shelf, two peaks, high-shelf) run first; the output then
passes through one of six `math-analog` coloration models (Harmonics, Static,
Hammerstein, Tape, Transformer, Console Preamp). With color at 0% the plugin
is a clean, transparent EQ; raising drive and color adds saturation character
staged at 0 VU = −18 dBFS.

## Features

- **Fixed 4-band core**: musical defaults, click-free coefficient updates
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Color off by default**: flat EQ until you dial color in
- **Zero added latency**
