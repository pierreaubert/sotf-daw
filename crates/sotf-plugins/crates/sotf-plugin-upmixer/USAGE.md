# Upmixer

## Overview

The upmixer converts stereo to multichannel audio or extends a 5.0, 5.1, or 7.1 bed with height channels. The surround-input path keeps the original bed by default and can create a subwoofer feed for full-range or bass-managed speakers.

Initialization accepts finite sample rates from 8 kHz through 384 kHz. A host or output device can impose a narrower range.

## Features

### Speaker Configuration

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Speaker Config | 2.0/5.0/5.1/7.1/5.1.2/5.1.4/7.1.2/7.1.4/9.1.4/9.1.6/5.1.6 | 5.1 | — | Output speaker layout (structural — requires rebuild) |

### Surround input

Select the input layout to match the actual channel order. The canonical SOTF order is:

| Input | Interleaved channels |
| --- | --- |
| 5.0 | Front left, front right, center, side left, side right |
| 5.1 | Front left, front right, center, LFE, side left, side right |
| 7.1 | Front left, front right, center, LFE, side left, side right, back left, back right |

Custom JSON layouts can assign the same roles to different indices. Unknown, duplicated, or missing roles and outputs that discard an input role are rejected. A 5.0 input can use any listed output except stereo. A 5.1 input requires an output with the same bed; 7.1 input requires an output with both side and back channels. Extra horizontal channels are silent. With bass mode off, a new LFE is silent and an incoming LFE is copied unchanged.

The engine checks a known preceding upmixer layout against the declared input, including through gain nodes. It currently receives only a channel count for a decoded source and cannot verify its speaker roles; the selected input layout must match the source's actual order. An intermediate plugin without declared role behavior makes that upstream layout unknown.

For four heights, the front and rear pairs come from separately analyzed front and surround sources. For 7.1, each rear source starts with one-third side and two-thirds back. Two heights blend the front and rear candidates equally; six heights use separate front and back pairs and their equal blend for the middle pair. Center and LFE never feed the height analysis. The height gain control multiplies a provisional -12 dB reference gain. Height bypass mutes the generated channels without bypassing bass routing. Bypass Surround is a real-time control that returns the delayed original bed and mutes generated channels; the legacy Bypass All setting remains structural.

Each output height has an independent short decorrelation delay, rounded to samples from 0.13–0.31 ms. These are deliberate height delays in addition to the reported FFT latency; they rotate phase and can affect acoustic summation. The decorrelation bypass fades between delayed and undelayed height signals. Neither the starting gain nor these delays are calibrated loudness targets.

The default FFT size is 2048, with a 1024-frame hop, periodic square-root Hann analysis/synthesis windows, and an unnormalized inverse transform scaled by `sqrt(2)/N` to compensate both the inverse transform and the input's `1/sqrt(2)` headroom. For surround input with heights, the reported algorithmic latency is `max(FFT size, 512)` frames, plus 128 frames at sample rates of 96 kHz or higher when the FFT size is at least 512. The extra delay spreads inverse FFT work across callbacks, and the bed and heights remain aligned. With the default FFT this is 2048 frames (42.67 ms) at 48 kHz and 2176 frames (22.67 ms) at 96 kHz. Low-latency mode uses an FFT of 1024, giving 1024 frames at 48 kHz and 1152 frames at 96 kHz. Changing that structural setting for surround input requires a new plugin instance and host latency renegotiation. A surround layout without heights reports zero FFT latency. Bass crossover phase and loudspeaker/room delays are separate from these figures.

Auto-gain, subharmonic synthesis, and binaural preview are unavailable for surround input. Stereo-only direct and spatial controls do not modify the preserved bed. The height safety cap affects generated heights only; it is not a true-peak limiter. Incoming bed samples are never limited in full-range mode.

### Native host compatibility

5.1.6 is available in the built-in host and VST3. CLAP cannot represent its top-middle left/right speakers in the current surround extension.

If a CLAP or VST3 host loads a saved state while the plugin is active, the plugin asks the host to reactivate it before applying the new state. The previous sound and latency remain in effect until then. If the host delays reactivation, reactivate the plugin to hear the saved settings. A state that is incompatible with the negotiated layout can cause activation to fail; select a compatible layout or load a valid state again.

### Surround bass routing

| Mode | Main speakers | Output LFE / subwoofer feed |
| --- | --- | --- |
| Off | Full range and unchanged | Existing LFE unchanged, or silence if the input has none |
| Derived | Full range and unchanged | Selected main channels are low-passed and summed |
| Managed | Selected mains are high-passed; other mains remain full range | The corresponding low bands are summed |

The initial crossover is a fourth-order Linkwitz–Riley filter at 80 Hz. The frequency, generated-feed gain, and individual source speakers, including center, are configurable. Every selected main channel enters the bass sum with unity source gain; no automatic normalization is applied, so correlated channels can produce a digital sum above full scale. Derived mode can reproduce the same bass from both the mains and subwoofer.

In the CLAP and VST3 plugins, bass settings are non-automatable controls. Changing one requests host reinitialization; the previous bass path continues until the host reactivates the plugin. If the host does not act on the request, reactivate the plugin to apply the change. Surround bass settings are retained but inactive for stereo input, then applied again when a compatible surround layout is activated.

If input LFE exists, enabling derived or managed bass requires **Combine existing LFE**. The programme LFE is not used as a bass source. At the final output, redirected bass is multiplied by `10^((bass_gain_db - lfe_playback_gain_db)/20)` and then added to the unmodified programme LFE. Set `lfe_playback_gain_db` to the boost actually applied by the downstream playback system; some systems use 10 dB, while others use a different convention. Do not apply the compensation twice.

The output LFE channel is the physical subwoofer feed in these modes. Ensure downstream routing retains it and does not bass-manage the same mains again. The electrical crossover's low/high sum has a flat magnitude but rotates phase; align subwoofer level, polarity, and delay using measurements at multiple listening positions.

For example, to manage a canonical 5.0 bed into 7.1.4:

```json
{
  "speaker_config": "7.1.4",
  "input_layout": {"channels": [
    {"index": 0, "role": "front_left"}, {"index": 1, "role": "front_right"},
    {"index": 2, "role": "front_center"}, {"index": 3, "role": "side_left"},
    {"index": 4, "role": "side_right"}
  ]},
  "bass_mode": "managed",
  "bass_cutoff_hz": 80.0,
  "bass_gain_db": 0.0,
  "lfe_playback_gain_db": 0.0
}
```

The player CLI provides `--upmixer-input-layout`, `--upmixer-bass-mode`, `--upmixer-bass-cutoff`, `--upmixer-bass-sources`, `--upmixer-bass-gain-db`, `--upmixer-lfe-playback-gain-db`, `--upmixer-combine-input-lfe`, `--upmixer-bypass-heights`, and `--upmixer-bypass-surround`. Layout and bass-routing changes rebuild the plugin outside the audio callback.

### Gains

Controls the level of each component routed to the speaker groups.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Front Direct | 0 to 2.0 | 1.0 | x | Gain for direct (center-panned) content to front speakers |
| Front Ambient | 0 to 2.0 | 0.5 | x | Gain for ambient content to front speakers |
| Rear Ambient | 0 to 2.0 | 1.0 | x | Gain for ambient content to rear/surround speakers |
| Height Gain | 0 to 2.0 | 1.0 | x | Gain for content routed to overhead/height speakers |

### LFE & Bass

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| LFE Gain | 0 to 2.0 | 1.0 | x | Subwoofer output level |
| LFE Cutoff | 20 to 180 | 120 | Hz | Low-pass filter cutoff for the LFE channel |
| Subharmonic Synth | On/Off | Off | — | Generate sub-harmonics below LFE cutoff for deeper bass |
| Sub Gain | 0 to 1.0 | 0.5 | x | Sub-harmonic synthesis level |
| Sub Freq | 20 to 80 | 40 | Hz | Sub-harmonic center frequency |
| Sub Attack | 1 to 100 | 10 | ms | Sub-harmonic envelope attack |
| Sub Release | 10 to 500 | 50 | ms | Sub-harmonic envelope release |

### Spatial

Controls how the stereo image is distributed across the surround field.

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Stereo Width | 0 to 1.0 | 0.5 | — | Spatial separation (0 = wide, 1 = narrow, 0.5 = balanced) |
| Center Spread | 0 to 1.0 | 0.0 | — | Spreads center content to L/R front speakers |
| Upmix Crossover | 150 to 350 | 250 | Hz | Frequency below which signals are treated as omnidirectional |

### Enhancement

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| HR Direct | On/Off | On | — | High-resolution FFT for sharper direct-path separation |
| HR Sharpen | 0 to 1.0 | 1.0 | — | Sharpening intensity for high-resolution direct path |
| Ambient Boost | 0.5 to 2.0 | 1.2 | x | Multiplier for ambient signal extraction |
| Safety Cap | 0 to 3 | 3 | dB | Maximum output peak level |

### Surround Routing (Advanced)

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Surround Direct Bleed | 0 to 1.0 | 0.15 | — | Direct signal leak into surround/height channels |
| Rear Ambient Boost | 1.0 to 3.0 | 1.0 | x | Extra gain for rear ambient content |
| Rear Late Reflection | 0 to 0.5 | 0.10 | — | Late reflection level for rear height channels |

### Height Channels (Advanced)

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Height HF Cap | 8000 to 20000 | 16000 | Hz | High-frequency limit for height channels |
| Height Transient Reduction | 0 to 1.0 | 0.6 | — | Reduce transients in height channels for smoother overhead sound |
| Height Direct Leak | 0 to 0.5 | 0.05 | — | Direct signal leak into height channels |

### Dialogue Detection (Advanced)

**Parameters:**

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Dialogue Weight | 0 to 1.0 | 0.4 | — | How aggressively dialogue is routed to center |
| Voice Freq Min | 200 to 800 | 500 | Hz | Lower bound of dialogue detection range |
| Voice Freq Max | 2000 to 5000 | 3000 | Hz | Upper bound of dialogue detection range |

## Demos

### Demo: Music in 5.1

**Scenario:** A stereo music track needs to be played on a 5.1 speaker system.
**Before:** Stereo audio plays only from front L/R speakers.
**After:** Full surround immersion — direct content in center/front, ambient in rears, bass in sub.
**Config:**
```json
{
  "speaker_config": "5.1",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.5,
  "gain_rear_ambient": 1.0,
  "stereo_width": 0.5,
  "lfe_cutoff_hz": 120.0
}
```

### Demo: Film Dialogue Enhancement

**Scenario:** A stereo film soundtrack needs surround upmixing with clear dialogue in the center.
**Before:** Dialogue and effects are mixed together in L/R.
**After:** Dialogue is locked to the center channel, effects and ambience envelope the room.
**Config:**
```json
{
  "speaker_config": "5.1",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.3,
  "gain_rear_ambient": 0.8,
  "dialogue_weight": 0.6,
  "stereo_width": 0.4
}
```

### Demo: Immersive Atmos Experience

**Scenario:** A stereo recording of a live concert needs to fill a 7.1.4 Atmos room.
**Before:** Stereo only — no height or surround presence.
**After:** Room ambience extracted to surrounds and overhead, stage presence maintained in front.
**Config:**
```json
{
  "speaker_config": "7.1.4",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.4,
  "gain_rear_ambient": 1.2,
  "height_gain": 0.6,
  "stereo_width": 0.5,
  "enable_hr_direct": true
}
```

## Presets

### Music 5.1
**Use case:** Balanced stereo-to-5.1 upmix for music listening
```json
{
  "speaker_config": "5.1",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.5,
  "gain_rear_ambient": 1.0,
  "height_gain": 0.5,
  "lfe_gain": 1.0,
  "lfe_cutoff_hz": 120.0,
  "stereo_width": 0.5,
  "center_spread": 0.0,
  "bandpass_hz": 250.0,
  "enable_hr_direct": true,
  "safety_cap_db": 0.0
}
```
**Tips:** Start here for most music. Increase rear ambient for more envelopment, decrease for tighter imaging.

### Cinema
**Use case:** Film/TV content with dialogue focus
```json
{
  "speaker_config": "5.1",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.3,
  "gain_rear_ambient": 0.8,
  "dialogue_weight": 0.6,
  "stereo_width": 0.4,
  "safety_cap_db": 0.0
}
```
**Tips:** Higher dialogue weight anchors speech to center. Reduce rear ambient if surrounds are distracting.

### Immersive Atmos
**Use case:** Height-enabled systems (5.1.4, 7.1.4)
```json
{
  "speaker_config": "7.1.4",
  "gain_front_direct": 1.0,
  "gain_front_ambient": 0.4,
  "gain_rear_ambient": 1.2,
  "height_gain": 0.6,
  "height_hf_cap_hz": 16000.0,
  "height_transient_reduction": 0.6,
  "height_direct_leak": 0.05,
  "enable_hr_direct": true,
  "safety_cap_db": 0.0
}
```
**Tips:** Height channels carry mostly ambient high-frequency content. Reduce height_direct_leak to prevent voice from appearing overhead.

### Wide Ambient
**Use case:** Maximum immersive envelopment
```json
{
  "speaker_config": "5.1",
  "gain_front_direct": 0.8,
  "gain_front_ambient": 0.6,
  "gain_rear_ambient": 1.4,
  "ambient_boost": 1.5,
  "stereo_width": 0.3,
  "safety_cap_db": 0.0
}
```
**Tips:** Emphasizes ambient content for a more enveloping experience. Best for ambient/electronic music.

## Tips & Best Practices

- The default stereo upmixer latency is 2048 samples. Surround-input latency depends on sample rate and FFT size as described above.
- The analysis starts with a zero-prefilled half-window. Negative-time synthesis
  samples are discarded, so startup includes the complete overlap needed to
  preserve the first input sample. Main and high-resolution paths use the same
  boundary convention; reset and FFT reconfiguration restart that timeline.
- High-resolution gain ramps are attached to individual output frames when the
  corresponding main analysis runs, so splitting an output hop across callbacks
  does not change its ramp.

### End of stream

After the last input block, hosts call `drain` to release startup scheduling,
partial analysis windows, main/HR overlap-add output, and the HR alignment delay.
Its finite bound covers full transform support and may include trailing zeros.
The crossover and decorrelation responses are applied within finite FFT blocks;
they do not run recursive IIR audio histories.

Subharmonic synthesis has recursive envelopes. If synthesis is enabled or an
envelope is still releasing, rendering continues for fourteen effective release
time constants plus one synthesis window after the finite bound, then clears
the oscillator/envelopes. The effective time constant comes from the actual
release coefficient `alpha`: `tau_samples = -1 / ln(1-alpha)`. This is a bounded
render truncation policy, not exact recursive silence or a promised residual
dB level. At ordinary sample rates, a 500 ms release adds approximately seven
seconds plus one FFT window. If coefficient quantization makes `alpha` zero at
an extreme rate, the configured release time provides the finite fallback cap.

The first drain freezes the bound. New nonempty input and parameter changes
require reset once draining starts. Repeated completed drains return zero
frames; empty streams and hard bypass finish immediately. Any positive capacity
of complete output frames is accepted. Invalid capacities leave state untouched.
Drain performs fixed-hop zero-input processing into preallocated storage, then
copies the requested frames, so changing destination capacity does not retime
its gain controls. Unused destination samples remain untouched.
- **Stereo Width** is the most important tuning parameter — 0.5 is balanced, lower values spread more to surrounds.
- Use **Safety Cap** at 0 dB to prevent clipping in the output channels.
- **HR Direct** improves center channel separation quality but uses more CPU.
- Height channels should carry mostly diffuse ambient content — keep **Height Direct Leak** low (0.05-0.15).
- The **Subharmonic Synth** generates sub-bass content for the LFE — use sparingly to avoid muddiness.
- **Dialogue Weight** controls how aggressively center-panned content goes to the center speaker — increase for speech-heavy content.
- Surround speakers should be wired to match the selected speaker configuration.

## Signal Flow

```
Input (Stereo) → FFT → Frequency Domain Analysis
                            ↓
              ┌── Direct Signal (high coherence, center-panned)
              ├── Ambient Signal (low coherence, diffuse)
              └── Dialogue Detection (optional)
                            ↓
              VBAP Panning → Front L/R (direct + ambient)
                           → Center (direct + dialogue)
                           → Surround L/R (ambient + decorrelation)
                           → Rear L/R (ambient + late reflections)
                           → Height (ambient, HF-capped, transient-reduced)
                           → LFE (low-pass filtered + sub-harmonic synth)
                            ↓
              Per-channel IFFT → Overlap-Add → Safety Cap → Output
```
