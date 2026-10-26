# XTC measured-transfer fitting: offline proposal

Issue10/plugin44. This checkpoint implements an off-thread learned horizontal
HRIR model and gain-bounded inverse-filter proposal. It does not add inference to
the audio callback, replace native XTC filters or enable a production selector.

The calibration source is the checked-in normal-pinna KEMAR SOFA file, SHA256
`e7035994f5fd754058424c061380ee92b1d5ed58fccef2887a4266916616acdf`.
Bill Gardner and Keith Martin measured this mannequin at MIT Media Laboratory.
The authors permit research and commercial use with attribution; the file and
model retain that attribution. See the [official author terms](https://sound.media.mit.edu/resources/KEMAR.html)
and [SOFA distribution](https://sofacoustics.org/data/database/mit/).
This is one measured mannequin, not independent human calibration or listening
data. The model uses horizontal measurements at the fixed1.4m acquisition radius
and44.1kHz. It cannot infer radial/elevation response or listener position.

The72 measured horizontal directions are split by their5-degree-grid index:
18 fitting anchors at index0 modulo4,18 validation directions at index1 modulo4,
and36 final directions at indices2/3 modulo4. All original SOFA indices are
recorded. These are disjoint directions of one correlated measured head, not
independent subjects. No interpolated reference is called a measurement.

Gaussian kernel ridge regression maps circular azimuth coordinates to both
512-sample HRIRs. Nine pre-specified angular-scale/ridge pairs are compared using
validation relative squared HRIR error. The chosen pair is10 degrees/0.1, with
validation error0.2263. The serialized model is frozen before final direction or
XTC-pose evaluation. It is bounded to128 anchors and8MiB, verifies its hash,
policy, coefficient domains, measured provenance and false admission on load,
and rejects unsupported elevation/radius requests. This is an explicit offline
model; rejection leaves existing native XTC unchanged.

Model SHA256:
`157b1f395717fcebb669d57e61450214823695f520dcef83545d35ce7c161248`.
The final mean relative squared HRIR error is0.3765, with worst direction0.9229.
Kernel smoothing of complex/temporal responses does not guarantee preserved
high-frequency phase or cancellation steering.

At each FFT2048 bin, the proposed inverse uses Tikhonov regularization with
beta0.001 times mean squared singular value, then one common real matrix scale
caps the largest inverse singular value at4. Every speaker-row L2 norm is
therefore also at most4. The shared scale preserves steering ratios within both
columns. This is a separate offline proposal bound, not the native XTC filter
preparation algorithm or its historical column knees. A frequency-domain operator
bound does not prove waveform true-peak safety or causal realizability.

Evaluation uses36 held-out pose pairs with fixed world speakers at±30 degrees.
Both measured directions in every pair belong to the final split. Positive yaw
turns right; SOFA azimuth is positive left, so lookup angles are30+yaw and-30+yaw.
The learned inverse, nearest-fitting-direction inverse and measured-oracle
inverse are evaluated against the actual measured four-path matrix. Nearest
lookup ties choose the first stored fitting direction. The oracle is a diagnostic
comparison at the same regularization/gain bound, not an attainable learned
reference or a globally optimal inverse.

Seven exact FFT bins near108/258/495/991/2003/4005/8010Hz and both intended-ear
cues give504 metric rows per method:

| Method | Median normalized separation dB | Worst separation dB | Worst intended gain dB | Negative separation rows |
| --- | ---: | ---: | ---: | ---: |
| Learned | 29.56 | -22.26 | -28.61 | 29 |
| Nearest fitting measurement | 16.98 | -35.79 | -30.77 | 60 |
| Measured oracle | 49.49 | -0.93 | -27.49 | 1 |

The learned method improves median separation at most selected frequencies, but
the approximately8kHz median is9.83dB versus14.43dB for nearest lookup. Its raw
crosstalk attenuation is negative in38 rows, reaching-27.20dB. Intended gain,
normalized separation and raw attenuation are retained together, including signed,
infinite and undefined outcomes. Muting is not counted as successful separation.
Severe desired-gain loss and negative separation prevent admission despite the
inverse gain bound. No final-case threshold or model tuning is performed.

Six tests cover circular seam/anchor fitting, unsupported geometry, explicit
ear/speaker delay ownership, inverse agreement with independent linear solves,
common gain-cap steering, rank deficiency, strict loading and mute/degradation
diagnostics. Independent analysis verifies disjoint source indices, every final
measured source position, the centered inverse using normal-equation/eigenvalue
arithmetic, and centered cancellation using a direct DFT of measured HRIRs.
Replaying the fit reproduces model and proposal bytes exactly; the original
report is retained alongside the numerically stable final diagnostic report.

The proposal JSON contains all1025 frequency-bin matrices with explicit ear/input
ownership conventions, clock and model hash. It is not imported into native XTC.
Remaining: improve phase/transfer fitting on development data and evaluate new
independent final material; native prepared-resource integration with bounded
transactional fallback; causal waveform/latency and measured mismatch validation;
gain/true-peak, memory, worker/contention and cross-platform DAW budgets; actual
human calibration, controlled listening and product review/activation semantics.
The entire31-plugin/five-program ML audit remains in progress.
