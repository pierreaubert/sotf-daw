# AUD-132 proposal: align above-512 HR output to the main source clock

## Status and scope

Astra-medium approved the bounded source-tag design below and accepted the
implementation after reviewing the N=8192 capped full-vector evidence and
coordinated broad workspace gate. This change is limited to main FFT sizes greater than 512.
It retains the existing HR transform and input delay, reported latency, and
main analysis schedule. N=512 stays on its previous path; accepted N<512
source-tag behavior from AUD-130 and minimum geometry from AUD-129 are retained.

## Source clock and bounded correction

The HR FFT is fixed at 512 frames with a 256-frame hop. For main FFT size N>512,
the existing input delay is D=N/2-256, so the HR analysis at processing
coordinate t receives x[t-D]. The main route emits source frame t after the
reported N-frame startup latency. The previous above-512 route tagged the HR
accumulator from -256 and drained whatever HR frames were ready, pairing those
frames with gain from the main output cursor. Controlled impulses showed that
this placed HR peaks D frames after main: N=1024 gave 1280 instead of 1024;
N=2048 gave 2816 instead of 2048.

Keep the input delay and transform. Start the HR source-tag clock at
-(256+D), and discard 256+D negative-source frames. A tagged HR result then
names the original source frame whose delayed input produced it. Prepare gain
on that same source clock and consume the HR sample only when its tag matches
the main source frame. Never pair a newly available HR head with an older main
frame. If a nonzero-gain main frame lacks its matching HR tag, keep the
existing explicit alignment error; do not skip the contribution silently.

The fixed HR window/hop schedule makes the HR result for source t ready after
at most t+D+512 accepted input frames. The main result is emitted after t+N.
For N>512, D+512=N/2+256<N, so matching HR data is available before main
drain, independent of callback partition. On HR re-enable, clear the delay
line, partial HR input, queued samples/tags, and pending gains, then start a
fresh source timeline at source_frame-(256+D). This prevents stale delayed
input from being replayed.

## Required evidence

The implementation review must verify the source-tag clock is derived from
accepted source frames after startup discard, that startup/EOS credits emit
the correct finite frame count, and that a nonzero-gain missing tag remains an
error. The tests must include:

- N=1024, 2048, 4096, and 8192 impulse arrival at the reported N latency,
  callback partitions [1] where practical, irregular [17,137,256], [512],
  and whole-stream, exact frame counts and EOS.
- Captured pre-edit N=2, 256, 512 controls; full saved pre-edit output
  comparisons at N=1024 and 2048; and N=8192 full-vector retiming comparisons
  with explicit cap-inactive assertions. Retain the raw HR-stream oracle as
  supplementary transform/tag evidence.
- Exact-bin and noninteger-bin phase projections over identical absolute
  emitted-frame indices, nonzero projected magnitudes, and the fixed 1%
  residual ceiling.
- Nonstationary source-time gain and active AutoGain on/off causal references,
  plus HR disable/re-enable mid-hop, EOS, reset, and reconfigure across
  512↔1024.
- Package tests, strict Clippy, formatting, and zero callback-allocation
  coverage against a stable source snapshot.

The first alternative, removing the HR input delay, is rejected: its N=2048
tone residual was 1.4197%, exceeding the predeclared 1% ceiling. Preserve this
result as an ignored/manual rejection test. Do not weaken the threshold or
report the rejected experiment as a passing gate.

The accepted implementation preserves the D-frame delay and uses matching
source tags. The N=8192 same-run reconstructed full-vector oracle passes below
the cap; it is not an archived historical whole-plugin output. The exact
results, source manifest, saved-render comparisons, phase measurements, broad
workspace gate, and remaining scope limits are recorded in
`audit/upmixer-above512-hr-timing.md`.

MIDI, IAMF, host metering, and shared ledger files are outside this proposal.
The shared AUDIT.md ledger is maintained by the audit owner.
