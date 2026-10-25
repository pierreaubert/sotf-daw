# Offline compressor static-curve assistance

Issue10, single-band alias and multiband compressor ML proposal. This implements
an explicit-target offline assistant: learned initializer plus bounded exact
refinement and classical multistart; actual native steady transfer qualification;
reviewable raw native presets for1–5bands; rejection without controls; no
runtime gain-computer, envelope, automation or default changes.

The model directory `crates/sotf-plugins/models/compressor-static-fit` documents
input format, use, limits, headroom and provenance. Static curves cannot identify
timing, programme dependence or hardware memory. Attack/release and crossover
layout stay explicit. Mixed-band programme/peak/phase behavior, temporal/hardware
fitting, measured data, UI/import and real-host acceptance remain unfinished.

Training uses4096 generated static parameter identities and512 disjoint
development identities; epoch100 minimizes output-response MSE. A ten-example
smoke first produced a loadable model and a qualified held-out fit. Portable
weights are SHA `30c80f93df4e25bbb80133099654329eca5ebe9779396a10fbd22f88b87fb06a`,
12676parameters. Source, normalization and learned coefficients are frozen.
No hardware, recording, user listening data or pretrained model was used.

`protocol.json` records source/model/renderer hashes and fresh final seed before
scoring. `split-verification.json` excludes smoke/full-training/dev identities.
All64 fresh model curves qualify after neural-seeded refinement; combined fits
also qualify64/64. Unrefined MSE0.0835609dB² versus identity158.1101dB². Strong
segmented seeds with6dB and16dB starting knees also qualify64/64 and use median35
residual calls versus40 neural. This supports a working initializer, not ML
superiority. Warm solver timings exclude imports/load/native measurement and
were not controlled benchmarks. Classical-only fitting remains operational.

The11 declared CLI/native probes export10 compatible cases: soft/hard knee,
near-unity, positive makeup, one/three/five bands,44.1/48/96/192kHz, noisy and
irregular grids, slow attack and classical-only operation. The incompatible
jagged curve rejects, with every parameters field cleared and no preset.
Invalid checksum, implicit timing, unknown fields and bad crossover counts fail
before output. Existing-output refusal preserves the test marker.

Native measurement resets before each constant-amplitude DC plateau, settles
32 attack constants +50ms and measures the final part. It validates full-preset
construction/finite stereo smoke and each band in isolation. This does not prove
recombined programme equivalence. All ten presets reopen at257-frame callbacks
and64 attack constants with bit-identical measured plateau levels. The high-rate
100ms-attack case reaches0.04049dB ideal/native deviation, inside the0.05dB gate.
It illustrates why native realization is checked rather than trusting ideal math.

Required successful gates:
- `native-build.json`: release QA binary build and strict Clippy.
- `native-tests.json` / `.stdout.txt`:173 native compressor tests.
- `python-tests.json`:9 tests, including independent scalar/Torch gain-law
  checks, model loading/forward parity, exact/rejected curves, the native0.1dB
  hard-knee branch,1/3/5bands at four rates, partial callbacks and settling.
- `final-source-checks.json`: focused Ruff and rustfmt checks.
- `independent-analysis.json`:64 scalar-reference curve checks, every accepted
  candidate,10 native preset reopens, exact explicit timing/layout and rejection
  checks. Maximum source/fit error disagreement1.43e-14dB; native reopen discrepancy0.
- `invalid-cli-validation.json`: malformed/unsafe contract and overwrite cases.

`python-tests-r1.json` records the earlier successful8-test set.
`python-tests-r2.json` preserves a failed additional test whose knee-jump bound
assumed ideal gain math. `knee-boundary-analysis.json` explains and measures the
existing fast-log/exp approximation contribution: actual jump0.015566dB versus
ideal0.009375dB. The corrected test budgets the documented approximation error,
positive Taylor truncation direction and f32 rounding. Fit/export gates and model
were unchanged; no native DSP was changed to satisfy the test. The final test
proves an observable hard/soft branch and local soft-knee continuity.

Reproduce from repository root with the ML Python environment:

```sh
python -m unittest discover -s scripts/ml -p test_compressor_curve.py
python scripts/ml/evaluate_compressor_curve.py --model crates/sotf-plugins/models/compressor-static-fit/model.json --renderer /path/to/qa-compressor-curve --artifacts /path/to/new-artifacts-containing-protocol
```

Set `SOTF_COMPRESSOR_CURVE_RENDERER` to include real native tests. Evaluation
requires a fresh `final` subdirectory. The independent analyzer consumes its
outputs and creates a fresh `final/reopen` directory. Exact commands, failure
history, package versions, source snapshots and hashes are recorded here.
Model/source/synthetic data is GPL-3.0-or-later. No external audio or paper
contents are redistributed. The full ML audit remains in progress.
