# Offline personal spectral-preference curves

This optional assistant learns a listener's preferred low/mid/high manual gains
versus explicitly supplied playback SPL. It proposes a native Manual-mode curve
at one requested level, with uncertainty and gain/response bounds. It does not
replace ISO compensation, measure loudness, infer acoustic calibration from PCM,
map digital volume to SPL, or run inference on the audio thread. The normative
manual/ISO/Auto implementation and defaults remain unchanged.

Run from the repository root with NumPy/SciPy:

```sh
cargo build -p sotf-plugin-loudness-compensation --bin qa-loudness-preference --features qa --release
python scripts/ml/fit_loudness_preferences.py --session preferences.json --level-db-spl 55 --renderer /path/to/qa-loudness-preference --output new-result-directory --work new-native-work-directory
```

Use new output/work paths. Accepted proposals write `report.json` and a raw
`LoudnessCompensationPluginParams` `preset.json` for a new matched audition.
Abstention writes only a report with `parameters: null` and exits2; keep existing
settings. Invalid input/calibration fails before output. Existing paths are never
overwritten. The CLI does not apply a preset or change playback volume.

The example `audit/artifacts/ml-loudness-preferences-20261025/consistent-session.json`
is explicitly **synthetic validation**, not real listening or SPL calibration.
Real observations must declare `data_origin: listener_observations` and identify
the listener, playback chain, listening position, session, presentation and sample
rate. Calibration supplies reference dBSPL, Z weighting, measurement ID/method,
confirmation and the basis `identity_reference_signal_at_listening_position`.
The measurement record must document stimulus/spectrum, averaging/duration,
transducer/microphone calibration and position. These inputs are self-reported;
the tool cannot prove the physical calibration or listening protocol occurred.
SPL is not phon, sone or LUFS. No PCM analysis is used to invent calibration.

Declare blinded/randomized auditions, an identity reference anchor, external
level matching (method and maximum mismatch≤0.25dB), and the fixed
`native_peak_normalized` policy. Preferred gains are relative to that identity
reference; zero at the reference is a defined anchor, not a claim that identity
is universally optimal. The exact zero mean/latent variance there is imposed,
not learned human certainty. External matching trims are not exported: the
proposal is a spectral shape, so re-match its level before judging it.

Fix the auditioned native filter shape (`low_freq`, `mid_freq`, `mid_q`,
`high_freq`) across observations. Each observation names its programme/ID,
identity-reference playback `level_db_spl`, three `preferred_gains_db` in order
[low,mid,high] and reported `uncertainty_db`. The same native five-biquad manual
family is used for export: two half-gain low shelves, one mid peak, two half-gain
high shelves. AutoGain is off and native peak normalization is explicit.
The exported mode is0, `auto_calibrated` false and `playback_volume_db`0 as an
inert Manual field; no digital0 calibration or Auto mapping is inferred.
`playback_level_db` is metadata in Manual mode, not a physical level command.

Required support: at least8 programme groups, each measured at3 or more
non-reference levels; at least4 programmes at each observed level. The model
accepts at most512 observations,128 programme-level cells,32 programmes and8
levels. Repeated trials within a cell are averaged without shrinking its noise
floor. Every programme has equal total likelihood weight. Use genuinely separate
recordings/masters for groups; relabelling correlated excerpts cannot create
independent evidence. Supported reference is60–100dBSPL and requested/native
playback40–90dBSPL; observations stay at/below the reference. Query outside the
observed-to-reference span abstains. Interpolation toward the exact anchor still
has prior-dependent uncertainty and may abstain.

A zero-mean RBF GP is conditioned on f(reference)=0. Fixed prior length10dB,
SD3dB and noise floor0.25dB were chosen before synthetic evaluation. Different
noise/priors can change predictions. The report combines conditional latent
variance with leave-one-programme-out error and200 whole-programme bootstrap
resamples. It rejects inconsistent repeats, held-programme RMSE>1dB,
uncertainty halfwidth>1.5dB, or any gain interval extending beyond±6dB. Bands
are admission heuristics, not calibrated human/population confidence or a
simultaneous three-output guarantee. GP latent variance alone ignores the observed
gain values; explicit inconsistency tests are essential.

Accepted mean gains are validated through the actual native stationary impulse:
2channels,131072samples per channel, zero reported latency, AutoGain off and
native peak normalization. A separate analog/bilinear five-section calculation
checks complex response and the normalization convention; tail decay is checked.
Dense20Hz–20kHz absolute response must remain≤12dB. Frequency-domain normalization
is not an instantaneous peak limiter or a guarantee of acoustic SPL/headroom.
Results are conditional on the recorded listener/chain/position/rate/level and
must be reviewed again when that context changes. Live volume-dependent user
curves and application import/review UI remain unfinished.

Synthetic validation:16 queries from8 fresh generated sessions, plus8 conflicting
or uncertain controls. All16 consistent queries passed the statistical gate;
all8 controls abstained. Median error against the known synthetic mean was
0.04381dB, versus0.09007 for an anchored linear baseline. The synthetic curve
family and correlated query pairs do not establish listener benefit, model
superiority or confidence calibration. Independent dense posterior solves,
120 bootstrap fits and native preset reopens agree. Tests also prove that
reference-only groups cannot satisfy support and repeats do not inflate evidence.

There are no pretrained weights. Per-session posterior coefficients/cells are
stored in the report. Source and committed synthetic fixtures are GPL-3.0-or-later;
no external audio, user observations or physical calibration data are distributed.
Primary context: [GPML](https://gaussianprocess.org/gpml/chapters/),
[McMullin2017](https://secure.aes.org/forum/pubs/conventions/?elib=19252), and
[Sherman/leCessie1997](https://doi.org/10.1080/03610919708813417). Kernel, weighting,
context restrictions and admission thresholds are implementation choices, not
validated by those sources. Real controlled listening, repeatability, uncertainty
calibration, broader material/contexts and host/UI acceptance remain open.
