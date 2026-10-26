# Native-feature direct/ambient mask v2

QA-only generated-source candidate. Deployment is not admitted.

`model.synthetic.json` SHA256:
`1f0b2b835abc0060913717b41588afffd76d47ec207c7b40906640de9d9c73c2`.
Project-generated panned harmonics and stereo noise beds; GPL-3.0-or-later.
No recordings or speech-identity labels were used.

The6→8→1 network retains the v1 feature definitions but uses the explicit
`sotf-upmixer-direct-ambient-mask-v2` contract. Training features come from the
actual native f32 FFT and f64 covariance accumulators. A band is eligible only
when its reference-scale stereo power is strictly between1e−16 and1e12 and at
least1e−6 of the strongest non-boundary analysis-band power in that frame.
DC/Nyquist are excluded. Unsupported, invalid or ineligible evidence retains
ordinary native decomposition. The v1 contract and coefficients remain separate.

One initialization converged after777 L-BFGS-B iterations on76873 native rows;
19266 disjoint validation rows were measured without candidate selection.
Training covers16 seeds across matched, hard-pan, direct-only and ambient-only
conditions; validation covers4 different seeds. Final evaluation used8 further
seeds across8 variants and3 gains, totaling192 correlated source cases in5.1/7.1.
All selected-band numerical and native-network gates pass. Model lookahead is0;
the native renderer declares512 samples total latency at48kHz.

Quality still fails: hard-pan desired-source attenuation remains worse than the
scalar analytic diagnostic, especially under a−40dB level shift; correlated-bed
leakage remains high. Conditional spectral mask metrics do not establish the
complete nonlinear multichannel renderer's source leakage or listener acceptance.
This model must remain a QA candidate until quality, shift, full-chain artifacts,
host/platform budgets and product activation requirements are satisfied.

See `audit/ml-upmixer-v2-20261026.md` and its artifact directory for the frozen
training/evaluation identities, numerical verification and retained failures.
