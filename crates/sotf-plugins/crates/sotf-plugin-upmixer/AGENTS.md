# sotf-plugin-upmixer

Stereo to multichannel surround upmixer using FFT-based Direct/Ambient decomposition and VBAP panning.

## Architecture

```
src/
  lib.rs             -- UpmixerPlugin (Plugin), main struct and Plugin trait impl
  config.rs          -- UpmixerConfig, UpmixerPluginParams, speaker config types
  params.rs          -- Centralized parameter specs, SPEAKER_CONFIGS
  setup.rs           -- Plugin initialization and FFT planner setup
  process.rs         -- Main process() implementation
  fft.rs             -- FFT/IFFT operations and overlap-add
  frequency_domain.rs -- Frequency-domain Direct/Ambient decomposition
  detection.rs       -- Direct sound detection algorithms
  panning.rs         -- VBAP (Vector Base Amplitude Panning) implementation
  bass.rs            -- Bass management (LFE extraction, bass redirection)
  height.rs          -- Height channel processing
  decorrelation.rs   -- Ambient decorrelation for surround channels
  hr_processing.rs   -- High-resolution FFT processing path
  output.rs          -- Output channel mapping and assembly
  ml_features.rs     -- ML feature extraction (behind onnx feature)
  ml_inference.rs    -- ONNX inference for ML-guided upmixing (behind onnx feature)
  test.rs            -- Integration tests
```

**Algorithm pipeline:**
1. Stereo input accumulated in overlap-add buffer (50% overlap)
2. Forward FFT to frequency domain
3. Direct/Ambient decomposition: separate common (direct) and difference (ambient) components
4. VBAP panning distributes direct sound to speaker positions based on detected direction
5. Ambient signal decorrelated and distributed to surround channels
6. Bass management: LFE extraction, optional bass redirection
7. Height channels controlled by height_gain parameter
8. Inverse FFT + overlap-add to time domain
9. Output mapped to selected speaker configuration

## Key Public API

- `UpmixerPlugin::new(config) -> Self` (`lib.rs` / `setup.rs`)
- `UpmixerPlugin::from_params(params) -> Self` (`lib.rs`)
- Implements `Plugin` trait: 2 input channels -> N output channels (varies by speaker config)

**Supported configs:** 5.1, 7.1, 5.1.2, 5.1.4, 7.1.2, 7.1.4, 9.1.4, 9.1.6

**Parameters:** `speaker_config`, `center_gain`, `surround_gain`, `height_gain`, `lfe_gain`, `ambient_level`, `direct_level`, `bass_crossover_hz`, `binaural_preview` (renders to 2ch binaural instead of surround), and many more via param_bridge.

## Testing

```bash
cargo test -p sotf-plugin-upmixer
```

`tests/upmixer_audio_regression.rs` compares output against golden WAVs in
`data_generated/test-plugin-upmixer/` (gitignored; tests skip when absent).
Regenerate with `just generate-upmixer-golden` (or
`cargo run -p sotf-plugin-upmixer --bin generate-upmixer-golden -- --out-dir
crates/sotf-plugins/crates/sotf-plugin-upmixer/data_generated/test-plugin-upmixer`
from the workspace root). The generator uses fixed seeds, so output is byte-stable.

## Important Notes

- This is the largest and most complex plugin in the workspace. The algorithm is split across many modules for maintainability.
- The `onnx` feature enables ML-guided upmixing via ONNX runtime (optional, not default).
- `binaural_preview` mode renders the surround output to binaural 2ch for headphone monitoring.
- FFT size and hop size determine latency. 50% overlap with Hann window is used for the overlap-add reconstruction.
- Gain/routing smoothers use the common main/HR analysis clock in `gain_clock.rs`.
  Flush elapsed sub-hop time before a changed target; same-value snapshots and
  empty calls must not snap or shift the grid. Keep it independent of HR enable.
- Preserve old-clock elapsed gain time on reset/reinitialization. HR input
  chunks must clock controls before HR analysis; disabled HR still advances
  through the same fixed quanta. Frequency tables share that grid through
  `table_clock.rs`, with separate pending frames. Gain events must not flush
  table time. Flush changed table targets and lifecycle transitions using the
  old clock; preserve table thresholds and prepared storage. Safety-cap control
  and final output safety still have separate callback-timing limitations.
- Realtime scalar setters refresh only their indexed cached value using the
  canonical DSP getter. Preserve PARAMS/cache order, coercions and clamping;
  never rebuild the metadata vector in a realtime setter. Resource controls,
  including ML enable, remain structural. Realtime ID owners belong to the host.
- Speaker configurations are defined in `params.rs` (`SPEAKER_CONFIGS`) and reference `SpeakerConfig` from sotf-host.
- Output channel count changes dynamically based on selected speaker configuration. The engine must handle channel count changes between plugins.
- VBAP uses triangulated speaker layouts for 3D panning (including height channels).
- Has benchmarks: `cargo bench -p sotf-plugin-upmixer --bench upmixer-benchmark`.
