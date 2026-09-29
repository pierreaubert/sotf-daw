# sotf-plugin-downmix

SOTF Downmix plugin for multichannel to stereo downmixing.

Phase-coherent downmix from multichannel formats to stereo with explicit speaker-layout awareness for correct channel summation. Ambiguous 8- and 10-channel inputs require `input_layout` (for example `7.1`, `5.1.2`, `5.1.4`, or `7.1.2`). Phase-coherent Lo/Ro and matrix Lt/Rt are mutually exclusive structural modes; both use a fixed 2048-sample WOLA latency. Lt/Rt surround encoding uses a unity-magnitude spectral ±90° rotation.

The plugin applies the documented matrix gains exactly and does not silently
normalize the mix. Correlated full-scale channels can exceed 0 dBFS; reserve
headroom or add an explicit limiter downstream.

## Stream boundaries

Both spectral modes preload one half-window of zero history. The first
negative-time synthesis hop is discarded, while its overlap at programme time
zero is retained. This preserves the first samples at the existing fixed
2048-frame delay, independent of callback partitioning. Constructor mode
selection, setup mode changes, initialization and reset establish the same
zero-history timeline. Simple matrix mode retains its immediate output.

Ordinary processing requires finite input samples and a context rate matching
the configured rate (44.1 kHz directly after construction). Invalid nonempty
calls preserve output and retained audio. Constructor-time processing at that
default rate remains supported; initialization configures the playback rate
and LFE filters.

`drain` preserves the stored spectral response when LFE is absent, discarded by
Lt/Rt, or its left/right gains have both settled exactly to zero. With S accepted
frames, it emits `3072 + ((1024 - S % 1024) % 1024)` further stereo frames, at most
1024 per call. This conservative support includes trailing silence. The uniform
finite tail bound is 4095 frames; simple matrix mode has zero finite support.

Draining requires initialization, its matching sample rate and whole stereo
output frames. A nonempty spectral tail requires positive capacity. Invalid
calls leave audio state and output unchanged. Once eligible nonempty EOS starts,
new input and changed controls require reset or reinitialization; identical
known parameter snapshots remain harmless. An empty stream does not freeze.

Observable LFE lowpass response is recursive and reports an infinite tail.
An ITU target still fading from a nonzero LFE gain remains in that category.
Those modes retain immediate default drain completion without freezing; use
ordinary zero continuation with an explicitly selected offline duration to
render that response. No finite recursive LFE bound is claimed (AUD073).
