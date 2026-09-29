# AUD122 — finite-stream true-peak measurements

Implemented and verified in [the completed evidence report](../true-peak-finite-stream.md).
The following records the proposal prepared before production edits.

The corrected true-peak filter retains eleven source-frame intervals after the
last accepted sample. `LoudnessMonitorPlugin` inherits immediate drain completion
without advancing that filter. Its audio is a transparent tap, so adding audio
latency or emitted zero frames would be incorrect.

First reproduce a final impulse through the direct public plugin and actual
linear host. Compare the final measurement against explicit, independently
computed full FIR support while requiring zero emitted drain frames.

## Proposed contracts

- Add allocation-free `finish_true_peak` to the low-level host meter and backend
  EBU meter. Advance only the true-peak FIR through its eleven pending source
  intervals. Preserve K-weighting, M/S/I, LRA, sample peaks and correlation;
  no artificial programme time is counted. Existing peaks are retained until
  queried. Repeated finalization adds no duplicate response.
- Finalization closes the interpolation segment. Subsequent accepted input may
  begin another segment; reset starts a fresh complete measurement epoch.
- The plugin drains in one bounded call, emits no audio and leaves destination
  sentinels untouched. Validate initialization, sample rate and zero source
  frames before advancing state. Existing output capacity is zero.
- Extend the most recent published peak interval through the interpolation
  suffix, retaining its raw sample peaks and taking the maximum true peak.
  All scalar loudness/status and prepared nested publication rules remain
  coherent. A reset/disable epoch must never merge pre-reset cached values.
- Repeated successful drain leaves the final snapshot stable. Snapshot
  publication remains best effort under retained readers; a direct repeated
  drain may retry a pending publication without flushing or consuming twice.
  Do not introduce an unbounded drain wait or change host queue protocols.

Verify full convolution through every final FIR phase, supported rates and
channel widths, arbitrary callbacks, reset/reinitialize/disable, invalid drain
preflight, resumed accepted input, retained strong/Weak snapshots, ordinary and
compiled host taps, upstream tails, and cold allocations/frees. Verify engine
final-cache retrieval reaches the plugin's completed state. Keep unrelated
host branch and engine transition protocols outside this issue.
