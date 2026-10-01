# plugins-denoiser

Shared denoiser DSP building blocks for SOTF plugins.

Modules:
- `transient` — fixed-lookahead robust click detection and interpolation.
- `hiss` — `HissReducer`, a zero-latency persistent low-level high-band
  downward expander with smoothed cutoff/bypass transitions and an
  optional max-depth linked-channel detector (`set_linked`).
  `initialize` preserves detector state (call `reset` for a fresh
  detector); non-finite `set_params` inputs fall back to defaults.
- `spectral_hiss` — `SpectralHissReducer`, an allocation-free 1024-point WOLA
  minimum-statistics reducer with fixed 1024-sample latency, plus
  default-off captured-profile noise (`set_external_noise`, broadband
  floors applied as a white-spread high-band reference), per-bin
  reduction curve (`set_curve_gains`), linked-channel reduction
  (`set_linked`: shared min-gain targets, reduce-only-while-all-quiet
  gate), and an opt-in transient guard (`set_transient_guard`:
  broadband-onset peak protection against a recent-mean reference,
  default off; seeds once the analysis window fills). Non-finite `set_params`
  inputs fall back to defaults; the live high-band estimate intentionally
  keeps the legacy Nyquist counting.
- `rnnoise` — `RnnoiseBackend` wrapping `nnnoiseless` for 48 kHz mono/stereo
  voice denoising, with arbitrary host framing, fixed 480-sample latency, warm
  crossfaded bypass, sanitized model input, preallocated model workspace, and
  fixed-size access to the model's smoothed 22-band gains/VAD probability.
  Stereo applies one polarity-aware detector's spectral decisions to both
  original channels; it never reconstructs suppression from broadband RMS.

Used by `sotf-plugin-declick`, `sotf-plugin-hiss-reducer`, and `sotf-plugin-speech-denoiser` so each plugin stays a thin host adapter.
