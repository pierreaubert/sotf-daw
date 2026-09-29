# Loudness Compensation

## Modes

- **Manual** uses two half-gain low shelves, an optional mid peak, and two
  half-gain high shelves. The requested shelf gain is the total asymptotic gain;
  it is not doubled by the cascade.
- **ISO 226** computes the requested equal-loudness delta at all 29 ISO 226:2003
  frequencies and jointly fits a 20-biquad bank. The bank is normalized around
  the standard's 1 kHz phon reference.
- **Auto** uses the ISO bank and derives playback SPL from a measured SPL at
  engine volume 0 dB plus `playback_volume_db`. It is rejected until
  `auto_calibrated` is enabled. Digital dBFS, LUFS, and a volume scalar alone are
  not acoustic calibration.

ISO 226 is a population-average free-field relationship. Headphone or room use
may require a separate transfer correction and listening validation.
The implemented edition is **ISO 226:2003**, clause 4.1 and Table 1, not the
2023 revision. Its specified range is 20–90 phon through 4 kHz and 20–80 phon at
5–12.5 kHz. The existing controls retain their range; computed treble contours
above 80 phon are extrapolations. Contour differences are a playback-EQ model,
not an ISO 532 broadband loudness calculation.

## Level and AutoGain policies

`headroom_normalized=false` is the default. It preserves the requested 1 kHz
reference and may create positive peaks, so provide downstream headroom or a
limiter. When `headroom_normalized=true`, the plugin scans the realized active
cascade through Nyquist and applies broadband attenuation equal to its positive
peak. Cuts never consume headroom. This changes the absolute 1 kHz level and is
therefore a visible user choice, not an implicit safety correction.

AutoGain has one canonical three-state control: `disabled`, `pre`, or `post`.
The legacy `auto_gain_enabled` boolean remains accepted for old presets and maps
to `post`/`disabled`. AutoGain is an LUFS matching loop and is separate from SPL
calibration and ISO contour generation.

The gain controller compares the signals immediately before and after the EQ.
In Post mode neither measurement includes its compensation gain; in Pre mode
both include it. This estimates the EQ's level change without counting the
correction twice. Separate metering reports the actual raw input and final
output in both modes. The gain target and displayed loudness update every
50 ms of processed audio (rounded down to whole frames), independently of host
callback sizes, and a new target affects only subsequent samples. Gain smoothing
advances once per frame. The meter first supplies an estimate after 100 ms;
its momentary window fills over 400 ms. This adaptation adds no audio latency.

## Realtime contract

All filter design, ISO optimization, and full-band peak scans happen during
construction or control updates. `process_in_place` only processes prepared
state. Coefficient and mode changes crossfade old and new banks over 256 samples.
Processing and reset allocate no memory, return exactly `context.num_frames`, and
require an exact interleaved buffer length and matching initialized sample rate.
Frequencies are clamped to 45% of Nyquist-safe sample-rate space at 16–192 kHz.

## Example

```json
{
  "mode": 2,
  "reference_level_db": 78.0,
  "playback_volume_db": -18.0,
  "auto_calibrated": true,
  "headroom_normalized": false,
  "auto_gain_position": "disabled"
}
```

Here `reference_level_db` must be the measured listener-position SPL produced by
the actual playback chain at engine volume 0 dB. Recalibrate after changing DAC,
OS/amp gain, speaker placement, listening distance, or headphones.
