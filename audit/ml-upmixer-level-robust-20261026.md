# Excitation-independent Upmixer mask qualification

Issue10/plugin43 remains in progress. This checkpoint constrains absolute-excitation
weights to zero, trains across a gain sweep and qualifies a new v2-contract
candidate. The native runtime, model architecture, reliability/fallback policy,
latency and previous candidate assets/reports remain unchanged. No production
activation or admission is added.

The earlier v2 fit lost desired hard-pan energy after a−40dB gain shift. Absolute
excitation is a nuisance variable for these gain-scaled generated sources. The
new fit fixes coefficients40–47 (the six-feature network's excitation input row)
to exactly zero through optimizer bounds and initialization. Other coefficients
retain the existing±16 bounds and objective. This does not add a new model format:
the fixed65-coefficient v2 network already supports zero weights. Its predictions
are exactly independent of the sixth feature in exact arithmetic, while FFT
rounding and absolute power eligibility remain relevant.

Training uses16 fresh seeds20305120–20305135, validation uses four fresh seeds
20306120–20306123. Matched/hard-pan/direct-only/ambient-only conditions are each
exported at gains−40/0/+20dB through the existing immutable renderer. The generated
labels follow the previous independent FFT/quantized-stem convention. After
eligibility and deterministic every-other-frame thinning, training has230919 rows
and validation58254. Augmented variants sharing a source are correlated. One
initialization converges after520 L-BFGS-B iterations; training/validation
objectives are0.069995/0.082368. These differ in dataset and constraints from the
earlier fit and are not an objective-value improvement claim.

Model SHA256:
`85d1609e715a8d558f4b85bf727b45f373d5ac59915d1ddf604082658c1f7d12`.
Renderer SHA256 remains
`3e75f082dfd2f399f959e03f8187b5b2cac2b7b9bed88ed6215795889be21739`.
The generated-source GPL-3.0-or-later asset is in
`crates/sotf-plugins/models/upmixer-source-mask-level-robust`.

Before inspecting fresh final sources, a separate native validation run declares
maximum shared-band mask drift≤1e−3 and conditional direct-retention drift≤0.05dB
across the gain sweep, in addition to independent network error≤2e−7 and nonempty
shared selection. Its48 cases/96 renders pass: maximum probability drift1.1623e−5,
retention drift2.984e−7dB and network error2.9799e−8. The final evaluator requires
this passing report, bound to the same candidate and training report. It receives
an explicit new final seed range. No final source tunes the fit or policy.

Final seeds20307120–20307127 cover eight variants and three gains, totaling192
cases/384 renders, split96 each between5.1/7.1 by seed parity. The level-shift
variant's−40dB gain case reaches−80dB overall. All192 feature/network gates pass
at unchanged tolerances: maximum selected-feature error2.31993e−5, network error
2.98023e−8, zero eligibility/selection disagreements,443517 eligible band/frame
rows and minimum retained reference energy99.999926%. The model-level gain gate
also passes: maximum shared-band probability drift2.7061e−5 and direct-retention
drift4.6783e−7dB. Eligibility is unchanged across all gain comparisons in this
final set. These are observations within the tested range, not guarantees below
the absolute validity floor or for arbitrary non-finite/unsupported input.

Independent analyzers regenerate quantized inputs and known direct/ambient stems,
verify288 validation and1152 final raw-file hashes, recompute numerical gates,
conditional spectral metrics and gain comparisons, and check exact matrix
coverage. The numerical and level analyzers both inspect the same1152 final
files; their hash counts should not be added as distinct evidence. All executables,
candidate bytes and evaluation sources are frozen for their runs.

Source quality remains insufficient. Means over eight final seeds at the original
level, measured on the same eligible spectral observations:

| Conditional metric | Learned | Scalar analytic diagnostic |
| --- | ---: | ---: |
|Matched direct retention, dB|−4.80|−3.70|
|Matched ambient leakage, dB|−11.59|−7.45|
|Hard-pan direct retention, dB|−3.64|−2.75|
|Hard-pan ambient leakage, dB|−9.86|−6.55|
|Correlated-bed ambient leakage, dB|−0.099|−0.059|
|Direct-only relative RMS mask damage|0.000671|0.02805|

The fit exchanges noise suppression for desired-source loss in mixtures. Removing
absolute excitation also leaves correlated backgrounds largely assigned to the
direct branch. Prior known observation/label ambiguity remains. These results use
different fresh sources from earlier candidates and do not provide a matched
candidate-versus-candidate quality comparison. No listener or recorded acceptance
is established. Conditional spectral metrics exclude fallback/boundary bins and
the full nonlinear spatial chain.

The normalized complete speaker output is not level invariant despite stable
masks. Worst selected-output relative RMS change is18.25% (correlated bed,+20dB);
ordinary native output changes19.06% on that same case. The ordinary native maximum
across all comparisons is27.46% (level-shift variant,−40dB), where selected output
changes13.10%. These are descriptive gain-normalized waveform comparisons, not
perceptual artifacts, causal attribution to a particular native stage or a general
improvement claim. Native covariance, routing, gain and safety remain nonlinear.

Eighteen Python tests pass, including exact asset independence from varied
excitation coordinates, new-candidate callback1/113 identity, v2 inference and
the previous v1 compatibility/reference checks. Ruff passes. No Rust code or
native executable changes in this checkpoint, so prior native timing/heap tests
are not presented as newly run budgets. Model lookahead remains0 and the tested
native renderer declares512 total audio-latency samples at48kHz.

Evidence: `audit/artifacts/ml-upmixer-level-robust-20261026`; raw exports/immutable
binaries: `/Volumes/home_tmp/tmp/ml-upmixer-level-robust-training-20261026`,
`/Volumes/home_tmp/tmp/ml-upmixer-level-validation-20261026` and
`/Volumes/home_tmp/tmp/ml-upmixer-level-robust-final-20261026`.
Remaining: solve desired-source retention/background ambiguity on development
material, freeze another candidate and use a fresh final set; complete full-chain
leakage/artifact and listener/recorded validation, host/platform budgets and
production activation. The full31-plugin/five-program ML audit remains active.
