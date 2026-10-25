# Identified hardware coloration: native baseline and capture contract

The audit permits learned coloration only with identified hardware and operating
levels. This checkpoint provides the comparison renderer and paired-data reader;
it does not train or admit a hardware model or replace any native coloration.
There are no pretrained weights, default learned models or realtime inference.

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

Nine Python tests,21 existing native tests and a96-case procedural native probe
matrix verify the current tools. Details and hashes are in
`audit/artifacts/ml-analog-hardware-native-20261025`. Public physical preamp
captures were found, but the I/O calibration and capture evidence needed by this
workflow were not established. No external audio was downloaded or used for
training. Linked source-code licensing is not treated as recording provenance.

Next work is a target-specific learned coloration model, classical/native and
WDF/state-space/Hammerstein comparisons as appropriate to that target, frozen
whole-source/operating-level validation, explicit opt-in and fallback, alias and
state behavior, actual native inference parity/cost and host/platform acceptance.
The existing six models remain available independently. The Analog common ML
proposal and the broader audit remain incomplete.
