# AUD119 — complete integrated history prepared at construction

The `math-dsp` crate in the sibling `../math-audio` checkout reserved 6,000 integrated gating entries but
retained up to 36,000. Two new public dependency/host tests independently
reproduced three allocation/free pairs as the first measurement epoch crossed
the former reserve boundaries. Reset retained the grown storage, explaining
why already warmed tests could miss the defect.

The constructor now reserves the existing `MAX_GATING_BLOCKS` bound. No filter,
gate, observation, eviction, peak, query or reset arithmetic changed. Modes
without integrated measurement retain no gating history. Production requested
storage increases from 48,000 to 288,000 bytes per integrated meter; wide
explicit layouts can own several meters. See
[the proposal](proposals/loudness-history-capacity.md) and
`../math-audio/crates/math-dsp/src/ebur128/ebu_r128.rs`.

Both formerly failing tests pass through old growth boundaries, an entire
production history, rolling wrap and reset with zero allocations and frees.
The direct backend includes a normal 8 kHz stereo stream and a non-integrated
control. Accepted 20 Hz silence clocks accelerate additional storage tests;
they make no filter-calibration claim.

All 24 existing backend EBU tests pass. Backend library Clippy passes with the
two previously documented upstream exceptions; host strict all-target Clippy
and its full regression suite also pass with AUD117. Logs:
`/tmp/sotf-loudness-history-{red,green,backend,backend-clippy}.log`,
`/tmp/sotf-lra-host-full.log`, `/tmp/sotf-lra-clippy-complete.log`.

The new nonzero full LRA-history allocation test is separate from this
constructor-only correction and is documented under [AUD117](loudness-range.md).
