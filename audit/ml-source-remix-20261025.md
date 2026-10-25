# Downmix source-remix ML checkpoint

Issue #10, plugin 21: separate optional source-aware remixing. Implementation is
in progress; production admission remains false. Ordinary Downmix's DSP body,
parameters and routing are preserved. The separate `SourceRemix` Plugin and its
renderer are gated by `qa`; no production factory selector exists yet.

The synthetic 65-coefficient model estimates centered mid-energy rather than
semantic dialogue identity. Training, validation and final scene seed sets are
disjoint (64/16/32 groups, 512 rows/group); final shift variants reuse final seeds.
The model was frozen before final evaluation. SHA-256:
`5fe02bae05ffe7994e755c93b89226d156803d1cd976e6406ba7315a8e6adb0c`.
L-BFGS-B stopped at its declared 300-iteration limit, without convergence.

Explicit methods are bypass, analytic M/S and learned. Foreground/bed gains are
bounded ±6 dB, with mix [0,1], 5 ms hop-rate smoothing, fixed FFT 512 and hop 256.
Uncertain or unavailable estimates use neutral remix. Equal source gains apply
the requested common gain independently of the estimator. The stage adds 512
samples to matrix Downmix; the model adds zero further lookahead. Complete-stream
energy is bounded by the requested conditional gain, but peaks are not limited.
EOF freezes controls/gains/counters and drains the finite overlap/delay state.

`training-report.json` contains mask errors, confidence coverage and shift
failures. The dual-mono bed probe exposes spatial ambiguity. The independently
generated native waveform set contains 10 six-channel mixtures and 30 outputs
at 44.1/48/96 kHz, including centered bed, panning, phase and level shifts.
`waveform-evaluation.json` records actual ITU Downmix output, native gain traces,
and independent float64 conditional WOLA/stem-sum comparisons. Stem components
use the mixture's frozen gain trace; they are not separately classified stems.
The largest conditional WOLA sample error is below 2e-7.

Learned minus analytic steady target-remix SNR ranges −4.55 to +1.90 dB, with
3 improvements and 7 regressions. Matched-rate probes regress at all three
clocks. The synthetic candidate remains unadmitted; lower matched spectral mask
MSE does not establish waveform quality or a reason to prefer the learned mode.

Validation command records and complete outputs are under
`audit/artifacts/ml-source-remix-20261025`. Initial compile and Python failures
are retained beside retries. Native tests cover frozen feature/network parity,
callback/drain boundaries, reset, symmetry, gain energy bounds, transactional
validation, neutral fallback and realtime allocation guards. Python tests cover
derivatives, feature invariance, model rejection and actual native trace replay.
The ignored callback-budget test measures ordinary routing and each separate
method against a fixed local 80% callback budget; it is not portable host admission.

Final focused results: 82 native tests passed, five Python tests passed, all 12
local callback-budget probes passed, and strict all-target QA Clippy, default
feature check, Python Ruff/format and the edited Rust test's format check passed.
The worst measured callback used 1.24% of its deadline on this machine.

Remaining: a demonstrated source-quality benefit, recorded multilingual and
music/noise validation with defensible source labels, confidence calibration,
additional layouts and measured decoding compatibility, product controls and
click-free activation semantics, complete heap accounting, and Linux/Windows
host deadline/DAW evidence. The user will supply held-out recordings/hosts later.
