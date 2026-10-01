# sotf-plugin-resampler

Active code is split across `resampler_plugin.rs`, `resampler_quality.rs`, `params.rs`, unit and
integration tests, and `bin/qa_resampler.rs`. Audio is interleaved at the plugin boundary and
planar inside rubato.

Preserve these contracts:

- returned output frames are authoritative; never pad zero-output callbacks to input frames;
- distinguish destination capacity from immediately available output;
- drain through the object-safe `Plugin` contract until complete and preserve the sinc tail;
- report latency only in output-clock frames;
- reject a host input clock different from the configured input rate;
- keep ratio automation allocation-free and quality structural/off-thread;
- keep quality indices Fast=0, Medium=1, High=2 consistent across ParamSpec, Plugin, bridge, FFI;
- unity non-dynamic operation is bit-exact and zero latency;
- all capacity errors are transactional and retry-equivalent;
- reset clears residual, drain, ratio, and last-output state.
- cutoff smoothing defaults off (bit-exact legacy audio); when enabled,
  upward widening slews one prepared table per backend chunk while
  downward narrowing jumps immediately; smoothing changes reject after
  finalization and the flag persists across reset/rebuild;
- quantify filter response against the independent analytic DTFT and
  direct-sinc references; never equate a preset label with rejection.
- automate ratio/smoothing on the audio thread with the typed
  allocation-free API; the String API is the control-thread compat wrapper
  with identical messages and transactional refusals;
- derive slew artifact bounds from the analytic table model (HF),
  flat-band ripple (LF 0.1 dB), and coherent residual (-50 dB via the true
  2x2 least-squares fit); the transition window is blocks 9..13 and the
  full trajectory is 9..17 with a linear floor from the 2e-6 alias bound;
  isolate chirp via the cumulative clock plus the fixed-2.0 control leg.

Use impulse, tone, residual-boundary, extreme-ratio, irregular-partition, high-channel, allocation,
latency, bridge, host topology, and EOF-chain tests for behavioral changes.
