# AUD112 — EQ causal AutoGain measurement clock

Implemented, verified, and source frozen, 2026-09-28. Source scope is the EQ crate only; no host,
shared AutoGain, engine, parameter schema, MIDI, or IAMF code was changed.

## Reproduced failure and correction

The original public fixtures failed before production changes:
`/tmp/sotf-eq-autogain-clock-red.log` contains 2/2 failures, including native
137/512 partition maximum error 0.009181127 and changed-later-suffix earlier-prefix
error 0.00014978088. The former implementation sampled meters every ten
callbacks, then applied a newly learned target to the start of that callback.

A private prepared `AutoGainClock` now ingests every native-rate original input
and uncompensated output frame, including diagnostic ingestion while disabled.
It refreshes both measurements every `max(sample_rate/10,1)` frames. Input is
refreshed before output. Compensation through the final interval frame uses the
previous target; the new target first affects the next frame. Publication occurs
after the refresh. The existing scalar gain recurrence and meter arithmetic are
unchanged.

Oversampled input measurement uses a reference-only delay equal to the prepared
oversampler's actual native-frame latency. Its scheduling queue is included
once. This does not add audio delay or compensate arbitrary IIR phase/filter
loss. Native raw processing uses 4096-frame prepared scratch spans and retains
transition objects until the original external callback ends. The oversampled
raw callback stays intact, including its original transition reconciliation.
Compiled native processing uses the same causal clock with immutable input.

All four direct constructors prepare checked `4096*channels` reference samples.
Oversampling additionally prepares `latency*channels` delay samples; both counts
are checked against multiplication and f32 address-space limits. No resizing
occurs in processing/reset/drain. A replacement reference epoch is prepared
alongside each actual resampler reconstruction, including existing same-factor
writes. `AutoGain::set_sample_rate` replaces meter histories while preserving
current/target gain. Local public meter fields clear at successful preparation;
initialization publishes after any existing post-EOS full reset so its gain
field agrees with actual state. Ordinary reset retains the existing deferred
publication behavior and clears all new clock/reference storage.

## Exact raw baseline

Before extraction, the temporary public harness captured 52 configurations:
48/192 kHz; empty, ordinary order 2/8, SVF, warped, Kautz, and mixed banks;
native DF1/TDF2; applicable 2x/4x routes. Native callbacks include 8193/4097-frame
blocks with transitions completing within a scratch span. Every output float
was saved as bits, together with declared latency and finite suffix.

`/tmp/sotf-eq-raw-baseline-compare.log` verifies **1,122,344 audio samples and
8,192 tail samples bit-for-bit equal**, with identical latency. The immutable
original artifact is `target/audit-tmp/sotf-eq-raw-before.json`. No numeric
tolerance was substituted for exact parity. The harness source is retained at `/tmp/sotf-eq-raw-baseline.rs`; the temporary
repository example was removed before the strict gate.

## Independent acceptance

Permanent `tests/autogain_clock.rs` contains ten tests:

- Both original public reds remain exact partition/prefix regressions.
- 24 configurations (44.1/48/96/192 kHz ×1/2/4 ×−9/+9 dB) compare complete
  compensated audio to a separate public AutoGain instance using an explicitly
  delayed input array and scalar per-frame gain recurrence. Two caller
  partitions each (17/4096) must equal every expected sample bit. The first
  complete interval must equal uncompensated audio exactly.
- Neutral first-sample impulses at three rates ×three factors independently
  measure the peak delay and compare it with the declared delay used for the
  reference; this checks that the scheduling queue is not counted twice.
- Actual DawHost compiled-enabled/disabled routes and DF1/TDF2 agree with direct
  processing before, during, and after a live transition. Separate host finite
  EOS tests cover native compiled zero-tail and 2x/4x fallback 1024-frame suffix.
- Warm finite empty-bank EOS crosses a derived interval boundary for three
  rates ×two factors ×two source-end phases ×four destination capacities
  (1/17/256/1024), 48 configurations. It exactly matches explicit canonical
  256-frame zero continuation, including published gain/LUFS and untouched
  sentinels. Post-EOS initialize clears gain/diagnostics and replays fresh state.
- Structural 1→2,2→4,4→1,2→2 reconstructions preserve nonunity gain, clear paired
  diagnostics, restart the measurement boundary, and reset/replay fresh state.
  Zero/invalid calls, failed rate-zero initialize and invalid factor writes do
  not change subsequent audio; disabled measurements remain live.
- Published aligned input/raw-output peak maxima match independent interval
  maxima for enabled/disabled, all factors, two partitions, first-frame and
  later markers, followed by a quiet interval. A retained earlier snapshot
  remains immutable. This is a negative control, not a peak fix: the pinned
  EbuR128 already accumulates maxima until `prev_sample_peak` is queried.

The initial warm-EOS single-frequency fixture at 0.45*Fs did not establish the
required >0.01 dB gain activity. It was replaced with 0.49*Fs plus 0.1*Fs, keeping
a passband component above the meter gate while the high component is removed.
The nontrivial-gain assertion and exact continuation oracle were retained;
production DSP and tolerances were unchanged.

Permanent `tests/autogain_realtime.rs` counts both allocations and frees:
56 fresh-thread processing fixtures cover 48/192 kHz, one/two channels,
1/2/4, enabled/disabled, DF1/TDF2 plus native SVF. Each exercises live prepared
control IDs, maximum legal oversampled/native >4096 callbacks, first statistics
publication, reset, and a second epoch. Another 24 fresh-thread fixtures first
publish during finite drain at capacities1/17/256, then reset. All measured
regions require **0 allocations and 0 frees**. Prepared ParameterId owners are
retained outside measured closures; the existing API may free a caller's last
owned ID when that value is consumed.

## Verification status

- Initial 2/2 public reds: `/tmp/sotf-eq-autogain-clock-red.log`.
- Exact original raw baseline: `/tmp/sotf-eq-raw-baseline-{capture,compare}.log`.
- Eight expanded clock tests and two realtime tests passed before the final
  stronger peak/host-EOS additions: `/tmp/sotf-eq-autogain-acceptance.log`.
- Peak negative-control run passed before any peak-production edit:
  `/tmp/sotf-eq-autogain-peaks-red.log` (historical filename; **not a failure**).
- Final full EQ suite: **147 passed, 0 failed/ignored**, including all ten final
  clock and both realtime tests; `/tmp/sotf-eq-autogain-full.log`.
- Strict all-target EQ Clippy passed: `/tmp/sotf-eq-autogain-clippy-final.log`
  (0.49 s). Its first run found only three constant-size test chunk-view style
  lints; these were replaced mechanically with array views, without changing
  assertions or production. Full test source was otherwise unchanged.
- Matched historical throughput: complete seven-trial evidence below.

Independent implementation review found no blocker:
[audit/eq-autogain-clock-independent-review.md](eq-autogain-clock-independent-review.md).
The proposal and transition-retirement proof are retained in
[audit/proposals/eq-autogain-clock.md](proposals/eq-autogain-clock.md).

## Compatibility and limits

No parameter indices/defaults, raw filter laws, resampling kernels, declared
latency, compiled eligibility, finite support/call quotas, or EOS control-freeze
policies changed. Native empty-bank support remains zero; oversampled finite
empty banks retain canonical 1024-frame zero continuation. Nonempty recursive
banks retain their existing unknown/unsupported native-tail behavior.

AutoGain sound intentionally changes from callback-dependent sampling to a
causal continuous meter timeline. Disabled diagnostic ingestion now processes
all frames, so it has a real computation cost; reported timing is local evidence,
not a cross-machine realtime guarantee. The ten-Hz update is a documented
control policy, not a claim of a proprietary or ISO loudness model match.

This work does not certify general initializer transactionality, inherited
output-copy-before-oversampling-size rejection, external-callback-dependent
coefficient retirement, or arbitrary effect phase alignment. Retaining all
telemetry buffers may delay publication while measurement/gain state continues.


## Matched historical throughput cost

Root retained the actual pre-change optimized-test EQ/host rlibs and compiled
identical typed standalone source against before/after matching dependency
pairs. Both EQ artifacts use Cargo profile `13998622292684476982`, identical
Rust flags and unchanged shared AutoGain source (SHA-256
`906929d3b61bfcaa3a3327089770b10fb60ad9ecb217d72caa53851cf6f166dd`). The before
rlib still contains `cache_update_counter` and no `AutoGainClock`. No working
source revert/rebuild was used. The probe was compiled at opt-level3 with the
same native CPU flags for both binaries. This is an optimized-test-library
comparison, not a claim of release-library absolute cost.

Each process measures two channels, two ordinary bands, 1024-frame callbacks,
100 warmup callbacks and five 300-callback timed spans per configuration. Root
ran seven alternating before/after trials. Values below are medians of those
seven per-process medians. Timings are elapsed throughput on this machine,
including all meter and filter work, not a portable bound or thread CPU counter.

| Rate | Factor | AutoGain | Before ns/frame | After ns/frame | Change | After time/audio duration |
|---|---:|---|---:|---:|---:|---:|
| 48000 | 1x | false | 34.814 | 205.280 | +489.64% | 0.985% |
| 48000 | 1x | true | 40.714 | 214.976 | +428.02% | 1.032% |
| 48000 | 2x | false | 114.888 | 286.951 | +149.77% | 1.377% |
| 48000 | 2x | true | 120.164 | 296.938 | +147.11% | 1.425% |
| 48000 | 4x | false | 183.718 | 357.511 | +94.60% | 1.716% |
| 48000 | 4x | true | 189.997 | 365.588 | +92.42% | 1.755% |
| 192000 | 1x | false | 17.308 | 55.138 | +218.57% | 1.059% |
| 192000 | 1x | true | 22.025 | 59.896 | +171.94% | 1.150% |
| 192000 | 2x | false | 97.147 | 136.390 | +40.40% | 2.619% |
| 192000 | 2x | true | 101.702 | 140.782 | +38.43% | 2.703% |
| 192000 | 4x | false | 166.022 | 205.601 | +23.84% | 3.948% |
| 192000 | 4x | true | 171.192 | 209.208 | +22.21% | 4.017% |

The cost increase is material: unlike the old ten-callback scheme, the new
measurement history includes every sample. Disabled compensation still
maintains the approved continuous diagnostic timeline. This report does not
claim a speedup or hide the former skipped-input baseline. Across these cases,
current throughput consumes about 0.985–4.017% of audio duration per instance.
Optimizing unused shared-meter work would require a separate reviewed change;
no statistical/audio inputs were dropped to reduce this cost.

Exact probe, immutable copied libraries, matching SHA-256 manifests, binaries,
and all 14 logs are retained in `target/audit-tmp/eq-autogain-aud112/`.
The typed source is also `/tmp/sotf-eq-autogain-cost-probe.rs`; its SHA-256 is
`33a56e778f735c1c7418dc350fe80e238a4bcb0e989d6177b2925ee99ebbfee5`.
The temporary Cargo examples were removed before the final strict lint gate.
