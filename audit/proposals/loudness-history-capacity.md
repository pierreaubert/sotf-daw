# AUD119 — prepare the complete integrated-loudness history

The active local `math-dsp` fork reserves 6,000 f64 gating energies but admits
36,000. Its audio-thread `push_back` therefore grows storage after the first
6,000 entries. The host's ordinary loudness and current AutoGain monitors use
this integrated mode. Existing short cold tests do not exercise that boundary.

Proposed correction: reserve `MAX_GATING_BLOCKS` at construction. Keep every
energy, gate, rolling eviction, time window and reset operation unchanged.
Modes without integrated measurement retain zero history capacity. This adds
240,000 requested bytes per production integrated meter at preparation time;
explicit wide layouts can contain several such meters. No claim of lower
memory use or changed whole-program semantics is made.

Verification will first reproduce allocation/free counts through the real
public dependency and host APIs. Cross the 6,000, 12,000, 24,000 and 36,000
entry boundaries, continue beyond rolling wrap, reset and repeat. A supported
20 Hz clock makes the hour-long storage timeline cheap; this is a storage
test, not a loudness calibration claim. Also exercise an ordinary audio rate
through the direct dependency. Prepare owners on a control thread and measure
the first callback thread without counting thread setup or owner destruction.
Existing backend gating/reset numerical tests and focused host checks must
remain green. Record the new fork delta and reserve cost in `SOTF_FORK.md`.

Scope: one constructor reserve/comment, public allocation regressions and fork
documentation. No host queue, engine protocol, standard constants, public API,
dependency version, MIDI or IAMF changes.
