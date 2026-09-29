# sotf-plugin-multiband-expander

SOTF Multiband Expander plugin for multiband dynamic range expansion.

Splits the signal into frequency bands and applies independent expansion to each band for frequency-selective dynamics processing.

The same crate also owns the `expander` factory identity, which is a genuine
one-band broadband path with no crossover controls. The multiband identity
requires 2-5 bands. Band count and time-domain/spectral mode are structural and
must be changed by rebuilding the plugin off the audio thread.

Time-domain mode supports Peak/RMS detection, linked or independent channels,
sidechain HPF, lookahead, and auto makeup. Spectral mode uses a 1024-point dual
Hann STFT at 75% overlap (`hop=N/4`), `1/(1.5N)` normalization, and 1024 samples
of latency; it accepts only linked Peak detection with zero lookahead/HPF and no
auto makeup, so unsupported behavior is never silently ignored.

## Soft knee and state boundaries

The threshold is the center of the configured knee. With knee width `K`,
attenuation reaches unity at `threshold + K/2`; the opening trigger uses that
upper edge, and the closing trigger is `hysteresis` dB below it. Hold preserves
its existing duration in samples (time mode) or rounded hops (spectral mode).
Knees below 0.1 dB keep the hard-knee trigger at the threshold.

This corrects an audible discontinuity in older nonzero-knee presets: their
state machine opened at the knee center and skipped its upper half. Such
presets can now attenuate more near the threshold. Parameter values and IDs
are unchanged; no legacy curve is silently substituted. Spectral gains apply
to individual Hann-window bins, so a reconstructed tone combines the gains
of its center and neighboring bins rather than following one broadband gain.

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
zero during a time-domain wet-to-dry fade does not establish finite support.

Time-domain multiband wet audio includes recursive crossover response even with ratio 1 or
per-band bypass. It reports `TailLength::Infinite`; its existing zero-frame
COMPLETE drain behavior is retained; automatic recursive completion remains
unresolved (AUD073). This finite-case implementation does not recover those recursive
wet tails. Callers that need a chosen wet-tail duration can continue processing
zero input before EOS. Detector, gain and smoothing state evolves normally
while supported finite audio is drained.

Spectral mode has finite audio support even while its detector and envelope
history continues to evolve. For `N=1024`, `H=256`, and `T>0` accepted input
frames, wet or fading output drains through output frame
`2N + floor((T-1)/H)H` (exclusive). This is 1792-2047 additional frames, with
conservative tail metadata of 2047. Settled-dry spectral mode keeps its shorter
exact 1024-frame dry delay. A full spectral EOS epoch retains its 2047-frame
declaration through completion, even if the mix settles dry during drain.
Drain generates canonical 256-frame zero-input hops
and retains unread output when callers supply smaller buffers; callback and
drain partitioning do not change the result. No detector or envelope freeze is
needed: zero spectra cannot synthesize new audio after the final window.

Spectral startup primes the three preceding zero-padded analysis windows and
discards their negative-time synthesis. Neutral reconstruction now preserves
the first sample and every initial hop phase at the unchanged 1024-frame delay.
Nonlinear startup can differ because the expander now observes those partial
windows. Controls and detector laws are unchanged.

`drain_output_frames_max` is a per-call capacity; `tail_length` is a bound from
the last input and does not count down. Queries are allocation-free. The generic
in-place adapter can copy input to output before an inner `process` error;
processing errors do not promise destination preservation. Drain capacity
errors and direct/compiled EOS preflight retain their transactional guarantees.
