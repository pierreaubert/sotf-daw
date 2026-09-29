# sotf-plugin-crossfeed

Stereo headphone crossfeed with Bauer, Meier, Multiband, and compact
parametric HRTF modes.

SOTF Crossfeed plugin for stereo crossfeed on headphones.

Blends a controlled amount of each stereo channel into the opposite ear to simulate a speaker-like stereo image when listening on headphones.

AutoGain measures every accepted frame while enabled and publishes a new target
after each completed `max(sample_rate / 10, 1)`-frame interval. That target first
affects the following frame. At conventional audio rates this is a 10 Hz
measurement clock, independent of callback size. Disabling AutoGain freezes its
meter history and interval phase; reset, initialization, and the plugin's Off
route restart the phase. The existing mix transition retains its own callback
ramp behavior.
