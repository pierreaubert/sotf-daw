# Source-group routing suggestion checkpoint

Implements an explicit offline per-session supervised linear/ridge learner and
native-qualified manual preset export. Runtime Matrix DSP/defaults are unchanged.
No pretrained weights or external datasets are involved. This checkpoint keeps
the Matrix ML item in progress; natural user examples and product acceptance are
not supplied by generated tests.

`protocol.json` freezes the estimator, gates and source hashes before evaluation.
The learner balances whole sources, includes OLS, uses nested source-group
validation, rejects unsupported unregularized designs and applies conditional
source-bootstrap uncertainty checks. It exports a labeled routing table and
output-major native matrix only after actual native validation.

The 15 fresh generated sessions admitted all eight expected linear cases and
rejected all seven controls: collinear, near-collinear, underexcited, single-source
rank support, contradictory targets, nonlinear clipping and coherent overload.
The admitted cases include rectangular, sparse physical maps, zero output,
identity, 16-input/8-output and 44.1/48/96/192 kHz examples. Four new 97-row
sources per admitted session use a wider input range and the known same matrix.
They are independent generated rows, not held-out real recordings.

`independent-analysis.json` uses pivoted QR instead of the learner's thin SVD.
Maximum coefficient discrepancy is 1.81e-15, interval discrepancy 1.12e-15 and
score discrepancy 1.39e-17. It verifies 2,200 bootstrap fits and 128 native
responses/538,132 output samples; maximum independent native product error is
8.04e-8. Basis and coherent probes check orientation, signed gains and headroom;
unmapped physical channels and callback parity are also checked. Actual native
held-out replays use outer-fold coefficients fitted without the excluded source.

Six Python tests cover learning, source-weight preservation, rank/independence
rejection, malformed input and excluded-source isolation. Six CLI checks cover
export, ambiguity/headroom abstention, invalid maps, native failure and unchanged
files after overwrite rejection. The Matrix crate reports 99 passing tests and
one existing ignored manual kernel crossover measurement. Strict Clippy passes.

`native-boundary.json` additionally verifies 16x16 logical routing over 128x128
physical channels at 192 kHz and 2,048 frames. Callback sizes 1 and 4,096 produce
bit-identical output, exactly matching the independent product; coherent peak is
1. Native processing and drain are guarded against allocation. Native latency
and retained tail are zero.

The single local macOS CLI profile reports 0.15 seconds and 40,157,184 bytes
maximum resident size. This includes subprocess/I/O and process accounting; it
is not a concurrent process-tree memory bound or realtime/DAW/cross-platform
performance result. Native request/response arrays remain under
`/Volumes/home_tmp/tmp/ml-matrix-routing-20261025`; generated examples and reports
are archived here with recipes, hashes and seeds.

OLS performs the specified linear task well. Ridge selection and uncertainty
checks are assistance around exact routing, without a neural superiority claim.
Source IDs are metadata; bootstrap intervals are conditional heuristics. They do
not calibrate probabilities or identify channel roles. See the model catalog
README and implementation ledger for the remaining acceptance work.
