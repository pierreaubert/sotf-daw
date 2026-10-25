# Optional offline Ambisonics layout preferences

This fitter learns categorical preferences over explicitly auditioned speaker
gain corrections. It leaves the native regularized/layout-aware decoder intact.
It does not generate decoder matrices, replace spherical harmonics, add realtime
inference or apply a setting automatically. HRTF selection is handled separately
by the Binaural preference workflow; this proposal uses the layout alternative.

The Bradley-Terry model uses one utility per candidate plus presentation-side
bias. Whole-source groups have equal total weight. A selection needs at least
eight groups,32 trials, both comparison orders, connected comparison graphs,
stable200-bootstrap winner/margin, held-source prediction advantage over uniform
choices and limited side bias. Thresholds come from the existing shared policy,
not tuning on these Ambisonics probes. Bootstrap stability is a heuristic rather
than calibrated population confidence. Ties use half labels. If evidence or
native qualification is inadequate, the fitter exports no preset and reports the
unchanged decoder baseline as fallback.

`model.synthetic.json` and `preset.synthetic.json` are functional fixtures made
from predetermined simulated choices, not real listener data or default models.
Their provenance and exact model hash are in `manifest.json`. No external data
or weights were downloaded. A real fit requires the user's own observations.

Build the QA renderer and run the fitter using NumPy and SciPy:

```sh
cargo build -p sotf-plugin-ambisonics --features qa --bin qa-ambisonics-render --release
python scripts/ml/fit_ambisonics_preferences.py \
  --session listener-session.json \
  --renderer /absolute/path/to/qa-ambisonics-render \
  --output reviewed-layout-proposal
```

The output directory must be new. The CLI returns0 for an admitted selection,
2 for abstention, and a failure for invalid input or native execution errors.
No active preset is modified. Accepted output contains a two-node `preset.json`
(Ambisonics decoder, then Matrix), `model.json`, `report.json`, and separate
`audition_setup.json`. Review the proposal before loading its nodes. Per-programme
audition trims are recorded separately and are not applied by the preset.

A session has `format: "sotf-ambisonics-layout-preferences-v1"`, explicit
`data_origin` (`listener_observations` or `synthetic_validation`), and:

- `context`: listener, playback setup, room, calibration and session identifiers;
  criterion `overall_preference`; normalization `ACN/SN3D`; sample rate;
  renderer SHA256; and complete native `decoder_parameters` (`order`,
  `target_layout`, `max_re_weighting`, `dual_band`, `algorithm`, plus full
  `custom_layout` geometry when selected). The same context applies to all trials.
- `protocol`: true declarations for blinded auditions, randomized order,
  whole-source grouping and fixed listener position; maximum RMS-level mismatch
  between0 and0.25dB. These declarations and source grouping are self-reported.
- `candidates`:2..12 unique IDs, each with `parameters.speaker_gains`, ordered
  exactly like the decoder's output speakers. `baseline_id` must identify unity
  gains, preserving the unchanged decoder. All LFE gains remain1.
- `programmes`:1..32 unique whole-source IDs and PCM hashes, explicit file paths,
  frame counts, and `post_gains_db` for every candidate's actual audition trim.
  PCM is interleaved float32 little-endian ACN/SN3D with complete HOA channels,
  finite samples within+/-32 and at least250ms of source audio. Different crops or
  gain variants of one recording belong to one source group rather than serving
  as independent evidence.
- `trials`:1..4096 unique IDs, programme ID, candidate IDs `a` and `b`, and
  `choice` (`a`, `b` or `tie`). Every declared programme must have trials.

The generator in `scripts/ml/ambisonics_preference_cases.py` supplies an executable
synthetic schema example. Its labels intentionally test fit/export mechanics;
they are not evidence that any gain vector sounds better.

Prepare gain candidates offline with `ambisonics_native.normalize_gains`, using
the native exported decoder matrix and, for dual-band decoding, its basic matrix
too. It preserves uniform-direction SN3D electrical energy for each band and
the LF/HF cross term. A fresh native audition must then qualify each exact gain
vector; the fitter does not renormalize recorded candidates. Gains must stay
within[0.8,1.25], with energy error no more than2e-6 after float32 conversion.
This does not preserve every direction's energy or establish calibrated acoustic
power, equal perceived loudness, localization or preference.

All auditions pass through the real decoder and real Matrix plugin. Raw and
trimmed sample peaks must stay at or below0.999, measured programme RMS levels
must match within the declared bound, and every candidate pair must produce
distinct audio somewhere in the session. Low output energy, silent/ineffective
layouts, energy mismatch and clipping risk abstain. The renderer performs no
limiting or normalization. Sample-peak admission is not a true-peak guarantee.
Appended4096 zero frames observe recursive state; they do not establish a finite
dual-band tail. Both native nodes report0 host latency. Accepted controls are
serialized and reopened at a different callback size, with output error<=1e-7.

Sessions are capped at1MiB JSON,32million aggregate source samples and128million
combined render samples across candidates. Each native render is capped at
16million combined input/output samples, including observation zeros. This
supports250ms high-rate order7/64-speaker auditions while bounding offline work.
Only two output arrays are loaded for a pairwise distinctness comparison.
The actual production factory reconstructs the emitted synthetic preset in its
regression test. Engine admission above16 outputs and DAW/platform integration
remain separate acceptance work.

Evidence is in `audit/artifacts/ml-ambisonics-preferences-20261025`. Controlled
real listener observations, spatial/listening quality versus the unchanged
decoder, independent source/level/rate/layout shifts, calibrated uncertainty,
resource and host/platform acceptance remain outstanding. No general quality
advantage or production-readiness claim follows from these synthetic tests.
