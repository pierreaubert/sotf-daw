# Native-feature Upmixer v2 qualification

Issue10/plugin43 remains in progress. This checkpoint implements the previously
developed raw-power reliability policy in an explicit v2 QA model contract,
fits one candidate on actual native features and evaluates fresh final sources.
The v1 asset, contract and failed reports remain intact. V2 is not admitted for
deployment and no production selector is added.

V2 retains the6→8→1 network and six bounded covariance/excitation features.
The loader requires exact v2 metadata, native-training provenance and the fixed
1e−6 relative band-power floor. Ordinary native decomposition handles ineligible
bands. The largest band power excludes DC/Nyquist and includes finite non-boundary
observations; eligibility also requires the unchanged strict absolute interval
1e−16<power<1e12. Native inference uses two bounded32-band passes on the existing
48kHz FFT512/hop256 analysis clock, with fixed storage and no callback heap,
worker or extra lookahead. Observed features remain available independently of
relative eligibility, while the optional mask records actual selection.

The training renderer is copied before export and never rebuilt during fitting
or final qualification. An explicitly untrained zero-coefficient v2 model enables
native observation export. Training uses seeds20302120–20302135, validation
20303120–20303123, across matched/hard-pan/direct-only/ambient-only conditions.
Every other frame is retained after numerical eligibility, producing76873
training rows and19266 validation rows. Labels are known generated direct-stem
power fractions from independent f64 FFTs of separately quantized f32 stems;
the mixture is quantized separately. The small stem-sum quantization discrepancy
is part of this declared label convention.

There is one fixed initialization and one fit, with bounded coefficients and
L-BFGS-B max1000 iterations. Optimization converges after777 iterations by relative
objective reduction. Training/validation objectives are0.063695/0.072290,
including the declared ridge term. Validation does not select among candidates.
The model is serialized before any fresh final source is inspected.

Model SHA256:
`1f0b2b835abc0060913717b41588afffd76d47ec207c7b40906640de9d9c73c2`.
Renderer SHA256:
`3e75f082dfd2f399f959e03f8187b5b2cac2b7b9bed88ed6215795889be21739`.
The asset is under `crates/sotf-plugins/models/upmixer-source-mask-v2`, with
project-generated provenance and GPL-3.0-or-later licensing. No external recording
or speech-identity supervision is used.

Final seeds20304120–20304127 are disjoint from policy development, training and
validation. Eight variants and gains0/−40/+20dB produce192 cases,384 actual
selected/native renders, split96 each between5.1 and7.1 by seed parity. Variants
and gain transforms sharing a seed are correlated. The level-shift variant
already applies−40dB, so its−40dB sweep case is−80dB overall. This is generated
stress evidence, not a human/recorded holdout or independent population of192
performances. The sole candidate, policy and tolerances remain frozen afterward.

All192 cases pass both numerical gates. Across441747 eligible band/frame rows,
maximum selected feature error is1.7981e−5 against the unchanged1e−4 gate;
maximum independent NumPy/native network error is2.9803e−8 against2e−7.
There are zero relative-eligibility or actual native-mask selection disagreements.
Minimum retained non-boundary reference energy is99.9999354% per non-silent
case, exceeding the99.9% gate. Silence selects no bands and remains silent.
Independent analysis verifies1152 raw hashes, regenerates every quantized source,
recomputes all numerical gates and reconstructs conditional spectral metrics from
known stems and native masks. These results support the v2 numerical repair;
they do not retroactively turn the preserved v1 failures into passes.

Quality remains insufficient. The following are means over8 fresh seeds at the
original level. Both methods are evaluated on the same eligible band/frame set.
The analytic column is a scalar diagnostic, distinct from full native routing.

| Condition and conditional spectral metric | Learned | Scalar analytic diagnostic |
| --- | ---: | ---: |
|Matched direct retention, dB|−4.81|−3.95|
|Matched ambient leakage, dB|−11.89|−7.73|
|Hard-pan direct retention, dB|−2.92|−2.22|
|Hard-pan ambient leakage, dB|−9.61|−6.46|
|Correlated-bed ambient leakage, dB|−2.17|−0.14|
|Direct-only relative RMS mask damage|0.00211|0.09270|
|Ambient-only leakage, dB|−17.92|−9.69|

After a−40dB gain shift, learned hard-pan direct retention degrades to−13.44dB;
matched retention is−7.26dB. Absolute excitation remains a training-distribution
dependency. Better mask MSE and noise suppression do not compensate for desired
source loss. Correlated-bed ambiguity is also unresolved. These selected-band
metrics isolate the shared amplitude mask on known source spectra; they exclude
fallback bands, boundary bins, WOLA time-domain artifacts and nonlinear center,
panning, decorrelation, gain and safety behavior. Complete multichannel outputs
and descriptive output-energy changes are retained separately. No perceptual
or complete-chain separation improvement is established.

Verification covers eight native integration tests and two raw-power/cutoff unit
tests, strict targeted QA Clippy, and16 Python contract/reference/native tests.
The unit cutoff probe straddles60dB using adjacent f32 amplitudes; native Python
tests prove v2 callback1/113 identity in both layouts and exact v1 output and
snapshot identity against the preceding immutable renderer on three development
conditions. Existing tests cover reset/drain/unsupported geometry, silent
fallback and process/drain heap guards. The measured declaration remains512
audio-latency samples and0 model lookahead. No new impulse timing or memory/
deadline budget claim is made for this candidate.

Reports and source snapshots are in `audit/artifacts/ml-upmixer-v2-20261026`.
Raw exports and immutable binaries are in
`/Volumes/home_tmp/tmp/ml-upmixer-v2-training-20261026` and
`/Volumes/home_tmp/tmp/ml-upmixer-v2-final-20261026`.
Remaining: improve level robustness and desired-source retention using development
sources, freeze another candidate and use another fresh final holdout; address
correlated-background limitations; measure complete-chain leakage/artifacts and
listener acceptance; finish memory/contended-host/platform budgets and product UX.
The complete31-plugin/five-program ML audit remains active.
