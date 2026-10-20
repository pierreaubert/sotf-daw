# sotf-plugin-pnd

PND is a fixed-frame, duration-preserving pitch-drift correction insert.

Preserve these contracts:

- every successful callback returns and overwrites exactly `context.num_frames`;
- device-clock correction and variable-duration SRC stay outside this plugin;
- causal phase-vocoder latency is 2047 frames for every callback partition;
- keep phase/frequency histories in f64 and FFT/audio/magnitudes in f32;
  wrap synthesis phase before oscillator evaluation instead of resetting phase
  at neutral-ratio crossings;
- initialization owns FFT planning/allocation; process and reset allocate nothing;
- all channels keep their order and frame count and share one correction ratio;
- multi-channel analysis excludes low-confidence observations before consensus;
- referenced weak fundamentals may use at most eight harmonics with coprime,
  material, bin-consistent support; preserve direct-pilot fallback and reject
  numerical sidelobes/lone octaves. This is not a polyphonic pitch oracle;
- zero reference is change-only tracking and cannot identify constant offset;
- legacy `phase_vocoder` false/true values both migrate explicitly to the sole duration-preserving engine;
- identity/peak phase locking and onset resets require objective transient and
  inter-channel phase tests;
- formant preservation is an optional structural mode backed by a bounded
  spectral-envelope model; retain independent spectral and listening gates.

Use impulse partition, long-duration correction, transactional error, detector
amplitude/SNR/bin-edge/motion, unity SNR, transient localization, multichannel,
and allocation tests for behavioral changes.

`tests/audit_estimator.rs` fixes known-trajectory accuracy/confidence gates at
the Hann-window center and guards heap activity. Do not substitute estimator
diagnostics for end-to-end corrected-audio/stereo validation.

`tests/audit_audio.rs` calibrates its f64 analytic-signal frequency probe before
checking rendered trajectories and stereo phase. Its fast-flutter rows are
characterization, not a suppression pass; keep unresolved quality work visible.
