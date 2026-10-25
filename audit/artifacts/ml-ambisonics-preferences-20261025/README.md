# Offline learned Ambisonics layout preferences

Issue: http://192.168.1.32:3001/pierre/sotf-daw/issues/10

This checkpoint implements categorical listener preference learning over native
speaker gain corrections. It reuses the shared regularized Bradley-Terry model,
presentation-side term, equal source weight,200 whole-source bootstraps and
leave-one-source-out prediction. Selection requires the existing fixed admission
policy. Only explicitly auditioned gain vectors can be selected. Native decoder
construction, harmonic conventions, realtime processing and latency are intact;
no default model, automatic apply path or realtime inference is introduced.

The fitter requires explicit listener/setup/room/calibration/session/decoder
context, ACN/SN3D input, native binary and programme hashes, whole-source
grouping, blinded/randomized/fixed-position protocol declarations and actual
per-candidate audition trims. Native audio qualification requires normalized
diffuse electrical energy in each decoder band and their cross term, bounded
positive gains, unity LFE correction, sample headroom, adequate excitation,
RMS-level matching and distinct candidate audio. These protocol declarations
remain self-reported, RMS matching is not perceived-loudness matching, and the
electrical energy model does not establish real-room or spatial quality.

The renderer's QA-only combined sample cap increases from4M to16M to accommodate
250ms of order7/64-speaker audio at192kHz plus observation zeros. This changes no
production DSP. The fitter adds aggregate32M source and128M combined render
sample budgets,1MiB session JSON, at most32 programmes,12 candidates and4096
trials. Reads and native requests remain bounded. Recursive dual-band tail is
unknown;4096 explicit observation zeros are processed input, not a finite drain.

Accepted output is a reviewed two-node JSON proposal: unchanged Ambisonics
decoder followed by the native diagonal Matrix. The chosen controls are reopened
at17-frame callbacks versus193-frame auditions and reproduce exact PCM. No preset
is emitted on abstention. Audition trims are kept in a separate setup record.
`model.json` records utilities, exact context, candidates, source-session identity,
data origin and admission result. Its synthetic fixture is1827 bytes with SHA256
`b25c1577190440a423f82046aaed30fc54aaef58df2143fd0a6467cd6d94d441`.
It is not a pretrained/default model or real listener evidence.

All14 combined Python tests pass:7 native/energy tests and7 preference tests.
They include high-rate/order/output capacity, source-equal weighting despite
repeated trials, insufficient sources, consistent and ambiguous observations,
CLI accepted/abstained exports, hashes/context/protocol/group/schema/size bounds,
energy and level mismatch, raw output clipping despite audition attenuation,
and unexcited planar layouts whose distinct controls produce identical audio.
All77 existing native tests and strict all-target Ambisonics Clippy pass. A new
production-factory test consumes the actual emitted preset bytes, reconstructs
both nodes, and compares193-frame processing with a fresh unchanged decoder
followed by explicit diagonal multiplication, including silent LFE and zeros.
That test passes. The fixture bytes match the final consistent-case export.

The retained evaluation has seven simulated observation conditions over the same
eight generated source groups and three normalized gain candidates. Consistent
choices select candidate1; baseline-preferring choices select unity candidate0.
Ties, source heterogeneity, side bias, level mismatch and energy mismatch abstain.
These are seven conditional probes, not seven independently collected listening
datasets. Audio is generated noise/tone material; preferences are predetermined.
There is no claimed learned quality improvement versus the classical decoder.

Independent BFGS reconstruction checks all1400 bootstrap fits and56 held-source
fits across those seven reports. Maximum utility error is1.6414678106e-7,
bootstrap error1.6562154004e-7 and held-source log-loss error8.4190764776e-9.
Direct PCM/hash accounting independently verifies168 native auditions, with zero
RMS/peak discrepancy. Covariance/trace calculations reconstruct energy reports
within1.1277658711e-16. Accepted callback reopen outputs are bit-identical.
The initial sequential synthetic trial order is retained; final trial order is
seeded and shuffled. No model/admission thresholds were changed for evaluation.

Commands, exit codes, timings, analysis and source/binary/model hashes are
adjacent. Final generated source PCM, native output, sessions and reports remain
under `/Volumes/home_tmp/tmp/ml-ambisonics-preferences-20261025-final`.
No external recordings, listener observations or weights were downloaded or
redistributed. Production decoder behavior and classical fallback remain usable
without this offline Python workflow.

The Ambisonics proposal remains in progress: controlled listener preference and
spatial-quality acceptance, independent source/level/rate/layout shifts,
uncertainty calibration, full host/resource/wide-output admission and
Linux/Windows DAW/CI validation are missing. The user will provide recordings
and hosts later. This checkpoint does not complete the broader ML audit.
