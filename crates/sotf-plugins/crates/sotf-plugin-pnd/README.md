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
