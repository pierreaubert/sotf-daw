# Experimental offline Linear-phase EQ fitting

This tool connects the existing learned six-band initializer to the actual native
FIR realization. It produces a separate preset, mono float32 FIR WAV and review
report after end-to-end qualification. No active plugin or default changes.
There is no model execution in audio processing. The initializer remains
experimental; the earlier multiband comparison favored classical seeds overall.

Install `scripts/ml/eq-requirements.txt` and build the native QA exporter:

```sh
cargo build --release -p sotf-plugin-linear-phase-eq --features qa \
  --bin qa-linear-phase-eq-export
python scripts/ml/fit_linear_phase_eq.py --target target.json \
  --weights crates/sotf-plugins/models/eq-multiband/weights.json \
  --weights-sha256 671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015 \
  --renderer target/release/qa-linear-phase-eq-export \
  --output ./reviewed-eq --work-dir ./eq-work
```

Use new, separate output/work directories. Targets contain `sample_rate`,
ascending `frequencies_hz` covering 20–20000 Hz below Nyquist, and `target_db`.
There must be 64..8192 matching samples, integer rate 44.1..192 kHz and levels
within +/-24 dB. Model training rates are 44.1/48/96 kHz; other rates are shifts,
recorded explicitly in the report. Noise, calibration and desired acoustic target
must be supplied by the user; the model does not infer acoustic calibration.

Weights and targets are read once with size caps. The pinned model and exact
target bytes are copied into a new private work directory before invoking the
existing bounded exact fitter. Its neural seed competes with two classical
seeds. The selected method, candidate results and model identity are retained.
No new training or model selection occurs. A flat target can export an identity
FIR without a model. Missing/mismatched model or rejected fit returns no preset.
The prior initializer's provenance, splits and negative adoption result remain in
`../eq-multiband`; these functional cases do not supersede that comparison.

Alternatively supply `--proposal proposal.json` from the existing EQ fitting tool
instead of weights. It must be qualified and identify the exact target byte hash.
The converter accepts at most six second-order peak/S=1 shelf bands within the
initializer's bounds, explicit AutoGain off and no channel-specific routing.
Claiming `accepted: true` cannot bypass the independent final-target checks.

## Native FIR and acceptance

The converted native parameters enable the existing opt-in automatic accuracy
mode, full wet and linear phase. The saved manual support remains the
`--manual-fir-taps` choice (default 1024). Native setup selects the shortest
supported 1024/2048/4096/8192 kernel satisfying its sampled 0.05 dB magnitude
criterion; it rejects a configuration that cannot qualify at the cap.

The final retained f32 kernel is independently checked against the **original
supplied target**, requiring RMS <=0.1 dB and maximum <=0.5 dB. The fitted ideal
response and actual FIR must agree within 0.05001 dB on a dense feature-aware
20–20000 Hz grid; the 1e-5 dB allowance covers independent numerical evaluation.
Additional checks constrain sampled response to +/-24 dB, residual linear-phase
error to 0.001 degrees, complex native-response agreement and allocation-guarded
streaming-impulse reconstruction. These are sampled guarantees, not a continuous
response, perception, calibration or speed guarantee.

`preset.json` is the native `LinearPhaseEqPluginParams` object, with accuracy mode
explicitly enabled. Apply it at the report's sample rate. The report exposes
selected support, total and added native latency in samples and milliseconds.
FIR design delay is N/2; native NUPC adds 32 samples. `fir.wav` contains the design
delay only; its receiving convolver adds its own processing delay. The tool never
shifts a kernel to hide latency or adjusts gain to improve measured errors.

Rejected qualification returns exit 2, `accepted: false`, `parameters: null`,
and no applicable preset/WAV. Successful exports return exit 0. Work files remain
available for diagnostics. Existing output/work directories and source inputs
are never overwritten. Review the report before importing; there is no automatic
apply. This initial tool checks the already selected IIR proposal, rather than
searching every refined candidate for the smallest realizable FIR latency.

## Verification

`audit/artifacts/ml-linear-phase-fit-20261025` freezes eight functional probes
before evaluation. Five qualify: flat at 44.1 kHz, peak at 48 kHz, bass at 96 kHz,
six-band response at 44.1 kHz and a 192 kHz rate shift. Two refined fits select
neural seeds and two select classical seeds; flat uses no model. A jagged target,
a narrow 30 Hz/Q=8 boost beyond the native tap cap, and a bad model hash reject
without exports. This is not evidence of general learned superiority.

The bass case selects 4096 taps and reports 2080 native latency samples, adding
1536 samples (16 ms at 96 kHz) over its saved 1024-tap choice. Other accepted
probes use 1024 taps. An independent direct FIR DTFT and dense uniform SciPy FFT
against a separate Tustin-polynomial reference confirm all five exports. Worst
supplied-target RMS is 0.002899 dB, maximum 0.009879 dB, and dense FIR-versus-ideal
error 0.009958 dB. Exported presets reopen with identical coefficients at a new
511-frame callback partition; maximum reopened impulse error is 5.97e-8.

Seven analytic/integration tests cover target identity, model-free flat, callback
partitioning, forged acceptance and tap-cap rejection. Native exporter build,
strict Clippy and focused formatting pass. Final-source reproduction retains all
eight decisions and five identical kernel/preset/WAV exports. Generated WAVs,
model snapshots and work arrays remain outside the repository; metrics, targets,
source snapshots and hashes are retained in the evidence bundle.

Broader measured/noisy targets, corner cases, controlled refinement/cost
comparisons, application review/import and real-host/platform acceptance remain
open. The Linear-phase EQ proposal and complete ML audit remain in progress.
