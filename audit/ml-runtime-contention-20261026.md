# Paced native model/host contention baseline

Issue10, program candidate Neural runtime choice. This implements a reproducible
native SOTF workload harness with actual model workers, parallel tracks, a
shared limiter bus, raw timing traces and worker-serving telemetry. It is an
initial runtime baseline, not a selected runtime winner or full-program closure.

Each stereo track contains PND, one native EQ band and Speech Denoiser. Speech
uses either the existing native RNNoise backend or the matching DeepFilterNet2
runtime; PND uses either classical analysis or its actual CREPE-tiny assistant.
The four combinations are crossed with1/4 tracks and0/2 bounded CPU load workers,
giving16 cases. Four DeepFilter+CREPE tracks create8 asynchronous model workers. The case order is
shuffled using a recorded fixed seed. Models load/hash-validate before the timed
stream; every case starts in its own process and uses an immutable executable
copy. Source is a generated220Hz harmonic pilot with per-track phase offsets,
not recorded speech, diverse music or a model-quality benchmark.

The host routes latency-bearing plugins through its existing graph scheduler;
the benchmark checks and records that actual route for tracks and limiter bus.
An initial compiled-linear assumption failed during smoke testing and was
corrected after inspecting host eligibility. No host optimization or fallback
change is made to clear the benchmark. The pipeline includes actual plugin
initialization, control caches, graph buffers, asynchronous workers and shared
bus processing. It does not include a physical audio device or engine playback
callback scheduling. Track DSP runs serially within the callback while inference
workers run concurrently; parallel track execution is not measured here.

At48kHz with128-frame callbacks, each case has1s warmup followed by2s measured
audio:750 measured callbacks with a2.6667ms period. The absolute release grid is
derived from source-frame position. The main thread sleeps until release, runs
all track hosts, mixes the prepared buffers and processes the limiter bus. Raw
traces preserve callback execution time, release/start lateness and total response
time. Execution overruns and response-deadline misses are separate: a quick
callback can still miss its release-relative deadline after late scheduling.
There is no dropped-callback catch-up policy hidden in the harness.

All source buffers, trace capacity, host plans and mix buffers are prepared
before pacing. Actual callback processing is heap-guarded. The source is generated
before timing, and telemetry snapshots/JSON/report writing happen outside the
timed callback. Telemetry-reading overhead can still affect later wakeups on this
normal application thread. CPU contention uses fixed-count busy workers with
bounded cancellation/join behavior. No agent compiler jobs run during a matrix.
Other machine activity, power state, affinity and device QoS are uncontrolled.

DeepFilterNet uses its declared4-packet playout budget; packet deltas distinguish
timely model output, deterministic fallback, dropped input and worker errors.
CREPE uses the existing0.8 confidence,150ms age and100-cent reference guards.
The benchmark counts snapshots whose last decision selected neural analysis;
repeated snapshots are not distinct inference completions or utilization.
Passing audio deadlines while models never serve is explicitly classified as
insufficient workload coverage. Worker heap/RSS and detailed job-time accounting
are not measured by audio-thread heap guards.

The first preserved matrix used speech→EQ→PND. DeepFilterNet's steady-state
suppression of the harmonic pilot left all combined profiles with zero measured
neural-pitch selection, despite timely DeepFilter packets. Those cases cannot
establish active combined-model capacity. The original binary, source, raw
reports and analysis remain under the speech-first checkpoint and external
scratch directory. They are not discarded because callback deadlines passed.

The second explicit profile uses PND→EQ→speech, so neural analysis sees the
unmodified pilot before suppression. Both orders are represented by a required
request field; old requests are not silently reinterpreted. The model weights,
confidence thresholds and playout budget remain unchanged. This is a workload
correction, not held-out model tuning. The same16-case matrix is rerun with all
outcomes retained.

Four deterministic analyzer tests verify actual trace arithmetic, reject hidden
deadline misses/wrong units/counter regression, and ensure timely audio with no
served models cannot be reported as runtime success. Independent analysis
recomputes quantiles and every deadline flag from raw traces.

The second matrix observes every requested model serving in all16 cases. In
each neural-pitch profile, every measured callback snapshot reports a neural
selection on every track; this still does not count distinct inference jobs.
DeepFilter timely fractions are99.5–100% over measured scheduled packets.
All12000 measured callbacks and6000 warmup callbacks pass audio-thread heap
guards. The measured total is34 execution overruns and827 response-deadline
misses across heterogeneous cases; it is not a single-workload failure rate.

Selected four-track rows from this single matrix, each with750 measured
callbacks and a2.6667ms budget:

| Speech / pitch analysis | CPU workers | Callback p99 / maximum ms | Execution overruns | Response deadline misses |
| --- | ---: | ---: | ---: | ---: |
| RNNoise / classical | 0 | 0.677 / 0.701 | 0 | 0 |
| RNNoise / classical | 2 | 0.757 / 2.696 | 1 | 25 |
| RNNoise / CREPE | 0 | 4.962 / 11.266 | 15 | 138 |
| RNNoise / CREPE | 2 | 1.786 / 10.064 | 6 | 86 |
| DeepFilter / classical | 0 | 0.403 / 0.420 | 0 | 0 |
| DeepFilter / classical | 2 | 2.410 / 13.728 | 7 | 131 |
| DeepFilter / CREPE | 0 | 1.118 / 5.488 | 2 | 99 |
| DeepFilter / CREPE | 2 | 0.493 / 1.384 | 0 | 19 |

Clock measurements are wall times, including preemption during processing.
Some loaded cases are faster than unloaded cases; uncontrolled scheduling,
power/thermal state and run-to-run variation prevent attributing that difference
to a backend or to helpful contention. RNNoise executes inference within its
audio path while DeepFilter/CREPE workers execute asynchronously. Callback cost
alone is therefore not total neural CPU cost. The models also have different
quality/latency semantics. No performance or sound-quality winner is selected.

This host is Darwin27/arm64 with14 reported logical CPUs. The first and second
matrices, smoke failures, source snapshot, immutable binaries, raw requests,
traces and analyses are preserved. Strict targeted Clippy passes with both real
model features enabled. The first compile used the wrong limiter adapter and a
smoke run rejected the incorrect compiled-linear assumption; corrected results
are retained alongside those failures. All artifacts are under
`audit/artifacts/ml-runtime-contention-20261026`; external raw cases/binaries are
under `/Volumes/home_tmp/tmp/ml-runtime-contention-{matrix,pitch-first}-20261026`.

Remaining: longer repeated sessions and varied source material, same-network
comparisons between alternative runtimes with numerical parity, GPU competition,
complete worker/memory accounting, actual device/DAW callback scheduling and
cross-platform hosts. Different model/backend combinations do not establish a
runtime speed winner or comparable sound quality. The full31-plugin/five-program
ML audit remains in progress.
