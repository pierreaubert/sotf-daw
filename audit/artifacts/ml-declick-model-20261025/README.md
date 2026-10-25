# Optional offline Declick classifier — negative adoption result

Issue 10. Implements native-group qualification with a trained tiny classifier,
a strict model loader, float32-WAV CLI, dry fallback, actual native renderer,
frozen synthetic training/development/final protocol and independent analysis.
**Not adopted for production.** Existing Declick processing is unchanged.

Model SHA `91dbe0e86f545c61ad714539510e6afe4f13ee45c61982f76adbc6cd2b7d4e9f`.
The model/recipes/provenance are under
`crates/sotf-plugins/models/declick-discriminator`. That README contains the CLI,
exact supported domain and measured failures. `methodlog.md` records primary
literature, method choice and limitations. Clean references appear only in
training labels and evaluation; runtime features see aligned dry/native audio.

The 289-parameter 16→16 tanh→1 sigmoid model uses two-sided multiscale context.
Training:128 background identities with click/percussion variants,242 supported
native groups. Development:32 separate identities,64 groups. Epoch138 minimizes
BCE. The development-only grid selects threshold0.99 with5 beneficial/0harmful
accepted groups. Unsupported groups retain dry audio. The simple classical tail
veto is selected separately on development; unprocessed dry is also explicitly
included. No claim is made that the model dominates every classical tradeoff.

`protocol.json` freezes seeds, supported rates/channels, training source hashes
and renderer identity before training; evaluation source is pinned before final
scoring. `split-verification.json` excludes earlier smoke/probe/training/dev
identities. Background IDs group correlated event variants and rate/channel
shifts rather than implying independent physical recordings.

Final:99 synthetic probes from16 new base signals with6 variants plus3 shifts.
The model preserves the tested longer-tail percussion and attack groups but
accepts only2 beneficial groups and2 harmful legitimate-impulse groups. The
identical-observation counterfactuals expose a semantic identifiability limit:
16 click/legitimate-impulse pairs have identical dry/native/learned arrays but
opposite reference intentions. No policy/model changed after this result.

| 48 kHz mono stratum,16 cases | Dry SSE | Native SSE | Learned SSE | Tail-veto SSE |
|---|---:|---:|---:|---:|
| Injected click |51.9693|5.12695|37.3938|45.7133|
| Legitimate percussion |0|42.8332|0|0|
| Legitimate isolated impulse |0|47.2946|14.7920|6.71929|
| Other attack |0|7.44593|0|0|
| Step / clean |0|0|0|0|

Learned legitimate-impulse peak error reaches1.80833. These are synthetic
waveform-error probes, not ratings of audibility or real musical preference.
The model needs independent recorded/counterfactual labels and a better declared
supported-material contract before general use. No measured-room, music-corpus,
listener, cross-platform or real-host acceptance is implied.

Authoritative successful checks:
- `final-checks.json`: current Python/model/native-binary tests, Ruff and rustfmt.
- Native artifact `build.json`: release build and strict Clippy of the renderer;
  `native-tests.json`:131 native tests including existing DSP contracts.
- `analysis-command.json` / `independent-analysis.json`: independent two-sided
  feature calculation and Torch forward equations,99 saved-array error checks,
  binary whole-group support invariants and16 identifiability pairs. Maximum
  probability discrepancy2.23e-16; SSE discrepancy1.78e-15.
- `cli-validation.json`: actual WAV CLI, invalid-hash and rate dry fallback,
  exact length/finiteness and refusal to overwrite. Exported PCM remains external.

`checks-r1.json` and the native `python-native-tests.json` preserve earlier
successful smaller test sets. No thresholds were relaxed to satisfy tests.
Raw native/clean/dry/learned/classical arrays live in the external work directory;
`external-array-manifest.json` pins them. Reproduce with the scripts and recorded
command arguments, choosing fresh output/work directories. Then run:

```sh
python scripts/ml/analyze_declick_model.py --model crates/sotf-plugins/models/declick-discriminator/model.json --artifacts audit/artifacts/ml-declick-model-20261025 --work /path/to/reproduced-final-work
python -m unittest discover -s scripts/ml -p test_declick_model.py
```

Set `SOTF_DECLICK_RENDERER` to the built QA binary to include native matrix tests.
The frozen model directory includes its exact training dependencies. Source and
artifact manifests pin hashes. Synthetic material/source is GPL-3.0-or-later;
no external audio or model assets are redistributed. The full ML audit remains
active, including better data, fresh validation, UI/import and any live-worker
integration.
