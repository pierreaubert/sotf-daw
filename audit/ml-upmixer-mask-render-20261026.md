# Native source-mask export and multichannel qualification

Issue10/plugin43. This checkpoint adds `qa-upmixer-mask-render`, a bounded native
renderer, and independent Python export/evaluation/analysis tools. Source-mask
runtime code and frozen model coefficients are unchanged from the preceding
native-boundary checkpoint. No production selector or model admission is added.

The renderer accepts finite stereo source samples bounded by64, at most one
million frames, callback sizes1–256 and actual5.1/7.1 layouts. Request/model reads
are bounded to64KiB, output files use exclusive creation, and native source/tail
generation uses process/drain heap guards. Complete output includes native finite
drain. Exported source snapshots carry exclusive analysis-end and callback
observation frame indices; decisions are frozen during drain. Speaker labels,
positions and channel indices come from the native layout definition.

The evaluation profile is48kHz, FFT512, HR/scalar ML/AutoGain disabled and
dialogue weight0, with other native defaults including final safety. These are
explicit qualification settings, not an assertion that every production setting
is covered. Total declared audio latency is512 samples and model lookahead0;
prior native impulse tests measured zero added onset delay.

The renderer is copied to an immutable executable before evaluation. Its SHA256:
`117cf6248a2f0598de0928e112fb041ea0fbd88e6b8de079111443a6d0dd6802`.
Model SHA256 remains
`2ca743740f6adeaa40738a82066c25f65d20633a9bc8a237692aa156818c6d6a`.
No fitting, coefficient or policy changes follow the evaluation.

The run contains200 source cases and400 native renders:32 original training,
8 validation and128 previously examined final cases in5.1, plus32 fresh7.1
probes from4 seeds20298120–20298123 across8 correlated variants. The fresh seeds
are disjoint from prior model splits. Previously examined final sources are a
replay, not a new generalization holdout. Source audio is quantized to f32 before
native rendering and independent reference analysis; the original generated-stem
identities and actual quantized input hashes are both retained.

All535680 mutually valid band/frame feature rows have matching validity flags.
Native sigmoid inference agrees with independent NumPy on its exported features
within2.981e-8. However, the fixed1e-4 absolute feature-error gate fails in all20
direct-only cases. The largest normalized covariance-feature difference is0.333;
reference-feature versus native-feature model outputs differ by as much as0.555.
Every other case passes that feature gate, with worst feature error below1.18e-5.

Independent analysis locates the discrepancy in extremely quiet spectral bands,
where f32 native FFT rounding affects normalized covariance. The worst band has
reference power5.51e-13 while total frame power is62.55. Across failed cases,
bands exceeding the gate contain7.52e-9–1.93e-8 of total reference spectral energy;
power-weighted feature errors are2.53e-8–4.36e-8. These energy observations do not
turn the failed absolute-feature gate into a pass. A future candidate needs a
validated numerical-excitation policy and/or training on actual native features,
with a new frozen contract and final holdout. The current failure is preserved.

Full-output diagnostics include front/surround/LFE energy, peak, maximum sample
difference and relative RMS change against ordinary native rendering. Example
means over16 prior final5.1 sources:

| Condition | Relative RMS output change | Front energy change versus native, dB |
| --- | ---: | ---: |
| Matched mixture | 0.509 | -3.20 |
| Hard-pan mixture | 0.590 | -4.58 |
| Correlated bed mixture | 0.291 | -0.75 |
| Direct only | 0.0537 | -0.24 |
| Ambient only | 0.579 | -5.19 |
| Silence | 0 | 0 |

These show routing changes, not a clean reference or isolated leakage for mixed
sources. Native covariance, center/panning and safety make the complete chain
nonlinear; independently rendering stems does not provide a valid additive
source decomposition. Direct-only surround energy and ambient-only front energy
are available as single-source placement diagnostics. Sample differences are
descriptive proxies, not measured perceptual artifacts. No audible benefit is
established. The previously recorded hard-pan desired-source loss and correlated
background leakage still prevent admission.

Three Python tests execute with the real renderer: both layouts preserve exact
callback1/113 audio/snapshot identity; independent features/network checks pass
on broadband material; rejected hashes retain complete native output; and invalid
wrapper geometry rejects before creating files. Strict all-target QA Clippy
passes for the new binary. The prior native suite remains recorded separately;
this checkpoint does not claim missing historical golden files were supplied.
Independent analysis verifies1200 raw input/output/feature hashes.

Commands, source identities and reports are in
`audit/artifacts/ml-upmixer-mask-render-20261026`; raw requests/outputs/features
and the frozen binary are in `/Volumes/home_tmp/tmp/ml-upmixer-mask-render-20261026`.
Remaining: repair the feature-policy mismatch using development sources, improve
the model, measure conditional multichannel leakage and perceptual artifacts,
complete memory/contended-host/platform budgets and production activation UX.
The full ML audit remains in progress.
