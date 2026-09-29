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

The callback is allocation-free, lock-free, frame-major, and accepts arbitrary
block sizes. Non-finite input is replaced locally with the last finite sample.
See `USAGE.md` and `UI.md` for contracts and controls.

## Finite streams

After a nonempty input stream, `drain` returns exactly eight frames of zero-input
continuation, including repaired or bypassed audio still held in lookahead. It
accepts any positive destination capacity aligned to the channel count, returns
at most eight frames per call, and leaves unused destination samples untouched.
An empty stream completes without output. The declared tail is eight frames.

The first successful drain closes the stream: subsequent nonempty input and
parameter changes require `reset` or successful reinitialization. Invalid drain
capacities and sample rates leave the stream untouched. Completion is stable,
and both draining and reset reuse prepared storage without allocating or freeing.
