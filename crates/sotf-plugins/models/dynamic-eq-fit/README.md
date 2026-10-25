# Offline Dynamic EQ setting proposals

`fit_dynamic_eq.py` fits four shared native dynamics controls to explicit paired
input/reference recordings. The opt-in `surrogate` method learns a per-session
Gaussian-process objective model; the default `classical` comparator uses a
Sobol design followed by bounded Nelder-Mead. There are no pretrained weights.
Both methods score actual native audio, retain native runtime modulation and
export a manual preset only after qualification. Active playback is unchanged.

This initial fitter learns global threshold, ratio, attack and release. The
user's band structures, Q/gain/shape/slope/placement, active/solo, knee, mix,
channel linking and stereo pairs remain explicit and fixed. Per-band threshold
or ratio overrides are refused; fitted global values are copied into every band
so the native constructor's override heuristic cannot mask them. Proposing those
structural and independent per-band settings remains open in the audit ledger.

## Input and identity

Use `sotf-dynamic-eq-paired-target-v1`, declare `data_origin` as `recorded_pairs`
or `synthetic_validation`, and use alignment convention
`sample_aligned_zero_latency_fresh_state`. No delay search, trimming, resampling,
normalization or EOF padding is inferred. References must match the supplied
input length and fresh-state processing convention. They can be imperfect;
a poor match causes refusal rather than silent alignment changes.

The target records sample rate, channel count, complete native `template`, and
3..32 paired clips. At least two whole-source training groups and one disjoint
validation group are required. Each clip has unique `id`, role `train` or
`validation`, `frames`, input/reference raw float32 little-endian file paths and
SHA256 identities. Input hashes cannot be reused under different IDs. Group all
related recordings and their gain/EQ/noise variants together before producing
the target; different hashes alone do not prove source independence. Context
and alignment declarations remain the caller's responsibility.

PCM is bounded to +/-32, at least 250 ms per clip, 1..8 channels, 44.1..192 kHz,
and 4 million aggregate input samples. References have matching geometry. JSON
is at most 2 MiB. Native renderer input/output bounds also apply. Inputs and
references are read into verified byte snapshots; the renderer identity is
checked before and after fitting. Files/executable must remain unchanged during
a run. Input bounds do not establish total RSS or realtime resource deadlines.

A complete synthetic schema is retained in
`audit/artifacts/ml-dynamic-eq-fit-20261025/targets/near.json`. Its paths point
to generated external fixtures, and it is not recorded listener/hardware data.
`dynamic_eq_fit_cases.py` regenerates those examples without placing hidden
control values in the fitter target (except the explicitly named known-control
wiring experiment).

## Learned search and comparison

Controls use normalized coordinates over threshold -60..0 dB, ratio 1..20,
attack 0.1..100 ms and release 10..1000 ms. Ratio/time coordinates are logarithmic;
all decoded controls are native float32 values. The objective averages squared
relative RMS error with equal weight per training source. Relative RMS divides
waveform-error RMS by input RMS. Validation waveforms never enter the objective,
GP observations or candidate selection. A regression changes validation audio
and confirms the complete search remains identical.

The fixed recipe has initial template plus seven scrambled Sobol points. At the
default 160-proposal allowance, 32 global proposals precede local refinement.
Classical search uses additional Sobol points. The GP uses Matérn-5/2 length
scale 0.25, diagonal jitter 1e-6 and normalized log10 loss with scale floor 0.1.
Expected improvement chooses among 256 fixed Sobol and 256 local normal queries
(scale 0.12) per step, removing previously observed float32 controls. Seed is
20581026. Remaining allowance goes to bounded Nelder-Mead with a normalized
0.08 simplex, x tolerance 1e-5 and loss tolerance 1e-10.

The CLI permits 40..256 proposals. Duplicate float32 controls reuse an observed
loss. Converged refinement can finish early; actual unique proposal and native
render counts are reported. The comparison uses the same **allowance**, not a
claim of identical runtime or identical counts after early convergence. Each
objective renders every training clip. Up to eight additional coordinate probes
and train/validation qualification plus reopening renders are separately counted.
All raw requests/audio/metadata and search observations are retained for audit.

## Admission and export

Every train/validation clip must have relative RMS <=0.002 and maximum absolute
error <=0.01. Compared with dry input, relative RMS must improve by at least
0.001 and by at least 50%. Selected native float32 parameters are reopened at
callback size 17 versus the scoring callback 193, with maximum difference <=1e-7.
No full IIR tail, listening match or hardware-emulation claim follows from these
finite paired waveform checks.

Training-only sensitivity probes shift each normalized coordinate by +/-0.1
where a shift of at least 0.099 fits the bounds. Search/probe candidates at
max-coordinate distance >=0.099 whose training relative RMS is within 0.0005
of the selected result cause refusal for observed ambiguity. This screens only
observed alternatives, not coupled directions or all possible parameter sets.
It is not a global-identifiability proof or calibrated uncertainty estimate.
Admission thresholds need separate development and final validation.

A successful run writes `report.json` and a complete native `preset.json`.
Abstention writes reasons with root parameters null and no preset. Failures and
existing-output directories never replace user data. Raw search candidates in a
report are diagnostic evidence; they are not accepted presets.

```sh
cargo build --release -p sotf-plugin-dynamic-eq --features qa --bin qa-dynamic-eq-render
python scripts/ml/fit_dynamic_eq.py --target paired.json --renderer /absolute/path/qa-dynamic-eq-render --output new-result --method surrogate --budget 160
```

Use `--method classical` for the default comparator. NumPy and SciPy are needed;
no neural runtime or model download is added. The exported decoder controls keep
the existing deterministic gain-envelope and filter bounds. This is an offline
manual workflow with no automatic host/UI application.

## Development evidence and limitations

Six fresh synthetic cases compare both methods: near, harder, two-band,
misaligned, dry, and known-control. Both qualify near/two-band and refuse the
harder, misaligned and dry cases. The known-control test starts at truth and
checks export wiring; it is not parameter discovery. On near validation relative
RMS, GP improves 0.0016097 to 0.000004828; on two-band it improves 0.0003822 to
0.0002750. On the harder case it worsens 0.0044458 to 0.0067750, and both abstain.
An earlier separate smoke target also failed validation despite low training
loss. These results do not establish general GP superiority; it remains opt-in.

Independent analysis verifies 3482 raw native renders, reconstructs 144 GP
acquisition decisions and all 12 admission decisions. Maximum objective
reconstruction discrepancy is 1.12e-16, metric discrepancy 5.56e-17, and preset
reopening difference zero. Seven tests cover GP algebra, bounds, export/refusal,
identity, validation isolation and CLI output preservation. A separate CLI run
successfully fits/exports the unknown-control near target.

All experiments use synthetic native references, 48 kHz mono and one/two fixed
band structures. This does not prove behavior across the renderer's wider
geometry, real recordings, structural proposals, independent per-band dynamics,
continuous transport, listeners or host/resource/cross-platform conditions.
These requirements remain open. Raw evidence is external under
`/Volumes/home_tmp/tmp/ml-dynamic-eq-fit-20261025`; no audio or pretrained weights
are redistributed.
