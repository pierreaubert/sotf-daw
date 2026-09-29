# AUD099 — XTC aligned warm bypass verified

2026-09-28. Scope: XTC crate only, plus the approved proposal copied to
`audit/proposals/xtc-aligned-bypass.md`. No host, native-wrapper, manager-protocol,
worker-generation or filter-design changes. MIDI/IAMF excluded. Production and
tests are frozen following the checks below; independent review is complete.

## Defect and permanent red → green evidence

The public reproduction in `/tmp/sotf-xtc-bypass-proposal.md` showed disabled
impulses emitted at zero despite declared N latency, paused wet audio replayed
on enable, and actual DawHost parallel branches split into peaks at 0 and N.
The permanent `tests/aligned_bypass.rs` first contained exactly three assertions:

- disabled impulse: old frame 0 = 0.25, expected zero;
- enable history: old sample 544 = zero, expected marker 0.25 at its proper delay;
- actual host PDC: old frame 0 = 0.25, expected zero.

All three failed before production changes, then all passed. Logs:
`/tmp/sotf-xtc-aligned-red.log`, `/tmp/sotf-xtc-aligned-green.log`.

## Production changes and review locations

Only `src/lib/xtc_plugin.rs` changes DSP behavior:

- `XtcBypassState` at line 230: prepared 2N-f32 stereo ring, sample cursor,
  f64 coefficient/step, integer duration/remaining. Duration is
  `max(1, floor((rate+50)/100))`, i.e. nearest 10 ms in frames. New targets start
  from the current coefficient; same bool snapshots leave progress unchanged.
- `apply` at line 255: one delay advance and one fade advance per accepted output
  frame. Dry mapping is L/R followed by zeros. Exact mix=1 leaves old wet bits
  untouched; exact mix=0 copies dry. Intermediate convex mixing uses f64 so
  opposite-sign finite f32 extremes do not overflow the subtraction intermediate.
- Constructor line 631 allocates the ring with the fixed FFT size. The already
  staged initializer is unchanged: its successful final reset initializes the
  new clock; all failed preparation exits precede ring/ramp mutation.
- `process_audio` line 1405 removes the immediate disabled return. Existing wet
  FFT/OLA, meters, compensation, limiter and retirement run continuously; final
  apply follows the unchanged wet denormal handling at line 1521. Dry receives
  neither gain compensation nor limiting/denormal conversion.
- Enabled setter line 1636 updates the existing cached bool in place. It neither
  rebuilds schema strings nor allocates, and identical snapshots do not ramp.
- Reset line 1908 clears ring/history/EOS and snaps to the configured bool target.
- Normal process line 1985 advances `input_phase` for all accepted input, after
  successful preflight. Zero-frame and rejected calls do not advance any clock.

Tail changes are scalar and remain inside the existing canonical-hop drain:

- Settled dry (`target=false`, mix exactly zero): remaining=N; uniform bound=N.
- Wet or transitioning: `R=2N-H + ((H-S%H)%H)`, H=N/4, S **all** accepted frames;
  uniform bound=2N−1. R already exceeds the dry ring delay.
- No accepted input: COMPLETE without freezing. Invalid rate, incomplete shape
  or zero capacity while audio remains do not latch or change the epoch.
- Accepted nonempty EOF latches the uniform support before the first refill.
  `tail_length` retains it through cached output and completion, even if a fade
  finishes in the refill. Reset clears the latch. Same-value setters are accepted;
  changed controls/new input remain reset-required. Pending adoption stays frozen.
- Existing call quota is unchanged: one call if unread cache exists, plus
  ceil((remaining−unread)/H), minimum one. At full capacity dry uses four calls;
  wet/transition at most eight (a previously partial cache is accounted separately).

There is no audible support beyond this union: the finite windowed operator and
N-frame dry delay are zero after their supports; gain, limiter, transition and
meter states only multiply/mix current audio. Hidden wet history may remain after
settled-dry completion but is inaudible, and reset is required before re-enable.

## Permanent validation

New `tests/aligned_bypass.rs` (6 tests):

- Three disabled impulse sizes; 27 stale/fresh marker histories; six actual
  parallel DawHost PDC graphs, enabled and disabled.
- 96 dense renders: all eight FFT sizes 128–16384, four rates 44.1/48/96/192 kHz,
  three partitions including one-frame and oversized calls, six interrupted
  toggle segments, ring wraps, identical snapshots and reset. Independent
  `[N zeros]+source` oracle with unchanged 1.5e−6 FFT tolerance; the three
  partitions are also bit-identical to each other.
- 18 nonidentity matrix renders: 2/4 outputs, three rates, three partitions.
  Independent pointwise matrix and closed-form segment ramps verify exact 10 ms
  completion/reversal and extra-channel fade; partitions are bit-identical.
- Public finite ±1e30 programme verifies unclipped dry values and finite fades.

New `src/lib/bypass_tests.rs` (7 tests):

- Actual blend handles ±f32::MAX at five weights including exact endpoints.
- Four-rate ramp counter, identical snapshot, reversal and reset endpoint proof.
- Warm disabled versus always-enabled wet state: both meters, nontrivial seeded
  compensation, limiter envelope, FFT history and OLA match; dry is exact, and
  post-fade output matches the continuously enabled instance bit for bit.
- 144 EOS comparisons (2 FFT sizes × 3 rates × 6 endpoint/fade histories × 4
  capacities), against identical pre-EOF history plus canonical ordinary zeros.
  Exact length, waveform, metadata latch, completion and frozen controls checked.
  This is a continuation oracle for the existing nonlinear wet processing; the
  independent finite matrix/window and delayed-source oracles remain in the suite.
- Empty/preflight failure canaries, unchanged history, partial-cache quota.
- Eight prepared fresh-thread configurations (2/4 outputs × initial state × AG),
  two epochs each, measure **allocations AND deallocations** around changed and
  identical bool setters, first/steady dry and wet processing, partial drain,
  full drain, queries, completed snapshots and reset: **0 allocations, 0 frees**.
- A ninth fresh-thread test completes a filter fade while disabled with **0/0**
  heap activity and proves the previous owner survives until control-side drop.

The existing 24-case failed matrix initialization test now enters an actual fade
and checks dry ring/cursor/mix/step/remaining/duration plus input phase/tail latch,
in addition to prior filter/clock/EOS state. Existing generation race, cold
publication/retirement, f64 circular FIR/WOLA, startup and finite tests remain.
Old immediate-bypass fixtures now require exact delayed source values; the old
reset fixture explicitly resets after configuring its desired endpoint.

## Unchanged enabled waveform evidence

Isolated `/tmp/sotf-xtc-enabled-preservation.rs` linked once against the immutable
pre-change rlib from the initial public reproduction and once against the corrected
rlib. It covers 3 FFT sizes × 3 rates × AG on/off × diagnostic neutral/real filter,
with two reset epochs and varied callbacks. **2,345,760 emitted f32 samples
(9,383,040 bytes) match exactly**, not only within a numeric tolerance.
`/tmp/sotf-xtc-enabled-preservation.log`; binaries and waveform artifacts are under
`target/audit-tmp/xtc-bypass-probe/`. The compared corrected artifact predates the
semantically equivalent chunks_exact→as_chunks lint edit; final source's exact
endpoint branch and unchanged wet arithmetic were also inspected.

## Final commands and results

- `cargo test -p sotf-plugin-xtc --all-features`: **204 passed**, zero failed;
  one preexisting ignored documentation example. Includes the final ownership test.
  Log `/tmp/sotf-xtc-aligned-full.log`.
- `cargo clippy -p sotf-plugin-xtc --all-targets --all-features -- -D warnings`:
  clean. Log `/tmp/sotf-xtc-aligned-clippy.log`.
- `cargo run --release -p sotf-plugin-xtc --features qa --bin qa-xtc`: all seven
  checks pass (mono/AutoGain plus standard enabled and disabled checks).
  Log `/tmp/sotf-xtc-aligned-release-qa.log`.
- Rustfmt and scoped whitespace check: clean.

Release QA's existing five-second/48 kHz/512-frame stereo benchmark measured
14.99 ms enabled and 14.81 ms disabled, both about 0.30% of one core on this run.
This is a local smoke performance measurement, not an isolated comparative
benchmark or a universal guarantee. It confirms disabled processing now costs
roughly the warm wet path. New dry storage is exactly 8N bytes: 1–128 KiB over
supported FFT sizes, 16 KiB at the default N2048, plus small scalar state.

## Compatibility and remaining limits

Disabled audio gains the already advertised N-frame latency; toggles fade over
10 ms instead of switching immediately; disabled CPU rises because wet history
continues. Parameter IDs/defaults, output layout, all-enabled arithmetic, staged
initialization, generation invalidation, asynchronous adoption/retirement and
host PDC are preserved. AutoGain's ordinary once-per-ten-callback cadence remains
partition dependent; this change does not claim to solve it. No other lifecycle,
parameter allocation or host/manager policy was expanded.

## Independent review

Root reviewed production delay/ramp state, initialization preservation, boolean
updates, finite support/cache accounting and the independent matrix/EOF tests.
A separate read-only reviewer checked the same lifecycle and ownership contracts
and found no introduced blocker. Review record:
`/tmp/sotf-xtc-aligned-independent-review.md`. Neither review expands the stated
limits on extreme FFT input or ordinary AutoGain callback cadence.
