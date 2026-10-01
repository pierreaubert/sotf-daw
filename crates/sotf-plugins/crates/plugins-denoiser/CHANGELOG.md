## Unreleased (restoration backend fix round 7, 2026-10-01)

- Fix transient-guard false triggering on stationary hiss: the onset
  detector now compares instantaneous high-band power against a
  guard-local recent-mean reference (per-channel short EMA, seeded at
  the first full-window hop, frozen during holds) with a 2.0x firing
  threshold, instead of the minimum-statistics aggregate. Startup hops
  hold a partially zero-filled window, so minima from those hops sit
  far below steady hiss and the old ratio fired spuriously there (hop 1
  provably, ~13x vs the old 4x threshold). Threshold and seeding carry
  an independent signal-model derivation plus an arithmetic regression;
  the guarded peak, no-amplification, coherence, and suppression bounds
  are unchanged, as are all fixtures. Guard-off renders are bit-identical
  (only an integer hop counter is unconditional). No version bump.
- Replace the hiss-only guard identity failure output with a
  first-difference diagnostic (index, values, differing count). Still
  bit-exact equality, no tolerance. No version bump.

## Unreleased (restoration backend fix round 6, 2026-10-01)

- Add an opt-in spectral transient guard
  (`SpectralHissReducer::set_transient_guard`, default off): confirmed
  broadband onsets lift per-bin targets toward unity and raise gains at
  transient speed over a short hold, so engaged peaks survive while
  stationary-hiss suppression between transients is unchanged. Guard-off
  renders are bit-identical to before (pinned by a new HEAD-identity
  test on the impulse fixture); guard-on behavior carries the hard
  transient peak bounds, a no-false-trigger identity pin, and a
  mid-stream toggle convergence test. No version bump.
- Correct the time-domain link test oracle (same leakage class as the
  Hiss R5-T1 fix): re-filtered high-band attenuation on the loud tone is
  provably capped at -0.15 dB for any gain, so drag is now measured as
  residual-drag power with backend-derived bounds, plus hard regressions
  (independent loud channel bit-exact dry, dual-mono identity,
  leakage-regime pin, f64 leakage-arithmetic test). Time-domain DSP is
  untouched. No version bump.

## Unreleased (restoration backend fix round, 2026-10-01)

- Add frozen HEAD-baseline identity tests (`tests/legacy_baseline.rs`):
  engaged spectral (0.65 hiss+tone) and time-domain (0.8 hiss) defaults
  render bit-exactly against a renamed transcription of the retained HEAD
  algorithm, mono and stereo at 48 kHz. HEAD baseline only (not the dirty
  pre-change worktree); no signature changed.
- Add profile-path selectivity tests: quiet 0.06 tone + hiss with the
  profile engaged (tonal bin within 1 dB, tone-excluded high-band hiss
  below -3 dB) and engaged-transient peak bounds (live loss below 3 dB,
  profile within 3 dB of live), plus live controls and coherence.
- Add mid-stream coverage: link on/off, curve reshape, and profile
  enable/disable after divergent histories pin finite output, a
  no-new-discontinuity jump bound, and 1 dB convergence to the
  from-start reference; cold first-use realtime paths expect zero
  allocs/frees; rejected setters expect balanced, bounded error counts;
  frozen engaged drains decay below -60 dBFS; spectral `initialize`
  retains configuration and time-domain `initialize` preserves detector
  state (both pinned behaviorally).
- Spectral `set_params` now falls back to defaults on non-finite input,
  matching the time-domain parity behavior. Finite inputs clamp exactly
  as before, so legacy settings reproduce bit-identically; only
  non-finite callers observe a change. No version bump.

## Unreleased (restoration backend, 2026-10-01)

- Add default-off shared hooks for captured-profile hiss reduction.
  `SpectralHissReducer` gains `set_external_noise` (v1 broadband floors
  mapped to per-bin noise by exact Parseval inversion of the live
  `noise_rms` formula, spread white; the live gate still applies),
  `set_curve_gains` (per-bin strength scale, flat reproduces legacy
  output bit-exactly), and `set_linked` (shared min-gain targets plus a
  reduce-only-while-every-channel-is-quiet gate). `HissReducer` gains
  `set_linked` (shared max-depth targets). All setters are
  finite-range validated and transactional; process/reset/setters stay
  allocation-free; reset retains configuration. Default construction is
  bit-identical to before. No version bump.

## Unreleased audit corrections (2026-09-28)

- Preserve RNNoise model output while correcting total reported latency to 960
  frames and aligning dry output at that delay. The existing 480-frame framing
  queue and 1920-sample rings are retained. Empty calls no longer latch bypass.
- Verify the full wet stream against direct model blocks and output-clock
  transitions against an independent 960-frame dry shift.

# 0.5.14

- Add the preallocated `SpectralHissReducer`: a causal 1024-point WOLA path
  with minimum-statistics noise tracking, smoothed Wiener gains, deterministic
  1024-sample latency, and sample-exact callback partition behavior.

# 0.5.13

- Expose RNNoise's bounded, release-smoothed 22-band suppression decisions and
  VAD probability instead of reconstructing a broadband gain from output RMS.
- Apply one polarity-aware, energy-normalized stereo detector's band gains to
  both original channels with fixed latency and the existing warm bypass path.

# 0.5.12

- Keep RNNoise state advancing through a latency-aligned, 480-sample bypass
  crossfade and reject layouts wider than stereo.
- Sanitize and clamp model input, remove unused reduction metering, and move
  large reusable model workspaces out of the callback stack while preserving
  reference-vector output.

# 0.5.11

- Rework `HissReducer` as an honestly documented zero-latency high-band
  downward expander with fast/slow power tracking, persistence, hysteresis,
  continuous reduction depth, and sample-rate-derived timing.
- Add warm click-free bypass, smoothed cutoff automation, non-finite recovery,
  denormal guards, and exact settled-dry/zero-strength behavior.

# 0.5.10

- Replace `TransientSuppressor`'s derivative clamp with a fixed eight-sample
  lookahead robust-context interpolator for Declick.
- Add linked stereo-pair decisions, warm smoothed bypass, smoothed sensitivity,
  exact delayed dry output, and frame-major allocation-free processing.
- Remove the unused Rayon dependency and the inaccurate parallel-processing
  claim from the shared transient path.

# 0.5.9

Linked-stereo RNNoise gain now transitions between detector decisions over
model frames. This bounds cancellation-prone stereo amplitude modulation while
retaining one common gain and preserving the stereo image.

# 0.5.8

Stereo RNNoise now detects cancellation-prone layouts before linking. Coherent
stereo retains one shared frame gain; anti-phase, hard-panned, and unequal-level
stereo use independent model detectors but apply one bounded gain to the
original channels, preserving the stereo image without collapse or sample-wise
gain modulation.

# 0.5.7

Bug fixes from the Speech Denoiser review (2026-08-12):

- Stream arbitrary host callback sizes through the fixed RNNoise quantum with
  a pre-seeded, constant-latency output queue.
- Sanitize non-finite samples, prepare shared FFT resources during initialize,
  and use frame-level linked stereo gain for cancellation-safe behavior.

# 0.5.6

Bug fixes from code review (2026-05-16):

- RNNoise reduction metering now averages input/output power across all processed channels instead
  of sampling channel 0 only. This keeps stereo and multichannel monitoring representative.

# 0.5.5

Bug fixes from code review (2026-05-11):

- **Fixed (1.4 major):** First processed 480-sample frame is now discarded to remove the fade-in artifact documented by `nnnoiseless`. This adds a fixed one-time 480-sample startup delay; subsequent frames are unaffected.
- **Fixed (3.1 medium):** Per-channel `input_buf` / `output_buf` scratch arrays are now pre-allocated in `initialize()` and stored in `scratch_input` / `scratch_output`. The audio callback no longer pushes 3.8 KB onto the stack on every 480-sample block.
- `initialize()` now returns `Result<(), String>` and rejects sample rates other than 48000 Hz with a descriptive error message.
- **Fixed (review issue 4):** `TransientSuppressor` now uses a sample-rate-aware release constant derived from a 20 ms target.
  `set_sample_rate` computes `decay` and `one_minus_decay` from sample rate, replacing the fixed `decay=0.99` fallback and making suppression release substantially less aggressive for music.
- **Fixed (review issue 5):** `TransientSuppressor` now applies a high-curvature discriminator before clamping.
  This rejects smoother onsets where `|\(x[n]-x[n-1])-(x[n-1]-x[n-2])| / |x[n]-x[n-1]|` is low, while still suppressing impulsive spikes.
- **Fixed (review issue 7):** `TransientSuppressor` now deinterleaves multi-channel input into
  planar scratch buffers and processes each channel in parallel, avoiding the single-threaded
  per-frame interleaved loop when `channels > 1`.

Deferred / noted:
- **2.1 (medium — ring buffer pointer overflow):** Absolute write/read positions remain monotonically increasing `usize`; practical overflow would require ~years of continuous audio on a 64-bit system. Full modulo wrap-on-advance deferred.
- **3.2 (major — alloc in reset):** `nnnoiseless::DenoiseState` has no `clear()` method, so `reset()` must allocate. Documented in code; callers must not invoke `reset()` per-callback.
- **1.5 (major — stereo image):** Independent per-channel RNNoise processing is a known limitation. Mid/side or gain-application-only approach deferred.

# 0.5.4

- Initial release. Shared DSP building blocks extracted from `sotf-plugin-denoiser` so that the new dedicated declick, hiss reducer, and speech denoiser plugins can reuse the same primitives.
- Modules: `transient` (TransientSuppressor for click repair), `hiss` (HissReducer for stationary high-frequency noise), `rnnoise` (RnnoiseBackend wrapping `nnnoiseless`).
# 0.5.6

- Improve the shared transient suppressor's startup priming, rejected-sample
  slope learning, sensitivity bounds, and non-finite recovery for Declick.
