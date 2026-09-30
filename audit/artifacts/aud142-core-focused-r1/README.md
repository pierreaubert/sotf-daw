# AUD142 focused core additions

Final-source r4 passes ten tests; strict integration-target Clippy r3 passes.
The source adds independent upper-cutoff response/convergence cases, exact and
above-rate-boundary populated refusals, BW42 bidirectional endpoint automation,
compensated four-way LR48 bidirectional scalar changes, and public per-channel
partition/refusal/reinitialize/reset cases. Accuracy, peak and partition limits
are unchanged.

Earlier compile/style failures and intermediate green logs are preserved.
Earlier r3 omitted two comparisons subsequently added in r4. Seven selected
source files are a post-gate snapshot; the final test source matches the owner's
reported tested SHA. This is not a complete compile closure or executable binding.
The index excludes itself. Independent Astra medium re-review is pending.
