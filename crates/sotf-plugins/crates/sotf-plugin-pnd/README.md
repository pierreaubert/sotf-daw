# sotf-plugin-pnd

PND is a fixed-frame, duration-preserving pitch-drift correction insert. Every
successful callback returns exactly `ProcessContext::num_frames`; device-clock
correction, FIFO fill control, timestamps, and variable-duration sample-rate
conversion belong at a stream boundary with independent producer and consumer
clocks. PND owns only its fixed-frame analysis/vocoder sample clock.

Correction uses a 2048-point, 512-hop Hann/WOLA phase vocoder with
instantaneous-frequency estimation, spectral-bin remapping, normalized
spectral-flux onset resets, and identity phase locking around remapped spectral
peaks. Its fixed causal latency is 2047 frames, including startup prefill and
group delay, independent of callback partitioning. An optional structural
`formant_preservation` mode estimates a smoothed log-magnitude envelope and
transports it to the original absolute frequencies with bounded gains;
`formant_strength` blends this correction from 0 to 1. The default mode remains
the legacy uniform correction path.

Automatic estimates compare adjacent analysis frames. Without an explicit
pilot, note, or clock reference they can detect change but cannot identify a
constant absolute pitch offset. Set `reference_frequency_hz` to a known pilot
or note for absolute correction; zero selects change-only tracking.

With an explicit reference, coherent harmonics can support a weak fundamental.
The bounded estimator examines the first eight harmonics, requires at least
two material, consistent observations with coprime harmonic indices, and
weights confidence by local prominence, noise and reference proximity. Its
consistency budget accounts for FFT bin width and harmonic number. Lone
octaves, even-only support and numerical sidelobes cannot establish harmonic
consensus; otherwise the original direct-pilot estimator remains the fallback.
This assumes coherent harmonic material around a known reference, and is not
a general polyphonic fundamental-frequency oracle. Referenced harmonic audio
can change; change-only tracking, parameter formats and vocoder latency are
unchanged. CPU cost of the added bounded search has not been benchmarked.

`tests/audit_estimator.rs` compares 96 known-phase static-offset, wow, flutter
and vibrato trajectories with analytic source frequency at the analysis-window
center, across 44.1/96 kHz, 220/440/880 Hz and four fundamental/overtone balances.
It checks accepted-frame accuracy, confidence coverage and zero heap activity;
six absent-reference octave/even-series cases check rejection. These are
estimator checks, not proof of corrected-audio or stereo-coherence performance.
Pitch trajectories alone do not reveal whether vibrato is deliberate; the
reference and correction strength express the caller's intended correction.

All channels are pitch-shifted independently with one shared correction ratio.
With multi-channel analysis enabled, low-confidence channels are excluded and
the remaining observations must form a confidence-weighted coherent cluster.
Contradictory channel estimates fail closed instead of being averaged into a
correction that no channel observed; silence and broadband noise do not outvote
a reliable tonal channel.

Legacy presets containing `phase_vocoder: false` or `true` migrate to the sole
duration-preserving engine. Schema v3 retains that migration and adds the
explicit formant mode while avoiding ambiguous fixed-frame SRC behavior.

## Finite streams

After the last program frame, call `drain` until it returns `complete`. PND
emits the retained suffix at the effective correction ratio and smoothed
strength present at EOF; padding does not update drift estimators or diagnostic
cadence. The fixed latency remains 2047 frames. For nonempty input the
conservative continuation is 3583–4094 frames, depending on the final 512-frame
hop phase; `tail_length` reports `Finite(4094)` after initialization.

Each drain call processes at most 512 frames and accepts any positive whole-frame
destination capacity, preserving its unused suffix. `drain_call_bound` reports
remaining successful calls when using that full capacity, including the final
completion call. Empty streams produce no padding and remain available for input.

Accepted nonempty EOF freezes new input and changed parameters until reset or
successful reinitialization. Identical recognized parameter snapshots, including
structural settings, are allocation-free no-ops during EOF; they do not reset
analyzers or rebuild metadata. Zero-frame process calls remain no-ops. Invalid
rate, capacity or reinitialization requests preserve the existing stream.
