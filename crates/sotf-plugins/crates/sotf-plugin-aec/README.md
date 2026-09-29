# sotf-plugin-aec

Acoustic Echo Cancellation plugin — PBFDAF with two-path and post-filter.

## What It Does

Removes echo from audio streams in real-time. When a microphone picks up audio playing through speakers, the AEC plugin subtracts the known playback signal to produce clean microphone audio. Uses a Partitioned Block Frequency Domain Adaptive Filter (PBFDAF) for efficient cancellation.

## Features

- **PBFDAF algorithm**: Efficient frequency-domain adaptive filtering
- **Two-path architecture**: Stable foreground plus a background explorer with double-talk adaptation gating
- **Post-filter**: Leakage-model residual echo suppression with click-free wet/dry switching
- **Real-time processing**: Low-latency operation suitable for live audio
- **Input policy**: Non-finite microphone/reference samples are replaced by silence before adaptation
- **Clock validation**: Ordinary callbacks must match the constructed or initialized sample rate; mismatches preserve audio and learned state

## Construction

`AecPlugin::new(rate)` supports direct processing at its construction rate and
uses the same default adaptive settings as `from_params(rate, defaults)` or
`new(rate)` followed by `initialize(rate)`. Both adaptive timing and the residual
post-filter use that rate. The default background learning step is the exposed
canonical value 0.5; foreground preparation uses 0.6 times that value.

This corrects an older constructor-only discrepancy: direct `new` processing
used a hidden background step of 0.7 and 48 kHz adaptive timing at every rate.
The configured and explicitly initialized paths keep their existing behavior.
Initialization remains available to start a fresh stream at a new rate.

## Finite streams

After the final microphone/reference input, call `drain()` until it reports
completion. It emits the partial input block, remaining echo-filter response,
and queued output at the unchanged 256-sample latency. Drain accepts any positive
mono output capacity and emits at most 256 frames per call; empty streams finish
without output. Invalid capacity or sample-rate requests leave stream state intact.

EOF zeros advance signal history while foreground/background weights, transfer
decisions, double-talk state, and residual-suppressor gains stay frozen. Existing
post-filter wet/dry ramps finish toward their already selected target. Thus drain
does not train the echo canceller on synthetic silence. New input or parameter
changes after drain begins require `reset()` or `initialize()`.

With block size B=256, P prepared filter partitions, and final input block phase
r in 1..=B, drain emits at most `(P+2)*B-r` frames. This includes spectral
post-filter spreading to the end of the final block and may include trailing
zeros. Both first-call and reset/drain paths allocate and deallocate nothing.

Native tail metadata reports a conservative `(P+2)*B` output frames. This also
bounds ordinary zero-input processing while learning continues: recursive
detectors, learned coefficients, suppressor gains, and wet/dry ramps cannot
produce audio after the finite microphone/reference history has cleared.
The declaration stays constant during drain and updates with prepared filter
storage after a sample-rate change.

## Architecture

```
src/
├── lib.rs          # Public module surface
├── lib/            # AecPlugin implementation and tests
├── pbfdaf.rs        # Partitioned adaptive filter
├── two_path.rs      # Foreground/background management and DTD gate
├── post_filter.rs   # Residual-leakage suppressor
└── params.rs        # Canonical schema and serializable state
```

## Testing

```bash
cargo test -p sotf-plugin-aec
```

## License

Part of the SOTF (Sound of the Future) project.
