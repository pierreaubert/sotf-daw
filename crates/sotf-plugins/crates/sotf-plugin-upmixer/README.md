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
separation quality. Latency-changing controls use the structural host admission
and preparation contract described below.


### AutoGain reference and timing

AutoGain compares the raw output with stereo source delayed by the declared FFT latency. A prepared stereo ring keeps that reference current even while AutoGain is off (8 bytes per FFT frame, 16 KiB at FFT2048). Direct bypass uses the current, undelayed source and retains its existing mode-change reset.

The target updates every `sample_rate / 10` enabled frames and first affects the following frame, using the existing scalar smoothing. Disable/enable pauses/resumes meter, gain and clock state; reset starts fresh. During native drain the reference and meter advance once per generated tail frame, including the final partial block, and not when cached frames are copied to a smaller destination. Scratch is bounded even for callbacks above 8192 frames.

Final safety protection runs after AutoGain on each generated output frame.
Processing immediately after prepared enable remains allocation-free.


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
establish perceptual quality or device deadline compliance.


### Safety-cap control and release

The earlier per-FFT bed-channel protection and HR headroom floor keep their
existing equations, coefficients and height-channel exclusion. Their
`safety_cap_db` control advances on the common
source analysis grid, with separate pending time flushed before a changed
safety target and during reset/reinitialization. Gain/table events and identical
snapshots do not observe that pending time.

The final cap measures each generated output frame after AutoGain, overlap-add,
HR mixing and binaural folding. It applies one gain to all channels, attacks
immediately and releases in physical time. Release retains the former 0.02 step
at 1024 frames/48 kHz (about a 1.056 s time constant). Future frame peaks cannot
attenuate an earlier quiet prefix. No lookahead or latency is added.

The final stage has an independent 50 ms cap-control trajectory, starting from
the configured preset value. Accepted targets affect subsequent output frames;
its elapsed time cannot be replaced by the analysis clock, which may already
have consumed more source within a large callback. Both stages retain the
0.001 dB cache threshold; the final cache also commits an exact snapped target.
Final control and release state use f64 to avoid high-rate convergence stalls.

Raw bypass remains uncapped and advances both control clocks; leaving bypass
keeps the established reset/latency behavior. Reset clears the final gain but
retains the current control value. Reinitialization commits pending analysis
time using the old rate before updating coefficients. Empty/invalid calls do
not advance controls. Native EOF processes generated tail-cache frames once;
copying them into smaller drain buffers does not advance the cap again.

Tests cover causality, linked ratios, an independent release law through
384 kHz, interrupted/lifecycle clocks, hot-signal rate/FFT/HR/AutoGain/binaural
routes, exact callback/tail partitions, strict configured-zero-dB bounds and
zero-heap processing/reset/EOF. They do not establish recorded-material
perceptual quality or worst-layout/device deadline compliance.


The historical AUD132 HR archive fixtures retain their original bytes, digests
and full-vector error limits. Their fixture controller explicitly replays the
archived callback-based cap-control trace, which also affected the HR headroom
floor below clipping. This isolates the HR routing oracle from the intentional
cap-clock change; no legacy safety mode exists in production. Current production
clocks are exercised separately by `tests/audit_safety_clock.rs`.


### Host latency and PDC preparation

Upmixer structural controls are rejected by every `DawHost` live queue route,
including the host-owned queue that previously allowed raw bypass to change
runtime latency without refreshing cached PDC. Low-level structural setters
remain available to prepare a detached plugin on the control thread. Build the
replacement graph before audio so channel geometry, latency and compensation
are prepared together; reset establishes a new stream boundary.

The real-plugin host tests verify 156 structural admissions and 36 parallel
upmixer/dry cases at 44.1/48/192 kHz, FFT2/512/4096 and raw bypass. The compensated
dry branch plus standalone filtered upmixer exactly matches the host graph.
Graph bypass/rebuild/reset and stereo-to-9.1.6-to-binaural reconstruction also
refresh latency correctly. Steady frequency-domain crossover pre-ringing is
part of the unchanged filtered response, not a pure-delay arrival oracle.

Native CLAP/VST3 wrapper activation tests verify saved raw bypass and low-latency
FFT states report 0/1024/2048 frames before audio. An active structural state
change returns error/silence while retaining the prepared DSP latency; the host
must deactivate/reactivate before the new latency is published. Automatic host
reactivation is not implemented. Packaged native bus-layout restrictions remain
unchanged. These are host integration checks, not an external DAW/device test.


### Recorded center-source observations

`tests/audit_recorded_center.rs` extends the existing coherent-source SL/C
energy guard to three recorded speech speakers, violin and cymbal, in 5.1 and
9.1.6 with HR disabled/enabled (20 cases). Identical L/R input is processed from
a cold start in irregular ordinary callbacks. The complete-source surround/center
energy ratio stays below the unchanged 5% limit. The old maximum was 4.8397%;
first-reliable coherence initialization reduces it to 2.4502%. All output is
finite. Source bytes and attribution remain in the existing speech-denoiser
and de-esser recording manifests.

The first 100 ms after declared latency is also reported separately, without an
inferred audibility gate. Cold-onset steering is improved, but these observations
do not establish onset audibility or speech intelligibility. These centered mono recordings do
not establish separation in mixed stereo programme, downmix compatibility,
listener localization, external DAW behavior or device deadlines.


### Coherence initialization

A band's first reliable covariance seeds all five median-history entries and
its smoothed coherence. Synthetic zero prehistory no longer describes a new
coherent source as diffuse while three median frames and the one-pole control
warm up. Subsequent median filtering, one-pole coefficients and energy threshold
are unchanged. Initialization/layout rebuild prepares one flag per band; reset
clears flags and history without heap activity. Empty/unreliable initial frames
do not complete bootstrap. No parameter, latency or channel-layout contract changes.

The cold/silence/reset reference fails with zero coherence before this fix and
passes after it. Existing AUD132 archive bytes, digests, comparison controllers
and error limits are unchanged and pass. Recorded speech0 first-100-ms SL/C drops
from about 14.73 to 0.1203; this is a numerical energy observation, not an audible
leakage threshold or a guarantee for mixed programme.

### macOS CLAP packaging

The macOS packaging recipe now writes a `.clap` directory with
`Contents/MacOS/<executable>` and `Contents/Info.plist`. A raw renamed dylib was
rejected by the installed CLAP validator as an invalid bundle. Isolated install
verification also covers replacement of that former flat artifact by a bundle.
Linux and Windows keep their existing flat shared-library formats.

The fresh Upmixer artifact passes 15 functional CLAP validator checks; five
unsupported optional checks are skipped. Scan-time is excluded while other
builds run. This validates native artifact loading, descriptor/audio/state and
parameter behavior for the stated build, not external DAW restart/PDC,
non-macOS runtime or device deadlines. No live user plugin folder is modified
by the audit harness.
