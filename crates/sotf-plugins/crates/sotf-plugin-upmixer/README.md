# sotf-plugin-upmixer

SOTF Upmixer plugin for stereo to surround upmixing.

Converts stereo audio to surround formats (5.1 through 9.1.6) using FFT-based Direct/Ambient decomposition combined with VBAP panning for spatial placement.

### Gain and routing control timing

The sixteen smoothed gain/routing controls advance on the common main/HR
analysis grid: the smaller of their two hops, normally 256 source frames.
Their 50 ms one-pole law is unchanged. A changed target first accounts for
elapsed time under its previous target, including a partial analysis hop;
same-value snapshots and empty callbacks do not change that timeline. HR
analysis sees the appropriate control state before each completed HR frame.

Reset commits pending elapsed time before clearing the grid. Successful
reinitialization commits it using the old clock before preparing the new one.
Raw bypass continues the control clock, and leaving bypass retains the
existing reset/latency behavior. Native EOF continues gain trajectories with
the established zero-input continuation policy.

Automation tests cover all sixteen controls, interrupted off-grid targets,
small/large FFTs, HR on/off, sample rates, no-op snapshots, complete tails and
recorded speech/violin input. The recorded cases establish automation timing,
not speech separation, localization or perceptual quality. Source attribution
remains in the speech-denoiser and de-esser fixture manifests.

`lfe_cutoff_hz`, `bandpass_hz`, and `height_hf_cap_hz` now use the same fixed
analysis grid. Their pending elapsed time is separate from gain controls;
only a changed table target flushes partial table time. Reset/reinitialization
commit it with the old coefficient, raw bypass advances it, and native EOF
continues it through the established generated-zero timeline. The existing
smoothing law, frequency response, thresholds and structural update policies
are preserved. Table rebuilds reuse prepared storage; unchanged tables do not
rebuild at grid boundaries.

Each table control has a measured nonzero rendering effect and matches exactly
across large/irregular callbacks versus one-frame calls, including recorded
speech/violin and full tails. These cases establish timing, not perceptual
separation quality. `safety_cap_db` automation and active final output capping
still remain callback-dependent. These guarantees do not imply host PDC
synchronization on raw-bypass latency changes.


### AutoGain reference and timing

AutoGain compares the raw output with stereo source delayed by the declared FFT latency. A prepared stereo ring keeps that reference current even while AutoGain is off (8 bytes per FFT frame, 16 KiB at FFT2048). Direct bypass uses the current, undelayed source and retains its existing mode-change reset.

The target updates every `sample_rate / 10` enabled frames and first affects the following frame, using the existing scalar smoothing. Disable/enable pauses/resumes meter, gain and clock state; reset starts fresh. During native drain the reference and meter advance once per generated tail frame, including the final partial block, and not when cached frames are copied to a smaller destination. Scratch is bounded even for callbacks above 8192 frames.

The raw renderer and final safety-cap behavior are unchanged. An active final cap still measures a whole callback and can make final output depend on callback size; the AutoGain timing guarantee does not remove that separate limitation. Processing immediately after prepared enable remains allocation-free.


### Realtime scalar updates

After initialization, the 34 realtime scalar setters update cached values in
place without allocating or freeing heap memory. Cached values use the accepted
DSP state, including coercion and clamping. Changed targets, repeated snapshots
and the following audio calls are checked at 48/192 kHz, FFT512/4096 and stereo/
9.1.6 output. Hosts retain ownership of parameter IDs across realtime calls.

Structural controls retain their preparation/rebuild paths. `enable_ml_detection`
is structural because enabling it can start inference resources. Optional ML
models and backends are tracked separately and are not validated by these tests.
Parameter enumeration and invalid-input error construction are control-thread
operations. Zero-heap setters do not establish a device deadline guarantee or
resolve the safety-cap and PDC timing limitations above.
