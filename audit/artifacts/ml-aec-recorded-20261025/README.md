# AEC recorded-source quality gate: rejected for general adoption

The model and hybrid policy from `2d59bd25` were frozen before inference. Seventy-two
12-second mixtures combine three existing recorded English excerpts (four unique
seconds per speaker, repeated three times), three rates (16/44.1/48 kHz), three
far/near speaker pairings and eight conditions. Paths are simulated, not measured
physical-device captures. Original MiniLibriSpeech fixture attribution/license is
in the speech-denoiser fixture directory and embedded source manifest. Sources
previously served other plugin tests but never synthetic-only AEC training.
No model weights or hybrid policy changed following these results.

`final/protocol.json` records source, renderer and evaluation script hashes before
inference, structural configuration, selection and measurement scope. The 72 case
records preserve native array hashes, counters and every metric. Derived float
arrays remain under `/Volumes/home_tmp/tmp/ml-aec-recorded-20261025`; they are not
committed. `evaluate.json` records the successful command and elapsed time.

`independent-quality-final.json` independently recomputes metrics and hashes with
SciPy full complex FFTs. It checks the native error spectrum against padded raw
adaptive output and reconstructs wet audio with maximum error 5.97e-8. Conditional
near-end estimates apply mixture-derived gains to clean near speech with those
decisions fixed; they exclude dependence of adaptive cancellation on near history.
The collector requires constant full wet mix, FFT 512, hop 256, leading zero
padding, and exactly compensated 256-sample latency. Coefficient capture is `qa`
only and changes no production audio calculation.

Results:

- All 72 pairs retain identical adaptive/error/features/targets hashes.
- All nine reference-silent near-only outputs equal classical output exactly.
- Learned cuts apply on 84,975 blocks. All 54 steady far-only intervals improve
  ERLE, with deltas from +0.0146 to +4.4951 dB.
- Near-active onset SNR worsens in 11/27 intervals, as much as 11.1808 dB.
- Sustained double-talk near-target SNR worsens in 16/18 intervals, as much as
  3.6303 dB. All 18 show additional conditional near-component distortion.
- Worst added conditional near-end projection loss is 5.4023 dB at onset and
  0.7867 dB during sustained double-talk.

The current near-end guard and training fail recorded-source speech preservation.
These measurements do not establish multilingual, listening or physical-device
acceptance. Keep defaults classical. Further training/protection development must
use separate development material and a new independent final evaluation rather
than tune against this frozen final set.

Checks passed: enabled AEC tests (79), inference-disabled QA tests (69), Python
analytic/regression tests (5), Python style, focused Rust formatting and strict
all-target Clippy in both QA feature configurations. Commands and durations are
in the named JSON records with matching text logs (trailing blank lines trimmed). The final exporter hash equals
the pre-evaluation protocol hash. `baseline-replay.json` confirms all 240 existing
arrays match the previous 24-case hybrid comparison, across 48 native runs.
`source-sha256.json` hashes current measurement/native/model/docs sources;
`artifact-sha256.json` hashes every other artifact in this directory.

Reproduction commands are in the AEC model README. Temporary array directories
must be retained for independent reanalysis, or regenerated with a new output
path. This checkpoint does not complete the AEC proposal or the full ML audit.
