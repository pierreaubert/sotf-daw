# Saturation neural-candidate integration checkpoint

Issue #10, plugin 38: measured nonlinear emulation with alias and parameter-sweep
tests, rather than waveform similarity alone. Implementation is in progress.
This checkpoint provides a capture-bound native QA path and analytic boundary
fixtures; no trained or measured-hardware Saturation model is admitted.

The shared `ExternalFallbackQualification` stream reuses the existing validated
hardware candidate loader and fixed coefficient/history storage. Each sample
receives its caller's deterministic classical value. The stream keeps raw input
history while disabled; wrong capture clocks or controls mark history invalid,
and domain failures preserve the exact supplied classical wet sample. The older
analog-color qualification API is unchanged.

The Saturation crate's optional `qa` path installs a model only before input, in
initialized mono at an integer 44.1–192 kHz clock. It validates explicit SHA-256,
hardware/calibration/rights context, and exact native nonlinear-control binding.
Capture hardware controls must contain `sotf_stage=saturation_wet_pre_dc` and
`sotf_controls_sha256=capture_controls_sha256(plugin)`. This hash includes mode,
drive, tone, exciter frequency, dynamic amount/times and ADAA choice. It is made
on the control thread. JSON/hash/model loading never occurs while processing.

Candidate selection replaces only the wet nonlinear result. The native
waveshaper, envelope and ADAA histories run independently on every source sample;
native DC blocking, output gain and wet/dry mixing follow selection. The DC
blocker consequently follows the selected wet signal, rather than maintaining
an independent hidden classical DC history. Switching can retain a DC transient;
exact post-stage reset is explicit. Immediate QA toggling is not click-free
product activation. No ceiling or inter-sample peak guarantee is introduced.

Wet-control mismatch latches classical selection until reset, preventing model
reentry during a smoothing ramp from depending on host callback size. Wrong
clock/current-or-past input outside the model domain also selects classical wet
samples. Input is transactionally required to be finite and bounded by 64 in
this prepared QA path. Native setters, defaults, factory behavior, oversampling
preference and production parameters are preserved. Native/model lookahead is
zero; host oversampling latency remains a separate contract. A model's exact
capture clock must match the actual processing clock, including a wrapper's rate.

Boundary tests use explicitly untrained tanh coefficients at drive 1/4/12/20.
They are analytic fixtures, not trained source groups or measured hardware.
Their provenance is synthetic validation and admission stays false. The tests
check numerical parity with independent float64 tanh, callback partitioning,
reset/reinitialize, missing/disabled/wrong-clock fallback, invalid history windows,
continuous native ADAA state, control mismatch/reset, hash/control binding,
transactional input validation and allocation/deallocation guards. A native
direct-tanh route independently checks post-stage DC/output/mix placement.

Alias probes use coherent sine bins 401 and 999 of 4096 at 48 kHz and amplitude
0.7 across all four drive settings. Selected folded odd harmonics 3–31 are
measured relative to the fundamental on the final steady period; this is not
total alias power. `analyze_saturation_alias.py` independently integrates a
continuous tanh curve over 65,536 phase points to verify those native probes.
The neural fixture has no built-in ADAA or oversampler. Its alias result cannot
be inferred from waveform parity or from the classical path's ADAA behavior.
All eight native probes agree with the independent Fourier integration within
0.000045 dB. Selected folded energy is 10.09–25.12 dB higher for the direct neural
fixture than native ADAA, relative to each route's own fundamental. The comparison
does not hold the fundamental response identical; it demonstrates that a direct
neural replacement cannot inherit the classical route's antialiasing claim.

The full focused run passes 74 Saturation tests and 28 shared analog-common
tests. Strict QA Clippy and the default-feature Saturation build pass. The final
focused candidate rerun also covers zero-frame callbacks: changing controls and
returning them before any audio must not invalidate otherwise unchanged history.
The native tail contract remains unknown; no finite neural/DC drain claim is made.

Command records and complete failures/retries are retained under
`audit/artifacts/ml-saturation-candidate-20261025`.

Remaining: actual identified-hardware captures with calibrated levels/rights and
independent train/validation/test sources, frozen fitted models, measured target
comparison across controls, oversampled/anti-aliased learned-route design and
reference comparisons, lifecycle/automation and DC-switching acceptance, complete
memory and callback/contended host budgets, stereo semantics and cross-platform
DAW validation. Existing generic hardware-capture/fitting infrastructure can
consume future recordings; synthetic fixtures do not satisfy these requirements.
