# Offline compressor static-curve assistant

This opt-in assistant fits explicitly supplied per-band input/output dB curves
to bounded threshold, ratio, knee and makeup controls. A trained initializer
proposes a seed, then exact bounded refinement compares it with three segmented
analytic seeds and a fixed seed. Each accepted result is measured through the
actual native compressor before a reviewable preset is exported. Classical-only
operation remains available. Runtime envelopes and compressor DSP are unchanged.

Example from the repository root (NumPy/SciPy for fitting; PyTorch for training):

```sh
cargo build -p sotf-plugin-multiband-compressor --bin qa-compressor-curve --features qa --release
python scripts/ml/fit_compressor_curve.py --target target.json --model crates/sotf-plugins/models/compressor-static-fit/model.json --model-sha256 30c80f93df4e25bbb80133099654329eca5ebe9779396a10fbd22f88b87fb06a --renderer /path/to/qa-compressor-curve --output new-result-directory --work new-native-work-directory
```

Omit both model arguments for classical-only fitting. Existing output/work paths
are rejected. Success (exit0) writes `report.json` and a native
`MultibandCompressorPluginParams` `preset.json`; any failed band or native gate
rejects the whole preset (exit2), clears every `parameters` field and writes no
preset. Invalid formats/checksums fail before output. No active preset changes.
The model and target are each hashed/parsed from one bounded byte buffer.

Input format: `sotf-compressor-static-target-v1`, with
`level_convention: constant_amplitude_dbfs`, integer `sample_rate` 44100–192000,
explicit ordered `crossovers_hz` (one per split), and one to five `bands`.
Each band supplies 64–128 ascending `input_dbfs` points covering−84..0dBFS,
matching `output_dbfs`, and explicit `attack_ms` (0.1–100) and `release_ms`
(10–1000). These levels describe constant-amplitude plateaus, not a sine peak
versus RMS conversion or acoustic SPL calibration. The artifact
`audit/artifacts/ml-compressor-curve-20261025/functional-target.json` is a complete
synthetic example. Learned controls are threshold[−60,0]dB,ratio[1,20],knee[0,20]dB,
makeup[−24,24]dB. Timing and crossovers are copied, never inferred.

The explicit profile uses full wet mix, linked channels, AutoGain/auto-makeup
off, range120dB,hold0,zero lookahead, no M/S or sidechain tilt/filter/external
mode. Native construction validates all controls and crossovers. A multiband
preset smoke check proves construction and finite processing; gain-curve
qualification measures each band in isolation using the same native processor.
It does not prove mixed-band programme response, crossover phase interaction,
transient overshoot or measured-hardware equivalence.

Mathematical sampled fit gates: RMS≤0.01dB,maximum≤0.03dB. Native plateau gates:
RMS against supplied points≤0.03dB,maximum≤0.06dB,and maximum deviation from the
ideal selected curve≤0.05dB. Native runs reset state before each level, settle
for32 attack time constants plus50ms, and measure the final DC plateau. Process
calls use allocation guards. The report states measured errors and latency.
The native fast-log/exp approximations and f32 envelope matter: the 192kHz,
100ms-attack probe deviated up to0.04049dB from the ideal curve while staying
inside the declared gate. No native thresholds were changed after evaluation.

Makeup is part of the explicit target. Positive gain can require headroom.
`maximum_isolated_band_sampled_output_dbfs` and
`required_isolated_band_sampled_headroom_db` describe isolated sampled bands;
they are not a bound on the recombined bus, a limiter ceiling or a live peak
prediction. The helper adds no limiter or opaque automatic gain changes.

Static curves do not identify attack, release, hold, programme dependence or
hardware memory. Ratio1 leaves threshold/knee unidentifiable; constant-gain
curves return a declared canonical representative. Sparse knee coverage may
also admit response-equivalent controls. Acceptance means agreement at supplied
samples and native plateaus, not recovery of the original controls or a
continuous guarantee for an unknown measured curve.

The12676-parameter128→64 SiLU→64 SiLU→4 tanh MLP was trained on4096 synthetic
parameter identities with512 disjoint development identities; epoch100 minimized
response MSE. The64 fresh final curves had raw initializer MSE0.0835609dB² versus
158.1101 for identity. All64 neural-seeded refinements qualified, as did both
stronger segmented classical seeds. Median residual calls were40 neural versus
35 for each stronger segmented seed. This is not evidence of ML speed or quality
advantage. Ten compatible functional/native exports qualified, including one,
three and five bands, hard knee, near-unity, noisy/irregular targets, a high-rate
slow attack and classical-only operation. A jagged curve was rejected.

Independent scalar-formula checks and preset reopens agree; changing callbacks
and doubling settling gave identical plateau results. Nine Python/native tests,
173 native crate tests and strict Clippy pass. The knee-boundary test includes
native approximation error rather than assuming exact pow/log. Evidence and
resolved test-oracle history are preserved under
`audit/artifacts/ml-compressor-curve-20261025`.

This implements static offline assistance. Temporal/identified-hardware fitting,
broader measured targets, mixed-band peak/phase behavior, controlled end-to-end
cost, application review/import and real-host acceptance remain unfinished.
The assistant has no inference or new latency on the audio thread. All source,
portable weights and generated mathematical data are GPL-3.0-or-later; no external
audio, hardware observations or pretrained weights were used.

Primary method context: [Giannoulis et al., JAES2012](https://aes.org/publications/elibrary/elibrary-page/?id=16354),
[DeepAFx-ST](https://csteinmetz1.github.io/DeepAFx-ST/), and
[Kates2005](https://pmc.ncbi.nlm.nih.gov/articles/PMC4111488/). The initializer,
training distribution, fit gates and static-only scope are implementation
choices; these publications do not establish this model's accuracy or adoption.
