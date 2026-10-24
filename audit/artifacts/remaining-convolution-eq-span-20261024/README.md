# True span processing trial, 2026-10-24

This rejected candidate was evaluated separately from identity-only checkpoint
`24e72a4`. All four modified tracked source files have been restored byte for byte
to that checkpoint, and the two trial-only active test modules were removed.
`restored-source.json` records restoration; `measured-source/` and
`measured-source.json` preserve the final measured candidate and fixture.
The previously rejected overlap/UPC fusion changes remain restored. The span path
retains the original transform boundaries, FFT/OLA arithmetic, scalar methods,
full IR support, latency, and UPC body. `verify-span-preflight.py` checks these
claims against archived exact source snapshots.

The affected release suite passed 274 tests with 10 prior/manual ignores before
adding the two comparator/setup tests. Strict Clippy passed. Twelve large sparse
IR cases matched the scalar engine bit for bit; maximum independent f64 error was
1.4901161193847656e-8 under the unchanged 2e-5 limit.

The first comparator attempt stopped on a 32-channel WAV decoder rejection.
Its 40 completed trials and exact source/build evidence are preserved under
`invalid-wav32-attempt/`; they are not acceptance evidence. The fixture was
corrected to the full QA harness AIFF representation without changing channel
counts, routing, coefficients, rates, horizons, or thresholds. An explicit
setup-only pass then loaded and processed all 48 planned cases before timing.

The complete fixed ABBA trial ran 48 cases / 192 trials / 817,200 callbacks and
matched the scalar waveform exactly. **The performance screen failed.** The
changed aggregate total ratio was 0.938187873, the largest changed ratio was
2.174127128 (limit 1.05), and common control ratios ranged 0.561576651–1.360999412
(required 0.90–1.10). These data do not establish an accepted performance gain.
No timing samples were removed and no guard was relaxed. A passive TokenSave
status request during timing reported an index rebuild, but the source of timing
excursions has not been established. `paired-distribution-diagnostic.json` is
descriptive only; it does not replace the original total-time guards.

`paired-plan.json`, `paired-build.json`, `fixture-setup-validation.json`,
`paired-measurement-source.json`, `paired-commands.json`, and `paired.stdout`
preserve the fixed plan, successful build/lint/setup, source/binary provenance,
command result, and all duration samples. `verify-span-paired.py` reproduces the
failed screen without running any new timing. Full Plugin/native capacity gates
are separate and remain unresolved.

The archived verifier uses measured snapshots by default; `--current` on the
preflight verifier is reserved for checking an actively installed candidate.
Current production intentionally differs from the rejected measured candidate.

After exact restoration, the affected release suite passed **273 tests / 10
ignored**, and strict all-target Clippy passed. `restored-validation.json` and
`restoration-verification.json` preserve the final checks. Active owned source
paths are clean; only this rejected-experiment artifact directory awaits the
parent checkpoint.
