# sotf-plugin-multiband-expander

SOTF Multiband Expander plugin for multiband dynamic range expansion.

Splits the signal into frequency bands and applies independent expansion to each band for frequency-selective dynamics processing.

The same crate also owns the `expander` factory identity, which is a genuine
one-band broadband path with no crossover controls. The multiband identity
requires 2-5 bands. Band count and time-domain/spectral mode are structural and
must be changed by rebuilding the plugin off the audio thread.

Canonical and bridge factories apply the same identity policy: `expander`
forces one time-domain band even when serialized band count/mode differs;
`multiband_expander` requires 2–5 bands and honors the chosen mode. Both validate
before publishing an instance. Older bridge presets that used `Expander` for
spectral processing now select time-domain audio. Migrate them deliberately to
an explicit multiband spectral configuration, or direct native one-band spectral
construction when preserving that topology is necessary.

The facade's Rust `ExpanderPlugin` and parameter types remain compatibility
aliases, so their direct default constructors retain three bands. For direct
broadband processing, select `num_bands = 1` and `processing_mode = "time_domain"`
before `try_from_params`. Native fallible construction rejects invalid clocks
before allocating DSP state; legacy infallible constructors remain available.

Time-domain mode supports Peak/RMS detection, linked or independent channels,
sidechain HPF, lookahead, and auto makeup. Spectral mode uses a 1024-point dual
Hann STFT at 75% overlap (`hop=N/4`), `1/(1.5N)` normalization, and 1024 samples
of latency; it accepts only linked Peak detection with zero lookahead/HPF and no
auto makeup, so unsupported behavior is never silently ignored.

Constructor detection and processing-mode labels are case-insensitive, matching
validation. Spectral mode links each bin to the largest magnitude across all
channels before computing gain. Every channel keeps its original spectrum and
receives the shared detector trajectory; quieter channels no longer receive
extra expansion. This corrects earlier independent-channel spectral behavior
and can audibly change stereo presets. The 1024-frame latency and spectral
finite-support declaration are unchanged.

Time-domain crossover coefficients, recursive state and serial split
intermediates use f64, with f32 band buffers for dynamics. This corrects
low-cutoff rounding error at high sample rates and changes multiband numerical
audio. Topology, latency and parameter formats are unchanged. CPU cost of this
precision change has not been benchmarked.

The cascaded LR4 bank splits each preceding high-pass output; three-plus-band
sums are not generally flat all-pass responses because earlier low bands lack
the later splits' phase rotation. Unity-expansion validation uses this actual
topology. Spectral unity is checked independently against delayed input.
`tests/audit_reconstruction.rs` covers 120 time-domain impulse/solo configurations
and 90 crossover-edge burst/quiet-onset configurations across both modes,
including reset and callback partitioning. Its measured process callbacks must
be finite and have zero heap allocations/deallocations; release heap checks
require `--features qa`. Quiet onsets in this matrix use unity expansion;
nonlinear quiet-material and threshold-chatter validation is a separate check.

`tests/audit_dynamics.rs` checks 192 nonlinear configurations against f64
detector/envelope references and a full-complex FFT/offline overlap-add
reference. Synthetic quiet harmonic onsets, repeated threshold excursions,
per-band spectral thresholds, Peak/RMS, knee, hysteresis, hold and bounded range
are covered. The quiet-onset error is normalized separately from later loud
material; both it and the full waveform have a fixed 0.0035 relative-peak bound.
Mono and imbalanced stereo cases also check callback partition equivalence and
zero heap activity. These deterministic synthetic signals are not a recorded
music listening evaluation.

The classical baseline already supplies hysteresis, bounded attenuation and
explicit frequency grouping through user-selected crossover frequencies and
per-band parameters. The nonlinear reference exercises these controls with
distinct spectral thresholds. This audit retains that predictable grouping;
it does not add an adaptive mask or a new fixed psychoacoustic band profile.

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
