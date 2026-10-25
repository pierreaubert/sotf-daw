# Native analog baseline and calibrated paired-hardware data

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The Analog common requirement permits learned coloration only against identified
hardware and operating levels. The shared stage currently wraps six classical
models; it is not a generic iterative circuit solver. This checkpoint adds a
native raw-PCM baseline renderer and a bounded, explicitly calibrated paired-data
contract for subsequent target-specific learning. It does not implement a neural
surrogate, train on synthetic hardware as though measured, or replace coloration.

The renderer activates serde/JSON and host QA helpers only through a new `qa`
feature. Existing production processing and library APIs remain unchanged. It
sets complete static controls, prepares and resets the actual shared stage,
guards cold process/reset against allocations and frees, and renders callbacks
through the stage's own prepared-maximum chunking. Model identity, nominal-18dBFS
reference label, geometry, actual controls, native zero latency and sample peak
are checked by the Python wrapper. That label is not hardware calibration.
The renderer preserves raw floating PCM, adds no normalization or limiting and
never overwrites output files. Explicit zeros observe recursive state without a
finite-tail claim. QA-loop timing includes guards/output copies and is not an
inference or production cost guarantee.

The capture manifest requires hardware identity and actual settings, measured
full-scale-sine I/O levels in dBu, impedances, shared clock, residual alignment
declarations, recording rights, whole-source grouping and explicit splits. Exact
raw input/output lengths and hashes are verified; capture ADC/DAC sample headroom
and excitation are checked. There is no hidden alignment, gain fitting,
normalization or resampling. The RMS conversion explicitly accounts for the
1/sqrt(2) digital RMS of a peak1 sine. Metadata declarations still require actual
capture evidence; the CLI always reports `hardware_model_admitted: false`.

All nine Python tests pass. They cover analytic dBu/sine/silence conversion;
exact paired bytes/levels; required calibration/clock/alignment/rights/origin;
cross-split groups; duplicate captures; hashes; nonfinite/low-energy/clipped or
short PCM; CLI no-overwrite/no-admission behavior; all-model/rate zero-color
identity; JSON reopen across callback/preparation partitions; explicit zero
extension; independent wet-trim/color-blend laws; and invalid controls/schema.
All21 existing native tests pass (8 unit,2 spectral and11 conformance), zero
ignored. Strict all-target QA Clippy and default-feature library compilation pass.

The retained96-case evaluation reuses four generated stereo source arrays across
six models and four control conditions at44.1/48/96/192kHz. These are conditional
functional probes, not96 independent recordings or hardware holdouts. Every
17-frame/prepared1 reopen matches the original193-frame/prepared127 output bit
for bit. Dry cases preserve source samples; trimmed and blended cases satisfy
independent algebraic control laws within a maximum1.4901161194e-8, below the fixed
2e-7 gate. The fully wet response is a native baseline, not an independent
physical-model oracle. Maximum sample peak is0.2364711165 for these low-level
probes. Observed QA-loop time over the96 original renders totals0.124854539s,
including guards/output copies; no portable performance threshold is asserted.

Three generated paired captures exercise the reader at train/validation/test
labels. They use a fictional tanh transfer, hardware identity and calibration,
and remain explicitly synthetic. They are not model-training or hardware-fidelity
evidence. The capture CLI output records this distinction.

Primary-source discovery found [identified UA610B preamp captures and settings](https://zenodo.org/records/10798111)
and their [original paired source](https://zenodo.org/records/3562442). The original
dry recordings point to the [IDMT Audio Effects dataset](https://www.idmt.fraunhofer.de/en/publications/datasets/audio_effects.html),
whose page declares evaluation licensing and primarily plugin-based effect
processing. Absolute I/O full-scale calibration, impedance and residual alignment
evidence were not established for the linked preamp archive. No external audio
was downloaded, redistributed or used for training; no rights were inferred from
a repository's source-code license. Discovery decisions are in
`data-candidates.json`.

Commands, exit codes, timings, versions and source/binary hashes are adjacent.
Generated raw requests/PCM and capture manifests remain under
`/Volumes/home_tmp/tmp/ml-analog-hardware-native-20261025`. Learned model training,
identified measured-hardware admission, independent source/level/operating-point
acceptance, comparison with suitable classical physical models, optional native
inference/fallback, alias/state/cost validation and actual hosts/platforms remain
outstanding. This infrastructure alone does not complete the proposal or audit.
