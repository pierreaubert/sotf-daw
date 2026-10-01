# sotf-plugin-hiss-reducer

Stationary high-frequency-noise reducer. Wraps the `HissReducer` block from `plugins-denoiser` in the SOTF host plugin trait.

## Architecture

- `lib.rs` — `HissReducerPluginParams` + `ParametricInPlacePlugin` impl driving `plugins_denoiser::hiss::HissReducer` and `spectral_hiss::SpectralHissReducer`, plus the profile live store.
- `params.rs` — `PARAMS` array (parameter specs) and registration via `param_specs::find_by_key`.
- `profile.rs` — `CaptureState` (pre-allocated 1 s high-band RMS capture), `NoiseProfileData` (persisted v1 floors), `ReductionCurve` (log-interpolated anchors).

## Parameters

- `enabled` — bypass toggle.
- `threshold_db` — absolute dBFS high-band level threshold (not SNR).
- `frequency_hz` — one-pole band-edge frequency, limited to 0.45 × sample rate.
- `strength` — reduction amount.
- `spectral_mode` — structural spectral WOLA toggle.
- `learn_noise` — trigger: start/cancel capture via named setter only.
- `use_captured_profile` — enable the stored profile (time-domain threshold follows floor + 6 dB; spectral per-bin noise estimate uses it).
- `clear_profile` — trigger: discard the stored profile.
- `curve_low`/`curve_mid`/`curve_high` — spectral-only per-frequency reduction scales (1/4/12 kHz anchors; applied spectrally, stored-only in time-domain mode).
- `link_mode` — `Independent`/`Linked` channel linking choice (shared detectors both modes; spectral vetoes on split program, time-domain drags).
- `transient_guard` — opt-in spectral broadband-onset guard (default off; stored both modes, applied spectrally only).

## Features

- `qa` — enables `sotf-host/qa` and the `qa-hiss-reducer` benchmark binary.

## Testing

```bash
cargo check -p sotf-plugin-hiss-reducer && cargo clippy -p sotf-plugin-hiss-reducer
cargo test -p sotf-plugin-hiss-reducer
cargo run -p sotf-plugin-hiss-reducer --features qa --bin qa-hiss-reducer
```

## Important Notes

- Implements `ParametricInPlacePlugin` — same in/out channel count.
- Parameter registration must appear in 3 places (see `param_bridge`).
- DSP body lives in `plugins-denoiser::hiss`; this crate is a thin host adapter.
- Processing requires initialization and rejects context sample-rate mismatches.
- Realtime setters and steady processing must remain allocation-free.
