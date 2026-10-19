# sotf-plugin-analog-eq

Analog EQ — a 4-band parametric EQ with an analog coloration stage.

## What It Does

Four fixed bands (low-shelf, two peaks, high-shelf) run first; the output then
passes through one of six `math-analog` coloration models (Harmonics, Static,
Hammerstein, Tape, Transformer, Console Preamp). With color at 0% the plugin
is a clean, transparent EQ; raising drive and color adds saturation character
staged at 0 VU = −18 dBFS.

## Features

- **Fixed 4-band core**: musical defaults, state-preserving coefficient updates
- **Six analog models**: fail-closed selection, shared drive/color/character/trim
- **Color off by default**: flat EQ until you dial color in
- **Zero added latency**

## Processing and automation

Each channel runs low shelf → peak 1 → peak 2 → high shelf in f64, then narrows
to f32 for the selected analog color stage. Color zero and trim zero bypass the
color contribution; with flat bands the complete plugin nulls against its input.
Coloration after EQ is intentional: boosting a band changes what drives the
nonlinearity, so reversing these stages changes the sound.

Band edits switch coefficients immediately while retaining Direct Form I input
and output histories. They are allocation-free, but abrupt changes can create
transients; retaining history alone does not guarantee click-free automation.
The analog stage owns its existing control smoothing. Model replacement is
structural after initialization. All batch fields validate before mutation;
invalid batches preserve EQ/color histories and cached metadata.

Primitive scalar validation and setters reuse prepared filter/metadata storage.
Bulk updates and canonical same-model snapshots require no new allocations, but
consume the caller-owned parameter map, whose destruction can free memory.
Use scalar queued controls in the audio path. The actual host immediate setter
and queue producer remain control-thread APIs with their own heap activity;
applying queued edits inside the host audio callback allocates and frees nothing
in the tested matrix. Successful initialization starts a fresh stream by clearing
EQ and color histories at the configured values.

## Validation

```bash
cargo test -p sotf-plugin-analog-eq
cargo test -p sotf-plugins --test audit_analog_eq_host
```

Independent analog shelf/peak prototypes and a bilinear transform check complex
response at 41 logarithmic frequencies plus all band corners, including mixed
and extreme gain/Q settings, at 44.1/48/96/192 kHz. The fixed error bound is
`|measured-reference| / (1+|reference|) < 2e-5`; no production coefficient or
response helper, gain fit or delay alignment is used.

A separate time-varying Direct Form I reference checks coefficient steps and
the complete retained-state trajectory. All six model routes verify EQ-before-
color ordering against a composed stage reference and distinguish the reversed
chain. Tests also cover flat zero-color null, malformed batch preservation,
rate reinitialization, public setter allocation, queued host controls and
callback partitions. These are numerical/streaming guarantees, not validation
against measured analog hardware or a claim of artifact-free parameter jumps.
