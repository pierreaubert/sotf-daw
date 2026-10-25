# Analog Limiter neural color qualification — 2026-10-25

The `qa` feature now exposes a mono `LimiterQualification` stream. It runs the
actual native limiter core, the existing `HardwareQualification` neural/classical
selector, and the production Analog Limiter final sample clamp. This is a native
qualification path for an unadmitted model, with no production factory selector,
new production parameter, automatic preset activation or quality admission.

The final guard remains independent of both the network and the core detector's
smoothed controls. At fully wet mix it clamps emitted samples to the current
threshold target. Explicit core automation uses the production validator;
candidate output cannot update controls. Dry blend and output inter-sample peak
retain their existing limited contracts. Neural color adds no lookahead; the
actual core lookahead remains reported.

The trained fixture is the existing synthetic three-tap/six-hidden-unit network
from `models/analog-hardware`. A separately labeled **untrained** stress fixture
sets network output to +2, with a validated software bound of 2. It tests the
ceiling independently of whether training or waveform similarity happens to
produce small values. Threshold steps to −20 dB, explicit release values from
10–1000 ms and wet/dry control changes probe the actual guard. The network
remains unable to replace the bound or to select release values.

Missing/disabled classical routes are compared with production across all six
color models and 44.1, 48, 96 and 192 kHz. The trained candidate is rate-bound to
48 kHz; other candidate rates are rejected. Additional checks cover exact
callback equivalence, populated reset, continuously advanced fallback after
neural output, transactional rate/geometry/control rejection, nonfinite input
recovery and an independent dense-network calculation after the actual native
limiter core. Valid process, reset and explicit automation paths are guarded
against allocations and deallocations. Preparation performs model loading and
context/hash checks off the processing path.

Verification logs are retained under
`audit/artifacts/ml-analog-limiter-candidate-20261025`, including initial test
compilation failures and their corrections. The ignored standalone timing test
reports medians for production, missing, disabled and enabled routes on the same
generated input, with resets/copies/model loading outside the measured interval.
Inline object size explicitly excludes heap-owned storage. This is local native
cost evidence, not portable deadline or complete memory validation.

The final run passed 48 native tests, including six new candidate tests, with
one timing probe intentionally ignored in the ordinary suite and then run alone.
Independent dense-network output matched the learned native chain exactly.
Strict Clippy with QA and the default-feature library check passed. Initial
compilation failures were test-only borrow/default-construction mistakes;
their corrections did not change production DSP.

For 65,537 mono frames at 48 kHz with 193-frame callbacks, 21 measured trials
after two warmups gave median seconds of 0.009118042 production, 0.00686 missing,
0.006989167 disabled and 0.010190125 enabled. Enabled cost is about 1.12 times
production and 1.46 times disabled qualification in this local run. The qualifier
prepares its classical fallback differently from the production block path, so
these numbers are measurements of the tested routes, not isolated neural kernel
cost. Core latency was 144 samples (3 ms), with zero added neural lookahead.
The inline qualifier object occupied 19,728 bytes, excluding heap-owned storage.

Measured hardware quality, alias/level/configuration shifts, perceptual artifacts,
multichannel model domains, activation transitions and platform host/deadline
validation remain. The original synthetic color model does not establish
Analog Limiter hardware fidelity. The implementation ledger stays in progress.
