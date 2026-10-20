# sotf-plugin-aec

Acoustic Echo Cancellation plugin — PBFDAF with two-path and post-filter.

## What It Does

Removes echo from audio streams in real-time. When a microphone picks up audio playing through speakers, the AEC plugin subtracts the known playback signal to produce clean microphone audio. Uses a Partitioned Block Frequency Domain Adaptive Filter (PBFDAF) for efficient cancellation.

## Features

- **PBFDAF algorithm**: Efficient frequency-domain adaptive filtering
- **Two-path architecture**: Stable foreground plus a continuously adapting background explorer with coherence-gated promotion
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

## Echo-path tracking and near-end protection

The background continues learning through ambiguous residuals such as delay
changes and double-talk. Promotion to the audible foreground requires at least
1 dB of smoothed improvement and squared microphone/estimate correlation of
0.5 in every qualifying block. Initial promotion requires 133 ms of evidence;
subsequent promotions require 25 ms to reduce drift-induced staleness.
The post-filter also bypasses an incoherent residual-power rise of 6 dB above
its learned leakage floor, preserving quieter near-end onsets.

`tests/audit_echo_paths.rs` builds microphone audio independently with a two-tap
path, a 7-to-19 ms delay jump, both signs of 100 ppm fractional clock drift,
memoryless nonlinear distortion, and independent colored-noise near-end
excitation at three levels, plus near-end-only audio after a learned echo path. It compares latency-aligned output across regular
and irregular callbacks at 16/44.1/48 kHz with the post-filter on and off.
The first two seconds of adaptation are reported in 250 ms windows; output
latency is checked independently at 256 frames. ERLE is measured only in
far-end-only windows; double-talk reports error relative
to the known near-end signal, including onset and subsequent far-end recovery.
These are deterministic engineering fixtures rather than recorded speech tests.

The current fixtures achieve 28–36 dB delay-jump ERLE without the post-filter,
at least 20 dB near-end SNR, and 4.4–9.6 dB ERLE under drift without the
post-filter (13.9–17.2 dB with it). Drift has a documented 3 dB regression floor,
representing removal of at least half the echo power; an initial 6 dB target
was not met at 48 kHz without suppression. This plugin does not synchronize
independent clocks or compensate reference delays beyond the prepared tail.
Exact commands, measurements, and baseline failures are recorded under
`audit/artifacts/plugin-audit-20261018/aec-*.json` at the workspace root.

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
