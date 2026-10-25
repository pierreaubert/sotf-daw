# Identified hardware coloration: native baseline and capture contract

The audit permits learned coloration only with identified hardware and operating
levels. The offline fitter now trains a target-specific causal neural candidate
and a classical Hammerstein comparison from the calibrated paired-data contract.
No hardware model has been admitted, no default learned model is selected, and
no production neural processing or native coloration replacement is introduced.
`candidate.synthetic.json` is a generated functional fixture, not measured
hardware evidence or a pretrained production model.

## Offline candidate training

```sh
python scripts/ml/train_analog_hardware.py --manifest paired-hardware.json \
  --output hardware-candidate --taps 8 --hidden 12 --iterations 300
```

The neural candidate is `y = u*linear + tanh(u*W+b)*v+d`, with current-then-past
causal input history `u`, scaled by the recorded training input RMS for numerical
conditioning. Predictions remain in original recorded output units; targets
are not gain-normalized, DC-centered or time-shifted. Architecture bounds are
1..64 taps and2..32 hidden units. It has0 lookahead, finite input history and an
optional constant quiescent response; finite excitation history does not imply
that its zero-input output is zero. Global output magnitude is bounded by the
linear L1 bound over the trained peak domain plus bounded tanh output weights
and bias. The128 software bound is not a full-scale ceiling or limiter.

The fitter needs eight whole-source training groups; each group has equal total
weight even with repeated captures. It samples at most8192 frames per record and
65536 total, with an additional architecture/iteration quota. Sampled causal
history is formed before sample selection. Training excludes the first taps-1
startup rows; held-source scoring includes every recorded sample, including
startup. Input scale and peak domain come only from training sources. L-BFGS-B
uses explicit seed, ridge and iteration limits, with full optimizer diagnostics.
The parallel-Hammerstein baseline uses powers1..5 of the same input history and
a constant, fitted with regularized weighted SVD on the same selected rows.

Neural and classical parameters are frozen to `candidate.json` before either
validation or test PCM is read. Each held split needs two source groups. Every
held capture must be within the trained peak domain, have neural NMSE<=-25dB,
at least1dB advantage over this declared Hammerstein baseline and sample
peak<=0.999. These fixed numerical comparison gates are development policy,
not calibrated hardware/listener acceptance. The report preserves optimizer
success/failure independently of the held numerical gates. A candidate can meet
the numerical gates after exhausting its optimizer budget, which does not prove
convergence or deployment readiness. There is no automatic hyperparameter search
against validation/test targets. Repeated user experiments still require a new
untouched final test corpus.

CLI exit0 means only that those offline numerical gates passed; exit2 means they
did not. Both outcomes explicitly state `hardware_model_admitted: false` and
`automatic_apply: false`, and neither emits a runtime preset. Loading requires
an explicit file/SHA256 and exact hardware/calibration/rate context, validates
parameter dimensions, finite bounds and the frozen policy, and rejects a changed
deployment flag. Python and native QA inference support qualification.
Production native activation and deployment admission remain outstanding; the
QA-only native inference/fallback path is described below.

## Native qualification

`sotf_plugin_analog_common::hardware_candidate` is gated entirely by `qa`. It
validates bounded candidate bytes, explicit SHA256, calibrated context and the
frozen comparison policy on the control thread. Its fixed-size weights/history
support the full64-tap/32-unit format. It accepts only the unadmitted candidate
format; no production plugin factory or model ID selects it.

`HardwareQualification` owns a mono native classical stage and an optional
candidate. It starts disabled. Classical processing and neural input history run
continuously, so fallback uses the current classical state and enabling does not
start the candidate with stale history. Missing/rejected models use classical
output. Nonfinite or out-of-domain inputs mark every affected neural history
window invalid; fallback continues until all required past samples are valid.
Invalid computed neural output also falls back. Callback-rate mismatch rejects
before output/history/counter changes. Reset clears both histories and counters
while preserving controls and enabled state. Process/reset/toggle/counter reads
have allocation/deallocation guards and use no hashing, file access or locks.

Candidate output replaces the sample during enabled valid qualification. The
fallback drive/color/character/trim controls affect the classical path only;
they do not retune the fixed captured hardware candidate. Enable and fallback
switches are immediate for testing. Click-free live transitions, full production
control/preset semantics and real-device admission are not implemented here.
The ordinary shared stage and its zero-color behavior are unchanged.

```sh
cargo build -p sotf-plugin-analog-common --features qa \
  --bin qa-hardware-candidate-render --release
```

`scripts/ml/hardware_candidate_native.py` writes bounded raw-PCM requests for this
renderer. Requests contain explicit candidate path/hash or null, expected
context, complete classical controls and sample/frame geometry. Sorted
`disabled_ranges` and `reset_frames` let qualification exercise exact event
boundaries independently of callback partition. Input float32 patterns including
NaN/infinity are retained to test native recovery. Output reports frame-decision
counters, loader errors, epoch counts, zero host latency, hashes and QA timing.
Every output remains finite within software rails; there is no additional gain
normalization, limiting or finite-zero-tail promise. Files are never overwritten.

The native evidence is in
`audit/artifacts/ml-analog-hardware-native-model-20261025`:28 Rust tests,21 combined
Python tests,54 conditional parity/fallback cases, strict QA Clippy and compilation
without QA pass. Maximum reference error is2.9801380141e-8 and callback reopening
is bit-identical. Five65537-frame timing runs per mode show a local median about
1.56 times higher with candidate inference plus warm fallback than disabled
qualification. This includes QA guards/copies and is not a portable speed,
standalone inference cost or production deadline guarantee.

Two retained generated controls exercise both outcomes: a cross-lag tanh target
has neural held error near-55dB, while a Hammerstein target is approximated about
44dB better by the classical baseline and rejects the neural choice. Both neural
fits hit their300-iteration limit. These results establish implementation
mechanics on known equations, not physical hardware fidelity or a general
neural advantage. Evidence is in `audit/artifacts/ml-analog-hardware-model-20261025`.

`qa-analog-color-render` runs the actual shared `AnalogColorStage` over explicit
float32 little-endian interleaved PCM. It covers Harmonics, Static, Hammerstein,
Tape, Transformer and Console Preamp with complete model/drive/color/character/
wet-trim controls. Model selection and controls are set before preparation and
fresh reset. Process/reset heap guards cover cold callbacks. The renderer reports
actual controls, model identity, zero native latency, input/output geometry,
sample peak and QA-loop timing. Timing includes guards and output copies and is
not a production performance guarantee. It applies no gain matching, input/output
normalization, extra defects or limiting.

The native reference label0VU=-18dBFS is not a measured hardware calibration.
Recursive state support is not claimed finite. Explicit appended zeros observe
state rather than constitute a finite drain. Bounds are44.1..192kHz,1..32
channels,1..4096-frame callbacks/prepared maximum, at most4M samples including
up to65536 zero frames,64KiB requests and finite PCM within native input rails
of+/-16. Output remains within the native+/-128 safety rails. The wet-trim range
uses the narrower shared production parameter schema[-24,+24]dB.

Build the optional renderer:

```sh
cargo build -p sotf-plugin-analog-common --features qa \
  --bin qa-analog-color-render --release
```

The paired capture reader requires `format: "sotf-analog-hardware-capture-v1"`
and a fixed mono hardware signal path at44.1..192kHz. Its `context` contains:

- `data_origin`: `measured_hardware` or `synthetic_validation`. Synthetic fixtures
  remain labelled synthetic; reading them does not admit a hardware model.
- `hardware`: manufacturer, model, unit ID, configuration ID and actual control
  values. Controls and hardware stay fixed within this manifest.
- `calibration`: calibration ID and measurement reference; `reference` set to
  `full_scale_sine_rms_dbu`; measured `input_full_scale_dbu` and
  `output_full_scale_dbu`; source/load impedances in ohms; and a true
  `shared_clock` declaration. Full-scale sine refers to a digital sinusoid with
  peak1 and RMS1/sqrt(2). Arbitrary digital RMS therefore converts by
  `dBu = full_scale_sine_dbu + 20*log10(digital_rms*sqrt(2))`.
- `alignment`: true `paired_sample_aligned`, a measurement reference, maximum
  residual delay<=0.1 sample and maximum residual drift<=1ppm. Provide already
  aligned pairs; this reader performs no correlation-based alignment, fractional
  delay compensation or resampling.
- `rights`: license, attribution and reference, plus true `training_authorized`.
  These fields are declarations and require actual capture/provenance evidence.

`captures` contains1..64 records with unique IDs, declared `source_group`, split
(`train`, `validation` or `test`), frame count, exact raw mono PCM paths
(`dry_file`, `wet_file`) and lowercase SHA256 (`dry_sha256`, `wet_sha256`). Each
pair contains250ms..4M frames; the whole corpus is bounded to8M paired frames.
Records from one source group cannot cross splits. Identical dry PCM cannot be
declared as different source groups, and duplicate pairs are rejected. Distinct
crops or level variants of a recording must still share a source group.

Both files must match their declared byte counts and hashes, contain finite
samples with peak<=0.999 and have RMS>=1e-5. These are explicit reader limits,
not calibrated confidence or a universal hardware-quality criterion. The reader
preserves the exact PCM bytes and reports digital RMS/peak and calibrated dBu.
It does not fit gain, normalize the target, infer I/O calibration or claim to
verify physical hardware from the declarations. Dataset split counts sufficient
for model learning/admission remain a separate training requirement.

```sh
python scripts/ml/analog_hardware_capture.py \
  --manifest paired-hardware.json --output paired-hardware-summary.json
```

The CLI refuses to overwrite existing evidence. Successful output always states
`hardware_model_admitted: false`; it qualifies the data contract rather than a
trained model. `test_analog_hardware.py` contains an executable synthetic schema
example with deliberately fictional hardware identity and calibration.

Twenty-one Python tests cover the capture/native and model paths. The earlier
21 native tests and96-case procedural probe matrix are retained in
`audit/artifacts/ml-analog-hardware-native-20261025`; new model evidence is in
`audit/artifacts/ml-analog-hardware-model-20261025`. Public physical preamp
captures were found, but the I/O calibration and capture evidence needed by this
workflow were not established. No external audio was downloaded or used for
training. Linked source-code licensing is not treated as recording provenance.

Next work is explicit production learned activation/fallback, classical/native and
WDF/state-space comparisons as appropriate to the measured target, frozen
whole-source/operating-level validation, explicit opt-in and fallback, alias and
state behavior, production resource profiling and host/platform acceptance.
The existing six models remain available independently. The Analog common ML
proposal and the broader audit remain incomplete.
