# Matched CREPE runtime comparison

Issue10's neural-runtime candidate remains in progress. This checkpoint compares
the same frozen CREPE graph and input tensors across the existing native Tract
API, ONNX Runtime CPU, CoreML CPU-only and CoreML CPU/GPU-allowed configurations.
It adds a standalone off-thread QA benchmark and Python tooling. No production
runtime, worker, model, policy, audio path or default changes.

The native package is`sotf-tract-onnx0.22.4`, workspace alias`tract-onnx`, from
the`sotf-3rdparties` lock revision`9b7c2134cb5831391c7863f584cd631c63719fd5`.
Separate unprefixed Tract0.18.3-pre lock entries belong to another dependency
and are not this PND backend. ORT is1.23.2, NumPy2.5.4, Python3.12.10. Exact
lock/model/input/executable/source hashes and provider options are retained.
The existing model/provenance/license is unchanged (MIT upstream CREPE weights;
training overlap remains unknown).

Model SHA256:
`57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3`.
Normalized corpus SHA256:
`b187c69ba92f255921abca3cbb6f5f42955da7a96b4fe7984b4b262255cc23f8`.
Native executable SHA256:
`0beb86ddf5b3b5269d7d272553e3eb462500b16a1a41ef517fc662b798381b92`.

The32 fixed1024-sample frames include tone, weak-fundamental harmonics, vibrato,
noisy tone, onset, chirp and noise across four reference frequencies, plus zero,
impulse, ramp and alternating samples. Normalization uses f64 sample standard
deviation before f32 quantization. Every runtime reads identical little-endian
f32 bytes at model rate16kHz. These are activation-contract probes, not independent
recordings or pitch-quality acceptance. Zero input deliberately exercises network
activations; the normal worker's silence bypass is outside this test.

All four configurations run in three predeclared randomized fresh-process rounds.
Each process loads one graph, measures the first inference, executes five complete
warmup passes, captures a32-frame parity pass, then times20 passes:640 measured
calls per process and7680 across the12 timed processes. Three further ORT
processes collect placement profiles after every timing process has finished.
Profiling overhead is excluded from the timing matrix. No compiler jobs were
visible before the smoke/full measurement sequence, and no builds were requested
during measurement. OS scheduling, power, affinity, competing activity and caches
are uncontrolled. Fresh process does not mean cold filesystem/CoreML caches.

ORT uses sequential execution with intra/inter-op threads set to1. CoreML device
scheduling remains outside that ORT setting; native Tract's existing internal
thread policy is unchanged. The exact common ONNX bytes are optimized separately
by each backend. CoreML uses`NeuralNetwork` format and static shapes. Provider
requests explicitly include CPU fallback, but a missing CoreML registration is
an error rather than a mislabeled CPU benchmark. The separately profiled CPU
session reports759 CPU kernel events across33 calls; each CoreML session reports
33 CoreML delegate events. These fused event counts are not comparable operator
counts or proof of the physical GPU. `CPUAndGPU` permits CPU/GPU execution;
the actual device is not established. See the [ORT CoreML options](https://onnxruntime.ai/docs/execution-providers/CoreML-ExecutionProvider.html)
and [Apple compute-unit definition](https://developer.apple.com/documentation/coreml/mlcomputeunits/cpuandgpu).

Numerical gates are fixed before full measurement: maximum activation error2e−5,
zero reference-policy disagreements, and≤0.1-cent pitch difference when either
runtime accepts the decoded estimate. Decode uses the same nine-bin weighted
neighborhood and final f32 frequency rounding, with an independent Python
implementation and last-maximum tie convention. Confidence0.8,100-cent reference
range and supported-frequency gates are checked separately. Tiny activation
changes can still fail policy stability; unit tests explicitly protect that case.

All15 processes exit successfully. ORT CPU matches native activations within
7.451e−7 and CoreML CPU-only within5.961e−7 in every repeated and profiled session.
Both have identical accepted decoded frequencies and policy decisions on this
corpus. GPU-allowed CoreML differs by8.253e−4 in all three measured sessions and
its placement session: four preserved failures of the activation gate. Maximum
accepted decoded-pitch difference is0.00925 cent and observed policy decisions
still agree, which does not turn the stricter activation failure into a pass.
No threshold or model changes follow these observations.

Local measured costs; median column is the median of the three process medians,
p99 is the range of per-process empirical p99s, and RSS is the median of three
whole-process high-water measurements:

| Profile | Warm median, ms | Per-process p99 range, ms | Peak RSS, MiB | Activation gate |
| --- | ---: | ---: | ---: | --- |
|Native Tract|0.451|0.481–0.533|26.14|pass|
|ORT CPU|2.745|2.939–3.102|61.25|pass|
|CoreML CPU-only|0.251|0.316–0.341|78.45|pass|
|CoreML GPU-allowed|0.890|1.915–2.487|83.36|fail|

First-call ranges are0.435–0.469ms native,2.785–2.927ms ORT CPU,
0.685–0.763ms CoreML CPU-only and9.464–9.666ms GPU-allowed. Model-load ranges
are8.38–12.36ms,3.67–3.74ms,20.86–23.37ms and32.36–33.46ms respectively.
Ready-from-entry includes language imports/input preparation: native8.47–12.53ms,
ORT49.87–51.00ms, CoreML CPU-only66.40–69.64ms and GPU-allowed78.09–79.02ms.
It excludes pre-entry interpreter/loader startup; complete supervised process
durations are retained separately. These figures are operational wrapper costs,
not isolated kernels: native copies into a Tract tensor and validates input/output;
Python checks the input, constructs a feed view/dict and validates/copies output.
Native load also prepares its16kHz source preprocessing structures. Source-rate
resampling, per-source normalization, decoding, workers, queues and audio are
excluded from warm inference timing.

RSS comes from Darwin`/usr/bin/time -l` for the entire child, including language,
framework, model and temporary allocations. It is not model heap alone, nor full
system/helper/device memory. CoreML CPU-only's lower warm latency has larger
startup and process-memory costs in this setup. GPU-allowed is both slower than
that CPU-only profile here and fails the numerical contract, but actual GPU use
is unproven. No global runtime winner or production migration is selected from
these finite wrapper-level results.

Independent analysis verifies75 raw run-file hashes, three raw placement-profile
hashes, frozen executable/script/model/input identities, corpus regeneration,
complete trial/profile coverage, timing counts/arithmetic, requested/registered
provider labels and output shape/domain. It recomputes every parity result and
quantile. Five Python tests cover normalization, activation-versus-policy gates,
missing timing rows, noninteger units, impossible timing arithmetic and timeout
cleanup of owned processes. Native
build/strict targeted Clippy and Ruff pass. The initial fixed-chunk Clippy failure
and a test's f32 accumulation assumption are retained; fixes leave corpus bytes
and runtime tolerances unchanged. No callback or device test is newly claimed.
After successful measurement, the orchestrator's POSIX timeout cleanup was
hardened to terminate its own entire process group rather than only the external
`time` wrapper; no measured run timed out. Pre-run and final source are retained.

Evidence: `audit/artifacts/ml-crepe-runtime-20261026`, including actual activation
arrays, normalized corpus and provider profiles. Large raw/frozen process state
is retained in `/Volumes/home_tmp/tmp/ml-crepe-runtime-matrix-20261026`; the smoke
run is `/Volumes/home_tmp/tmp/ml-crepe-runtime-smoke-20261026`.
Remaining: native alternative-backend worker integration with preprocessing and
lifecycle parity, numerical-boundary/material validation, complete worker/system
memory/CPU and freshness accounting, repeated real-track/device contention,
verified GPU execution and GPU-load tests, platform builds and product selection.
The prior pitch-quality regressions are unchanged; accelerating a model does not
establish better source correction. The full31-plugin/five-program audit stays active.
