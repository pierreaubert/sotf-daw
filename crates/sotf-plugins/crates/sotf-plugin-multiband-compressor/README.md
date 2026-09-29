# sotf-plugin-multiband-compressor

SOTF Multiband Compressor plugin for multiband dynamic range compression.

Splits the signal into frequency bands and applies independent compression to each band for frequency-selective dynamics control.

## Finite streams

Call `initialize` before processing and repeatedly call `drain` after the last
input block. Drain writes only its returned output frames, up to 256 per call.
Invalid rate, incomplete channel frames or insufficient capacity for a pending
tail leave drain history untouched and can be retried. A supported nonempty EOS
freezes controls and new input until `reset` or reinitialization; identical
parameter snapshots remain accepted. Empty-stream drain is a no-op. Reset
retains parameter targets and clears EOS and audio history.

Exact finite drain is supported for one-band time-domain processing and a dry
mix whose current smoothed value and target are both exactly zero. The bound is
the active lookahead delay, including the one-frame minimum for a positive
sub-sample request. Lookahead metadata uses the actual ring delay. A target of
zero during a wet-to-dry fade does not establish finite support.

Multiband wet audio includes recursive crossover response even with ratio 1 or
per-band bypass. It reports `TailLength::Infinite`; its existing zero-frame
COMPLETE drain behavior is retained; automatic recursive completion remains
unresolved (AUD073). This finite-case implementation does not recover those recursive
wet tails. Callers that need a chosen wet-tail duration can continue processing
zero input before EOS. Detector, gain and smoothing state evolves normally
while supported finite audio is drained.

`drain_output_frames_max` is a per-call capacity; `tail_length` is a bound from
the last input and does not count down. Queries are allocation-free. The generic
in-place adapter can copy input to output before an inner `process` error;
processing errors do not promise destination preservation. Drain capacity
errors and direct/compiled EOS preflight retain their transactional guarantees.
