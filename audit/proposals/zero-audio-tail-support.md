# AUD073: BandMerge and TransientShaper zero-audio tails

Source inspection confirms both plugins retain coefficient/envelope/diagnostic state only. No program-audio delay, FIR/IIR audio filter, convolution, oscillator, or synthesis history exists in either path. Dependencies are the host utilities plus serde; BandMerge's BandSplit dependency is dev-only.

- BandMerge computes each output as the current input-band samples multiplied by finite smoothed band gains, then sums them. Gain/mute ramps and the optional reconstruction metric cannot generate output from zero input.
- TransientShaper's fast/slow followers and parameter smoothers compute a bounded gain applied to the current input frame. Peak protection also multiplies the current frame. Decaying detector state cannot generate output from a zero frame; it contains no audio-path filter.

Authorized minimal implementation: declare `TailLength::Finite(0)` and `drain_call_bound() == Some(1)` on the existing public traits. Preserve the existing default `drain` (immediate complete, no output), native capacity zero, process arithmetic, parameter behavior and reset behavior. The metadata is static even before initialization because there is no retained program-audio response in any state.

Before production edits, add public deterministic regressions that warm nonzero program, change gains/mutes or shaping controls immediately at the boundary, and independently require exact zero for every subsequent zero-input sample. Cover all 2–8 BandMerge band counts, mono/stereo/6-channel layouts, 44.1/48/192 kHz, attack/sustain extremes, dry/mixed/wet shaping, callback boundaries and reset. Separately pin metadata, capacity and one-call completion with untouched destination canaries and continued processing after no-op EOS. Fresh-thread allocation/deallocation counters cover metadata queries, begin/no-op drain and reset; TransientShaper's public adapter must forward the same metadata.

Only these two crates' source methods, new focused test files and changelogs will change. Run targeted red metadata tests, full two-crate tests and strict scoped Clippy. No new DSP behavior, lifecycle freeze, host/native wrapper edits, MIDI or IAMF work.
