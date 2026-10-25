# Dynamic EQ channel admission correction and synthetic shifts

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

The expanded native validation exposed aggregate-error masking. An independent
Stereo band receives exact silence on channel8 and emits zero there, while a
negative reference contains noise at RMS about0.00015. Both methods originally
admitted the target: aggregate validation relative RMS0.000538 and absolute
error below0.01 concealed a channel-relative error of1. Initial accepted
reports, original fitter, targeted failing regression and raw native evidence
are retained.

The fitter now additionally qualifies every channel. Its error RMS is normalized
by max(input RMS, reference RMS,1e-6). The same0.002 relative and0.01 absolute
limits apply. Reference energy handles legitimate Mid/Side cross-channel output
from a quiet input; the floor keeps silence finite. Existing aggregate checks
remain. The correction changes admission only; no optimizer, GP, candidate,
threshold/ratio/attack/release law or production DSP changed.

Both corrected negative runs refuse without a preset. Their complete search and
sensitivity traces are bit-identical to the earlier accepted runs. Eight new
integration/model tests pass, including this end-to-end native regression. The
new gate also refuses two ordinary routed cases that passed aggregate RMS:
classical Mid/Tilt96k training and surrogate Left/LowShelf48k validation.

Eight fixed synthetic cases compare both methods under the same160-proposal
allowance. Physical rates are44.1/48/96/192kHz; channel counts1/2/6/8; band
counts1/2/8; all filter shapes and placements occur. Cases include custom
six-channel pairs and an eight-band routed stereo preset. Each method admits
four cases; they differ on Mid/Tilt (GP only) and Left/LowShelf (classical only).
Both admit192k mono, six-channel custom pairs and eight-band stereo; both refuse
Side/HighShelf, silent-channel corruption and unexcited Side. Eight-channel data
is a negative control, not a positive eight-channel-quality result. Models are
not tuned from these outcomes and no superiority/adoption claim follows.

Independent channel analysis reconstructs admission from source/output hashes,
raw qualification/reopening audio, separately computed double-precision channel
metrics, original aggregate gates and observed sensitivity. It audits the12
prior mono runs,2 earlier invalid admissions and16 corrected shift runs. Only
the earlier two invalid admissions change; all12 prior decisions stay the same.
It verifies180 raw native render outputs and90 qualification clips. Optimizer
algebra was previously independently verified; this audit targets qualification
rather than repeating every search render. Requests/audio remain external under
`/Volumes/home_tmp/tmp/ml-dynamic-eq-shifts-20261025`.

Full reports, targets and separate synthetic truth records are preserved. The
corruption negative starts at true dynamics intentionally to isolate admission;
it is not a parameter-discovery test. Whole-source seeds are20621026+1000*case
index, with+0/+1 train and+2 validation. Duration is floor(rate/4)+1 frames;
physical frequencies, time controls and seeded level slots are declared in
`dynamic_eq_shift_cases.py`. No audio, weights or personal data are committed.

Remaining requirements include structural/per-band learned proposals, real
paired source data, broader solvers/uncertainty calibration, listener/host/UI,
resource/deadline and cross-platform validation. The full ML audit stays open.
