# sotf-plugin-matrix

SOTF Matrix plugin for channel matrix mixing.

Arbitrary N-to-P channel routing with linear per-crosspoint gain control, smoothed gain/phase/state transitions, sparse-route pruning, output mute/solo/dim, and defined stereo/5.1 and M/S presets.

Nonzero coefficients are retained even below 0.0001. A route is retired only
after its target and current gain are exactly zero; callback boundaries do not
shorten a fade. Ramps finish their last representable f32 step before the
settled kernel is adopted.

Routing preserves the actual linear sum without clipping or automatic
normalization. Correlated full-scale inputs can therefore produce values above
unity. Sparse maps follow their declared logical input/output order, and
unrouted physical outputs are cleared.
