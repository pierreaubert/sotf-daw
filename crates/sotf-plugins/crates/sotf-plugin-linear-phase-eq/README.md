# sotf-plugin-linear-phase-eq

Parametric FIR EQ with selectable linear or minimum phase.

## What It Does

A parametric equalizer using FIR convolution. Linear-phase mode applies the same delay to every frequency in a fixed filter. Its centered impulse can ring before a transient and adds `N/2 + 32` samples per stage. Minimum-phase mode concentrates energy near the impulse start and reports the 32-sample convolution delay per stage; its frequency-dependent group delay is not zero. Mixed-phase design is not offered.

## Features

- **Linear-phase mode**: Constant group delay for a fixed filter
- **Minimum-phase mode**: Low-latency, causal response without linear-phase pre-ringing
- **FIR convolution**: Non-uniform partitioned convolution with a 32-sample head
- **Parametric bands**: Standard frequency, Q, and gain controls
- **High precision**: Ideal for mastering and critical listening
- **Auto Gain**: Normalizes the FIR's DC gain to unity. It is a predictable
  reference-point correction, not a perceptual loudness match.

## When to Use

- Crossover alignment where phase coherence matters
- Mastering chains where phase transparency is critical
- Situations where latency is acceptable (not live monitoring)

For the lowest CPU cost, prefer `sotf-plugin-eq` (minimum-phase IIR).

## Architecture

```
src/
├── lib.rs, params.rs       # Public facade and canonical parameter schema
└── lib/                    # FIR design, streaming convolution, and tests

The implementation preallocates steady-state partitioned-convolution state.
Band count/settings, FIR length, phase and Auto Gain are structural: rebuild
the plugin to change them. FIR design and planning therefore stay outside the
realtime callback. The prepared-update API below retains and blends both
filter histories. Mix remains
realtime-automatable and is smoothed per sample. Linear-phase latency is
`N/2 + 32` samples; minimum phase reports the 32-sample partition latency.

## Per-band channel routing

Each band accepts an optional `placement` (`stereo`, `left`, `right`, `mid`,
`side`; absent keeps the legacy stereo-linked route). Any `left`/`right`/
`mid`/`side` band selects the ordered cascade route: every band slot occupies
one FIR stage in band order (identity when inactive, so latency never depends
on which bands are enabled), Mid/Side bands process pairs via 0.5·(L±R)
encode/filter/decode, and bypassed domains run through identity-FIR engines
to stay delay-aligned. Latency is `stages * (fir_length / 2 + 32)` samples in
linear phase and `stages * 32` in minimum phase. Pair placements need
explicit disjoint `stereo_pairs` except on stereo input, which defaults to
`[[0, 1]]`. `auto_gain` stays stereo-linked only.
`channel_complex_response()` / `channel_group_delay_samples()` report the
per-channel cascade response for charts, using single-channel excitation
(diagonal transfer: Mid/Side spreading included, not correlated input).

## Dynamic band updates

Band filter shapes (`filter_type`, `frequency`, `q`, `gain_db`, `active`)
update without a rebuild: capture `snapshot_config()`, design
`prepare_band_update()` off the audio thread, then
`try_commit_prepared_update()` with a caller-owned `Option` slot (bounded,
allocation-free on success and refusal; primes the full cascade support from
recorded history; retains the prepared update on typed `CommitRefusal` for
correction or retry) to start a fixed 513-frame exact-0/1 output crossfade.
`commit_prepared_update()` is the control-thread compatibility wrapper (it
allocates its `String` error and frees the prepared update on refusal).
Phase mode and latency never change across an update. Reclaim the previous
banks with `take_retired_route()` off the audio thread before the next
commit. At commit, controls and the chart-facing response APIs report the
target design immediately while audio morphs old-to-new over the blend.

For incremental priming, use
`try_commit_prepared_update_with_budget(&mut prepared, max_prime_frames)`.
It returns `PreparedUpdateProgress`: input frames replayed by this call,
frames still needed to catch the current clock, and whether acceptance occurred.
While pending, the slot stays owned by the caller; live controls, response APIs
and output continue to use the old filter. Replay more frames than the live
stream advances between attempts (for example, twice each callback's frames)
to catch up. The usual 513-frame blend starts only after catch-up and acceptance;
signal latency does not change. A zero budget can accept when no history exists.
Reset, rate change, foreign-plugin reuse or overwritten unread history invalidates
a partly primed target with `StaleBase`; reprepare it off audio. Drop cancelled
candidates off audio as well. The budget counts whole target-route input frames,
so work still scales with channels/stages and FFT partition boundaries. It is
not a CPU-time budget. The synchronous API can finish a partly primed target
without replaying completed work.

The default host wrapper uses incremental priming. It permits two history
frames before each incoming sample, keeping the acceptance sample independent
of callback partitions. For initial retained history `H`, catch-up takes
`max(0, H - 2)` incoming frames before the fixed blend starts. This adds control
reaction time, not signal delay: a populated two-stage 8192-tap route at 48 kHz
needs 16444 frames (about 343 ms) before acceptance, followed by the blend.
The synchronous direct-DSP API remains available for explicit callers.

`control_status()` reports `priming_in_progress`, `priming_frames_replayed`
and `priming_frames_remaining` separately from accepted generation and blend
state. Each callback replays at most twice its input-frame count. Queue pops,
head cancellation and retirement routing occur once per callback, with at most
one acceptance. Status refreshes after render/drain as well as control reset.
An unaccepted candidate remains queued across EOF; it never changes the emitted
old-filter tail. Reset/rate change before acceptance invalidates partly primed
state, retains the old accepted band controls and requires cancellation/reprepare.

`dynamic_host::LinearPhaseEqDynamicPlugin` provides a host wrapper and a
shared `LinearPhaseEqControlHandle` through `get_data()`. Workers prepare from
`try_accepted_snapshot()` and submit via the bounded mailbox; audio accepts
at most one result per callback, and control reclaims retired/cancelled banks.
Every `LiveFilterSnapshot` carries an opaque `FilterRevision`, binding it to
one live plugin and its accepted generation. Priming/commit require that token
and an exact base-configuration match. An old result cannot revive after an
accepted no-op or A→B→A control changes; a snapshot from an identical other
graph is also refused. Rejected results remain retained with `StaleBase` until
explicit cancellation and off-audio reprepare. Detached snapshot tokens and
`accepted_generation()` agree with the DSP revision.

Jobs captured from the same current accepted revision still follow the queue
policy: the first eligible result accepts, then siblings become stale. This is
not a latest-request-wins sequencer. Callers must cancel or serialize intents
when that policy is required. Reset alone preserves the accepted revision;
partly primed history is separately invalidated. A real rate change consumes a
revision even if the rate later returns to its original value.

The in-memory Rust snapshot API gains the `revision` field. Obtain snapshots
from `snapshot_config()` or `try_accepted_snapshot()` and clone them for worker
preparation; do not assemble a new snapshot from copied parameter values or
transfer a token between graphs. The token is never serialized, and parameter
IDs, presets and FIR profile/design formats are unchanged. The 63-bit accepted
revision limit reserves the seqlock publication bit; further commits return
`RevisionExhausted` without consuming the payload or priming, and rate changes
refuse before mutation. Rebuild the plugin to obtain a new identity at that
limit instead of wrapping an old token into eligibility.
Acceptance is published only on a successful commit. Both f32 and direct f64
rendering validate the prepared sample clock
before consuming a mailbox result or writing output.

Ordinary band parameter setters still require structural rebuilding; this
prepared-update mechanism does not establish loaded native/FFI automation
coverage. The f64 bridge can grow scratch on the calling thread and is not
advertised as a native f64 realtime path.

The blend is a time-varying weighted sum of two FIR outputs. Each fixed linear
filter has constant group delay; a morph has no single stationary transfer
function. Charts report the target immediately. Reset installs that target
and clears history; EOF completes the blend while emitting finite support.
Processing two banks bounds blend work to twice the convolution work, but
a fresh synchronous commit also replays up to `stages * (N - 1 + 32)` frames
to prime the target. Finishing a partly primed candidate can include additional
intervening input, bounded by the retained ring capacity.
Allocation-free and bounded work alone do not establish a callback deadline.

## Audit validation and remaining limits

`tests/audit_dynamic_response.rs` compares whole and irregular callbacks with
an independent full-length, zero-padded f64 convolution, using exported FIR
taps and explicit Left/Mid matrix routing. It covers all four tap counts,
both phase modes, 44.1/96 kHz, and maximum-length two-stage ordered routes.
EOF starts 37 frames into the blend. Output and tail are bit-identical across
callback partitions; worst error against the offline reference is
`7.96309e-8` under the unchanged `1e-5` sample bound. This validates filtering,
priming and transition behavior; using the exported taps is not an independent
proof of the FIR designer.

Reset during a blend is separately measured with a streamed impulse. Maximum
complex-response error is `3.35951e-7` (bound `1e-5`); linear aligned phase is
`5.99268e-8` radians (bound `1e-4`); aligned group-delay error is
`5.40210e-5` samples (bound `0.5`). An independent bilinear-mapped peaking
prototype gives magnitude error `3.64639e-5` dB (bound `0.05`) for the tested
broad 6 kHz band. These probes do not establish arbitrary narrow-band accuracy.
The detached tests also cover out-of-order workers at maximum ordered length,
stale refusal/cancellation, fresh recovery, and reset against a fresh twin.
Successful commit/render/drain/reset legs assert zero allocations and frees.

Default-host tests cover both sample formats (the warmed f64 bridge matches
f32 output exactly), pending controls/progress, wrong-clock refusal during
priming, cancellation/reset/rate/EOF recovery and off-audio disposal. For both
phase modes at two 8192-tap Left/Mid stages, block sizes 1/73/128/257/1024 emit
bit-identical complete streams. The independently weighted old/target twin
reference has worst error `2.98024e-7` under `1e-5`. Factory, facade/DawHost and
running embedded-engine tests preserve exact-zero blend start, partition
invariance, finite EOF, dry alignment and unchanged `1e-5` bounds using the
specified catch-up acceptance clock. The real-manager ALSA-null test was only
compiled on this host; device playback and loaded native/FFI paths are not
established by these checks.

Eight additional incremental cases cover both phases, 44.1/96 kHz and legacy
or maximum-length two-stage ordered routes, with dense input and an initial
prefix beyond ring wrap. Every call checks its replay-frame budget; controls
stay unchanged until acceptance. Worst error against independent convolution
is `3.05565e-7` under `1e-5`. Lifecycle checks retain stale candidates without
allocation/free after reset, overwrite, transfer, rate change and drain.
Empty history and switching to synchronous completion are also covered.

The existing explicitly deferred `FIR-M1` diagnostic remains failing when
run: 96 kHz / 1024 taps / 500 Hz multiband error `-0.2457439959` dB versus the
unchanged `0.05` dB bound. It remains ignored in the regular suite under
`audit/RELEASE-DEFERRED.md`; no taps, phase, tolerances or case changed.
The original local QA run reported 25 physical callback deadline misses; the
latest run reported 30. Both reported zero negotiated-work-horizon misses. Local test-profile dynamic commit timing
also reached 14.927 ms in the original checkpoint; a follow-up release run
reached 8.351 ms for synchronous replay. Incremental calls in the tested
variable-budget release matrix reached 1.127 ms. A concurrent full-test run
observed 18.899 ms synchronous and 5.254 ms incremental calls. These runs use
different budgets and scheduling conditions; none establishes a portable
worst-case bound or complete callback deadline. The release default-host matrix measured complete 128-frame callbacks at
up to 0.423 ms with zero physical misses for the tested two-channel/two-stage
routes at 48 kHz. This is a local observation; full channel/stage/rate-load and
native direct-callback compliance remain open. Settings, commands and measurements are recorded in
`audit/artifacts/plugin-audit-20261018/linear-phase-eq-dynamic.json`.

## Testing

```bash
cargo test -p sotf-plugin-linear-phase-eq
```

## License

Part of the SOTF (Sound of the Future) project.
