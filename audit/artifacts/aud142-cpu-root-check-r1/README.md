# AUD142 timing arithmetic cross-check

Root recalculated the persisted Criterion results on 2026-09-30 after Luna
reported all eight timing commands terminal with exit code 0.

`receipt.json` records the SHA-256 of every inspected sample/estimate file,
the median nanoseconds per iteration, and the sample coefficient of variation:
`100 * sample_stdev(times / iters) / mean(times / iters)`. Each file contains
30 positive finite observations. Every recalculated median agrees with the
corresponding Criterion point estimate within relative tolerance `1e-10`.

The archived/current sets contain the same 31 legacy IDs. The current set adds
16 setup and 48 processing cases. Across legacy cases, current/archived median
ratios range from 0.931940 to 1.032559. The largest ratio is stereo two-band LR24
Both at 32 frames (478.352 ns archived, 493.927 ns current). The maximum raw CV
among new cases is 8.507324%; case-level variation is retained in the receipt.

This independently checks the arithmetic and case coverage of the same local
timing run. It is not another timing experiment, a hardware deadline guarantee,
or Astra acceptance. The owner's report records binary identity, benchmark
definitions, shared-machine load, affinity and setup/processing interpretation.
