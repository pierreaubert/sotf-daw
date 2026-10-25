# Offline target-specific neural hardware candidate

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint implements an offline causal finite-memory neural candidate,
explicit model validation/loading, a trained classical Hammerstein comparison,
and frozen source-split evaluation. It does not admit a measured hardware model,
add native inference or change the existing six coloration models. Synthetic
fixtures remain labelled synthetic and every candidate has
`deployment_admitted: false`. No runtime preset or automatic apply is emitted.

Architecture is a linear input-history path plus one tanh hidden layer and an
output bias. It supports1..64 taps and2..32 units; the retained fixture uses3 taps,
6 units and34 parameters. Inputs are current-then-past, with no future samples
or lookahead. Recorded output units, DC and startup are retained. Internal input
scaling is measured only from training data and serialized. Tanh boundedness
plus a linear L1 bound provides a finite magnitude bound over the trained input
peak domain. This is not a full-scale ceiling: the positive-control global bound
is1.1332018713 despite its observed held peak<0.776. Bias can give nonzero
quiescent output; finite history is not a claim of a zero-output finite tail.

Eight training source groups have equal total weight, including repeated capture
records. Sampling is bounded to8192 rows/record and65536 total, further limited
by architecture/iteration budget. Causal history is formed before subsampling.
The first taps-1 rows are omitted only during training; full held PCM is scored.
The fifth-degree parallel Hammerstein uses the same sampled rows/history and
weights, with a regularized SVD solve rather than unstable polynomial normal
equations. This is one fixed classical comparison, not an exhaustive physical
model/tuned-baseline study.

The complete candidate is serialized before validation/test target PCM is read.
Each held split requires two source groups. Per-capture numerical gates are
NMSE<=-25dB, neural advantage>=1dB over this baseline, peak<=0.999 and supported
trained peak domain. No alignment, gain fit, centering or discarded startup is
used to improve scores. Hardware/calibration/rate context must match exactly.
JSON loading is bounded to1MiB and explicit SHA256; parameter dimensions,
finiteness, software output bound and frozen policy are validated. Held target
NaNs/geometry/headroom/excitation violations raise rather than pass comparisons.
Declared identical dry hashes cannot cross source groups before training begins.

All17 Python tests pass with the native renderer configured, zero skipped in
the final strict run:9 capture/native regressions and8 model tests. Finite
differences verify the analytic neural gradient within1e-9. Tests prove causal
history and finite excitation memory, exact JSON reopen, context/domain/bounds
refusal, total source-equal weighting, and actual candidate-file freezing before
either held PCM read. Replacing all held training-call inputs/targets with
unreadable sentinels produces the identical trained model. Ruff checks pass.
Earlier combined tests skipped the four renderer-dependent tests when its
environment variable was absent; the final full run includes all of them.

The retained evaluation uses12 generated Gaussian source groups reused over two
known target equations, with8/2/2 train/validation/test groups. The positive
control combines tanh of cross-lag input mixtures; it deliberately tests capacity
that additive per-lag Hammerstein terms lack. Its held neural NMSE ranges
-55.23..-54.99dB versus-12.08..-11.87dB for the fixed classical fit. The negative
control is a Hammerstein equation: the classical baseline achieves approximately
-93dB versus-49..-48dB neural error. It rejects the neural candidate because the
classical result is about44dB better. These are conditional generated controls,
not two independent real-device datasets or evidence of a general ML benefit.

Both L-BFGS fits exhaust the predeclared300 iterations and report
`optimizer_success: false`. Gradient maxima are approximately5.08e-4 and8.07e-4;
convergence is not claimed. Offline numerical-gate success is distinct from
optimizer convergence and hardware admission. Neither target admits a hardware
model. Direct CLI runs retain exit0 for positive numerical gates and exit2 for
the classical win; hardware admission remains false for both.

Independent scalar `math.tanh`/`fsum` prediction reconstructs all eight held
record metrics within8.5265128291e-13. Independent regularized normal equations
reconstruct the two training SVD Hammerstein fits within1.3877787808e-14.
Raw candidate/file/source hashes are checked. The positive fixture is5496 bytes,
SHA256 `f4eea207a98774d8f9027cac2d96bb031b736a7dd8358cabf5419f5f551bdf44`,
and matches the direct CLI candidate byte for byte. It is not a pretrained or
default hardware model. Generated raw captures and both model/report outputs
remain under `/Volumes/home_tmp/tmp/ml-analog-hardware-model-20261025`.

Commands, expected exit codes, timings, model/source hashes and independent
analysis are adjacent. No external data or weights were downloaded. Identified
measured-hardware calibration/provenance, training convergence protocol,
independent operating-level/source/rate assessment, native inference parity and
opt-in/fallback, alias/state/cost and actual host/platform acceptance remain
outstanding. This checkpoint does not complete the Analog common proposal or
the full ML audit.
