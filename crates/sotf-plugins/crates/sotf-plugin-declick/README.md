# sotf-plugin-declick

SOTF Declick detects short impulsive corruption from robust context and
reconstructs it by interpolation. It uses eight samples of fixed lookahead
(0.167 ms at 48 kHz), reports that latency to the host, and keeps the dry path
latency-matched while bypassed.

The detector compares each candidate with pre/post medians and median absolute
deviation, while rejecting persistent steps and high-variation programme.
Adjacent channel pairs share the detection decision by default so stereo and
surround images remain coherent; interpolation remains channel-specific.

Parameters:

- `enabled`: smoothly crossfade between delayed dry and repaired audio.
- `sensitivity` (1–100): lower values repair more candidates; changes are
  smoothed over 5 ms.
- `link_channels`: link decisions in adjacent channel pairs; disable for fully
  independent channels.
- `mode` (Random/Periodic): periodic repetition tracking with phase
  prediction; falls back to random-style detection without a lock.
- `bands` (Fullband/2-band/3-band): complementary multiband detection with a
  fullband supervisor gate; fullband preserves legacy behavior.
- `crossover_hz` (80–12000): band split frequency.
- `frequency_skew` (−1–1): bias detection toward high (+) or low (−) bands.
- `repair_width` (0–8 samples): symmetric repair extension; adds equal
  latency on new-mode paths (legacy path stays at eight samples).
- `audition_residual`: output the aligned residual (removed clicks) instead
  of repaired audio.

Default settings (random mode, fullband, zero width) use the legacy path
bit-exactly: eight samples of latency and the accepted detector behavior.
New-mode paths add only the configured repair width to that latency.

The callback is allocation-free, lock-free, frame-major, and accepts arbitrary
block sizes. Non-finite input is replaced locally with the last finite sample.
See `USAGE.md` and `UI.md` for contracts and controls.

## Finite streams

After a nonempty input stream, `drain` returns exactly `latency_samples`
frames of zero-input continuation (eight by default, plus repair width on
new-mode paths), including repaired or bypassed audio still held in lookahead.
It accepts any positive destination capacity aligned to the channel count and
leaves unused destination samples untouched. An empty stream completes without
output. The declared tail equals the reported latency.

The first successful drain closes the stream: subsequent nonempty input and
parameter changes require `reset` or successful reinitialization. Invalid drain
capacities and sample rates leave the stream untouched. Completion is stable,
and both draining and reset reuse prepared storage without allocating or freeing.
