# sotf-plugin-upmixer

SOTF Upmixer plugin for stereo to surround upmixing.

Converts stereo audio to surround formats (5.1 through 9.1.6) using FFT-based Direct/Ambient decomposition combined with VBAP panning for spatial placement.


### AutoGain reference and timing

AutoGain compares the raw output with stereo source delayed by the declared FFT latency. A prepared stereo ring keeps that reference current even while AutoGain is off (8 bytes per FFT frame, 16 KiB at FFT2048). Direct bypass uses the current, undelayed source and retains its existing mode-change reset.

The target updates every `sample_rate / 10` enabled frames and first affects the following frame, using the existing scalar smoothing. Disable/enable pauses/resumes meter, gain and clock state; reset starts fresh. During native drain the reference and meter advance once per generated tail frame, including the final partial block, and not when cached frames are copied to a smaller destination. Scratch is bounded even for callbacks above 8192 frames.

The raw renderer and final safety-cap behavior are unchanged. An active final cap still measures a whole callback and can make final output depend on callback size; the AutoGain timing guarantee does not remove that separate limitation. Public parameter setters also retain the existing metadata-cache rebuild, while processing immediately after prepared enable remains allocation-free.
