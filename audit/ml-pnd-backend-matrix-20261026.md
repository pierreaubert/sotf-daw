# Paced native PND backend comparison

Issue 10 remains in progress. The comparison harness now supports the existing
Tract backend and the QA-only native ORT CPU/CoreML CPU-only alternatives in
the actual PND worker. Model bytes, source preprocessing, decoder, confidence,
source-age and reference gates, correction settings and declared 2047-frame
latency stay fixed. Ordinary presets and the default backend are unchanged.

Read-only QA snapshots report the prepared worker backend even when a control
decision uses classical fallback. The backend name is cached before spawning
the worker; reading it does not call a foreign runtime. The harness checks that
the raw estimator and prepared worker agree. A null classical backend and the
requested neural backend must persist in every callback snapshot.

The protocol is one predeclared randomized round of 96 fresh processes:
three backends × zero/two CPU-load threads × the original 16 source cases.
Sources are the existing two seeds/base frequencies, eight generated trajectories
and 3 s + 17-frame, 48 kHz geometry. They are correlated generated probes, not
independent recorded music. Within each source release, 128-frame classical and
assisted callbacks run sequentially in the existing seed-dependent order.
Order remains confounded with seed/frequency. There is no affinity, thermal,
device-clock or hardware-DAW control.

Raw synchronous estimates and plugin/model preparation finish before CPU load
starts. The load runs until both finite drains finish, then joins off the audio
thread. Failed preparation/thread creation/process/drain cannot yield a score.
The runner checks build/test executable names before and after every case and
rejects observed interference; this is boundary sampling, not continuous system
activity surveillance. Smoke output is explicitly refused by the final analyzer.

The analyzer reuses the original calibrated phase probe and all original 16-case
source, policy, timing, latency-alignment and finite-output checks separately
for each backend/load group. It also checks the frozen executable/model/library
and script hashes, execution-plan coverage/order, actual backend identity,
exact classical audio/raw estimates across all six variants, and raw backend
confidence/pitch parity. Predeclared parity bounds remain 2e−5 confidence and
0.1 cent on jointly accepted windows; confidence-threshold disagreement fails.
Rejected windows have no invented zero pitch error.

Dense service probes are the two offset sources in each group. They require
at least 90% confidence-accepted raw windows and 80% neural-selected settled
control decisions. Counts/fallbacks for all other sources remain visible.
A repeated selected control decision is not evidence of a distinct inference
job. This does not measure observation/completion age or queue occupancy.

Whole-process high-water RSS includes startup, raw estimation, runtime/model
preparation, source/output buffers, worker and load threads. It is not isolated
worker/model memory. Pair-response misses include wakeup lateness and sequential
processing; they are not hardware audio underruns. Callback heap guards cover
the Rust audio-side allocator, not foreign runtime allocation.

The release harness build, strict combined-feature Clippy, heap-free read-only
snapshot regression, QA-only library check without neural inference, eight
Python tests and Ruff pass. The updated shared analyzer reverified the previous
16-case dataset and all80 raw hashes, with zero unqualified audio probes.
All six0.25s functional smoke processes pass and report the correct prepared/raw
backend identities. Smoke timing is unqualified because unrelated compilation
was active. The full96-case qualification is pending a compiler/test-free window;
no deadline, memory or service comparison is scored at this checkpoint.
The bounded1800s quiet-window wait exited with external compilation still active.
No case started and no full-matrix output directory was created. Task validation
also ran during part of the wait; it did not replace the queued executable.
`timing-deferred.json` records the terminal outcome. Rerun the unchanged protocol
when a quiet window is available; do not interpret this deferral as a failed
neural service or audio-quality gate.

Evidence: `audit/artifacts/ml-pnd-backend-matrix-20261026`. Raw smoke files remain
in `/Volumes/home_tmp/tmp/ml-pnd-backend-smoke-20261026`. The pending runner targets
`/Volumes/home_tmp/tmp/ml-pnd-backend-matrix-20261026`; it does not create that
directory until the quiet-window check passes.

The earlier model's corrected-audio regressions
remain unresolved; backend parity and timely service do not establish pitch
correction quality. No runtime winner or deployment admission is claimed.
