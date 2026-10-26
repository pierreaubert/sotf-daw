# Matched classical/CREPE pitch and correction probes

Issue10/plugin36 and the pitch-assistance program remain in progress. This
checkpoint adds a bounded native comparison harness and independent generated
trajectory analysis. No model, confidence policy, correction behavior, production
parameter or default changes. The QA-only read-only snapshot exposes exact last
control decisions without relying on the throttled UI cache.

The16 cases cover offset,0.7Hz wow,6Hz flutter, intentional5Hz vibrato, a25-cent
step, weak fundamental with strong second/third harmonics, noisy wow and lone
octave. Two seed identities use220/440Hz references and different phase/noise.
Variants sharing a seed are correlated; this is two base frequencies and procedural
source families, not16 independent performances or held-out recorded material.
Each mono48kHz source is3s+17 frames, with a20ms edge fade. Wow/vibrato amplitudes
are20 cents, flutter5 cents and static offset15 cents. The noisy case adds Gaussian
noise with standard deviation0.03 to a0.2-amplitude carrier. Weak-fundamental
amplitudes are0.0001/0.2/0.1 at partials1/2/3. Lone octave has no supported latent
fundamental truth: its observed carrier is twice the supplied reference.

Raw comparison runs native `PndAnalyzer` and the actual pretrained `CrepeModel`
on identical source PCM. Their windows are2048 frames and3072 frames respectively;
the model runs on the same64ms window and10ms hop as its worker. Frequency error
is evaluated at each window's own midpoint, with separate confidence acceptance
denominators (classical0.5, CREPE0.8). This accommodates their different window
durations and clocks rather than treating late estimates as current-time truth.
Raw synchronous inference completes before the paced stage and its timing is
not counted as audio callback execution.

The paced native plugins consume the same128-frame callbacks, with the same
explicit reference, correction strength1,100ms drift smoothing,100ms analysis
configuration, confidence threshold0.5 and mono analysis. The assisted path adds
the existing worker policy: confidence0.8, maximum age150ms including the64ms
window, and maximum reference deviation100 cents. Classical-first ordering is
used for the220Hz seed, neural-first for440Hz; order/frequency/seed are confounded
and this is not a causal runtime comparison. An absolute source-frame release
grid paces the pair without waiting for inference. Reset/process/drain run under
heap guards; complete output includes bounded finite drain, with decision state
verified frozen at EOF.

The renderer is copied before the run and its identity checked afterward:
`24d488cc4029b126cacf06fe824893123481c49415689c017855e748e03e16c4`.
CREPE model bytes are copied and checked against the existing provenance manifest:
`57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3`.
The model/provenance/license remain those in `models/crepe-tiny`.

Independent analysis regenerates all quantized source/truth identities, verifies
80 raw input/audio/trace/request hashes, checks matrix coverage, callback clocks,
generation monotonicity and timing arithmetic, and reconstructs output pitch.
Its positive-frequency f64 FFT phase probe uses a cosine-tapered isolation band
and10ms phase average. Input calibration RMS error is below1.13 cents for every
case, meeting the fixed2-cent diagnostic gate. Clean calibration tests recover
known modulated trajectories within0.1 cent. A stronger-noise negative control
fails the gate. Output scoring aligns2047 frames, excludes the first/last0.5s,
and excludes±0.15s around the step for both probe calibration and output scoring;
step response is measured separately from control decisions. Weak/lone-octave
waveform probes use the observed second partial, without inventing absent
fundamental truth. Intentional vibrato is scored against its known uncorrected
modulation; wow/flutter/offset/step target zero drift.

Classical raw accepted pitch error is lower in all14 supported cases. Its RMS
error spans0.000096–1.04 cents, versus1.28–7.25 cents for CREPE on confidence-
accepted observations. Both have zero accepted octave errors in these supported
cases, using proximity within100 cents of a nonzero1200-cent multiple; gross
errors above600 cents are recorded separately. Classical confidence acceptance
is100% for supported cases. CREPE acceptance varies from2.5% (440Hz weak
fundamental, only5 settled raw observations) to100%, so low error is never
reported as a successful estimate when all observations are rejected.

Corrected-audio RMS error against the declared intent, in cents:

| Condition | Classical220Hz | Assisted220Hz | Classical440Hz | Assisted440Hz |
| --- | ---: | ---: | ---: | ---: |
|Offset|0.019|5.35|0.018|1.18|
|Wow|5.25|8.38|5.25|7.44|
|Flutter|3.06|7.40|3.06|3.79|
|Intentional vibrato|4.20|6.34|4.19|3.67|
|Step, excluding transition|0.87|6.13|0.87|1.40|
|Weak fundamental|5.25|6.03|5.25|5.19|
|Noisy wow|5.36|9.32|5.37|7.44|

Assistance regresses12 of14 supported waveform comparisons. Retained vibrato
amplitudes are17.07/17.07 cents for classical and19.22/20.17 for assisted, against
20 cents input. Amplitude alone does not establish preserved intent or perceived
quality; the220Hz assisted vibrato still has greater total trajectory error.
The440Hz weak-fundamental assisted path uses neural estimates in only6 of235
settled control decisions, consistent with low raw confidence acceptance. Other
supported cases select neural estimates in129–235 of235 decisions. These are
control decisions, not distinct completed inference jobs. Control counts exclude
the first0.5s and continue through source EOF; raw/audio accuracy additionally
excludes the final0.5s.

Both lone-octave cases have high-confidence raw CREPE estimates at twice the
reference, but the reference-deviation policy rejects them: zero neural selections
and complete assisted/classical output bytes are identical. Classical also refuses
that absent-reference pilot. These are policy negatives, not labelled octave
errors against an unobservable intended fundamental.

The native audio-latency declaration remains2047 frames (42.646ms at48kHz).
No additional audio buffering is introduced. First accepted neural control is
observed at74.67ms in13 supported cases and586.67ms in the440Hz weak-fundamental
case. For the25-cent step, control half-response is88ms for classical and66.67/
109.33ms for assisted220/440Hz. The half-response uses a fixed12.5-cent control
threshold and can be shifted by estimator bias; it is not an unbiased worker
delay estimate. No per-job timestamps/age are exposed by this snapshot, and no
worker freshness guarantee follows from repeated selected decisions.

Across18016 paired callbacks, combined classical+assisted execution has zero
period overruns; there are16 paced response misses including scheduling lateness.
Maximum per-case p99 callback execution is0.0814ms classical and0.0918ms assisted;
maximum observed assisted execution is0.1347ms. The worker's CPU/RSS and inference
latency are excluded from these callback durations. Compiler jobs had completed
before starting and no builds were requested during the matrix. OS scheduling,
affinity, power and other host activity are uncontrolled; this is not a device,
contended-host, cross-platform or worst-case deadline budget.

Seven native model/snapshot tests, strict targeted Clippy, the default library
build and four Python probe/denominator tests pass. The initial compile borrow
error, harness-owned heap-sentinel deallocation failure and overly pessimistic
noise-probe test assumption are retained in `initial-failures.json`; fixes change
neither production drain nor the comparison source/noise/probe thresholds.

Reports and source snapshots are in `audit/artifacts/ml-pnd-matched-20261026`;
raw source/audio/traces and the frozen renderer are in
`/Volumes/home_tmp/tmp/ml-pnd-matched-20261026`. This finite generated comparison
does not support adopting CREPE over the existing estimator. Remaining: broader
source/rate/SNR/polyphonic and labelled recorded acceptance, calibrated policy
improvement using development data and fresh final material, per-job age/resource
and real DAW/platform budgets, and native UI/application build coverage. The full
31-plugin/five-program ML audit remains active.
