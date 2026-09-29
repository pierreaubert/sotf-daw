# AUD-059 dynamic EOS endpoint — isolated proof for review

2026-09-28. **Prototype approved and production integration completed; see section6.** Prototype sources:
`/tmp/sotf-resampler-endpoint-probe/src/lib.rs`; isolated manifest in that project.
Backend dependency: `/tmp/sotf-rubato-cutoff-bank`, with the approved exact
`input_positions_next()` snapshot API. No SOTF host dependency. Build products
are under the workspace's `target/audit-resampler-endpoint` on `/home`, after the
root filesystem filled during the first attempted run.

Result: **5 tests pass, 4,416 case executions**, no ignored cases.
Log: `/tmp/sotf-resampler-endpoint-prototype.log`.

## 1. Coordinates and endpoint

Let:

- S be the accepted real-input length, frozen at valid EOF;
- L be the prepared sinc length,64/128/256;
- b be the integer number of input frames submitted to the backend, including
  zero padding (different from S and different from output frames);
- p_j be a raw interpolation anchor from the upcoming backend block's exact
  iterator, measured relative to input origin b;
- m be the one-based cumulative output count.

At fixed ratio r the ideal anchor is

`A_m = -(L-1) + m/r`.

The old exact fixed-ratio completion count is

`F_fixed = ceil(S*r) + floor(L*r/2)`.

This trims to converted programme extent after leading-delay removal. It does
**not** promise every finite sinc ringing sample. Preserve this count whenever
the emitted trajectory remained at one fixed ratio.

For a variable trajectory choose the source-anchor boundary

`B = S - L/2 + 1`.

Emit **through the first exact anchor at or beyond B**, then finish. In a given
backend block the comparison is

`p_j >= S - b - L/2 + 1`.

This is a right-bracket endpoint on the actual interpolation clock, not a sum of
accepted input times target ratios. A ratio selected while input is pending
therefore applies consistently to the same samples that the backend processes.

If `A_(m-1) < B <= A_m`, the endpoint overshoot satisfies

`0 <= A_m-B < A_m-A_(m-1)`.

Thus the endpoint is less than one actual output interval beyond B. In the ideal
fixed-ratio limit the right-bracket count is `ceil((S+L/2)*r)`, which equals the
legacy count or exceeds it by exactly one. The dedicated fixed branch retains
legacy rounding exactly; variable streams explicitly use the conservative
right bracket. No epsilon, approximate frame tolerance, or blanket extra block
is used.

### Source coordinate is not physical delay

`make_sincs()` reverses the phase table: phase0 uses table offset factor-1.
Its phase-zero center is `L/2-1+1/factor` relative to the raw anchor. The modeled
signal center is consequently approximately

`anchor + L/2 - 1 + 1/factor`.

B maps to approximately `S+1/factor` in signal-center coordinates. This explains
its relationship to the legacy programme extent; it does not justify emitting
all later filter support. The existing `signal_delay_samples()` model and all
realtime `latency_samples()`/PDC calculations stay separate and unchanged.

## 2. Exact comparison requires integer block origins

The first prototype attempt added `b` to each f64 anchor before comparing against
B. A represented anchor just below an integer rounded onto that integer after
addition, producing a one-frame oracle disagreement. This was an oracle
coordinate-rounding error, not justification for widening a tolerance.

The corrected implementation compares anchors against the local integer
boundary `S-b-L/2+1`. The independent endpoint-count oracle instead compares
`b + floor(p_j)` against the integer `S-L/2+1`, using integer arithmetic. This is
mathematically equivalent for integer B and preserves the exact represented
anchor side without rounding global positions. Production should subtract
integer input origins before conversion to f64, with checked/wide arithmetic;
never subtract two large converted counters. The experiment uses modest stream
lengths, so all local integer boundaries are exactly representable.

## 3. Fixed-trajectory classification

The prototype records a fixed effective ratio only when output anchors actually
exist:

- A backend chunk with zero outputs advances input storage/base but establishes
  no output trajectory. A pending ramp may complete during that chunk; its
  target must not retroactively classify nonexistent output samples.
- A constant-ratio block establishes its effective ratio. A later emitted block
  at a different ratio makes the trajectory variable.
- A multi-output ramp between different ratios is variable.
- A one-output ramp has just its final step. It can remain fixed at the target
  ratio if the actual repeated arithmetic `h0+(h1-h0)` equals h1 exactly and no
  earlier emitted ratio differs. Otherwise classification is conservatively
  variable.
- Multiple ratio updates before any backend output are coalesced by the
  backend's actual current/target state. Nonramped pending-input updates before
  the first processed block preserve the equivalent fixed-ratio stream.

This is deliberately conservative for nontrivial ramps whose sub-ULP step
changes might happen to collapse to identical represented values. No claim is
made that differently requested floating-point ratios are always recognized as
identical trajectories. Ordinary fixed-rate operation, unchanged ratio writes,
and tested zero/one-output cases preserve the exact legacy count. A conservative
variable classification uses the explicitly documented bracket policy.

## 4. Independent evidence

The prototype has two separate mechanisms:

1. The candidate adapter selects EOF output using the backend's snapshot.
2. A separate repeated reciprocal-step/index recurrence computes every anchor
   without calling the snapshot implementation. It checks each represented
   anchor bit-for-bit and checks the backend's post-call cursor. Integer-origin
   endpoint counting independently determines the retained frame count.

Both obey the documented recurrence:

`h_j = h_(j-1) + (1/r_target - 1/r_start)/M`

`p_j = p_(j-1) + h_j`, for j=1..M.

The equivalent real-arithmetic expression
`p_j=p0+j*h0+(h1-h0)*j*(j+1)/(2M)` explains the trajectory, but is not used as an
exact f64 oracle near integer boundaries. Backend M/count sizing is validated
separately by AUD-058; this experiment verifies the wrapper's endpoint given
that emitted trajectory.

For waveform retention, the same deterministic resampler continues with ample
explicit zeros after the candidate endpoint. The test includes output omitted
from the final processed block, rather than accidentally beginning its reference
only at the next block. The highest final-marker sample in this longer response
must already occur in the retained stream and match exactly. This tests EOF
retention, not independent sinc-filter amplitude certification.

### Matrix

| Cases | Scope |
| ---: | --- |
| 1,440 | Fixed count:3sinc lengths ×8ratios (0.25,0.5,1,2,24,48,48/44.1,44.1/48) ×5chunks (1,17,64,256,1024) ×12input lengths through1025, including1frame |
| 6 | Reported pending900+100 case, both1→2 and2→1, all sinc lengths, with an extra overwritten pending ratio update; exact complete-waveform/count equality with the final-ratio instance |
| 1,920 | Variable steps and ramps, both directions including2→0.5/0.5→2,5nominal ratios,4chunks,4terminal lengths including1frame; exact endpoint and final-marker retention |
| 192 | Fixed8/384 and8/192 nominal ratios,3sinc lengths,4chunks,8short input lengths; exact legacy counts |
| 672 | Very-low-ratio changes, both directions/ramped and immediate,7terminal lengths including1frame;26,996zero-output chunks and42one-output ramps observed |
| 180 | Repeated updates before first output and one-output ramps, including chunks1/2/3/4/7 and clips1/2/3/17;607zero-output chunks,36one-output ramps;60actual variable cases |
| 6 | Explicit original small1.1→1 and1→1.1 failure fixtures, all3qualities |

The1,920-case variable matrix's largest measured normalized overshoot is
`0.9999999999999943` output intervals, satisfying the strict one-interval bound.
The same exact bracket assertions pass for low-ratio and early-ramp cases.

### Original reported ramp cases after correction

These counts use the corrected backend planner plus the proposed EOF policy.

| Sinc length | Ratio change | Former wrapper target | Corrected output count | Final-marker peak index |
| ---: | --- | ---: | ---: | ---: |
| 64 | 1.1→1 | 1108 | 1132 | 1129 |
| 128 | 1.1→1 | 1140 | 1164 | 1161 |
| 256 | 1.1→1 | 1204 | 1228 | 1225 |
| 64 | 1→1.1 | 1111 | 1084 | 1082 |
| 128 | 1→1.1 | 1146 | 1120 | 1117 |
| 256 | 1→1.1 | 1216 | 1190 | 1187 |

All previously discarded downward-ramp peaks are retained. Pending900+100
updates now match the constant equivalent exactly, removing the±900-frame error.

### Very-low-rate waveform limitation is explicit

24of672very-low change fixtures produce an entirely zero impulse response even
in the long continuation. Example:64taps, nominal8/384, final ratio half that
nominal, output steps96input frames, and a one-frame impulse located between
nonoverlapping64-tap windows. There is no nonzero peak for an EOF policy to
retain. These cases still pass exact count/bracket checks, and their finite
output is also exactly zero. They are reported separately rather than calling
an arbitrary all-zero maximum index a missing tail or claiming nonzero waveform
coverage. All other marker cases retain the exact long-continuation peak.

## 5. Proposed production integration after review

Keep root's newly added cutoff-selection calls and same-length prepared bank.
No backend history or PDC changes are needed for this wrapper algorithm.

- Store integer backend-submitted input count independently from accepted real
  input and emitted output count. Include the backend's padded consumption in
  the submitted count, never in S.
- Track fixed/variable trajectory from each **successful** emitted backend block.
  Compute prospective classification locally before processing, committing it
  only after a successful call.
- At valid EOF freeze S and reject further ratio/control mutation as already
  enforced by the lifecycle patch. Do not freeze the old target-ratio duration
  sum. A queued ramp or zero-output backend chunk can determine the effective
  trajectory during drain.
- Before each drain step snapshot upcoming anchors and inspect the prospective
  trajectory. For fixed operation use the exact legacy count with its effective
  ratio; otherwise select through the first local anchor crossing the endpoint.
- Preserve destination-capacity/rate transactionality, prepared storage, output
  canaries, reset semantics, and cold zero allocations/deallocations. No-output
  drain chunks may make real internal progress; they must not cause premature
  completion before later anchors exist.
- Reset all endpoint/classification/submitted-counter state. Preserve all
  physical-delay and existing fixed-rate exact-count regression oracles. Replace
  the old ramp test that mirrored `accepted*target` with independent cursor and
  final-marker assertions.

## Reproduction

`CARGO_TARGET_DIR=/home/pierre/src/all_of_sotf/sotf-daw/target/audit-resampler-endpoint cargo test --release --offline --manifest-path /tmp/sotf-resampler-endpoint-probe/Cargo.toml -- --nocapture`

Source hashes after formatting:

- prototype lib.rs: `9b8db13a618e3d938b58be16597319985e31e97fd7dd1237fff4414d4ff94f5f`
- isolated backend asynchro.rs: `5e142496a0d0ecbb165fd530473bd937f2a6642556e1c3f420ad4b204f114425`
- isolated backend asynchro_sinc.rs: `6e56a5fc3dbf736c26446a7eecc621aa4a9d6b5ae2ab7e62d9246dda0aa47ab0`

Prototype source was rustfmt-formatted after its successful run; formatting made
no semantic changes. Production source-position accounting remains unmodified
pending root review.


## 6. Approved production integration and verification

Implemented in `sotf-plugin-resampler/src/stream_endpoint.rs`, registered in
`src/lib.rs`, and integrated into `resampler_plugin.rs`. No backend, dependency,
physical signal-delay, realtime latency/PDC, or cutoff policy changes by this
integration. The root's prepared cutoff selection is retained at all setters,
backend block completions and resets.

The new state separates accepted real input, exact integer backend-submitted
input origin, actually exposed output, emitted trajectory classification, and
Active/Draining/Complete lifecycle. Block candidates are computed before backend
work and committed only after success. Drain validates clock and destination
capacity before finalization or mutation; output beyond the returned frame count
is untouched. A planned zero-output block can consume padded input and finish a
pending ramp while returning zero frames and incomplete. Empty valid drain and
idempotent completion remain explicit. Reset clears all endpoint history.

### Reproduction and green evidence

The first new public regression failed before the production change: selecting
ratio 1, accepting 900 buffered frames, overwriting the pending ratio with 1.5 then 2,
and feeding 100 more frames produced a different complete stream from choosing 2
before all 1000 frames. Log `/tmp/sotf-resampler-endpoint-red.log`. The implementation
now yields bit-exact equal full streams and counts in both directions, across all
qualities and 44.1/48/96 kHz.

Full suite log `/tmp/sotf-resampler-endpoint-full.log`: **93 tests passed, zero
ignored**, including six new public test functions. A subsequent focused
large-integer-origin regression passes (one additional unit test, 94 total tests
now present), proving an anchor immediately below an integer boundary remains
below it even with an origin beyond 2^53. Log
`/tmp/sotf-resampler-endpoint-wide-origin.log`. Focused all-target Clippy is clean:
`/tmp/sotf-resampler-endpoint-clippy.log`.

New public API regression source: `tests/dynamic_endpoint.rs`:

- 1,728 variable trajectories across all 3 qualities, unity/fractional/downsampling
 and 8/384, 8/192 nominal ratios, chunks 1/17/64/256, both small and full-range changes,
 immediate changes and ramps, terminal lengths 1/17/123.
- 180 tiny/pre-output-ramp cases, chunks 1/2/3/4/7, clips 1/2/3/17, overwritten pending
 targets. These explicitly observe both zero-output chunks and one-output ramps.
- 192 very-low fixed-ratio cases preserve the exact legacy count through reset.
- 18 buffered-target equivalences reproduce both ±900-frame failures.
- Transactional wrong-rate and capacity rejection before first drain and during
 drain, preserving the exact stream across retry and reset.
- 4 cold-thread 1/8 channel normal/extreme-ratio cases measure **zero allocations and
 zero deallocations** across process, ratio updates, complete drain and reset,
 twice per instance. This uses explicit TLS allocation/deallocation instrumentation,
 not CountingAlloc's allocation-only convenience assertion.

The public count oracle does not call backend position/sizing helpers or production
endpoint logic. It independently finds the maximum output count by bisection over
the FIR support inequality, replays scalar reciprocal-step/index additions, and
compares integer-origin floors against the source boundary. The waveform reference
is a separate plugin continued with explicit zeros, including the candidate's
omitted final-block output. Every retained sample matches exactly; every nonzero
full-response final-marker peak is retained. Fully zero extreme-decimation impulse
responses are explicitly counted as count-only evidence, not marker evidence.
All production test helpers check output canaries beyond actual returned counts.

Removed the old `ratio_ramps_use_cumulative_stream_duration_when_draining`
assertion, which merely reproduced the incorrect accepted-input times requested-
ratio sum. Its intended behavior is now covered by the independent dynamic clock
and waveform matrix, with no ignored regression or relaxed sample tolerance.

README and CHANGELOG now state the fixed count, variable right-bracket rule,
conservative trajectory classification, incomplete zero-output drain semantics,
and finite-ringing limitation. This corrects truncation/extension at dynamic EOF;
it does not claim every finite FIR sample beyond the programme endpoint or exact
fractional signal alignment.
