# Gate

## Overview

A dynamics processor with downward gating, upward expansion, and ducking. The default downward mode removes background noise and bleed below threshold. Upward mode boosts levels above threshold; Duck reduces program gain when the detector rises above threshold. All modes have adjustable ratio, hold time, sidechain filtering, and a quadratic knee.

## Features

### Mode and boost limit

| Parameter | Range | Default | Description |
|-----------|-------|---------|-------------|
| Mode | Downward / Upward / Duck | Downward | Structural choice; changing mode requires reconstruction |
| Max Boost | 0–24 dB | 12 dB | Upward gain ceiling; zero disables boost |

Mode and Max Boost are appended parameter indices 15 and 16. Existing presets
without these fields retain Downward mode. Mode JSON accepts the labels above,
case-insensitively, or numeric indices 0/1/2. Max Boost can change during playback;
its new ceiling applies immediately, including during hold and release.

With hysteresis and hold zero, Upward and Duck reflect the downward knee about
the threshold. Put `x = L - T`. Effect magnitude is zero for `x <= -K/2`,
`(R-1)*(x+K/2)^2/(2K)` inside the knee, and `(R-1)*x` above `K/2`.
For a hard knee it is `(R-1)*max(L-T,0)`. Upward adds this gain in dB,
capped by Max Boost; Duck subtracts it, capped by Range. A zero Range retains
the finite 240 dB attenuation ceiling. Ratio 1 gives no effect.

For threshold −20 dB, knee 6 dB, ratio 4 and detector levels −23/−20/−17 dB,
the effect magnitudes are 0/2.25/9 dB. Upward boosts; Duck attenuates.
The ducking ratio specifies this reflected expansion slope, not a conventional
compressor ratio. These are documented SOTF laws, not replicas of proprietary
gate styles.

For these two modes the detector activates at the **lower** knee edge
`T-K/2`, where effect starts continuously from zero. While above it, the target
follows the curve. If the detector falls into the hysteresis band, the most
recent effect target is retained. Below `T-K/2-H`, hold retains that target
for the configured duration, then release returns toward unity. Attack follows
increasing effect; release follows decreasing effect. A gradual fall through
the knee leaves a smaller held target than an abrupt sidechain dropout.

Use an external sidechain in Duck mode to reduce music beneath speech. The
program and detector signals can have independent levels. Stereo linking uses
the largest channel detector level in every mode.

Upward gain can exceed full scale; no output limiter is applied. Multiplications
that would exceed the finite f32 range saturate at that numeric limit. Normal
floating-point headroom is retained.

### Downward gating

Reduces gain when the input signal falls below the threshold. The ratio controls how aggressively the signal is attenuated — low ratios provide gentle noise reduction, high ratios approach full silence.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Threshold | -80 to 0 | -40 | dB | Center of the attenuation knee; opening level for a hard knee |
| Ratio | 1:1 to 100:1 | 10:1 | :1 | Attenuation depth. 1:1 = no gating, 100:1 ≈ full silence below threshold |
| Knee | 0 to 20 | 0 | dB | Width of a quadratic soft-knee transition around the threshold |

With hysteresis and hold both zero, the settled attenuation follows the full
quadratic knee. For threshold `T`, width `K`, ratio `R`, and detector level `L`
(all levels in dB), attenuation within the knee is
`(R - 1) * (T + K/2 - L)^2 / (2K)`. Below `T - K/2` it is
`(R - 1) * (T - L)`, and above `T + K/2` it is zero. Range caps this
attenuation. At `T = -20`, `K = 6`, and `R = 4`, an input at the threshold
receives 2.25 dB attenuation. Widths below 0.1 dB use a hard knee.

The gate's state opens at the **upper knee edge** `T + K/2`, or `T` for a hard
knee. With hysteresis `H`, it stays open until the detector falls below
`T + K/2 - H`. Hold then retains the unity-gain target for the configured
duration before the attenuation curve takes over. Hysteresis and hold therefore
make the result depend on previous signal levels; the static curve describes
the closed state after hold expires. Live knee changes and the smoothed
threshold move both state thresholds together.

### Experimental speech protection

Build the Gate crate, plugin facade, or engine with `neural-speech`, then opt in
through the structural preset configuration:

```json
{
  "mode": "Downward",
  "link_channels": true,
  "speech_protection": {
    "min_probability": 0.8,
    "max_age_ms": 150.0,
    "source_channel": 0
  }
}
```

By default, the RNNoise Full model analyzes the selected physical program channel
on a bounded worker. The classical detector keeps running. Accepted speech
evidence smoothly removes attenuation toward unity; the mode cannot add
attenuation or boost. Every linked channel receives the same protection gain.
An external sidechain continues to drive the classical detector, while speech
analysis uses the selected program channel before lookahead and gain.

`min_probability` accepts 0.5–1.0 and thresholds a raw model score; the score is
not a calibrated correctness probability. `max_age_ms` accepts 120–1000 ms and
counts the entire 100 ms source window plus worker/poll delay. Source rates
from 8 to 192 kHz are supported. Upward, Duck, unlinked operation, unavailable
features, and invalid source channels are rejected during configuration.

Audio latency remains the configured lookahead. Initial protection needs a
complete 100 ms observation window and worker/poll time, so speech onsets can
still receive classical gating. Missing, uncertain, stale, or late results
return toward classical gain using Release; entering protection uses Attack.
These protection smoothers operate in linear gain fraction. Worker backpressure
drops windows rather than waiting in the audio callback. Reset invalidates
queued results and starts a fresh observation window.

The feature is experimental. Synthetic model, gain-bound, allocation, reset,
and source-age checks do not establish false-cut rates on real speech or false
protection rates on music/noise. Held-out recordings, listening, and real-host
CPU/memory/deadline validation remain outstanding. Omitting
`speech_protection` retains classical gating. Changing this structural option
requires reconstructing the plugin; existing scalar parameter indices remain
unchanged. `GateData.speech_protection` reports score, source age, usability,
protection amount, and submitted/dropped/observed window counts.

For recurrent Silero protection, build with `silero-speech` and select its
verified local model explicitly:

```json
{
  "mode": "Downward",
  "link_channels": true,
  "speech_protection": {
    "model": {
      "type": "silero",
      "model_path": "crates/sotf-plugins/models/silero-vad/silero_vad_16k_batch1.onnx",
      "sha256": "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51"
    },
    "min_probability": 0.8,
    "max_age_ms": 150.0,
    "source_channel": 0
  }
}
```

The model is prepared on a control thread and runs on a bounded worker. It
retains recurrent history across chronological packets. The worker resamples
causally to 16 kHz and evaluates 32 ms hops. Four complete hops (128 ms), plus
resampling, worker, and polling time, are needed after reset or a source gap.
The source-age policy adds a conservative 100 ms allowance to the age of the
latest frame endpoint; retained recurrent history may be older. Audio latency
remains the configured Gate lookahead. Backpressure retires pending
observations immediately and returns protection toward the classical result;
processing resumes from fresh model history. Reset clears queued generations.
Hosts that retain realtime preparation during a fast render can trigger this
fallback and are not guaranteed to reproduce learned playback decisions.
`warmup_complete`, `model_frames`, and `stream_restarts` supplement speech
telemetry. Omitting `model` preserves the previous RNNoise detector.

For file-chain offline rendering, the engine prepares the chain with
`ProcessingMode::Offline`. Both RNNoise and Silero then evaluate complete
analysis packets synchronously on the rendering thread, with no worker queue
or wall-clock pacing. The parallel rendering API falls back to one chronological
stream for learned Gate configurations; a finite slice preroll cannot guarantee
matching recurrent context or analysis-clock phase. Program audio keeps its existing frame count and
lookahead latency. Confidence, source-age, warmup, and gain bounds still apply.
Incomplete final analysis packets remain incomplete; future audio is never
invented to protect an earlier onset.

Custom hosts select this mode before processing with
`DawHost::new_with_processing_mode(channels, rate, ProcessingMode::Offline)`.
Direct plugin callers use `initialize_with_mode(rate, ProcessingMode::Offline)`
through the appropriate plugin trait. Reset preserves mode and restarts model
history; ordinary `initialize(rate)` selects realtime behavior again. Offline
inference may allocate and block, so an offline-prepared plugin or host belongs
on a rendering thread. Existing timeline graphs retain their preparation mode;
callers must prepare their track/master hosts offline explicitly. Automatic
timeline graph reconstruction remains outstanding. Mode is not a preset
parameter or an automation event.
Repeated offline rendering is independent of worker scheduling; its decisions
can differ from realtime playback, which may use delayed observations or fall
back under load. The local regressions cover both models, resets, irregular
block partitions, external-key adaptation, and actual WAV rendering.

Silero remains experimental: rejecting the existing violin/cymbal probes and
matching independent inference states does not establish held-out speech
recall, music rejection, or DAW deadline compliance. The pinned model, license,
and source manifest are in `crates/sotf-plugins/models/silero-vad`. Its fixed-batch
derivative changes only shape annotations, with unchanged weights/operators.
Verify its source identities and ONNX Runtime reference cases with
`scripts/ml/verify_gate_vad.py` in the pinned
`scripts/ml/gate-vad-requirements.txt` environment. The native comparison uses
the same model loader/inference implementation as the live worker:

```bash
cargo run -p sotf-plugin-gate --bin qa-gate-silero \
  --features qa,silero-vad-eval --release --locked -- \
  crates/sotf-plugins/models/silero-vad/silero_vad_16k_batch1.onnx \
  audit/artifacts/ml-gate-silero-20261025/fixed-batch/ort-golden.json
```

### Speech quality development comparison

Run the frozen comparison with an unused output directory:

```bash
cargo run -p sotf-plugin-gate --bin qa-gate-speech \
  --features qa,silero-speech --release --locked -- \
  crates/sotf-plugins/models/silero-vad/silero_vad_16k_batch1.onnx \
  /path/to/new-results
```

The 22 cases compare classical, RNNoise, and Silero with a fixed gate curve and
confidence threshold: original and 0.1 gain, clean and 10 dB white-noise mixtures,
plus noise-only controls. A linked pilot measures actual gain while a separate
key reproduces mono input detection. Analytic gain/loss, mono/pilot agreement,
activity exclusions, and annotation endpoint selection have calibration tests.
Saved per-window component energies permit independent reduction.

Results in `audit/artifacts/ml-gate-quality-20261025` are conditional development
proxies. Silero reduced cuts in six speech-region/case rows and triggered no
false protection on these negative controls. It failed to activate on the
quieter plosive clip, leaving the classical 65 cut windows unchanged; RNNoise
reduced that count to 30 but also protected instrument tails. Energy-weighted
speech loss was often already small. These window counts establish neither
word intelligibility nor population accuracy. No model or threshold was tuned
from the comparison.

Both runs are retained. `scripts/ml/analyze_gate_quality.py` independently
checks 46 region summaries and source-frame coverage from saved energies. Its
final analysis repairs a binary-float endpoint error in the original reports;
the current native tool also repairs near-integer timestamp conversion. The
correction changes measurement selection, not controller output.

### Timing

Controls the gate's response speed. Fast attack reduces attenuation quickly to preserve transients. Hold keeps the unity-gain target for a set time after the signal drops below the closing threshold (prevents chattering). Release controls increasing attenuation. Attack and release are one-pole time constants in dB; after one time constant, approximately 63% of a step toward the new gain target is complete.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Attack | 0.1 to 50 | 1 | ms | Time constant for decreasing attenuation |
| Hold | 0 to 1000 | 10 | ms | Time the unity-gain target remains after signal drops below the closing threshold |
| Release | 10 to 2000 | 100 | ms | Time constant for increasing attenuation after hold expires |

### Sidechain & Channel Linking

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Mix | 0 to 100 | 100 | % | Dry/wet blend. Allows parallel gating |
| Link Channels | Linked/Unlinked | Linked | — | Linked: gate opens/closes for all channels together |
| Sidechain HPF | 0 to 200 | 0 | Hz | High-pass filter on detector. Prevents low rumble from holding gate open |
| HPF Order | 2nd / 4th | 2nd | — | Detector HPF slope; structural |
| Detection | Peak / RMS | Peak | — | Detector model; structural |
| External Sidechain | Off / On | Off | — | Uses a matching detector channel after each frame's programme channels; structural |
| Range | 0 to 120 | 80 | dB | Maximum attenuation; 0 means unlimited with a finite 240 dB ceiling |
| Hysteresis | 0 to 12 | 4 | dB | Difference between opening and closing thresholds |
| Lookahead | 0 to 20 | 0 | ms | Programme delay for transient detection; structural latency |

### Host and realtime contract

- Initialize before the first callback. The process context must retain that
  sample rate and the buffer must contain exactly `num_frames * input_channels()`
  interleaved samples; checked arithmetic errors are reported.
- `mode`, `link_channels`, sidechain HPF frequency/order, detection mode, external
  sidechain mode, and lookahead require graph replacement. Writing the current
  value is an accepted no-op; actual live changes are rejected transactionally
  without moving or clearing the active delay line. The host must latency-align
  old and replacement graph plans before any crossfade.
- External-sidechain samples are read-only. Non-finite programme or detector
  samples are interpreted as silence before filters, detectors, and delay state.
- Processing, realtime parameter setters, and reset allocate no memory. Reset
  deterministically clears envelopes, hold state, smoothers, detectors, filters,
  delay lines, diagnostic counters, and scratch storage.
- Monitoring snapshots are immutable after publication and update at a
  sample-derived 30 Hz cadence independent of callback partitioning.
- Monitoring `gain_db` reports signed wet gain before Mix: positive for
  Upward, negative for Downward/Duck. `attenuation_db` remains nonnegative and
  is zero in Upward. `effect_active` means any wet gain magnitude is at least
  0.1 dB. `gate_open` is the detector hysteresis latch (upper knee edge for
  Downward, lower for Upward/Duck); it excludes the subsequent hold period.
  Legacy `is_open` means any channel has less than 0.1 dB attenuation, so it
  remains true in Upward even while boosting. These are wet-effect indicators,
  including when Mix is zero.

## Demos

### Demo: Drum Gate

**Scenario:** A snare drum mic picks up hi-hat and kick bleed.
**Before:** Hi-hat and kick are audible between snare hits.
**After:** Clean snare hits with silence between — bleed is removed.
**Config:**
```json
{
  "threshold_db": -30.0,
  "ratio": 50.0,
  "attack_ms": 0.5,
  "hold_ms": 50.0,
  "release_ms": 100.0,
  "sidechain_hpf_hz": 100.0
}
```

### Demo: Vocal Noise Reduction

**Scenario:** A vocal recording has room noise and HVAC hum between phrases.
**Before:** Audible background noise during pauses.
**After:** Clean silence between vocal phrases with natural-sounding transitions.
**Config:**
```json
{
  "threshold_db": -45.0,
  "ratio": 8.0,
  "attack_ms": 2.0,
  "hold_ms": 200.0,
  "release_ms": 300.0,
  "sidechain_hpf_hz": 80.0
}
```

### Demo: Gentle Noise Floor Reduction

**Scenario:** A recording has a mild noise floor that's only noticeable in quiet sections.
**Before:** Slight hiss audible during pauses.
**After:** Noise floor is gently reduced without obvious gating artifacts.
**Config:**
```json
{
  "threshold_db": -55.0,
  "ratio": 3.0,
  "attack_ms": 5.0,
  "hold_ms": 100.0,
  "release_ms": 500.0,
  "mix": 0.7
}
```

## Presets

### Drum Gate (Tight)
**Use case:** Clean drum mic isolation
```json
{
  "threshold_db": -30.0,
  "ratio": 50.0,
  "attack_ms": 0.5,
  "hold_ms": 30.0,
  "release_ms": 80.0,
  "link_channels": true,
  "sidechain_hpf_hz": 100.0,
  "mix": 1.0
}
```
**Tips:** Adjust threshold to just above the bleed level. Short hold for tight gating.

### Vocal Gate
**Use case:** Remove background noise between vocal phrases
```json
{
  "threshold_db": -45.0,
  "ratio": 10.0,
  "attack_ms": 1.0,
  "hold_ms": 200.0,
  "release_ms": 300.0,
  "link_channels": true,
  "sidechain_hpf_hz": 80.0,
  "mix": 1.0
}
```
**Tips:** Long hold (200+ ms) prevents the gate from closing during natural pauses within phrases.

### Gentle Denoise
**Use case:** Subtle noise floor reduction
```json
{
  "threshold_db": -55.0,
  "ratio": 3.0,
  "attack_ms": 5.0,
  "hold_ms": 100.0,
  "release_ms": 500.0,
  "link_channels": true,
  "sidechain_hpf_hz": 0.0,
  "mix": 1.0
}
```
**Tips:** Low ratio (2-4) creates a gentle "duck" rather than a hard cut. Less obvious than high ratios.

### Podcast Cleanup
**Use case:** Remove background noise for podcasts and conference calls
```json
{
  "threshold_db": -40.0,
  "ratio": 15.0,
  "attack_ms": 2.0,
  "hold_ms": 300.0,
  "release_ms": 400.0,
  "link_channels": true,
  "sidechain_hpf_hz": 60.0,
  "mix": 1.0
}
```
**Tips:** Generous hold and release prevent cutting off words. Test with natural speech to verify.

## Tips & Best Practices

- Set the threshold just above the noise floor — too high and you'll cut off quiet parts of the performance.
- Use hold time (50-300 ms) to prevent the gate from chattering on sustaining notes or reverb tails.
- The sidechain HPF helps when low-frequency rumble or HVAC hum keeps the gate open falsely.
- Link channels for stereo content to prevent the gate from opening on only one side.
- A low ratio (2-4:1) creates more natural-sounding noise reduction than a hard gate (50-100:1).
- For drum gating, use very fast attack (< 1 ms) to preserve the transient.
- For speech/vocals, use longer hold and release (200-500 ms) to avoid cutting off word endings.

## Signal Flow

```
Input → Sidechain HPF → Level Detection → Threshold Comparison
                                              ↓
                              Hold Timer → Gate State (Open/Hold/Closing)
                                              ↓
Input → Envelope Follower (attack/release) → Attenuation → Mix → Output
```

The gate has three states: Open (signal reached the upper knee edge and remains above the hysteresis closing threshold), Hold (signal dropped but timer active), Closing (hold expired, attenuation follows the curve with attack/release smoothing).
