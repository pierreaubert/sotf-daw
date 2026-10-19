# sotf-plugin-channel-mute-solo

SOTF Channel Mute/Solo plugin for per-channel mute, solo, and dim.

Provides independent mute, solo, and dim controls for each audio channel with
smoothed gain transitions to avoid clicks. The default 5 ms setting is the time
constant of a per-sample one-pole smoother (about 63% of the gain change), not
an exact fade duration.

Solo has priority over mute and dim; a soloed channel remains audible even when
its mute flag is set. Initial configured mute states take effect on the first
sample. Reset preserves an in-flight fade for continuous transport stop/resume;
construct a fresh instance for a new initial routing state.

Callbacks must match the prepared rate. Bulk control batches reject malformed
values and channel-state lists before changing any targets. Settled state and
later re-entry use the same sample clock across callback sizes.

Mute changes newly processed samples. Audio already queued in downstream delays
or playback buffers retains its earlier gain. The host regression with a 5 ms
delay verifies the audible pre-mute prefix followed by silence.
