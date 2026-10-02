# sotf-plugin-speech-denoiser

RNNoise-based voice denoiser plugin. Wraps the `RnnoiseBackend` block from `plugins-denoiser` (which itself wraps `nnnoiseless`) in the SOTF host plugin trait.

## Architecture

- `lib.rs` — `SpeechDenoiserPluginParams` + `ParametricInPlacePlugin` impl driving `plugins_denoiser::rnnoise::RnnoiseBackend`, plus the wrapper 960-frame dry delay and strength blend.
- `model.rs` — append-only model registry (`MODEL_LABELS`, `SpeechDenoiserModel`).
- `params.rs` — `PARAMS` array (parameter specs), v2 schema, UI layout.

## Parameters

- `enabled` — bypass toggle (default: enabled).
- `strength` — suppression blend 0..1 (default: 1.0 full wet), realtime, 480-frame slew.
- `model` — model identity (default: `RNNoise Full` index 0), structural; `RNNoise Legacy LQ`/`RNNoise Legacy SH` load staged `.rnnn` weights at init; unknown rejected, changed-after-init needs graph rebuild.

## Features

- `qa` — enables `sotf-host/qa` and the `qa-speech-denoiser` benchmark binary.

## Testing

```bash
cargo check -p sotf-plugin-speech-denoiser && cargo clippy -p sotf-plugin-speech-denoiser
cargo test -p sotf-plugin-speech-denoiser
cargo run -p sotf-plugin-speech-denoiser --features qa --bin qa-speech-denoiser
```

## Important Notes

- Implements `ParametricInPlacePlugin` — same in/out channel count.
- RNNoise expects 48 kHz mono frames; the wrapper handles arbitrary host
  framing and rejects non-48-kHz formats rather than resampling.
- Stereo uses one polarity-aware, energy-normalized detector and applies its
  22 bounded, smoothed model gains to both original channels.
- `get_data()` publishes fixed-size band-gain/VAD diagnostics through a
  preallocated realtime cache.
- DSP body lives in `plugins-denoiser::rnnoise`; this crate is a thin host adapter.
