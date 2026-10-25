# Experimental offline AAE fitting

The opt-in fitter matches finite-window, gain-normalized energy decay from a
deconvolved impulse response. A learned forward model proposes six bounded AAE
controls; the unchanged native renderer verifies every proposal. A classical
dictionary/physics fit runs alongside it, and the lower native error selects the
output. No model runs in an audio callback and no preset is applied automatically.

This is an implementation checkpoint, not a measured-room quality claim. The
original audit requirement remains open until independent measured-room and
shift validation establish useful behavior.

## Usage

Install `scripts/ml/aae-room-requirements.txt` in a Python environment. Build the
reference renderer from the workspace root:

```sh
cargo build --release -p sotf-plugin-aae --features qa --bin qa-aae-fit-render
```

Then run (substitute the executable path if using another Cargo target directory):

```sh
OPENBLAS_NUM_THREADS=1 OMP_NUM_THREADS=1 VECLIB_MAXIMUM_THREADS=1 \
python scripts/ml/fit_aae_room.py \
  --ir measured-deconvolved-ir.wav --direct-sample 0 --layout 5.1 \
  --renderer target/release/qa-aae-fit-render --temporary ./aae-fit-temporary \
  --model crates/sotf-plugins/models/aae-room-fit/model.json \
  --model-sha256 f13de094177d7d8270d34bb8e1bc4a49fcd6f43d9a04536220169721df97c2d8 \
  --dictionary crates/sotf-plugins/models/aae-room-fit/native_dictionary.npz \
  --budget 64 --report fit-report.json --preset candidate-aae.json
```

The arrival index is supplied in original input samples; no peak alignment is
guessed. Input rates are 16–192 kHz. Analysis antialiases to 16 kHz and masks
unobserved times rather than treating them as future silence. A record needs
25 dB of observed wideband decay and coverage in at least three analysis curves.
Flat or inadequate records fail explicitly. Input must already be deconvolved;
raw sweeps, songs and unidentified local WAV files are unsuitable.

The report includes both native search traces, hashes, timings and errors.
`--budget` limits exact renders **per method**; enabling ML can use twice that
many renders because the classical comparison is retained. Missing or invalid
models use the classical fit. A failed neural search also retains the classical
result. A preset is written only if the selected exact profile RMSE is at most
3 dB. This fixed engineering proxy is not perceptual confidence. Inspect the
report and audition before applying the candidate.

## Identified scope

- Nine output layouts and four early-reflection presets are explicit categories.
- Six fitted controls: room size, RT60, bass/treble decay ratios, predelay and
  normalized early/late fraction. Positive scale controls use log coordinates.
- Wet-only, unmodulated, non-adaptive profile; fixed diffusion/envelopment/height.
  Absolute wet level, dry level, spatial geometry and listener preference are
  outside this estimate. Room size can be unidentifiable when FDN delays cap.
- Seven observed energy-decay curves, 32 times from 1 ms to 6 s, clipped at
  −40 dB. Six constant-Q RBJ bands are analysis filters, not IEC-standard bands.
- Each native response observes 8 seconds with no fade or drain. Its native tail
  remains `Infinite`; RT60 and record duration do not imply finite support.

## Model and reproduction

`manifest.json` records source, license, sizes, hashes and dependency versions.
The weights and synthetic dictionary use this workspace's GPL-3.0-or-later
license. No external recordings or pretrained weights entered training.
NumPy inference has 191,872 bytes of weight arrays; full process memory includes
the interpreter, SciPy, buffers and native worker.

Corpus generation, training source preimages, split metadata and descriptors,
commands and validation evidence are under
`audit/artifacts/ml-aae-fit-20261025`. The 1,536/192/256 native train/development/
holdout requests and audio hashes are disjoint. Development MSE selects weights;
the holdout is opened after saving and hashing them. Forward holdout median
RMSE is 0.217 dB, p95 0.752 dB, worst 6.419 dB. NumPy/Torch maximum difference
is 0.0000229 dB. These are synthetic forward prediction results.

Reproduction commands:

```sh
python scripts/ml/train_aae_room.py generate --renderer target/release/qa-aae-fit-render \
  --temporary ./aae-corpus-temporary --output ./aae-corpus
# Additionally install torch==2.14.1 for training. Use a new output directory;
# training refuses to overwrite frozen weights.
python scripts/ml/train_aae_room.py train --data ./aae-corpus --output ./new-aae-model --epochs 1500
python -m unittest discover -s scripts/ml -p 'test_aae_room*.py' -v
```

The captured training preimages precede formatting, removal of redundant Python
dictionary keys (which did not survive serialization), and annotation cleanup.
The weight file has not been rewritten or retrained after holdout evaluation.
The native renderer binary used by the corpus and fitter is pinned in metadata.

Nine development inverse cases improved native profile error with ML, with both
methods qualifying all nine. ML took longer at equal render budgets; this is
not a speedup claim. A separate low-level amplitude-scaling probe missed its
strict 1e-5 relative tolerance on four of nine corners (worst 1.97e-5), so exact
linearity is not claimed. See raw evidence for the independent final inverse
evaluation and remaining validation limits.

Final synthetic inverse comparison: all 36 layout/preset pairs passed the 3 dB
profile proxy with both methods at 64 native renders each. Neural proposals
lowered exact error in 34 cases; cases 6 and 33 retain the classical result.
Median RMSE was 0.0923 dB neural versus 0.3565 dB classical; median fit time
was 3.971 s versus 3.716 s. No speedup or measured-room acceptance is claimed.
The independent reducer checked each trace minimum, render budget, Infinite
native tail, category coverage and zero target audio-hash overlap with all
1,984 corpus records. See `inverse-final-independent-reduction.json`.
