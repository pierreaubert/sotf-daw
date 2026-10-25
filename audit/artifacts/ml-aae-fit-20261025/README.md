# AAE learned fitting implementation checkpoint

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The original requirement is offline parameter fitting to measured rooms while
retaining the bounded deterministic renderer. The opt-in fitter and native
verification are implemented; independent measured-room acceptance remains open.
No runtime AAE DSP source changed. No result below establishes a spatial,
complex-response, absolute-level or perceptual room match.

## Evidence map

- `protocol.json`: initial training and fitting protocol.
- `corpus/`: all three split descriptors and request/audio provenance. These are
  native synthetic responses, not measured recordings. Raw audio is not retained.
- `environment-corpus.json`: environment and corpus hashes.
- `native-source.json`: base commit, renderer/DSP source and executable hashes.
- `generate.json`, `train.json`: terminal command status; corresponding `.txt`
  files contain output. Training selected on development only and froze/hash-saved
  weights before reading its 256-case final holdout.
- `*.training.txt`, `training-source-sha256.json`: exact training preimages.
- `inverse-development/`: nine inverse cases, equal native budgets, alternating
  method order. Native dictionary uses the same corpus as the neural model.
- `inverse-final/`: independent 36-case inverse comparison, seed 20267025. Each
  layout/preset pair is represented once. Controls are not reused from training.
- `evaluation-source/`: exact inference/fitting preimages for evaluation; the
  final evaluator adjusts category enumeration to cover every pair. The final
  protocol pins its source hash. No fitting equation or weight was tuned from
  final results.
- `loader-hardening-parity.json`: subsequent malformed-model rejection changes
  leave every inference output bit-for-bit unchanged on 256 held-out inputs.
  The fitter additionally catches these validation exceptions for fallback.
- `native-tests.{json,txt}`, `native-clippy.{json,txt}`: two native renderer tests
  and strict Clippy passed. Callback allocation assertions are active during
  every native render. Partition 1/257/1024 responses hash-identical in the test.
- `python-checks.json`, `python-check-*.txt`: twelve independent descriptor,
  inference-equation, admission, budget, malformed-model and native-rejection
  tests; Ruff and formatting checks.
- `verify_cli.py`, `cli-checks.json`, `cli-*/`: actual CLI execution without a
  model, with bad hash, with correctly hashed malformed model, and valid model.
  All terminate successfully with bounded native counts, no automatic apply,
  and correct classical fallback. Each uses a synthetic development response,
  budget eight: these are control-flow smoke checks, not quality estimates.
  macOS `time -l` logs full command process memory; no realtime budget is claimed.
  The initial profiling attempt was sandbox-blocked by `sysctl kern.clockrate`;
  the preserved attempt log precedes the successful scoped run.
- `low-level-homogeneity.json`: nine selected control corners across 16/48/96 kHz,
  excitation 0.001/0.0005 and callback partitions 257/1024. Strict relative L2
  limit 1e-5 failed on four cases, worst 1.972e-5. Retained as negative evidence;
  exact global linearity is not assumed or claimed.

The model documentation explains invocation, finite-window analysis, normalized
control bounds, fixed wet-only profile, conservative Infinite tail and fallback.
Training uses no external recordings or pretrained weights. Model, dictionary
and synthetic descriptors follow the workspace GPL-3.0-or-later license.

## Open acceptance work

The local `autoeq` measured L/R WAVs have not been established as deconvolved IRs;
they were excluded. Reliable measured IR provenance/time origin, cross-rate and
noise/truncation/arrival-error shifts, spatial and listening validation remain.
The 3 dB profile threshold is a fixed engineering proxy, not calibrated model
confidence. The full 31-plugin/five-program ML audit remains active.

Final synthetic inverse comparison: all 36 layout/preset pairs passed the 3 dB
profile proxy with both methods at 64 native renders each. Neural proposals
lowered exact error in 34 cases; cases 6 and 33 retain the classical result.
Median RMSE was 0.0923 dB neural versus 0.3565 dB classical; median fit time
was 3.971 s versus 3.716 s. No speedup or measured-room acceptance is claimed.
The independent reducer checked each trace minimum, render budget, Infinite
native tail, category coverage and zero target audio-hash overlap with all
1,984 corpus records. See `inverse-final-independent-reduction.json`.
