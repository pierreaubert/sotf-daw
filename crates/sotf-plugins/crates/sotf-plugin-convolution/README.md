# sotf-plugin-convolution

SOTF realtime impulse-response convolution with uniform and non-uniform partitioned FFT backends.

Normal UPC/NUPC operation reports and preserves 1024 samples of latency, including while no IR is
loaded or a replacement is pending. The optional NUPC time-domain head reports zero latency.

## True-stereo routing

Enable the structural `true_stereo` option for a stereo input/output plugin and a four-channel IR.
IR channels must be ordered **LL, LR, RL, RR**, with the first letter identifying the input:
`wet_left = left * LL + right * RL`, `wet_right = left * LR + right * RR`, where `*` is convolution.
UPC, NUPC, and the NUPC direct head support this routing. Mix, gain, latency, and drain behavior
follow the same contracts as ordinary convolution.

The option defaults to `false`. Existing four-channel IR presets retain their original mapping:
left uses IR channel 0, right uses channel 1, and channels 2 and 3 are unused. True-stereo mode
requires exactly two plugin channels and exactly four loaded IR channels. It can also be selected
before loading an IR; the plugin then uses its normal latency-aligned dry path.

Factory JSON and engine presets persist `true_stereo`. Direct Rust callers can use
`from_params_with_routing(channels, sample_rate, params, true_stereo)`; the existing params struct
and `from_params` constructor retain their default routing. Changing the mode requires a
control-thread rebuild. The realtime scalar setter rejects mode changes.

See [usage examples](USAGE.md#true-stereo) for the channel map and configuration.

## Lifecycle

`tail_length()` reports a bound in output-rate frames, including that latency once. After a
history reset, the bound is `latency + longest_resampled_IR_length - 1`; with no IR it is the dry
delay alone. The getter uses cached scalar metadata and does not clone the active IR pointer.
While an asynchronous replacement is pending, it reports `Unknown`. Accepted replacements and
clears retain the prior bound and add the 128-frame held-output fade horizon, so a shorter IR
cannot prematurely stop a host that is still counting silence from earlier input. This can
over-report the tail after several live replacements; `reset()` clears that retained history.
`initialize()` ordinarily preserves DSP history; after accepted EOS it clears history and rearms
input together. A rate-change reload remains unknown until its prepared state is accepted.

IR decoding, resampling, FFT planning, and backend construction run off the audio thread. Async
requests are generation-tagged; failed or stale replacements leave the last-known-good IR active.
Old large states are reclaimed in the background. Use `load_status()` to distinguish idle, loading,
ready, and failed states.

WAV, FLAC, and AIFF IRs are supported. The loader requires valid sample-rate metadata and enforces
32 channels, 30 seconds, and a 512 MiB estimated realtime-backend budget.

## End of stream

Native `drain` emits zero-input continuation for the active UPC, NUPC, direct-head,
or inactive dry-delay path. The bound is the greater of `latency + IR length - 1`
and the remaining held-output replacement fade. Already-retired IRs do not add
another response; the native tail metadata can conservatively advertise more
history than this frozen stream actually retains. Latency is counted once.

Each call writes at most 1024 output frames and accepts any positive whole-frame
capacity. Only the returned prefix is written. Invalid rate, partial-frame or
insufficient-capacity calls leave the stream and destination unchanged. An
empty stream completes without entering EOS. The first valid drain freezes
control changes and new nonempty input until reset or successful initialize;
unchanged parameter writes remain accepted.

An asynchronous result, including one already ready when drain begins, remains
in its mailbox while the active response finishes. Workers cannot mutate active
DSP state. Reset permits ordinary processing to adopt the completion again.
Pending load metadata remains `Unknown` throughout this frozen drain.

The active state is an audio-owned Arc, so cold callbacks do not allocate an
ArcSwap reader slot. Replaced states move into existing background retirement
queues. Both consumed completion endpoints remain owned through process, drain,
and reset: releasing the receiver alone could free its message storage in the
callback. A subsequent control-thread load/clear or teardown releases them.

The engine's separate 4096-call drain limit can still reject long high-rate IRs
or multiple serial tails. This plugin makes bounded finite progress and does
not truncate its response to fit that policy.
