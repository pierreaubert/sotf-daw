# sotf-plugins (lib: `sotf_plugins`)

A comprehensive audio plugin system for real-time audio processing, built as a workspace of individual plugin crates with a facade crate that re-exports everything.

## Overview

This crate provides:

- **Core Plugin System** (`sotf-host`): Traits, interfaces, DAG-based host, parameter system, analyzers
- **Processing Plugins**: 30+ audio processors as individual crates
- **Analyzer Plugins**: Spectrum analyzer, loudness monitor (EBU R128)
- **Plugin Host**: DAG-based host supporting parallel processing with thread-safe execution
- **Test Utilities**: Signal generators, allocation tracking, performance profilers

## Architecture

### Crate Organization

The plugins workspace is organized as a facade crate (`sotf-plugins`) that re-exports:
- **`sotf-host`**: Core infrastructure — `Plugin`/`ParametricInPlacePlugin` traits, `DawHost`, parameter system, analyzers, SIMD, smoothing, SOFA/HRTF, speaker configs, STFT common utilities
- **`sotf-plugin-*`**: Individual plugin crates, each self-contained with their own tests and dependencies

```
crates/sotf-plugins/
├── src/lib.rs           # Facade: re-exports sotf-host + all plugin crates
├── Cargo.toml           # Depends on sotf-host + all sotf-plugin-* crates
├── crates/
│   ├── sotf-host/       # Core infrastructure (traits, host, parameters, analyzers)
│   ├── sotf-plugin-eq/
│   ├── sotf-plugin-compressor/
│   ├── sotf-plugin-upmixer/
│   ├── sotf-plugin-binaural/
│   ├── ... (30+ plugin crates)
├── tests/               # Integration tests
├── benches/             # Criterion benchmarks
└── bin/                 # plugin-fuzzer binary
```

### Core Traits

#### `Plugin` Trait

For plugins that may change the channel count (e.g., upmixers, downmixers):

```rust
pub trait Plugin: Send {
    fn info(&self) -> PluginInfo;
    fn input_channels(&self) -> usize;
    fn output_channels(&self) -> usize;
    fn parameters(&self) -> Vec<Parameter>;
    fn set_parameter(&mut self, id: ParameterId, value: ParameterValue) -> PluginResult<()>;
    fn get_parameter(&self, id: &ParameterId) -> Option<ParameterValue>;
    fn initialize(&mut self, sample_rate: u32) -> PluginResult<()>;
    fn reset(&mut self);
    fn process(&mut self, input: &[f32], output: &mut [f32], context: &ProcessContext) -> PluginResult<()>;
    fn latency_samples(&self) -> usize;
    fn get_data(&self) -> Option<Arc<dyn Any + Send + Sync>>;
}
```

#### `ParametricInPlacePlugin` Trait

For plugins that process audio in-place (same input/output channel count):

```rust
pub trait ParametricInPlacePlugin: Send {
    fn info(&self) -> PluginInfo;
    fn channels(&self) -> usize;
    fn process_in_place(&mut self, buffer: &mut [f32], context: &ProcessContext) -> PluginResult<()>;
    // ... parameter methods ...
}
```

Use `ParametricInPlacePluginAdapter` to wrap an `ParametricInPlacePlugin` as a `Plugin`.

### Audio Format

All audio data uses **interleaved f32** format:
- Stereo: `[L0, R0, L1, R1, L2, R2, ...]`
- 5.1 surround: `[FL0, FR0, C0, LFE0, SL0, SR0, FL1, FR1, ...]`

Sample values are typically in the range -1.0 to +1.0.

## Plugin Catalog

### Processing Plugins

| Plugin | Crate | Description | Channels |
|--------|-------|-------------|----------|
| EQ | `sotf-plugin-eq` | Parametric EQ with biquad filters | N → N |
| Gain | `sotf-plugin-gain` | Volume control with smoothing | N → N |
| Compressor | `sotf-plugin-compressor` | Dynamic range compression | N → N |
| Expander | `sotf-plugin-expander` | Dynamic range expansion | N → N |
| Gate | `sotf-plugin-gate` | Noise gate | N → N |
| Limiter | `sotf-plugin-limiter` | Peak limiter | N → N |
| Delay | `sotf-plugin-delay` | Multi-channel delay with feedback | N → N |
| Convolution | `sotf-plugin-convolution` | FFT-based IR convolution | N → N |
| Crossover | `sotf-plugin-crossover` | Frequency band splitting (Linkwitz-Riley) | N → N×bands |
| Matrix | `sotf-plugin-matrix` | Channel routing/mixing with gain smoothing | N → M |
| Resampler | `sotf-plugin-resampler` | Sample rate conversion | N → N |
| Upmixer | `sotf-plugin-upmixer` | Stereo to surround via FFT spatial processing + VBAP | 2 → M |
| Downmix | `sotf-plugin-downmix` | Surround to stereo downmixing | M → 2 |
| Binaural | `sotf-plugin-binaural` | HRTF-based surround to binaural rendering | M → 2 |
| XTC | `sotf-plugin-xtc` | Crosstalk cancellation for speakers | N → N |
| PND | `sotf-plugin-pnd` | Perceptual Noise Diffusion | N → N |
| Crossfeed | `sotf-plugin-crossfeed` | Headphone crossfeed for natural stereo | 2 → 2 |
| Loudness Comp | `sotf-plugin-loudness-compensation` | Equal-loudness contour compensation | N → N |
| Fletcher-Munson | `sotf-plugin-fletcher-munson` | Fletcher-Munson equal-loudness correction | N → N |
| Channel Mute/Solo | `sotf-plugin-channel-mute-solo` | Per-channel mute/solo/dim with fade smoothing | N → N |
| Mono to Stereo | `sotf-plugin-mono-to-stereo` | Mono to stereo conversion | 1 → 2 |
| Multiband Comp | `sotf-plugin-multiband-compressor` | Multiband dynamic range compression (2-5 bands) | N → N |
| Multiband Exp | `sotf-plugin-multiband-expander` | Multiband dynamic range expansion (2-5 bands) | N → N |
| Denoiser | `sotf-plugin-denoiser` | Audio denoising (MCRA/Wiener) | N → N |
| AB Compare | `sotf-plugin-ab-compare` | A/B comparison plugin | N → N |
| Band Split | `sotf-plugin-band-split` | Split signal into frequency bands | N → N×bands |
| Band Merge | `sotf-plugin-band-merge` | Merge frequency bands back together | N×bands → N |
| AEC | `sotf-plugin-aec` | Acoustic echo cancellation | N → N |
| Beamformer | `sotf-plugin-beamformer` | Microphone beamforming | N → M |

### Analyzer Plugins (in `sotf-host`)

| Plugin | Description | Output Data |
|--------|-------------|-------------|
| `SpectrumAnalyzerPlugin` | FFT-based spectrum analysis | `SpectrumInfo { frequencies, magnitudes, peak }` |
| `LoudnessMonitorPlugin` | EBU R128-style loudness measurement | `LoudnessData` includes validity/enabled state, explicit channel-layout compliance, BS.1770 true peak at 44.1/48/88.2/96 kHz, an explicit one-hour integrated-history policy, and centered Pearson correlation. Other rates publish true peak as unavailable rather than silently applying a mismatched interpolation rate. |

Analyzers pass audio through unmodified while extracting measurements accessible via `get_data()`.

### Infrastructure (in `sotf-host`)

| Module | Description |
|--------|-------------|
| `host.rs` | `DawHost` — DAG-based plugin routing with parallel processing |
| `plugin.rs` | Core `Plugin`/`ParametricInPlacePlugin` traits |
| `parameters.rs` | Parameter system (Float, Int, Bool, Choice) |
| `param_specs.rs` | Centralized parameter defaults/ranges for all plugins |
| `param_registry.rs` | Parameter registry |
| `automation.rs` | Parameter automation |
| `smoothing.rs` | One-pole parameter smoother for click-free transitions |
| `auto_gain.rs` | Automatic gain normalization |
| `serialization.rs` | Preset management |
| `speaker_config.rs` | Speaker layout definitions |
| `sofa.rs` | SOFA HRTF file parsing |
| `simd.rs` | SIMD optimizations |
| `stft_common.rs` | Shared STFT utilities for FFT-based plugins |
| `layout_solver.rs` | Channel layout solving |
| `plugin_layout.rs` | Plugin layout definitions |
| `test_utils.rs` | Signal generators, allocation tracking, performance profilers |

### macOS-Specific Plugins

| Plugin | Crate | Feature Flag |
|--------|-------|--------------|
| HAL Input | `sotf-plugin-hal-input` | `hal` |
| HAL Output | `sotf-plugin-hal-output` | `hal` |

## Plugin Host

### DawHost (PluginHost)

The main host supports two modes:

#### Chain Mode (Linear)

```rust
let mut host = PluginHost::new(2, 48000);
host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(GainPlugin::new(2, -6.0))))?;
host.process(&input, &mut output)?;
```

#### Graph Mode (DAG)

For complex routing topologies with parallel processing:

```rust
let mut host = DawHost::new(2, 48000);
let node1 = host.add_node("input".into(), Box::new(plugin1))?;
let node2 = host.add_node("branch_a".into(), Box::new(plugin2))?;
host.add_edge(GraphEdge::new(node1, node2))?;
host.build()?;
host.process(&input, &mut output)?;
```

Features:
- Automatic cycle detection
- Topological sorting for correct processing order
- Stage-based parallel execution
- Thread-safe plugins via `Arc<Mutex<>>`
- Custom channel mapping via `GraphEdge::with_channels()`

## Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `hal` | Enable macOS CoreAudio HAL plugins | No |
| `onnx` | Enable ONNX runtime for ML-based plugins (upmixer) | No |
| `qa` | Enable QA benchmark utilities and test helpers | No |

## Testing

```bash
# Run all library tests
cargo test -p sotf-plugins --lib

# Run integration tests
cargo test -p sotf-plugins --test test_sample_rate_support
cargo test -p sotf-plugins --test property_tests
cargo test -p sotf-plugins --test realtime_tests
cargo test -p sotf-plugins --test realtime_allocation_tests
cargo test -p sotf-plugins --test rt_safety_tests

# Check + clippy
cargo check -p sotf-plugins && cargo clippy -p sotf-plugins
```

### Integration Tests

- `test_plugins.rs` — Basic plugin tests
- `test_dynamics_plugins.rs` — Compressor/limiter/gate tests
- `test_sample_rate_support.rs` — All plugins at 22050-192000 Hz
- `property_tests.rs` — Property-based tests (proptest)
- `realtime_tests.rs` — Real-time constraint verification
- `realtime_allocation_tests.rs` — Zero-allocation on audio thread
- `rt_safety_tests.rs` — Real-time safety verification
- `stft_normalization_tests.rs` — STFT normalization correctness
- `host_integration_tests.rs` — DAG host integration
- `automation_tests.rs` — Parameter automation
- `distortion_regression_tests.rs` — Regression tests for audio distortion
- `parameter_robustness_tests.rs` — Parameter edge cases
- `performance_report_tests.rs` — Performance regression detection
- `test_harness_sanity.rs` — Test infrastructure verification

### Fuzz Testing

```bash
cargo run -p sotf-plugins --bin plugin-fuzzer -- \
    --file audio.wav \
    --plugin compressor \
    --iterations 1000
```

## Benchmarks

```bash
# All benchmarks
cargo bench -p sotf-plugins

# Specific suites
cargo bench -p sotf-plugins --bench all-plugins-benchmark
cargo bench -p sotf-plugins --bench allocation-benchmark
cargo bench -p sotf-plugins --bench plugin-benchmark

# Specific plugin
cargo bench -p sotf-plugins --bench all-plugins-benchmark -- EqPlugin
```

## Performance Targets

For real-time audio at 48kHz with 512-sample buffers (10.67ms of audio):
- **Target**: < 2ms processing time (5x real-time margin)
- **Maximum**: < 10ms (1x real-time)

## Performance Considerations

1. **In-Place Processing**: Prefer `ParametricInPlacePlugin` when channel count doesn't change — avoids buffer copies
2. **Buffer Sizes**: Larger buffers amortize per-call overhead but increase latency
3. **Memory Allocation**: Pre-allocate buffers in `initialize()`, avoid allocations in `process()`
4. **SIMD**: Use SIMD intrinsics in hot paths (see `simd.rs`)
5. **Parallel Processing**: Enable with `host.set_parallel_enabled(true)` for complex graphs

## DSP Plugin Parameter Registration

DSP plugins need parameters registered in 3 places:
1. `rebuild_cached_parameters` — Cached parameter state
2. `set_parameter` — Parameter setter
3. `get_parameter` — Parameter getter

Missing `cached_parameters` causes silent parameter rejection.

## License

Part of the SOTF (Sound of the Future) project.

### Audit callback measurements

`audit-perf` produces JSON Lines for real host chains, with graph-path waveform
and finite-drain comparison before measurement. Run optimized builds:

```sh
cargo run --release -p sotf-plugins --bin audit-perf -- \
  --channels 2,8,32,64 --rates 44100,48000,96000,192000 \
  --blocks 1,32,64,128,512,0 --iterations 10000 > dynamics.jsonl
cargo run --release -p sotf-plugins --bin audit-perf -- \
  --chain spectral --channels 2 --rates 48000 --blocks 64,128,0
cargo run --release -p sotf-plugins --bin audit-perf -- \
  --chain spatial --channels 2 --rates 48000 --blocks 64,128,0 \
  --cpu-workers 4 --paced
```

Block `0` selects `[1,7,31,257,512]`. Dynamics uses gain, negotiated Dynamic EQ,
limiter and loudness monitoring; spectral uses two coincident denoisers;
spatial uses a 4096-point 9.1.6 upmixer followed by limiting and monitoring.
`--instances N` repeats the selected processor family to measure many-instance
heap and callback costs. `--chain analog` selects analog compression.
The phases cover cold first callback, steady signal, sample-offset gain
automation, silence, denormals, reset plus processing, and finite draining.
EOF uses the host's declared drain capacity as its budget and restarts a stream
outside timing after each completed drain. Every processing error and every
warning/error diagnostic fails the run, including isolated-plugin fallbacks.

Each row records p50/p95/p99/p99.9/max, deadline misses, maximum budget fraction,
callback-thread allocation/free counts, process-wide requested heap live/peak
bytes, parameter-event queue high water and dropped events. Heap bytes exclude
allocator overhead, thread stacks, mappings and GPU resources; they are not RSS.
Parameter queue occupancy does not describe a device playback ring. Allocator
instrumentation contributes to timing. A one-sample cold row cannot estimate
tail probabilities; even 10000 callbacks provide few p99.9 observations.

`--cpu-workers` creates actual competing CPU work. For externally managed GPU
or DAW contention, record its identity with `--external-load`; that label does
not start a workload. `--paced` sleeps between callbacks; OS/device scheduling
still requires an engine/device benchmark. `--max-budget-fraction 0.8` enables
an opt-in maximum-callback gate with 20% reserve. Results default to measurement
without a machine-dependent pass/fail threshold.

The host storage contract is opt-in: call `DawHost::set_max_block_frames` before
`build()` to prepare graph scratch and opted-in plugins for a bounded input
wave. Expanded rate-domain envelopes remain included. Oversized f32/f64 input
calls fail before consuming automation. EmbeddedAudioEngine uses its configured
frame size automatically. Generic hosts keep their previous policy unless the
caller negotiates a bound. Dynamic EQ also provides
`new_with_max_block_frames` to avoid an initial default-size temporary buffer.

Prepared-filter transition measurements are explicit release tests:

```sh
cargo test --release -p sotf-plugin-binaural -p sotf-plugin-xtc -p sotf-plugin-convolution \
  --features sotf-host/qa --lib audit_transition_timing -- --ignored --nocapture
```

These synthetic-spectrum fixtures exercise first/repeated adoption, interrupted
crossfades, stale XTC results, and stalled retirement queues, asserting zero
callback allocations and frees while reporting latency tails. They exclude
filter computation and publication from callback timing; model/SOFA loading is
control-thread work. Numerical/reference and ownership regressions remain in
the ordinary plugin suites.

Use `--legacy-capacity` for a paired memory comparison with the historical
8192-frame host/Dynamic EQ storage. The reference render always uses that
historical storage policy and the graph path. For example, compare both modes
with `--instances 16 --channels 64 --rates 48000 --blocks 128 --iterations 256`.

The threaded engine also carries `EngineConfig::frame_size` through initial
host preparation and worker-built chain/graph replacements. Storage preparation
finishes on the manager/builder thread before handoff. `--require-zero-heap`
turns callback allocations or frees into a failed measurement gate.

Recorded 2026-10-19 measurements are in `audit/perf-20261019-*.jsonl` at the
workspace root. Each chain artifact includes its exact invocation. These runs
used macOS 27.0.1 on aarch64, Rust/Cargo 1.99.0 and the workspace release profile.
Builds and other test suites finished before the final chain sweep. The 96-case
geometry sweep uses 256 callbacks per phase to bound runtime; representative
chains use 10000. All 990640 measured chain callbacks passed the zero-heap gate.
Deadline misses remain visible: the geometry sweep recorded 3321, the spectral
cases 321, isolated spatial cases 2033, and CPU-contention spatial cases 2031.
These are measurements, not a device deadline certification. GPU/device-load
validation still needs an externally managed workload and device timing.

For 16 Dynamic EQ instances at 64 channels and 128/48 kHz, requested scenario
heap high-water growth fell from 625474972 bytes with legacy preparation to
78470556 bytes with negotiated preparation (87.45%). This compares capacity
policies in the same implementation, not complete historical releases. The
synthetic 256-event manager publication benchmark measured median times of
35958 ns per-event publication versus 666 ns for one batch publication. Filter/IR
transition fixtures measured 100004 callbacks with no allocations or frees.

Verification: host/Dynamic EQ/limiter library suites passed 878 tests with one
ignored; the final facade/inference/streaming and performance selections passed
185 with one ignored; manager selections passed 105 with one ignored; device-free
integration selections passed seven. The full engine library suite retains the
capture-to-device-free-output failure documented in the source audit (817 passed,
one failed, five ignored). Changed DSP/harness targets pass warning-denied Clippy;
engine-inclusive Clippy also encounters existing `too_many_arguments` in the PCM
server and `manual_map` in the driver-capture reader. Miri is unavailable in the
installed stable toolchain. No complete-workspace or hardware QA pass is claimed.

The convolution fixture separately measures a synthetic delta IR's publication
and retirement overhead. It does not stand in for long-IR convolution CPU cost;
its first/repeated/stale/full-queue cases assert zero callback allocations/frees.
