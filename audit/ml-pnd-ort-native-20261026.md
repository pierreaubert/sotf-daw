# Native ORT pitch worker qualification

Issue10's runtime and pitch-assistance tracks remain in progress. This checkpoint
implements a QA-only native alternative using the local ONNX Runtime C library,
through pinned safe Rust bindings. It extends the existing CREPE model wrapper
and bounded worker rather than invoking a Python process. Tract remains the
default. Model bytes, source preprocessing, decoder, confidence/age/reference
policy and normal preset serialization are unchanged. No quality admission,
production runtime selector or latency/deadline advantage is claimed.

The optional`neural-pitch-ort` feature includes the existing neural/QA features
and`ort=2.0.0-rc.12`, with default features disabled and explicit`std`,
`load-dynamic`,`api-23`,`coreml`. This selects C API23 for the installed1.23.2
library rather than the binding's default API24. No build-time binary download
feature is enabled. Only three cached packages are added to the lockfile:
`ort`,`ort-sys` and`libloading0.9`. The runtime binary is not vendored or fetched.
The public boundary contains project-owned configuration and fixed f32 data,
without exposing foreign session/tensor types or adding project unsafe code.

`OrtPitchConfig` requires an absolute local library path, mandatory SHA256 and
one of`cpu`/`core_ml_cpu_only`. GPU-allowed CoreML is intentionally absent after
the preceding activation-parity failure. CoreML requests fail on non-Apple
targets or registration errors. CPU-only graph fallback is allowed, and separate
native profiling verifies actual delegation for the tested model. ORT uses one
intra/inter-op thread, sequential execution and full graph optimization; CPU
arena allocation is explicitly enabled. CoreML uses NeuralNetwork format,
static shapes and CPUOnly compute units.

The dynamic library is streamed through a64KiB hash buffer with a256MiB file
bound. Model reads have a64MiB bound and check the existing hash, metadata,
one-input/one-output shape and f32 tensor contract. Preparation performs a zero
activation call before returning a usable model. Source windows still use the
same16kHz resampling weights, mean/sample-standard-deviation normalization,
silence floor and nine-bin decoder. The alternative replaces only the activation
session. Both runtime handles are boxed on preparation to keep their ownership
enum small; this adds no callback-time allocation or runtime switching.

ORT's loader/API/environment are process-global. A control-thread mutex binds
this module to one canonical library path/hash and refuses later substitutions,
even an identical file copied to a different path. Failed hash checks do not
bind it. An already-configured external ORT environment is rejected. The caller
must construct this binding before using other ORT APIs; this is an ownership
precondition, not isolation from arbitrary external initializers. A loaded
process library is retained by the upstream runtime and is not hot-swappable.
App-wide initialization ownership and distribution remain product-integration
requirements. A model/session failure does not imply process-global library
loading can be rolled back.

`CrepeModel::load_ort` and`NeuralPitchAssistant::new_ort` reuse the same
preprocessing, frame producer, two-slot worker, source-age gating and reset epoch.
`PndPlugin::configure_qa_ort_pitch` requires an explicit neural model and an
uninitialized plugin. Changes after initialization are errors, including after
reset. The QA choice stays outside ordinary preset fields/factories, so preset
reopen retains the existing behavior. Resource construction still precedes
replacement of the accepted plugin state in initialization. Ordinary reset
retains the prepared backend and invalidates source observations.

Actual library SHA256:
`92500010659b052368797e30c1841956b0efd699a8061ef8b3b25f27449c86e7`.
Model SHA256 remains
`57209bf565bce9de445b31a175294dd2f297432864599c6aa237a895353918d3`.
The actual-library integration test is explicitly ignored by default, with its
resource requirements named. Qualification runs it with`--include-ignored`,
`SOTF_ORT_DYLIB` and`SOTF_ORT_SHA256`; absent SDK/library environments cannot
silently report an actual-model pass. Negative resource/policy tests run normally.

Final actual qualification covers both CPU providers:

- 32 pre-normalized frames per provider from the frozen runtime corpus, with
  maximum full-activation difference7.451e−7 versus the2e−5 bound.
- 100 source windows: five rates16/44.1/48/96/192kHz, five frequencies82.41–1318.51Hz,
  pure and weak-fundamental variants, and two providers. Maximum decoded frequency
  difference is0.000291 cent and confidence difference4.173e−7, below the0.1-cent/
  2e−5 bounds. These are backend differences, not pitch error against source truth.
- The independent native placement sessions record23 CPU kernel events and one
  CoreML delegate event respectively. Raw profiles and hashes are retained.
  These fused counts establish delegation, not comparable operator counts.
- Actual asynchronous results publish, source-stale non-silent observations are
  refused after sufficient silence advances the source clock, and reset rejects
  previous-source publication immediately and after a control-thread wait.
- At48kHz, below-model-floor plugin output is exactly equal to the existing
  fallback; Rust process/reset/producer/drain heap guards pass. An active normal
  tone is adopted by the plugin, and finite drain freezes its decision state.
  The audio-latency declaration remains2047 samples. Rejected8kHz reinitialization
  preserves the current control snapshot; no failed-rate audio reconstruction
  comparison or fresh impulse timing is claimed.
- Wrong/missing resources, non-finite policy, initialized-plugin replacement,
  unsupported GPU selection, unknown configuration fields and process-library
  substitution reject explicitly.

The worker publication checks wait off the audio thread with a stationary source
clock. They qualify framing/lifecycle and do not measure wall-clock freshness,
device scheduling or callback deadlines. Source-age policy still includes the
whole64ms window. CountingAlloc guards cover Rust audio-side heap activity;
foreign/session/helper allocations and complete RSS/worker CPU remain unmeasured
here. Plugin waveform/lifecycle probes are mono48kHz; the broader rates above
qualify source preprocessing/forward behavior, not whole-stream multichannel audio.

All nine selected tests pass: the two new resource/actual-library tests, six
existing neural-model tests (including independent PyTorch parity) and the
read-only snapshot regression. Strict combined-feature Clippy checks the library,
test and standalone tools; default-feature library compilation passes. The report
verifier independently checks case coverage, recorded maxima, actual test success,
library/model/corpus identity and native profile hashes/provider event counts.
Numerical forward assertions execute in Rust. Initial layout/format/hash-printing
compile/lint failures and preliminary results remain in the artifact directory.

Standalone comparison binaries now require`qa-neural-tools`, which includes
`qa,neural-pitch`. This prevents unrelated large benchmark executables from being
linked during every ORT integration test. For current-source rebuilds use
`cargo build -p sotf-plugin-pnd --features qa-neural-tools --bin qa-crepe-runtime`
or`--bin qa-pnd-neural-compare`. Earlier frozen build commands describe their
historical manifest state; no previous executable or result was replaced.

Evidence: `audit/artifacts/ml-pnd-ort-native-20261026`, including the final
qualified test log,100-row parity report, native placement profiles, source
snapshots and verifier. The new API is research-only. Remaining: paced native
backend/queue comparisons and contended real-host timing, complete foreign and
worker memory/CPU accounting, app-global initializer coordination and packaging,
cross-platform library/provider qualification, product selection/UI, and the
recorded/independent pitch-quality work. The earlier source-quality regressions
remain; equivalent activations do not make the pretrained model a better corrector.
The full31-plugin/five-program ML audit remains active.
