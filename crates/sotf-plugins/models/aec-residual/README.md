# Experimental AEC residual-mask prototype

The pinned model now has an explicit opt-in native hybrid path. The existing
adaptive canceller and classical post-filter remain active; a bounded worker
proposes additional spectral attenuation. Default presets remain classical.
Build the facade with `neural-aec` or the AEC crate with `neural-residual`.

This is still experimental. The fresh 24-scene synthetic offline comparison
preserves the classical adaptive analysis hashes exactly, improves three steady
drift cases by more than 1 dB (up to 4.88 dB), and leaves reference-silent near-only
output bit-identical. Worst near-target SNR change is -0.123 dB. Natural speech,
physical devices and realtime quality acceptance remain open; no automatic
adoption is recommended.

The earlier neural-only prototype won only 2/12 steady echo comparisons and had
poor 48 kHz double-talk onset residual echo. Those negative results remain under
`audit/artifacts/ml-aec-residual-20261025`. The hybrid consequently keeps classical
cuts and near-end protection rather than substituting the neural-only mask.
Its fresh final data (seed 20274025, 16/44.1/48 kHz) are disjoint from all initial
64 scene inputs. No weights or policy were tuned from the new final results.

## Model and contract

Weights SHA256:
`2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752`.

Sixteen disjoint bands cover all 257 unique bins of the existing 512-point AEC
transform. A shared 12→24 tanh→1 sigmoid network proposes one real gain per band,
bounded to 0.25–1.0 (at most about 12 dB attenuation; no gain boost). It has 337
parameters, 1,348 bytes of f32 weight storage. The JSON file is 7,007 bytes.
Parsing/hash verification occurs outside processing. Native prediction uses
fixed storage and returns no proposal for invalid features.

Feature version 1 uses physical 50 ms exponential statistics updated at native
256-sample endpoints. Device rates are 16–48 kHz. Feature packets are 768 bytes.
Frequency and rate are normalized to 24/48 kHz, powers to −120..0 dB, and ratios
to −40..40 dB, all mapped to [0,1]. Power sums include mirrored-bin weights and
use f64 arithmetic before conversion. The exact source contract is
`sotf-plugin-aec/src/residual_features.rs`.

| Index | Feature |
| ---: | --- |
| 0 | Band center frequency / 24 kHz |
| 1, 2 | Current error and estimated-echo powers, normalized by 512² |
| 3, 4 | Current and smoothed error/echo power ratios |
| 5, 6 | Current and smoothed normalized complex coherence |
| 7, 8 | Whole-spectrum error/echo ratio and coherence |
| 9 | Current reference mean-square power |
| 10 | Current microphone/reference power ratio |
| 11 | Device sample rate / 48 kHz |

Reference replay smooths mask attack over 10 ms with immediate release, and
retains raw adaptive output when reference mean-square power is at most −100 dB.
It uses the current completed block, with no additional lookahead. The native
AEC's existing 256-sample delay is compensated exactly before scoring. This
reference convention is not proof of a live worker's timing or audio behavior.

## Training and evidence

Only project-generated voiced/noise envelopes entered training: independent
sparse time-domain echo paths, nonlinear playback before the room path, delay
jumps, continuous ±50–150 ppm drift, near-only, near-after-echo and two levels of
double-talk. There are 32 train, 16 development and 16 final scenes, with disjoint
input hashes and fresh native two-path filter states. Both 16 and 48 kHz occur in
each split. These are generated signals, not recorded speech or device captures.
The model and synthetic data follow the workspace GPL-3.0-or-later license.

Native targets minimize complex squared error to the known clean near-end FFT
under a real per-band mask. Near-end preservation gets extra bounded training
weight. Mixture or near-end energy is never credited as echo removal. Development
weighted MSE selects weights; they are saved/hashed before final arrays are opened.
Final weighted mask MSE is 0.01710; this is not an audio-quality acceptance metric.

`audit/artifacts/ml-aec-residual-20261025` records the corpus manifest, original
training sources, commands, weights, native tests, feature/label/inference parity
and independent quality analysis. Torch/NumPy maximum gain difference is
7.45e-7. Native/NumPy maximum gain difference is 4.77e-7 over 12,320 band
predictions, with native allocation assertions active for every frame. An
independent NumPy FFT recomputes all 64 scenes' near-end oracle targets.

Conditional near-end decomposition freezes mixture-derived neural masks and
applies them separately to the known near-end signal. Worst projection loss in
these synthetic cases is about 0.103 dB. That separates mask attenuation from
uncancelled echo in near-target SNR. Classical near-end attenuation is not
separately decomposed, and numerical passthrough noise floors are not model wins.

## Reproduction

Build the QA executables from the workspace root:

```sh
cargo build --release -p sotf-plugin-aec --features qa,neural-residual \
  --bin qa-aec-residual-corpus --bin qa-aec-residual-infer
```

Install `scripts/ml/aec-residual-requirements.txt`. Additionally install
`torch==2.14.1` only for training. Substitute the executable path when using a
different Cargo target directory. Use new output paths to preserve frozen data.

```sh
python scripts/ml/train_aec_residual.py generate \
  --renderer target/release/qa-aec-residual-corpus --output ./aec-corpus
python scripts/ml/train_aec_residual.py train --data ./aec-corpus \
  --output ./new-aec-model --epochs 200
python scripts/ml/train_aec_residual.py evaluate --data ./aec-corpus \
  --model crates/sotf-plugins/models/aec-residual/model.json \
  --model-sha256 2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752 \
  --output ./aec-reference-evaluation
python scripts/ml/verify_aec_residual.py --data ./aec-corpus \
  --model crates/sotf-plugins/models/aec-residual/model.json \
  --model-sha256 2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752 \
  --native target/release/qa-aec-residual-infer --output ./aec-native-parity
python -m unittest discover -s scripts/ml -p 'test_aec_residual.py' -v
```

Limit CPU math-library threads to one for comparable replay timings. The exact
commands and environment are captured in the audit artifacts.

## Native hybrid configuration

```json
{
  "echo_tail_ms": 50.0,
  "step_size": 0.5,
  "post_filter_enabled": true,
  "residual_suppression": {
    "model_path": "/absolute/path/to/aec-residual/model.json",
    "sha256": "2c39c23a5015cecbde5ea950381f80c18907558e897ce2a93ad4df8f72b42752",
    "max_age_ms": 40.0,
    "max_extra_attenuation_db": 6.0
  }
}
```

Omit `residual_suppression` for the original classical path. The structural field
roundtrips through `AecPluginParams` and the existing facade factory; the three
scalar parameter IDs and old preset defaults are unchanged. Unsupported builds,
missing/corrupt files, unsupported device rates and preparation failures retain
classical audio. Invalid structural policy/identity is rejected explicitly.
Native callers can inspect `residual_suppression_data()` and
`residual_suppression_error()` without executing inference.

Realtime preparation creates the shared `plugins-inference` bridge with two
queued fixed-size packets. Processing never waits, loads a file, creates a worker
or runs the model on that thread. The current classical near-end protection
and reference-silent guard veto learned cuts. A missing, contended, failed,
future, stale or rejected-queue observation restores exact classical spectral
output immediately. The learned absolute gain is compared with the current
classical gain; it may add a cut, never relax a classical cut. Extra cut is capped
at the configured 0..12 dB (default 6 dB), with a 10 ms attack and immediate
release. This is a spectral bound, not a per-sample amplitude guarantee.

Initialization via `Plugin::initialize_with_mode(..., ProcessingMode::Offline)`
selects synchronous fixed-size inference and deterministic callback partitioning.
Calling `initialize` directly selects realtime execution. Reset preserves the
prepared mode. All loading, replacing and dropping workers must occur on a
control thread. Both modes retain the native 256-sample I/O latency.

Source-endpoint age is bounded separately from the 256-sample observation
interval and the causal 50 ms EMA time constant. Warmup requires 50 ms of valid
observations. Reset invalidates queued and in-flight work. Explicit host-position
discontinuities also invalidate learned work; a mixed partial native block is
skipped before new feature history accumulates. Callers that supply zero on every
callback use a local monotonic clock and must call `reset` when their source
changes. Nonadvancing nonzero positions invalidate learned history and retain
classical audio; continuity hints must describe the input stream. Automatic learned invalidation does not reset the adaptive canceller;
full source replacement still requires the ordinary plugin reset contract.

The first valid EOF drain invalidates late worker publications and freezes the
last extra gains. Synthetic EOF input does not update features, queue inference
or change decisions. The finite native tail and drain bounds remain unchanged.
Post-filter toggles invalidate learned work; the classical state continues.

`audit/artifacts/ml-aec-worker-20261025` records worker/fallback/allocation tests,
source-clock and in-flight reset regressions, enabled/disabled builds and the
fresh independently reduced native comparison. Broader natural/device-domain,
listening, cross-platform host and realtime timing/memory budgets remain open.
The AEC proposal and full ML audit remain in progress.
