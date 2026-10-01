# Unreleased

- Fix the transient-guard proof fixtures to the derived operating points:
  stationary tone-plus-hiss runs at -20 dBFS (gate open, backend parity),
  settled/linked/toggle impulses run at the proven unit detection floor,
  and preservation is pinned worst-case absolute (every peak within
  -3/+2 dB) with per-impulse prints, multi-period/seed, one-sided linked
  OR, 44.1 kHz, lowpassed-hiss, and silence-transition coverage. No DSP,
  default, or API change.
- Add an opt-in spectral transient guard (`transient_guard`, default off,
  serde backward default): confirmed broadband onsets lift reduction
  instead of smearing through shaped steady-state gains, with a ~21 ms
  startup blind window, shared onset decisions when linked, and
  allocation-free realtime toggling. Old presets/profiles/defaults never
  enable it; guard-off audio is bit-identical to previous behavior.
- Add noise-profile capture (1 s high-band RMS reference), persisted profile
  restoration, and time-domain threshold following (louder of user threshold
  and floor + 6 dB). Capture is drain-frozen, reset preserves stored
  profiles, and trigger/process/reset paths stay allocation-free.
- Add a spectral-only per-frequency reduction curve (1/4/12 kHz log anchors)
  and a selectable channel-link mode with full registry/schema/preset
  round-trip. Per-bin curve application, linked detectors, and spectral
  profile use require the shared plugins-denoiser backend hooks.
- Adopt the plugins-denoiser profile/curve/link backend: the stored
  profile drives spectral per-bin noise, the curve scales spectral
  per-bin maximum reduction, and linking shares detectors in both
  modes (spectral vetoes on split program, time-domain shares
  max depth). Defaults stay bit-identical; the curve remains
  stored-only in time-domain mode by design.
- Add independent accuracy suites for capture floors, curve interpolation,
  hiss/tone/transient separation, expansion law, cross-rate consistency,
  unity reconstruction, and EOF with profile state.
- Add finite-stream drain to spectral mode, preserving transform and dry-path
  audio through the derived endpoint using prepared storage. Conventional IIR
  mode retains its unknown-tail/no-drain contract.
- Add phase-complete delayed-unity, nonlinear zero-continuation, control
  transition, lifecycle and cold allocation/deallocation regressions.

# 0.5.8

- Add an opt-in structural Spectral mode using a 1024-point, 75%-overlap
  WOLA reducer and bounded minimum-statistics noise tracking. The existing
  zero-latency high-band expander remains the default and preset-compatible.
- Report and realize exactly 1024 samples of causal spectral latency across
  arbitrary callback partitions; disabled Spectral mode emits the exactly
  latency-matched dry signal.
- Smooth per-bin Wiener gains to reduce musical-noise holes and cover impulse
  latency, partition equality, delayed bypass, stationary-noise SNR, and
  realtime allocation behavior.

# 0.5.7

- Replace the waveform-cycle binary gate with fast/slow power tracking,
  persistence, hysteresis/hold, continuous reduction depth, and sample-rate
  derived gain timing.
- Smooth live cutoff and bypass transitions while keeping detector/filter state
  warm; zero strength and settled bypass remain exactly transparent.
- Canonicalize the visible cutoff against the active sample rate, reject invalid
  topology/rates and unknown preset fields, sanitize non-finite audio, and snap
  decaying state out of the denormal range.
- Make realtime parameter setters allocation-free, use the crate version in host
  metadata, and expand deterministic DSP, metadata, factory, and QA coverage.

# 0.5.6

- **Fix:** require nonzero initialization and reject processing at an uninitialized or mismatched sample rate.
- **Fix:** canonicalize persisted and runtime parameters against the documented ranges.
- **Fix:** use sample-rate-derived envelope/gain smoothing and a fast/slow envelope detector to avoid per-sample modulation clicks.
- **Performance:** classify the plugin as IIR and reuse the cached parameter schema instead of rebuilding it on every query.

# 0.5.5

- **Fix:** disabled/bypassed processing now validates host buffer size before returning. The plugin
  no longer accepts malformed buffers only because hiss reduction is bypassed.
- **Docs:** clarified that latency reporting is delegated to the underlying IIR reducer and correctly
  reports zero algorithmic latency.

# 0.5.5

- **Fix (critical): removed dead `low_latency` parameter** (`src/params.rs`, `src/lib.rs`). The
  underlying `HissReducer` is a simple first-order IIR filter with no FFT at all; exposing a
  "Low Latency (smaller FFT)" toggle was pure dead UI surface that silently did nothing. The
  parameter is removed from `PARAMS`, `LAYOUT`, `HissReducerPluginParams`, and `params::Params`.
  Serialised presets that include `low_latency` will silently drop the field via serde's default
  handling — no migration needed.
- **Fix (high): parameter changes no longer reset DSP state** (`src/lib.rs:set_parameter`).
  Previously every change to `threshold_db`, `frequency_hz`, or `strength` called
  `rebuild_reducer()`, which re-created the `HissReducer` from scratch and reset all IIR history
  and envelope-follower state, causing audible clicks. The fix calls
  `reducer.set_params(frequency_hz, threshold_db, strength)` in-place instead; internal
  coefficients are updated without touching filter state. `rebuild_reducer()` is now removed.
- **Fix (medium): initial sample-rate mismatch** (`src/lib.rs:from_params`). The plugin stored
  `sample_rate: 44100` before `initialize()` was called, but `HissReducer::new()` internally
  defaults to 48000 Hz. Changing it later in `initialize(44100)` would silently re-derive filter
  coefficients for a different rate, altering the frequency response. The stored default is now
  48000 to match the reducer's construction-time default. Callers must still call `initialize()`
  with the actual host sample rate — this fix only removes the inconsistency before that call.
- **Fix (medium): `latency_samples` reporting** (review issue 4). The review assumed FFT-based
  processing. `HissReducer` is a sample-by-sample IIR lowpass with no look-ahead or buffering;
  the plugin now delegates to `HissReducer::latency_samples()`, which correctly returns `0`.

# 0.5.4

- Initial release. Split out of `sotf-plugin-denoiser` into a dedicated stationary high-frequency hiss reducer.
- Uses the shared `HissReducer` core from `plugins-denoiser`.
- Parameters: `enabled`, `threshold_db` (SNR threshold), `frequency_hz` (cutoff above which hiss removal applies), `strength` (0.0–1.0), `low_latency` (smaller FFT path).
