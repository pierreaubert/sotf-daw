# Selected neural pitch source evidence

This extends the QA snapshot after the comparison-harness checkpoint `ebfe20e4`.
The queued backend matrix uses the earlier compiled executable; these source
changes are a separate qualification and do not reinterpret that experiment.

The pitch assistant now returns one internally accepted observation together with
its reference ratio. The public tuple-returning API keeps its existing behavior.
PND copies QA evidence from that same nonblocking worker read at the control
decision: observed-window end, decision source clock, window length, fundamental
and confidence. Snapshot reads copy the cached evidence and never query a newer
publication. A classical decision stores no neural evidence. Finite drain retains
it; reset clears it while keeping the prepared backend identity.

Source lag is decision clock minus observed-window end. Admission age adds the
entire causal window, matching the existing policy. Neither is wall-clock
inference duration or time since publication. Counting distinct observation end
clocks within one reset epoch counts selected source windows, not every completed
job; rejected, overwritten and never-selected publications remain unobserved.
There is no queue-drop, worker-CPU or foreign-memory accounting in this change.

The new evidence is QA-only, Copy and serialized for trace consumers. Normal
presets, latency, confidence/reference/source-age admission, synthesis and model
preprocessing are unchanged. The production path still performs one latest-result
read at each classical analysis hop. No additional worker poll, allocation,
blocking synchronization or foreign-runtime call is added to snapshot reads.

The actual-library regression checks both admitted CPU providers, source-age and
confidence/reference invariants, idle publication versus frozen snapshots, finite
drain, and heap-free reset. The existing read-only snapshot regression additionally
checks absent evidence and backend identity for classical processing. All nine
selected tests pass, including both actual CPU providers. Strict library/test
Clippy, standalone-harness Clippy, QA without neural inference, and neural
inference without QA all pass. The independent verifier rechecks 100 source
windows, 32 activation frames per provider, model/library/corpus hashes and native
delegation. Maximum backend differences remain 0.000291 cent, 4.173e−7 confidence
and 7.451e−7 activation; these are parity results, not source-truth accuracy.

Evidence: `audit/artifacts/ml-pnd-selected-evidence-20261026`, including captured
commands/stdout/stderr, source snapshots, native profiles and verifier output.
The pending benchmark executable still has SHA256
`193a16bc540791ef1afa04bfdad8b0cf268651c13c769c2ffaf278d7cb363c2f`,
matching the earlier six-case smoke run; telemetry test compilation and Clippy
did not replace it.

This provides the source evidence needed for a future job-reuse/selected-age
analysis; it does not claim that the pending older-binary matrix measured those
values. Pitch-quality and real DAW/platform acceptance remain open.
