# Offline learned proposals through native Linear-phase EQ FIR export

This checkpoint implements the pending Linear-phase EQ ML boundary: offline
learned/classical initialization and exact refinement followed by the actual
native FIR design, final-coefficient validation and separate reviewed exports.
No active plugin/default, model weights or realtime DSP implementation changes.
The new Rust binary is QA-only and uses existing public FIR/response APIs.

The existing six-band model is pinned by SHA-256:
`671fbe820883249c00c228d5497d4585a1836afba62f6e2f86d0e39ecbdf3015`.
Model and target bytes are read with caps and copied into a new private work
snapshot. Neural and two classical seeds compete in the existing bounded exact
fitter. A supplied qualified proposal may also be converted, with target-identity
and schema checks. Flat targets need no model. The model's prior negative general
adoption result remains unchanged; no new training occurs.

Native setup enables the existing opt-in automatic accuracy mode, shared linear
phase, full wet, AutoGain off. It selects actual support from 1024..8192 taps or
rejects an infeasible target. The final retained f32 FIR must independently meet
the original supplied target (RMS <=0.1 dB, maximum <=0.5 dB), dense FIR/ideal
agreement <=0.05001 dB, sampled +/-24 dB gain bound and 0.001-degree residual phase
bound. Native complex response and allocation-guarded streaming impulse must also
agree. These are finite/sampled checks, not continuous or perceptual guarantees.

Only qualified runs export `preset.json`, `fir.wav` and `report.json`. Rejected
runs return `parameters: null`, exit 2 and no applicable preset/WAV. Source inputs
and previous directories are not overwritten. Generated data/work files stay in
external temporary storage. No automatic apply occurs. The report distinguishes
N/2 FIR design delay from 32 samples of native processing delay; it reports total
and added native latency in samples and ms. A WAV consumer adds its own delay.

`experiment/protocol.json` freezes eight functional probes and source hashes
before evaluation. Declared synthetic source-parameter identities are disjoint from the
model's train/development/initial/final split identities. Required rejection
fixtures are explicit capability/identity probes, not blind population tests.

| Probe | Result | Selected method | Taps | Native latency | Added samples |
| --- | --- | --- | ---: | ---: | ---: |
| Flat 44.1 kHz | Qualified | No model | 1024 | 544 | 0 |
| Peak 48 kHz | Qualified | Classical constant | 1024 | 544 | 0 |
| Bass 96 kHz | Qualified | Neural | 4096 | 2080 | 1536 |
| Six bands 44.1 kHz | Qualified | Classical edges | 1024 | 544 | 0 |
| 192 kHz rate shift | Qualified | Neural | 1024 | 544 | 0 |
| Jagged target | Rejected | No qualifying refined fit | — | — | — |
| 30 Hz/Q=8 boost at 96 kHz | Rejected | FIR tap-cap failure | — | — | — |
| Bad model identity | Rejected | No inference | — | — | — |

The 96 kHz bass case adds 16 ms over its saved 1024-tap baseline. Native support
is selected from the actual response, not hardcoded from sample rate. The 192 kHz
case is outside model training rates and is flagged as a shift. These few cases
validate functionality; they do not supersede the larger negative initializer
comparison or prove a speed/quality advantage over classical fitting.

`independent-validation.json` checks every supplied target point by direct FIR
DTFT and a dense uniform SciPy FFT against separately Tustin-converted analog
polynomials. It reopens all five saved presets with 511-frame callbacks and
checks identical kernels and streaming responses. Worst target RMS/max errors
are 0.002899/0.009879 dB; uniform FIR-versus-ideal error is 0.009958 dB. Reopened
impulse maximum error is 5.97e-8. The initial exporter uses 193-frame callbacks.

Seven analytic/native integration tests cover target identity and level policy,
model-free flat, callback partition invariance, forged acceptance, tap limits and
malformed proposal rejection. The native build, strict Clippy, Python style and
focused formatting pass. `reproduce-final.json` reruns the complete final-source study
after malformed-proposal/logging refinements and confirms identical acceptance
for all eight cases and byte-identical kernels/presets/WAVs for all five exports.

Commands/status/durations are in named JSON files with matching text logs; trailing
blank lines in logs are trimmed. Frozen initial sources remain under
`experiment/evaluation-source`. Final-source reproduction stores its protocol in
`reproduce-final.json`. Generated model snapshots, FIR arrays/WAVs and work data stay
under `/Volumes/home_tmp/tmp/ml-linear-phase-*`. All committed target curves are
project-generated synthetic data, not external or user recordings.

Reproduce using the model README and `evaluate.json`, `independent-analysis.json`,
`python-tests.json` or `reproduce-final.json`, with new output paths. `source-sha256.json`
hashes final sources/model/docs, and `artifact-sha256.json` hashes the other files
in this bundle. The initial tool validates the already selected IIR proposal;
FIR-aware comparison across all refined candidates, measured/noisy targets,
application review/import and real-host/platform acceptance remain open. The full
31-plugin/five-program audit remains in progress.

The first final regression check (`python-tests-r1.json`) exposed a missing
dictionary guard for non-object proposals. It is retained as resolved failure
evidence. `python-tests-final.json` passes after the explicit guard, and
`malformed-proposal-check.json` verifies actual CLI exit 2, a rejection report and
no preset/WAV. Final reproduction preserves all eight decisions and five exports.
