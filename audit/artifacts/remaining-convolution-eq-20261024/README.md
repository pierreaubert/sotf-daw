# Convolution and Linear-phase EQ remaining-work batch

The first measured candidate combined direct overlap emission in shared NUPC,
fused UPC output/mix emission, and an exact delay-only implementation for known
ordered Linear-phase EQ bypass domains. Source snapshots preserve each change.

## First full workload

`verify-first-batch.py` checks the immutable measured snapshots and original
workload fields. Affected release validation passed 273 tests with eight retained
ignores, strict all-target QA Clippy and formatting. The new delay oracle checks
sample bits (including negative zero), zero/odd/large delays, arbitrary callbacks,
reset and agreement with the old FFT identity implementation.

Both strict full gates remained red: Linear-phase EQ had 23 failing rows and
8,214 misses across 108 rows / 32,958 callbacks; Convolution had 43 failing cases
and 41,067 misses (731 head misses) across 63 cases / 562,686 callbacks. Numerical,
finite-output, zero callback allocation/free, replay and resource checks passed.
All admission/configuration/budget/callback-count fields match their preserved
baselines. Standard diagnostics passed, including zero Convolution local misses.
Historical timing comparisons are observational and do not establish acceptance.

## Frozen paired investigation: rejected performance changes

`verify-paired.py` checks exact historical/current engine source and UPC stream
method bodies, then replays the original declared ABBA screens. The 33 cases
span 48/96/192 kHz admission mappings and controls. All 132 trials and 147,456
callback durations remain in `paired-*.stdout`; every measured output matches
exactly. These are isolated engine/stream costs, not native or full capacity
acceptance. UPC uses moderate dense 65,537-frame IRs.

NUPC changed-case aggregate median cost was 0.9643 times baseline, with every
changed case below baseline, but unchanged control ratios ranged from 0.9096 to
1.2873. UPC changed aggregate cost was 0.8562 times baseline, also with every
changed case below baseline, but controls ranged from 0.9751 to 1.1133. Both
failed the predeclared 0.90–1.10 control bounds. The NUPC 96 kHz/control12 trial
contains a 12.300 ms wall excursion amid roughly 89 us median callbacks; its
cause is unproven. UPC control32 also has a higher median, so that failure is
not explained by one extreme callback. No outliers or cases were removed.

`paired-acceptance-guard.py` independently replays those failed screens and exits
1; its preserved output is `paired-acceptance-failure.json`. Both overlap/UPC
performance hunks were restored byte-for-byte. Frozen comparators remain only
under `cfg(test)` and their manual diagnostics are ignored by default. No timing
assertion is added to CI. The exact identity-delay path is reviewed separately;
its semantic/storage benefit does not establish maximum callback capacity.

## FIR-M1 remains open

The original ignored FIR-M1 test is unchanged and was explicitly replayed:
96 kHz / 1024 taps / linear phase / 500 Hz remains approximately -0.246297 dB
against the 0.05 dB bound. An independent scalar recheck of the prior finite-grid
witness retains a conservative 0.211205 dB full-spectrum lower bound for that
centered support. It is a floating-point witness, not interval certification or
proof about only four isolated probes. Neither the designer, support, latency,
assertions nor threshold changed in this batch.

Further work is a separate true span/block processing implementation and caller
integration, while preserving exact sample chronology and replay budgets. All
full capacity, FIR-M1, aggregate/native and platform requirements remain open.
