# XTC feasibility checkpoint — exploratory remedy rejected

Production sources are restored to the exact hashes in `baseline-source.json`.
No new uncertainty setting, calibration assumption, or filter-generation change
was shipped. All three spatial plugin audit scopes remain partial.

The fresh production report still passes 16/17 nominal checks: selected 93.75 Hz
raw attenuation is `4.810583311` dB against the unchanged 5 dB minimum. The inverse
before identity blending provides `25.811234416` dB; after blending, row power is
`4.639932439` against `15.848933863`. The blend causes this nominal shortfall;
the final effort cap is inactive there. An independent modal calculation matches
96 production stage cases within `1.11201e-7` coefficient error.

Fresh rendered mismatch remains 146/600 degraded per normalization mode, with
worst improvements `-5.382083284` and `-5.382084138` dB. Measured KEMAR remains
4/140 degraded, worst `-1.516376941` dB. No thresholds or measured rows changed.
`baseline-nominal.json` matched zero tests and is explicitly incomplete; the
correctly filtered report is `nominal-correct-filter.json`.

## Distinct metrics and gain contracts

Raw attenuation compares filtered wrong-ear pressure with passthrough wrong-ear
pressure. Absolute separation compares desired with undesired pressure. The first
`initial-absolute-separation-analysis.json` certified only the latter; it does not
establish the retained raw 5 dB gate. The corrected analysis preserves that fact.

The exploratory solver requested **worst-case** intended amplitude at least
`0.996817364`, whereas that number was only the legacy **nominal** observation.
That is a stronger new contract, not an existing production guarantee.
Its nominal amplitude ceiling `1.25` was an exploratory target supplied in this
experiment, not a user calibration or an existing normalization tolerance.

For the explicitly stated controlled point-field 5 mm / 1 degree / no-echo
region, the continuous row-error bound is `0.05128023655`, including the declared
nominal point-field/model offset `0.03018208541`. This is not measured human or
room provenance, and is not a default calibration.

With that unchanged bound and the exploratory 1.25 nominal ceiling, quantized
coefficients certify raw attenuation `5.001458583` dB, absolute separation
`5.279199584` dB, intended lower amplitude `0.99681736418`, nominal amplitude
`1.197089232`, and row power `15.252458620`. **Nominal amplitude rises about
1.59 dB relative to the baseline**, so this is rejected as an unchanged-loudness
remedy. No production path was connected.

With nominal amplitude ceiling 1.0, the same **new worst-case** gain floor is
infeasible even before separation: Cauchy bounds its maximum guaranteed amplitude
by `1 - error / ||desired_row||`, below the requested floor. The independent test
records the numeric bound. **This does not prove the original nominal-only LF or
600-row mismatch requirements infeasible.** The wide physical-region row ball is
also a conservative overapproximation, not a physical impossibility proof.

## Exact correlated raw certificate

For an uncertain wrong-ear row `h=b+delta`, `||delta|| <= error`, and cue `j`,
the same `h[j]` appears in passthrough and filtered leakage. When
`|b[j]| > error`, `q=h[other]/h[j]` lies in the exact complex disk

`|b[other] - q*b[j]|² <= error² * (1 + |q|²)`.

Its center is `b[other]*conj(b[j])/(|b[j]|²-error²)` and radius is
`error*sqrt(||b||²-error²)/(|b[j]|²-error²)`. The worst filtered/passthrough
ratio is `|w[j]+center*w[other]| + radius*|w[other]|`. This preserves the shared
row correlation that separate numerator/denominator amplitude bounds discard.

Independent nonreal boundary-row projections cover both cues, zero error and
near-zero-reference boundaries, attaining the reported raw bound within
`7.335e-10` dB. Undefined references are refused and muted columns retain NaN.
The independent solver references include closed-form scalar/modal witnesses,
adversarial response bounds, binary scaling and row-phase gauge covariance.
Final validation has 29 passing tests and strict all-target Clippy passes.
Initial compiler/lint failures are retained. Five exact prototype source files
are archived under `rejected-source/`; they are not current production code.

Any future caller-bound option must bind the exact nominal model/settings
revision, not just sample rate and FFT size. Geometry, head pose and SOFA changes
must not relabel stale bounds as calibrated for a new model.
