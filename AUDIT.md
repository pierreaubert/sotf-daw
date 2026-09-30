# DAW feature and accuracy audit

Status: in progress, 2026-10-01. MIDI and IAMF are excluded at the user's request.

This document tracks the full workspace audit. A passing robustness test proves
neither numerical accuracy nor parity with current professional implementations.
Each crate needs an explicit feature comparison and relevant signal measurements.

Latest checkpoints:

- DynamicEQ core, native CLAP lifecycle and qualified CPU evidence are
  Astra-accepted. The new VST3 lifecycle checkpoint passes two tests and strict
  NIH all-target Clippy. Astra accepted the corrected bounded Linux VST3 COM
  checkpoint and its source provenance.
  Crossover default-sync now uses the real plugin-aware wrapper constructor;
  the subsequent NIH library run passes 118 tests with one ignored and strict
  all-target Clippy passes.
- Crossover bounded core and FFI checkpoints are Astra-accepted. The final
  numerical/lifecycle additions pass ten tests and strict lint. Typed engine
  and native scalar/cache gates are recorded separately. Astra also accepted
  the mounted UI/model/persistence checkpoint: four serial tests, including
  positive per-channel controls and recovery, with 39 selected source bindings.
  Native buses and live-manager audio remain open; unrelated lint failures
  remain documented. The default-sync regression is fixed.
- BandSplit native buffer handling and loaded all-band delivery are
  Astra-accepted. The remaining late-refusal history evidence now requires
  exact finite, equal-length live/twin vectors; both actual CLAP/VST3 cases
  pass. Astra accepted the narrow correction and closed that finding. A separate
  host ABI follow-up now supplies width-sized arrays for inactive VST3 buses.
  Its six-layout loaded descriptor probe, five loaded waveform/canary cases and
  strict host lint pass. Astra accepted this bounded host checkpoint against
  the unchanged r3 native artifact; current combined NIH integration is separate.
- The actual Ambisonics engine candidate/refusal, valid 64→16 retry and EOS
  fixture now passes. It checks the complete shifted waveform and exact drain
  counts, with a nonzero final block immediately followed by EOS. Earlier
  callbacks use a 250 ms scheduling margin, so this proves EOS delivery under
  that schedule, not real-time throughput. Host tail tests (2), multi-call drain
  (1), fresh worker build and strict host/engine Clippy also pass. Astra now
  accepts the stateful multi-block tail and deterministic preflight/failure
  evidence together with worker reset recovery. The publication/acknowledgment
  race is corrected using a local typed outcome; the deterministic real-host
  regression, shipped subprocess checks and strict lint support its closure.
  Structural VST3 restart servicing remains unsupported.
- Astra accepted native Convolution's bounded state/resource callback checkpoint
  in packet r4: actual CLAP/VST3 lifecycle, saved-resource replay, failed-restore
  preservation, replacement/clear and activation rollback pass at the original
  independent waveform limits. These are in-process wrapper callbacks. Reachable
  packaged editor and VST3 GUI evidence remain open. Astra accepted actual
  Linux CLAP embedded GUI packet r9: real IR selection, hide/reshow, deferred
  restart, latest Mix serialization, retry and reactivation with independent
  complete waveform comparison. The callback module passes four tests and the
  actual GUI test passes one, bound to 29 source inputs and a copied executable.
  Astra also accepted generated editor
  preparation/restart/geometry lifecycle packet r3 (two service and two generated
  lifecycle tests). The later CLAP GUI result supplies real host restart evidence
  for that Linux route; other platforms and VST3 editor delivery remain open.
- The bounded preset-envelope checkpoint remains Astra-accepted, including
  nine invalid-envelope refusals and full FletcherMunson→LoudnessCompensation
  state/audio migration evidence.

Luna xhigh implements native Crossover bus routes and Convolution editor/resource
activation. Astra medium accepted the concrete AUD145 per-band EQ design;
pre-edit audio/source/CPU baselines are preserved, and all EQ targets compile
with the new public schema. The base-rate order-2 placement matrix now passes
42 independent full-waveform cases, and four frozen legacy cases replay exactly.
The earlier EQ package gate passed 150 tests with two ignored. Astra accepted
84 cold multirate prefixes and 56 homogeneous advanced/SVF route-composition
cases; shared math realizations do not prove coefficient accuracy. Independent
audio subsequently exposed a Warped sign error (+7 dB requested, +0.505903 dB
measured) and incorrect Bark units. The correction now passes ten focused math
tests and two public regressions. All nine post-fix public impulse vectors and
45 complex-response points pass the independent reference at unchanged bounds
(maximum peak error 5.91e-8); Astra accepted this bounded static checkpoint. Historical nonzero-lambda
output intentionally changes. Measured SVF audio then reproduced all 36 shelf
failures while 12 controls passed. Astra accepted the corrected shelf/helper
checkpoint: 48 independent waveform cases, 240 complex points and 15 focused
math tests pass, including exact populated-state retention. Automation, realtime and consuming-route
validation remain in progress. The mounted Crossover and inactive VST3 bus-array checkpoints are
accepted within their documented scopes. No user approval is pending. The full
feature and accuracy audit remains open, including consuming app/UI and native
platform work.

The workspace minor version is now 0.8.0; affected independently versioned
crates and changelogs were also bumped at the user's request. Full offline locked
Cargo metadata succeeds after synchronizing local lockfile versions. Historical
packets retain their original versions and hashes.

The latest fixed-source workspace run remains a failed gate (6,250 passed,
three failed, 37 skipped): two native-prefix regressions pass after the local
correction, and the limiter timing test passes alone. These later focused
results do not constitute a fresh full-workspace pass. The implementation and
minor-version batch is now committed as `aa0a3d1`; follow-up documentation and
implementation continue on top of that baseline. Current owners, evidence and remaining work are in
[the implementation plan](audit/IMPLEMENTATION_PLAN.md).

## Active implementation issues

| ID | Area | Confirmed gap | Required evidence | Status |
|---|---|---|---|---|
| AUD-001 | Host chain | Terminal DAG outputs are mixed without latency compensation; reset leaves compensation history | f32/f64 impulse alignment, uneven blocks, reset silence | Fixed for same-rate paths; 477 host tests pass |
| AUD-002 | Limiter | Shared dual-release envelope advances once per channel; ISP output overshoots on bursts | Channel linking, final reconstructed burst/two-tone peaks, threshold steps, reported latency | Fixed with predictive output correction; 116 limiter tests pass |
| AUD-003 | De-esser | Missing adjustable stereo linking and reduction range | Gain-reduction bound, linked image preservation, independent/intermediate link, block invariance, parameter/preset wiring | Implemented; 61 plugin tests and 8 engine accessor tests pass |
| AUD-004 | Plugin QA | Library recipe selects only facade; diagnostic recipe omits existing AAE, declick, hiss and speech denoiser binaries | Package selection and diagnostic manifest coverage; complete plugin test run | Recipes fixed; 3,937 tests and all 41 self-contained diagnostics passed before follow-ups |
| AUD-005 | Host tail processing | Drain ignores bypass on source and downstream plugins | Tail and downstream bypass regressions | Fixed; 477 host tests pass |
| AUD-006 | Host resampling | DAG sample rates follow linear node order and latency sums use different sample clocks | Branched resampling and output-clock latency oracle | 490 host unit tests pass after fractional cursor/rounding and oversampler timing fixes; unequal branch bursts still lose unmatched samples |
| AUD-007 | Host automation | Sample-offset event splitting is disabled when any plugin has latency | Delayed-plugin automation at exact signal offsets | Fixed for supported same-rate processing; focused timing tests pass |
| AUD-008 | Compressor | Missing gain-reduction range and hold controls | Gain-law bounds and timed hold, full parameter wiring | Implemented; 152 compressor tests and 10 engine accessor tests pass |
| AUD-009 | Gate | Missing upward/ducking modes | Independent upward and ducking gain-law reference | Implemented and wired for internal keys: 87 Gate tests, 8 Gate-specific external tests, 25 layout snapshot tests and focused Clippy pass; external-key routes remain AUD-068/AUD-070 |
| AUD-010 | Limiter | Oversampling applies to detection rather than nonlinear audio processing | Aliasing measurements and final downsampled ceiling | Fixed with prepared 2x/4x audio processing, native output protection, aligned dry signal and queued controls; 151 limiter tests, strict Clippy, release oracles/QA, 840 ceiling cases and cold zero-allocation/free checks pass; native baselines remain exact |
| AUD-011 | NIH wrappers | Parameter steps incorrectly determine float/integer type; ordinary EQ bands omitted from metadata | Typed automation and complete EQ band exposure | Typed controls and 20 neutral EQ bands implemented; EQ automation allocation tests pass |
| AUD-012 | Spatial convolution | Direct-head NUPC reports block latency despite zero-latency output | Direct-convolution timing oracle | Fixed; 10 spatial tests pass |
| AUD-013 | DSP test accuracy | Some response/reconstruction tests have thresholds too weak to establish accuracy | Direct-convolution oracle, analytic EQ response, reference waveform/gain error | New convolution and EQ oracles pass; RNNoise reference error measured |
| AUD-014 | Wrapper construction/layout | Default configs fail; asymmetric DSP layouts declared as stereo | All-wrapper construction/sync and generated wrapper routing vs direct DSP | 43 defaults sync; 10 layout cases pass over irregular blocks |
| AUD-015 | NIH preset restore | Structural values saved but ignored on recreation outside EQ/FIR EQ | Nondefault constructor state and explicit unsupported-layout errors | Implemented; structural mapping matrix covers 32 families |
| AUD-016 | Dither precision | f32 dither addition biases 20/24-bit PCM rounding | Final-output zero mean and quarter-LSB² second moment across levels | Fixed with f64 guard arithmetic; 42 tests pass |
| AUD-017 | Resampler ratio | Relative updates report cumulative ratio but backend applies from nominal | Actual audio clock and equivalent absolute-update waveform | Fixed; 74 tests pass |
| AUD-018 | Resampler antialiasing | Dynamic downward ratio retains original filter cutoff | Independent above-Nyquist tone rejection after rate change | Fixed with prepared tables: former alias regression and five independent production tests pass; cold allocation/free checks and Clippy pass |
| AUD-019 | FFI frame contract | Successful partial/overlong DSP returns are not checked; BandMerge input width passed as constructor output width | Injected return-count tests and initialized complete output | Fixed; 47 FFI tests pass |
| AUD-020 | Async inference | Generic input/output destruction and cloning may allocate on callback despite blanket safety claim | Ownership-preserving send, borrowed result read, worker-side reset disposal | Implemented; all 10 tests pass |
| AUD-021 | Metering cache | First ArcSwap publication allocates audio-thread bookkeeping | First-use callback allocation test, held-reader lifetimes and contention | Fixed; isolated EQ regression and five cache tests pass |
| AUD-022 | Saturation native wrappers | Mode/oversampling omitted; preferred oversampling ignored; scalar parameter access allocates | Restored choice matrix, actual oversampled output/latency and callback allocation | Native wrappers fixed: all 15 mode/factor combinations; 34 NIH and 61 saturation tests pass |
| AUD-023 | FFI presets | Failed restore can partially change the active DSP | Invalid preset preserves settings/history; valid restore retains configuration | Transactional staging implemented; 57 FFI tests pass, no ignored tests |
| AUD-024 | Oversampling | Negotiated small/large callbacks can grow scratch; reported latency may depend on callback partition | Cold allocation checks below chunk size and above 4096; independent impulse timing across blocks | Fixed capacity and constant latency; independent impulse/reset/block tests pass |
| AUD-025 | Stereo imager | Subtracting phase-rotated LR bands from dry side gives incorrect band attenuation | Coherent-tone complex response and all-band-zero collapse | Fixed with documented complementary 6 dB/octave bands; 147 response cases pass |
| AUD-026 | Mono to stereo | Duplicate path caches nonfinite last input used to prime later decorrelation | Final-sample NaN/Inf followed by width change matches sanitized reference | Fixed; both stereo crates pass 114 tests and Clippy |
| AUD-027 | Gate/expander | Downward ratio law uses compressor slope instead of conventional expansion ratio | Independent static gain law, knee/range and release controls | Fixed; 247 dynamics/analog tests pass, including spectral suffix-loss fix |
| AUD-028 | Analog models | Model switch can discard retained drive/color/character/trim | Preset order independence and live model change vs fresh reference | Fixed; control retention and update-order tests pass |
| AUD-029 | Spatial publication | Direct ArcSwap reads may allocate on first Binaural/XTC callback | Cold-thread and active-update allocation/deallocation checks | Fixed; 282 tests and Clippy pass |
| AUD-030 | Oversampling precision | f32 oversampler advertises inner native-f64 support and reaches allocating fallback | Native-f64 host processing vs f32 reference without allocations | Fixed; cold-thread regression passes |
| AUD-031 | Oversampled transport | Adapters replace incoming transport with a new position-zero context for every chunk | Nonzero origins, loop sample units, PPQ, buffered input and multiple chunks | Fixed; all 15 oversampling tests pass |
| AUD-032 | AB comparison | Loudness-match target changes are applied retroactively to a whole callback | Gain magnitude and callback-partition invariance | Fixed; 113 tests and Clippy pass |
| AUD-033 | Oversampling drain | Adapters report immediate completion and discard residual input and FIR tails | Finite streams vs zero-padded reference, inner tails, arbitrary drain capacity, reset and allocation | Fixed; 960 reference configurations plus host/reset/error/capacity regressions pass |
| AUD-034 | In-place adapter | Native f64 capability is advertised but processing always converts to f32 | Values below f32 resolution/above range and cold allocation check, asymmetric layouts | Fixed; native dispatch and drain forwarding tests pass |
| AUD-035 | Loudness compensation | ISO 226:2003 equation constants/exponent differ from the published equation | Independent published contour checkpoints | Fixed; published 20/40/60/80-phon checkpoints pass |
| AUD-036 | Loudness AutoGain | Feedback measures compensated output but computes absolute correction | Pre/Post settled level, varying input and callback partitions | Fixed; 110 crate tests and Clippy pass |
| AUD-037 | Offline render | Rate-changing plugin chain output is labeled with pre-host rate/count | WAV header, tone pitch, duration and exact clock count | Fixed; 15 offline tests and 756 independent resampler impulse cases pass |
| AUD-038 | Engine crossfade | Cached first-block step changes 50 ms transition under variable callbacks | Transition duration across rates/partitions and reset | Fixed; processing and delay/reset tests pass |
| AUD-039 | Native transport | NIH wrapper discards native transport and builds position-zero context | Actual native callback metadata and oversampled context | Fixed; 39 NIH tests and Clippy pass |
| AUD-040 | Oversampling latency | Inner oversampled latency is truncated toward zero | Independent impulse first moment and conservative sub-frame bound | Fixed; impulse moment matrix and all 21 oversampling tests pass |
| AUD-041 | FFI automation | ID creation and alias normalization allocate during successful scalar updates | Cold C ABI setter/getter ramp loops and expanded-band controls | Fixed bridge ID allocation; six cold scalar/band ramps and all 58 FFI tests pass |
| AUD-042 | Shared bridge typing | String choices and expanded-band kinds are converted incorrectly; raw band Hz conversion is linear | Runtime typed defaults/nondefaults, all choice labels and raw frequency round trips | Fixed; 39 bridge and 62 FFI tests pass |
| AUD-043 | Loudness calibration | Rejected calibration change may mutate active Auto-mode state | Rejected setter preserves settings and DSP history | Fixed; 112 loudness tests and Clippy pass |
| AUD-044 | Native automation | Nonzero event offsets can change the whole native callback | Actual CLAP dispatch versus independent split DSP and analytic Gain | Gain and Gate verified plus pinned NIH CLAP timing fixes; 52 NIH tests and Clippy pass; other families remain scoped separately |
| AUD-045 | Spatial EOS | Binaural/Upmixer may discard retained input and filter overlap at EOS | Finite stream process+drain versus independently zero-padded reference | Implemented; 119 Binaural and 142 Upmixer tests pass; focused Clippy passes |
| AUD-046 | Engine EOS | Global bypass still drains old DSP; shutdown breaks only inner send loop | Deterministic drain/bypass/shutdown state regressions | Fixed; 51 processing tests and Clippy pass |
| AUD-047 | Integration evidence | Direct bridge comparison ignores operation errors and is labeled cross-format | Fail on unexpected errors, check produced frames and nondefault behavior | Fixed claims and error checks; seven direct integration tests pass; unchanged structural restore retains DSP history |
| AUD-048 | Mute/solo defaults | Omitted JSON enabled is false while shared schema defaults to true | Omitted/explicit false state and muted audio | Fixed; 45 unit and 21 integration tests pass |
| AUD-049 | Upmixer startup | Empty analysis window and zero Hann endpoint can discard first input sample | Independent startup impulse/phase matrix and declared latency | Fixed; 142 Upmixer tests and Clippy pass including EOS follow-up |
| AUD-050 | Gate realtime getter | Default scalar getter allocates a full parameter map during native synchronization | Cold typed reads and live setter/getter ramps | Fixed; all 73 Gate tests and Clippy pass; native timing oracle now passes |
| AUD-051 | Native tails | Every wrapper reports zero tail, including feedback delay and convolution | Explicit conservative bounds, native setup/reset/process queries, tail-change notification and independent last-tap oracles | Implemented for Gain/Matrix/ChannelMuteSolo/Delay/Convolution/oversampling; native CLAP/VST3 lifecycle checks pass |
| AUD-052 | Parametric in-place adapter | Native f64 support is advertised but the in-place entry converts through f32 | Sub-f32 precision, values above f32 range and cold allocation check | Fixed; independent precision/cold allocation regression passes |
| AUD-053 | Scalar parameter reads | Twelve more DSP classes exposed by fourteen native exports build complete maps on every scalar read; analog wrappers share the default | Cold primitive reads matching default/nondefault snapshots and native synchronization | Fixed; 14 cold native cases, 750 DSP tests and 43 analog tests pass; live setter allocation remains separately scoped |
| AUD-054 | Beamformer startup | MVDR/Superdirective omit the preceding Hann window, losing sample zero and attenuating onset | Independent broadside reconstruction from first sample, full phase sweep and unchanged latency | Fixed; 6,912 startup phase cases plus dense/reset checks pass with unchanged public latency |
| AUD-055 | AEC/Beamformer EOS | Immediate completion loses buffered mic/reference and adaptive FIR histories | Independent learned final-tap/partition oracles, frozen adaptation, capacities/reset/realtime | Fixed; 124 tests include 333 independent frozen-state drain cases; all-target Clippy passes |
| AUD-056 | HAL staging | Decrypted suffixes can be reinterpreted after format changes or survive key reload | Identity-tagged staging, exclusive read guard, native transition/reload regressions | Implemented; eight portable tests pass and macOS tests compile/Clippy; native execution pending |
| AUD-057 | Native GUI state return | A queued-state callback intermittently allocates 48 bytes | Exact allocator stack and nonblocking return ownership proof | Fixed; preallocated reply queue/control-only serialization, 69 NIH tests and all-target Clippy pass |
| AUD-058 | Resampler ramp sizing | Valid large downward ratio ramps panic in pinned Rubato before completing a block | Independent inverse-ratio trajectory/count oracle, both fixed modes, bounds and history | Private fork integrated after 656 upstream + 8 strict regressions pass; corrected sizing/history and production exact-waveform/cold checks pass |
| AUD-059 | Resampler lifecycle | Ratio changes miscount buffered/ramped input; same-rate dynamic toggles discard or resurrect pending audio; rejected drain latches EOS | Public impulse/count probes, exact backend source cursor, transactional lifecycle checks | Fixed with emitted-clock endpoint accounting and transactional lifecycle: 94 tests pass; independent variable/tiny/extreme matrices, cold allocation/free checks and Clippy pass |
| AUD-060 | Scalar parameter writes | Generic schema validation and singleton maps allocate during native live automation | Cold changed-value synchronization, retained audio history, schema/error compatibility | Fixed: 81 NIH tests include seven-export changed-value dispatch; 137 Compressor, 69 Denoiser and 261 other DSP/backend tests pass with Clippy |
| AUD-061 | Speech denoiser buffering | Ring cursor rebasing changes modulo positions; oversized callbacks overwrite pending dry/wet samples | Fixed480-frame dry impulse/dense oracle, irregular/oversized blocks and wet partition equivalence | Fixed; exact mono/stereo dry-delay and wet partition oracles, oversized callback canaries and cold native allocation checks pass |
| AUD-062 | Engine frame interruption | Stop retries an already-rendered unsent frame; Shutdown exits only the inner send loop | Deterministic real-worker rendezvous, new-stream marker and termination with live channels | Accepted-command outcome fix verified: 57 processing tests and Clippy pass; cancelled Stop preserves audio, manager quiescence barrier open |
| AUD-063 | Engine output format | Same-channel host commits can emit old-rate pending/queued frames; bypass changes rate without playback reconfiguration | Real-worker rate/duration regressions and acknowledged output-format transition | Three red worker repros confirmed; local pending normal/tail guards implemented, coordinated playback format protocol open |
| AUD-064 | Desktop transport | Resume or Pause can erase Stop's pending FIFO Flush requirement | Actual playback state helpers and ring writes across both callback orderings | Narrow ordering fix verified by 45 playback tests, including six actual state/ring regressions; broader acknowledged transition awaits specific authorization |
| AUD-065 | Gate soft knee | Actual processing skips the upper half of the centered knee when the detector crosses the nominal threshold | Independent settled waveform and knee continuity, hold/hysteresis and hard-knee regressions | Fixed; 79 Gate tests and all-target Clippy pass, including independent full-knee waveform, hold timing and cold allocation/free regressions |
| AUD-066 | iOS feeder | Fresh post-Flush frames can be erased; disconnected EOS delays Shutdown; oversized frames cannot enter a smaller ring | Actual feeder/callback waveform, control responsiveness, channel alignment and cold allocation/free checks | Fixed with frame-aligned partial writes and flush guards; 10 actual feeder/callback tests and engine Clippy pass, including cold allocation/free checks; native iOS execution pending |
| AUD-067 | Shared RMS detector | f32 squaring can overflow finite samples before conversion to f64 | Pinned dependency proof and post-extreme recovery oracle | Fixed with private math-dsp sum tree: 641 vendor and 1,147 downstream tests pass, strict downstream Clippy passes; detector storage/CPU costs and two upstream-only lint warnings documented |
| AUD-068 | Native Gate sidechain | Fixed native stereo layout has no external-key auxiliary input | Actual declared bus layout and host activation/audio routing | Implemented native key bus; 90 NIH tests plus final focused CLAP/VST3 routing and boundary regressions pass; independent review complete |
| AUD-069 | Limiter EOS | Lookahead and ISP delay histories have no finite-stream drain | Final impulse versus independently zero-continued processing | Fixed: 123 limiter tests and two public adapter drain tests pass; 180 exact-delay and 108 nonlinear continuation cases, cold allocation/free checks and Clippy pass |
| AUD-070 | Sidechain adapters | Asymmetric adapters advertise program output width but require input-width output storage; C factory passes total input as program width | Exact4→2 output buffers, independent key/program audio, f32/f64 and cold allocation checks | Fixed: prepared opt-in subdivision and exact output widths; 610 host plus 116 bridge/C tests pass, native-f64/cold allocation checks and Clippy pass |
| AUD-071 | Gate EOS and tiny lookahead | Default drain discards retained program; tiny positive delay reports zero while the ring delays one frame | Exact delayed program and zero-continued nonlinear output, actual-ring latency | Fixed: 93 Gate tests pass; 864 exact-delay and 288 nonlinear continuation cases, 54 cold allocation/free configurations and Clippy pass |
| AUD-072 | Native auxiliary buses | CLAP permits the count boundary as an index; VST3 checks main width as key width; missing key buffers keep prior callback lengths | Actual native bus negotiation and short supplied key to larger missing-key callbacks | Fixed: actual CLAP/VST3 key and auxiliary-output regressions pass; missing-port sentinels and callback-length changes, independent review and focused Clippy pass |
| AUD-073 | Remaining plugin EOS | Additional buffered plugins report immediate drain completion despite retained audible program | Public final-marker probes, independent finite-support/zero-continuation oracles, bounded callbacks and lifecycle | Partly fixed: finite Delay, Convolution, Declick, SpectralCompressor, Denoiser, spectral Hiss, PND, XTC, spectral MultibandExpander, eligible Downmix, disabled SpeechDenoiser, empty-bank EQ and eligible AnalogLimiter/MBC/MBE states now drain. Remaining buffered families and recursive modes stay open |
| AUD-074 | FIR crossover EOS | FIR history and inter-band alignment are discarded at EOS | Independent direct convolution, final marker, delayed band sum, capacity/reset and cold allocation/free oracles | Fixed for FIR: 97 tests, 324 direct-convolution configurations, 18 cold allocation/free configurations and Clippy pass; independent review complete; LR recurrence policy remains open |
| AUD-075 | Multiband lookahead latency | Tiny positive lookahead reports zero despite the active minimum-one-frame ring | Exact dry waveform across rates, channels, band counts and partitions | Fixed: 268 tests and all-target Clippy pass, including 480 time-domain and 12 spectral dry runs |
| AUD-076 | Spectral compressor startup | Initial empty history omits negative-origin analysis windows, so Hann[0] can erase first wet input | Independent first-sample and all-hop-phase WOLA reconstruction | Fixed with negative-origin priming at unchanged latency; 7,168 impulse phases, dense unity/reset/ring-wrap checks and 64 focused tests pass, none ignored; strict Clippy passes |
| AUD-077 | Engine drain budget | Fixed 4,096-call limit can stop legitimate long finite tails or small-capacity drains | Declared per-stage successful-call bounds, monotone completed prefix, real long tail and control responsiveness | Fixed with prepared per-stage work bounds, monotone completed prefix and bounded oversampling setup. Real 30-second/192 kHz convolution completes in 5,626 calls; focused suites and strict Clippy pass. Preexisting terminal replacement edge is separate |
| AUD-078 | FIR EQ EOS | Prepared NUPC convolution and aligned dry history are discarded at EOS | Direct convolution with actual FIR coefficients, analytic dry delay, arbitrary capacities and cold allocation/free checks | Fixed: 71 tests verified, 96 independent convolution configurations, 32 cold allocation/free configurations and Clippy pass; 96 real-chain waveform runs also pass |
| AUD-079 | Offline recursive tails | Export only preserves program duration; callers cannot request a defined extra response interval | Exact WAV duration, analytic feedback echoes, original default compatibility, rate conversion and progress | Fixed by additive render_offline_with_tail API; original zero-extra-duration default preserved, source-converter suffix retained; all 19 offline tests and strict engine Clippy pass |
| AUD-080 | XTC streaming | Missing startup windows and callback-dependent output scheduling alter delayed identity; mixed callbacks can skip accepted input | Independent neutral reconstruction from sample zero, exact source acceptance and callback partition comparison | Fixed at unchanged latency: 175 tests pass, 160 dense and 259 impulse fixtures, 32 cold allocation/free cases and Clippy pass; EOS and AutoGain callback cadence remain separate |
| AUD-081 | Downmix spectral startup | Missing preceding analysis window erases the first stereo input sample and attenuates onset | Independent stereo matrix identity across initial window phases and callback partitions | Fixed with one negative-origin window at unchanged 2048-frame latency; 61 tests, 480 dense fixtures, 2048 initial-hop impulses, 72 cold allocation/free cases and Clippy pass; recursive LFE/drain unchanged |
| AUD-082 | Offline endpoint composition | Source conversion can stop before compensated downstream filter support; continuation budget omits chunk buffering | Long explicit source-padding oracle cropped to endpoint and tiny render blocks through large resampler chunks | Fixed after both independent regressions failed; source converter remains active to the endpoint and work budget includes chunk waiting; all 21 offline tests and strict engine Clippy pass, independent review complete |
| AUD-083 | Speech denoiser timing | Wet RNNoise adds an intrinsic frame beyond the wrapper queue, while dry and metadata retain only one frame | Public mono/stereo dry/wet impulses and direct model-stage timing | Fixed: dry and metadata now960, existing wet waveform unchanged; 106 tests and strict Clippy pass, including direct model, transitions and cold allocation/free checks; native EOS policy remains open |
| AUD-084 | Denoiser startup | Unprimed analysis windows lose source sample zero | Independent first-sample and dense unity reconstruction for both FFT modes | Fixed with one negative-origin window at unchanged latency; all 2560 initial-window phases, dense unity/ring-wrap and cold checks pass; combined Denoiser/Hiss suite passes 117 tests with strict Clippy |
| AUD-085 | Denoiser tonal detection timing | Whole-callback PND analysis exposes future samples to earlier spectral windows | Independent callback partition comparison with tonal detection enabled and disabled | Fixed by feeding source slices at canonical analysis boundaries; all eight FFT/multi-resolution/PND configurations agree within 2e-6; combined suite passes 117 tests |
| AUD-086 | EQ internal oversampling preparation | Constructor output queue is smaller than EQ's advertised 4096-frame callback capacity | Cold maximum-size process with explicit allocation and deallocation counters | Fixed by reserving EQ's advertised capacity during setup; cold maximum-block processing and first drain now allocate/free zero; 227 focused tests and strict Clippy pass |
| AUD-087 | A/B variable-rate paths | Nested path rate and actual produced length are ignored by fixed-width comparison mixing | Public factory constant-input oracle through a nested resampler across callback partitions | Confirmed: accepted 48→24 kHz path reports 48 kHz and inserts 32 silent frames per 64-frame callback into settled constant audio; variable-rate path composition remains open |
| AUD-088 | Denoiser reset | Enabled harmonic/percussive separator retains prior audio-analysis history across reset | Warm signal followed by reset and a different program, compared with a fresh configured instance | Fixed with existing separator reset and scratch clears; 12 reset/reinitialize waveform cases are bit-exact, 6 cold reset cases allocate/free zero, 78 Denoiser tests and strict Clippy pass |
| AUD-089 | Denoiser parameter persistence | Harmonic/percussive and spatial controls are exposed as parameters but absent from JSON configuration | Public factory configuration versus scalar setter and round-trip parameter values | Fixed with schema defaults, strength validation and constructor wiring; 83 Denoiser plus 5 facade/bridge/engine route tests and strict Clippy pass; exhaustive Rust struct-literal compatibility documented |
| AUD-090 | Host drain capacity | Destination capacity is checked after consuming a plugin's retained output | Public finite-tail plugin, undersized destination and full-capacity retry versus untouched twin | Fixed with conservative capacity preflight before DSP; four regressions failed before the fix, all 617 host tests and strict Clippy pass; caller capacity requirement documented |
| AUD-091 | Downmix input validation | Direct ordinary processing accepts wrong-rate and nonfinite input | Invalid callbacks preserve output and match an untouched configured twin afterward | Fixed with preflight before DSP; all 72 Downmix tests and strict Clippy pass, preserving explicitly tested constructor-time default-rate processing; structural drain capacity also passes minimum-size 2x/4x wrappers |
| AUD-092 | Engine terminal host replacement | Same-format replacement during blocked terminal tail/EOS can publish old completion and skip replacement drainage | Deterministic real-worker rendezvous, accepted/rejected commits and exact output order | Fixed with a private successful-commit epoch and unsent EOS restart; 26 EOS tests and strict engine Clippy pass, independent review complete; broader manager protocol unchanged |
| AUD-093 | XTC filter generation | A delayed old-rate worker can publish after reinitialization and be adopted at the new rate | Deterministic worker publication barrier, active filter identity and coefficient oracle | Fixed with synchronous-install generation invalidation and existing AutoGain rate preflight; deterministic real-worker race/ownership tests, 186 XTC tests and strict Clippy pass |
| AUD-094 | MultibandExpander spectral startup | Unprimed Hann/WOLA history loses the first wet input sample and attenuates onset | Independent neutral startup phase reconstruction at unchanged latency | Fixed at unchanged 1024-frame latency; 146 tests, release QA and strict Clippy pass, including independent initial phases and full spectral support; active drain metadata stays conservative through cached output |
| AUD-095 | MultibandExpander soft knee | Both gate-state paths bypass the defined upper soft-knee gain law | Independent periodic-Hann/DC curves, open/closed histories, hold/hysteresis and automation | Fixed by comparing state transitions to the knee's unity edge; 152 tests, strict Clippy and release QA pass; measured spectral jump falls from 4.179 to 0.033 dB, independent review complete |
| AUD-096 | XTC initialization | Filter-source errors return success after changing live clock/history | Missing/malformed/rate/width source failures versus untouched twins; pending worker and partial drain preservation | Fixed with target-rate preparation before one successful commit; 191 tests and strict Clippy pass, including 40 new artifact/lifecycle/worker scenarios; new HRTF-specific fixture coverage remains unclaimed |
| AUD-097 | Beamformer GSC arithmetic | Accepted finite extreme input overflows blocking references and permanently poisons adaptive weights | Independent f64 blocking/NLMS oracle, extreme-to-small recovery without reset | Fixed with wider private adaptive state and finite output/coefficient commits; 76 Beamformer tests, strict Clippy and release QA pass; measured ordinary rounding error and 9.5–11.7% local kernel cost documented |
| AUD-098 | Beamformer MVDR silence | Low-energy complex normalization overflows after several seconds of pure silence | Long exact-zero streams and independent distortionless normalization across covariance scales | Fixed with wider normalization and whole-bin finite validation; prolonged silence, 63 covariance/steering cases and cold zero-heap checks pass within the 76-test suite |
| AUD-099 | XTC disabled timing | Immediate disabled audio contradicts fixed latency and paused wet history replays stale program | Independent disabled/reenabled marker timeline and actual two-branch host PDC | Fixed with N-frame delayed dry audio, continuously warm wet history and 10 ms fades; 204 tests, strict Clippy, release QA and 2,345,760 exact enabled baseline samples pass |
| AUD-100 | AEC process clock | Ordinary callbacks accept a rate different from prepared adaptive/filter timing | Wrong-rate output canaries, learned state and exact subsequent process/drain replay | Fixed with scalar preflight; 36 mismatch/lifecycle cases, all 59 AEC tests and strict Clippy pass; constructor-time processing preserved |
| AUD-101 | Beamformer extreme spectral state | Finite large input can overflow spectral audio or permanently poison MVDR covariance | Ordinary zero continuation and subsequent adaptive rejection versus healthy/reset references | Fixed with validated covariance commits, wider overflow retries and finite output boundary; 90 tests, strict Clippy and release QA pass; retained finite overload values decay at the original rate and recovery duration is documented |
| AUD-102 | Limiter engine settings | Configuration conversion drops link_amount and feed_forward | Persisted engine settings through real factory plus independent quiet-channel waveform | Fixed by forwarding both fields; 54 route configurations, linked/unlinked waveform controls, 12 converter tests and strict engine Clippy pass |
| AUD-103 | AEC constructor preparation | Direct constructor uses a 48 kHz adaptive clock and background step 0.7 despite requested rate and exposed default 0.5 | Public construction routes at identical settings, independent coefficient/timing proof | Fixed with canonical default step and rate-aware preparation; 18 exact public histories, independent timing checks, all 62 AEC tests and strict Clippy pass |
| AUD-104 | XTC AutoGain clock | Measurement ingests only every tenth caller block and refreshes gain before applying it to that whole block | Same source and controls across callback partitions, causal measurement boundary and independent gain reference | Fixed with continuous delayed-reference measurement and fixed sample-clock refresh; 211 tests, strict Clippy and release QA pass; independent full-stream equality and 204–297% local CPU increase documented |
| AUD-105 | Shared AutoGain smoothing | Applied compensation advances the configured smoother but ignores its value | Public 25/1000 ms trajectories, scalar/block equivalence and caller accuracy | One shared recurrence implemented; helper and new caller regressions pass; original scalar rounding floor exposed by unchanged XTC accuracy gate, tracked separately as AUD110 |
| AUD-106 | Standalone channel correlation | Fixed sample ring drops oversized callback suffixes and can shift subsequent channel alignment | Accepted frame counts and independent centered Pearson matrix across partitions, invalid-callback state preservation | Direct synchronous ingestion and preflight implemented; four previously failing accuracy/lifecycle tests, strict focused Clippy and the fifteenth aggregate checkpoint pass |
| AUD-107 | Correlation realtime storage | First split frame, cold publication/reset and retained nested matrix readers allocate or free on the callback | Fresh-thread allocation/free counts, immutable held snapshots, nonregressing publication | Prepared matrices and conditional publication implemented; nine new tests and full host gate (658 passed), strict Clippy and independent review pass |
| AUD-108 | EQ reset measurement phase | Reset clears DSP/AutoGain history but retains its tenth-callback measurement counter | Identical source/settings after reset versus a fresh instance in ordinary and compiled processing | One counter reset implemented; 24 exact fresh-instance comparisons, all 135 EQ tests and strict Clippy pass |
| AUD-109 | SOFA additional delay | Pinned reader and Binaural/XTC consumers discard Data.Delay | Shared/per-measurement impulse timing and independent phase, support/rate/cache integrity | Exact integer adapter and transactional preparation implemented; 10 public fixture tests, private ownership regression, 879 host/XTC tests, 126 Binaural tests and strict Clippy pass; cross-rate timing verified with AUD114; fractional/signed support remains a capability limit |
| AUD-110 | AutoGain numerical precision | Legacy scalar f32 poles and approximate dB conversion settle below the requested gain | Independent analytic gain/time-constant oracle, unchanged XTC 0.01 dB bound, local CPU and allocation gates | Local f64 state and accurate exponential implemented; analytic oracles, 1,119 owned-package tests, 432 caller tests and strict Clippy pass; active smoothing adds 0.05–0.07 ms per half-second stereo programme in the local probe, settled cost preserved |
| AUD-111 | Loudness nested snapshot storage | Retained nested peak or correlation buffers can trigger copy-on-write allocation | Public cold/retained-reader process and reset probes, immutable snapshots and current generation recovery | Authoritative nested readiness and coherent deferred publication implemented; five regressions, 667 host tests, strict Clippy and independent review pass; 912 process calls, 192 resets and 152 prepared-ID setters allocate/free nothing |
| AUD-112 | EQ AutoGain measurement clock | Only every tenth callback is ingested; current callback measurements influence its earlier audio | Public partition and identical-prefix comparisons with exact disabled controls; native/compiled/oversampled alignment and EOS | Fixed with continuous aligned 10 Hz measurement; 147 EQ tests, strict Clippy and 80 cold zero-heap fixtures pass; raw audio/latency/EOS preserved and measured CPU increase documented |
| AUD-113 | Crossfeed AutoGain measurement clock | Current whole-callback measurement influences its earlier audio | Public fixed-mode/mix partition and identical-prefix comparisons with exact disabled controls | Fixed with a causal active-frame clock; 92 tests, strict Clippy, cold zero-heap checks and independent review pass; 48 matched CPU cases and exact raw/mix controls documented |
| AUD-114 | Binaural HRIR resampling | Resampler latency is retained and output is truncated before delayed response is flushed | Public impulse timing, late-source support, independent rate/time and frequency-response checks | Even FFT grids, delay removal and zero flushing implemented; 126 Binaural tests, strict Clippy and independent review pass; 18 exact loader/render waveform pairs and 72 physical-time peaks verify real propagation delay is preserved |
| AUD-115 | Multichannel AutoGain callers | AAE and Upmixer measure whole callbacks before applying gain to their earlier audio | Public partition and identical-prefix comparisons with exact disabled controls; aligned causal reference, EOS and prepared storage | Fixed with paired causal measurement and a prepared Upmixer reference delay; 927 host/AAE/Upmixer tests and strict Clippy pass; independent review and exact disabled controls pass; matched CPU measurement pending |
| AUD-116 | Spectrum endpoint power | Exact maximum-frequency lines are dropped; Nyquist band power uses peak-amplitude weighting | Independent Hann-weighted time-domain energy, upper-bound line and coherent peak, channel-max controls | Fixed endpoint inclusion and energy weighting; independent power regressions, all 17 spectrum unit tests, the full host suite and strict Clippy pass with AUD115 |
| AUD-117 | Loudness Range feature | Host loudness telemetry has no LRA measurement | EBU Tech 3342 synthetic requirements, independent gates/quantiles, channel roles, bounded history and realtime publication | Implemented prepared optional rolling/whole-program LRA with exact quantiles; EBU synthetic, public timeline/layout/serialization, cold/full-history and retained-reader tests pass; host suite and strict Clippy pass; official authentic corpus unavailable |
| AUD-118 | Upmixer small FFT geometry | Accepted FFT64/128 surround initialization reaches an inverted sequence-length clamp | Public constructors/layout changes, independent complex filter response and unchanged ordinary-size controls | Corrected; 157 tests and strict Clippy pass, 34 waveform controls exact, cold/reset heap checks clean; [evidence](audit/upmixer-small-fft.md) |
| AUD-119 | Integrated loudness history storage | Backend prepares 6,000 entries but admits 36,000, causing callback allocation | Public dependency/host allocation counts through every growth boundary, wrap and reset | Fixed by reserving the existing complete window during construction; two red-to-green public regressions, 24 backend tests, full host suite and lint pass; +240,000 prepared bytes per integrated meter |
| AUD-120 | AutoGain meter cost | Generic meters calculate unused true peaks, correlation and integrated history | Exact public gain/telemetry against generic-meter references, matched CPU and allocation measurements | Implemented; exact public/EQ baselines, zero callback heap activity, strict caller Clippy and matched CPU reductions; [evidence](audit/auto-gain-meter-cost.md) |
| AUD-121 | True-peak FIR calibration | Host and backend coefficients differ from the published ITU interpolation table; existing oracle repeats the error | Public analytic tone and independent zero-stuffed convolution, interval/reset/realtime checks | Fixed after a public 4.675 dB overread; independent regressions, unchanged remaining telemetry, cold heap checks, Clippy and checkpoint 18 pass; [evidence](audit/true-peak-coefficients.md) |
| AUD-122 | True-peak finite streams | Analyzer omits final interpolation response; engine UI cache stays stale after drain | Public final impulse/full convolution, real worker EOS, lifecycle/retained readers and cold heap checks | Fixed bounded metering finalization and engine final-cache refresh; checkpoint 19 passes 5,988 tests |
| AUD-123 | True-peak rate coverage | Most audio rates report unavailable; 44.1/88.2 kHz interpolation remains below the ITU 192 kHz guideline | Analytic EBU tones, independent convolution/partitions/EOS, prepared memory and CPU cost across 8–384 kHz | Implemented and independently accepted for the host private meter; 5,997 workspace tests pass (11 skipped), strict lint, accuracy/lifecycle/heap tests and CPU evidence pass; [report](audit/true-peak-rates.md). The vendored math-dsp detector remains at 48 kHz scope. |
| AUD-124 | Programme maximum true peak | `true_peaks_dbtp` is consumed per query interval; no programme-latched scalar survives later lower peaks, and UI takes only the current snapshot/channel max | Core: red-to-green multi-channel impulses across queries/cache contention/EOS, reset/reinitialize/disable epoch, rejected input, supported/unsupported rates, finite-safe serialization and zero-allocation evidence. UI: finite max retained across lower intervals, cold/unsupported states, narrow localized layout, routed query, translation completeness, and redraw tracking. | Core/API stage accepted by Astra on 2026-09-28: six focused tests, strict host Clippy, and offline nextest 6,003 passed/11 skipped including FFI; final-suffix recovery checked against published-table and independent Lanczos references under retained strong/Weak readers. See `audit/proposals/programme-maximum-true-peak.md`, `audit/proposals/programme-maximum-true-peak-ui.md`, `audit/proposals/programme-maximum-true-peak-ui.patch`, and `audit/reviews/AUD124-astra.md`. User authorized the sibling patch on 2026-09-28. TUI finite/unsupported rendering and redraw-signature tests, GPUI translation helper and mounted `Screen::Studio` Loudness Monitor rendering, and live dev-API initial/higher/lower interval queries pass; GPUI all-target check, rustfmt, design-token, diff, and pseudo-locale generator checks pass. These gates used temporary offline resolver lock SHA `3d1581cebf529b878c201e52ebe9beec3099434f5409a1f1206038da3bff8009`; the original sibling lock was restored byte-identically to SHA `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f` and is not the resolution tested. Core and UI were accepted by Astra on 2026-09-28; AUD-124 is complete. The broader audit remains open; no EBU Mode or certification claim. |
| AUD-125 | Maximum momentary and short-term loudness | `LoudnessData` exposes current momentary (400 ms) and short-term (3 s) readings but no maxima. The host accepts arbitrary callback sizes, so a max updated only when publishing a snapshot could miss louder 100 ms windows inside a large callback. | Finite-only maxima sampled at each fixed 100 ms measurement boundary; maximum momentary becomes available after 400 ms and maximum short-term after 3 s; callback/query partition invariance, retained-reader/reset epochs, serialization compatibility, and zero callback allocation. Both maxima reset with Integrated measurement reset. | Confirmed against EBU Tech 3341 (2023) §2.1; its §2.2 specifies ungated 400 ms/3 s windows and at least 10 Hz Short-term updates. Core/API implementation and planned host gates are complete; Astra accepted the evidence on 2026-09-28. Focused 7 realtime + 9 M/S tests, strict all-target Clippy, matched Criterion, and the offline workspace gate (6,020 passed/11 skipped) pass. Full report: `audit/maximum-ms-loudness.md`. The UI proposal `audit/proposals/maximum-ms-loudness-ui.md` and sibling TUI/Studio integration were accepted by Astra on 2026-09-28. TUI finite/unsupported rendering, localized short-height layout, bottom-border preservation, and redraw tests pass; GPUI translations, compact and mounted Studio rendering, and live routes for finite latches, lower intervals, reset/null, and nonfinite-to-null pass. The GPUI dev-api all-target check, rustfmt, pseudo-locale, design-token, and diff checks pass. Sibling Cargo validation used temporary lock SHA-256 `87f37029677509822f0164117da1a37fcf39d72a2523908b1b036f0b46bd554a`; the exact original lock SHA-256 `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f` is restored. No EBU Mode claim is made. |
| AUD-126 | Coupled I/LRA pause and continue | The only runtime measurement switch is `enabled`; disabling clears integrated and LRA history, and there is no stand-by/pause state | Start/pause/continue/reset public API; audio pass-through while paused; I/LRA jointly frozen and resumed; reset while running/paused; no callback allocation | Confirmed against EBU Tech 3341 §2.2. Astra accepted the core/API design on 2026-09-29: a prepared active-time I/LRA lane excludes paused samples while live M/S/TP/sample-peak/correlation continue; reset preserves Running/Paused, disabled/reconfiguration behavior and cache lag are explicit. Pre-edit 48 kHz 480-frame monitor baseline: `/tmp/sotf-aud126-cpu-pre-pause.log`; host source+lock manifest start/end SHA-256 `e328254f500ad9bd2a65a15b6e08758cd15980d710621c9b53581ca6637eccd0`. Core proposal: `audit/proposals/coupled-integrated-lra-pause.md`. Core/API implementation accepted by Astra on 2026-09-29. The offline workspace gate passes 6,043 tests/13 skipped, strict host Clippy and focused lifecycle/realtime gates pass, and source manifests match at `fc8ef98a8323141d4837a7eb92da01b74ab251f9af7235f21a32ad242459e12c`; Cargo.lock remains `6fd8186e6c6ef66ac2a18c243fd3320bfd98f07fa54b230041b72cab1e3ed088`. Full core evidence: `audit/coupled-integrated-lra-pause.md`. Astra accepted the reachable sibling TUI/Studio controls on 2026-09-29; host payload→receipt behavior is covered by actual input/output plugin instances. Sibling checks used temporary lock SHA-256 `7a03b9f561ee929aa189ee70881d84563ab1b1d0036b3a76078d50c033a5af3a`; exact original lock SHA-256 `2c87468c46063817fee68c909bf46f9117724a70654b04c933790420b335a31f` is restored and not the resolution tested. UI evidence and the explicit no-Player-transport limitation: `audit/coupled-integrated-lra-pause-ui.md`. AUD-126 is complete; AUD-127, AUD-128 and the broader audit remain open. |
| AUD-127 | First-60-second LRA instability indication | LRA may become numerically `Valid` after its first complete 3 s observation, but neither `LoudnessRangeData` nor the application surfaces indicate the required first-minute instability | Separate range availability from stability; exact active I/LRA frame clock through 60 s; pause/reset/serialization/UI lifecycle and realtime checks | Confirmed against EBU Tech 3341 (2023) §2.4. Astra accepted host/API and reachable TUI/Studio stages on 2026-09-29. Focused host integration 10/10, strict host Clippy and offline workspace nextest 6,055 passed/13 skipped across 359 binaries pass. Final source manifest SHA-256 `cc5cce1410755e67e88844715890761c287998ac9562c9cd626e7bca0c1a1bb8`; DAW lock unchanged `6fd8186e6c6ef66ac2a18c243fd3320bfd98f07fa54b230041b72cab1e3ed088`. Evidence: `audit/first-minute-lra-stability.md`; proposal: `audit/proposals/first-minute-lra-stability.md`. Scoped stages accepted; no EBU corpus or certification claim. |
| AUD-128 | Complete EBU loudness corpus evidence | Host tests cover synthetic requirements but no official EBU Loudness Test Set v5.0 corpus is present or executed in this workspace | Establish permitted internal R&D use and acquisition; inventory all 70 advertised audio files and official expected readings/tolerances; map relevant tests to supported host routes; reproducible complete run and mismatch report | Evidence gap confirmed by fixture search and earlier HTTP 403 record. Official EBU publication page lists an 87.4 MB v5.0 ZIP containing 70 audio files for Tech 3341/3342 and links restrictive terms. Astra accepted the public Tech 3341/3342 case map on 2026-09-29. Archive-level inventory/execution remains open pending project-owner determination of permitted use/access and actual archive README; no archive was downloaded, extracted, copied into the repository, or tested. Proposal: `audit/proposals/ebu-loudness-test-set.md`. |
| AUD-129 | Upmixer minimum FFT geometry and HR ring capacity | FFT constructor/factory admitted unsupported N=1 and N=32+ HR ring overflowed during processing | Public lower bound, factory mapping, frame-capacity coverage past `rate / 10 + 512`, independent waveform and heap controls | Bounded geometry/capacity fix accepted by Astra on 2026-09-28; Upmixer package tests 164 passed, strict all-target Clippy and fmt pass, source manifest SHA-256 `d0ef11f8e20a005c3483bdefee80b9dea3cb945c498938c64e15c5fb18f06835`; no workspace-wide gate claimed. Evidence: `audit/upmixer-minimum-fft.md`, `audit/reviews/AUD129-astra.md`. |
| AUD-130 | Upmixer high-resolution contribution timing below 512 | A scheduler-level characterization probe placed an N=2/N=32 high-resolution-only impulse contribution at frame 512, 510/480 frames after the main contribution's API-latency frame | Independent direct-convolution arrival and phase reference, regular/irregular callback partition invariance, finite-stream/EOS behavior, and an explicit intended-latency policy | Bounded N<512 source-tag scheduler correction accepted by Astra on 2026-09-28: accepted-input credits gate a shared 512-frame HR/main latency across single, irregular and 512-frame callbacks, with long callback/EOS, HR-resume, AutoGain, and layout regressions. Final package has 175 passed/2 ignored, strict all-target Clippy and fmt; start/end source+lock manifest SHA-256 `f5b180b05ff15981b20ea2e5ad306bf0001daafc34dba5258bc74a6394eb45f9`. The N>=512 follow-up is now addressed by accepted AUD-132; this report does not claim general HR quality or CPU performance. Evidence: `audit/upmixer-hr-timing.md`, `audit/reviews/AUD130-astra.md`, `/tmp/sotf-aud130-final-accepted-package.log`. Keep separate from AUD-129 geometry/capacity. |
| AUD-131 | Fractional and signed SOFA `Data.Delay` | The shared loader materialized exact nonnegative integer samples but rejected finite fractional and negative values; SOFA defines delay units but not this interpolation or negative-delay causalization policy | Independent phase/magnitude oracle, relative timing across `[I,R]`/`[M,R]`, causal-rebase provenance through Binaural/XTC, exact integer compatibility, rate conversion, finite tails, support/size bounds, transactional failure, and realtime safety | Scoped implementation and source review accepted by Astra on 2026-09-29. Exact integer-only loader and full Binaural/XTC output arrays match preserved pre-edit arrays byte-for-byte. Public facade suite passes 16/16 (1 ignored), covering subnormal/near-integer boundaries, cross-rate Binaural full EOS, XTC plant/cascade and transactional state; strict all-target Clippy and scoped rustfmt pass. Source manifest SHA-256 `37df45680a7704c83861d0969a033fb60f8ff313b3b597cab0e887105d7575d9`; focused run details are in `audit/sofa-fractional-delay.md`. The later coordinated offline workspace gate with final AUD-132 sources passed 6,071 tests, 0 failed, 15 skipped across 359 binaries; matching whole-tree start/end manifest SHA-256 `1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb`, lock `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`, log `/tmp/sotf-aud132-aud131-workspace-final.log`. The earlier 6,064/1/14 run remains recorded as historical evidence in the report. Fractional accuracy is gated only through 0.45 cycles/sample; signed rebasing is explicit SOTF policy, not a claim about every SOFA convention. Evidence: `audit/sofa-fractional-delay.md`; design: `audit/proposals/sofa-fractional-delay.md`; Astra review: `audit/reviews/AUD131-astra.md`. |
| AUD-132 | Upmixer high-resolution contribution timing at or above 512 | AUD-130 established a corrected common 512-frame path only for N<512; the N>=512 prepared-gain path had an unverified HR/main source-time pairing | Freeze the N>=512 baseline; independent coherent-phase and full-waveform oracle; callback-partition/EOS timing; define source-time gain pairing; preserve AUD-115 AutoGain controls | Bounded source-tag retiming accepted by Astra on 2026-09-29. N>512 HR contributions now mix against matching main source frames and prepared gains while retaining the D-frame HR input delay; N=512 stays on its existing path. Upmixer package tests (184 passed, 3 ignored), post-capfix strict Clippy, formatting, and coordinated offline workspace nextest (6,071 passed, 15 skipped, 0 failed; MIDI/IAMF excluded) pass. Whole-tree start/end manifest SHA-256 `1e4bb4097b37cc238911b211606d76f8e3c4f071de6683b5a388e9c0a61d6aeb`; Cargo.lock SHA-256 `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5`. N8192 below-cap full-vector comparison matches exactly; initial cap-contaminated comparison remains explicitly inconclusive. No general HR quality or CPU claim. Evidence: `audit/upmixer-above512-hr-timing.md`, `audit/reviews/AUD132-astra.md`, `/tmp/sotf-aud132-aud131-workspace-final.log`. Distinct from AUD-129 and AUD-130. | 

| AUD-133 | Ambisonics orders 4–7 on named layouts | Decoder math, scratch capacity, factory widths, engine input admission and app graph ports stopped at order 3/16/32 | Preserve orders 1–3; independent harmonic/max-rE and decoder oracles; virtual/physical diagnostics; 64-channel AIFF→decoder→engine→host plus 64-port graph route; realtime behavior and measured performance | Scoped implementation/evidence accepted by Astra on 2026-09-29. Focused Ambisonics suite (59 library, 21 public integration, 2 tail, 1 independent baseline pass; capture helper ignored), strict Ambisonics Clippy, lower-order bitwise controls, numerical oracles, and performance report pass. Engine config/capacity, 64-channel AIFF full-waveform route, 65-channel engine admission rejection, literal 64/65 service-PCM route, factory/catalog widths 4/9/16/25/36/49/64 and mismatch rejection, and Ambisonics render snapshots pass. Graph-model reconciliation/canvas persistence passes 1/1 under reviewed sibling resolver `475d5890…` (not a mounted order-edit click). Coordinated workspace gate passes 6,100, 0 failed, 19 skipped across 362 binaries; log `/tmp/sotf-aud133-134-workspace-rerun.log`, manifest aggregate `85dcb633fc9d01db143f4bf5c598b565775ea28c2c4ad49b1a4dd92b6ba6d91c`. Post-gate test-only Clippy cleanup, affected focused tests, and strict `sotf-engine`/`plugins-bridge` Clippy pass; current whole-tree hash `d408e22055188bbec6031e416c71ac3240f3fa86ac20ab2e1254012465155bfc` differs from the tested broad snapshot only in those two test fixtures. Exact evidence, CPU limitations and prior non-green history: `audit/ambisonics-orders-4-through-7.md`. Capacity results are vector pointer/capacity reuse, not whole-callback allocation. No WAV64 claim. Native bridge/FFI/NIH routes, hidden structural controls and custom layouts remain open.
| AUD-134 | True-stereo convolution routing | Convolution engine follows a diagonal stereo IR mapping and has no persisted user choice for four-path true stereo | Independent four-path convolution oracle, legacy-output controls, partitioned EOS, config/factory/preset persistence, routed plugin UI, latency/memory and realtime evidence | Astra accepted the bounded true-stereo DSP, FFI and mounted GPUI/preset routes. Independent four-path and legacy waveform evidence and the historical 6,100-test workspace gate remain recorded in `audit/true-stereo-convolution.md` and `audit/reviews/AUD134-astra.md`. Astra also accepted the bounded in-process CLAP/VST3 resource restoration, invalid/missing-resource refusal, rollback/retry and complete independent waveform checkpoint (native-resource-r4). Native editor selection/reactivation and packaged loaded audio remain open; the new egui adapter dependency check passes. No overall convolution parity claim. |
| AUD-135 | Packaged native Ambisonics orders 1–7 | Accepted AUD133 DSP/engine/model support does not yet reach native CLAP/VST3/FFI wrappers or SOTF's external-plugin host with typed ACN/SN3D input configuration and visible reactivation | All orders 1–7 × eight existing named output layouts through 9.1.6; whole-vector 4–64 input, including 64→16; exact CLAP/VST3 metadata and arrangement negotiation; per-instance setup/preset restore; mounted host selection/reactivation; full waveform and legacy controls | Astra accepted bounded wrapper ABI, loaded CLAP/VST3 restore/reconfiguration, late candidate refusal and isolated VST3 checkpoints. The actual engine test now exercises populated-state refusal, acknowledged valid 64→16 replacement and nonzero final-program EOS with complete shifted waveform/count checks. Earlier callbacks use 250 ms scheduling margins, so this is functional delivery evidence rather than realtime performance. Host/engine lint and focused native gates pass. Stateful multi-block and pending/timeout waveform gates now pass, and the isolated module passes 25/25. Astra accepted classified recoverable errors, shipped subprocess recovery and the publication/acknowledgment race correction using a local typed outcome. Deterministic real-host, worker, IPC and subprocess regressions plus strict host lint pass in the bounded worker-classification-race-r1 packet; broader failure/platform coverage remains separate. Isolated CLAP, mounted setup/reactivation and wider native interoperability remain open. |
| AUD-136 | SpeechDenoiser enabled accepted-program EOS | Enabled drain returned zero frames despite a partial terminal model block and queued output; natural model response remains Unknown | Full-vector ordinary zero-continuation oracle, all 480 terminal phases in mono/stereo, enable transitions, preflight/reset, complete heap guards, actual 48 kHz DawHost endpoint; preserve disabled AUD083 timing | Scoped implementation accepted by Astra on 2026-09-29. Pre-edit public regression reproduced a nonzero omitted 960-frame suffix: 1,513 actual versus 2,473 reference frames. Final package tests pass 46/0 with two manual utilities ignored; explicit pre-edit audio byte replay passes 1/1; strict plugin/backend Clippy and formatting pass. Selected source/lock manifests match at `c7aca58a5882162c229d38e8b8b282b2b131c3501e18486410a1655093e786e9`. The explicit 960-frame rendering cutoff resets the backend without callback allocation; enabled `TailLength::Unknown` is retained. Report: `audit/speech-denoiser-enabled-accepted-queue.md`; review: `audit/reviews/AUD136-astra.md`. No host queue/manager behavior change, natural finite-tail claim, model-quality claim, or new workspace-wide gate. |
| AUD-137 | ABCompare child and alignment EOS composition | Two nested DawHosts and A/B/dry alignment rings advance during ordinary processing, but inherited outer drain omitted their final audio | Public finite-child final-marker regression; independently drained child/alignment full-vector oracle; same-rate Plugin/Rack/Graph routes, mix/difference/bypass and lifecycle/heap checks; truthful recursive-tail metadata | Scoped implementation accepted by Astra. Both review findings are fixed: admission requires an explicit conservative per-node geometry capability and equal negotiated rates, and drain uses the actual remaining timeline without excess silent frames. Final package: 131 passed, four manual utilities ignored; composition: 15 passed; saved seven-control byte replay: 1/1; strict ABCompare/host/Delay Clippy passes. Nine-entry source/lock manifest: `5d35cc3b55868eb1078af9dbabe9a5698211c6134a711f954110d0fa033a0425`. Independent FIR vectors, real Delay, serial Rack/Graph, outer host and lifecycle heap checks support the bounded same-rate identity-frame scope. Undeclared child geometry, branching and active/prior recursive band-mask EOF remain open. Report: `audit/abcompare-finite-stream.md`; review: `audit/reviews/AUD137-astra.md`. No generic queue or manager rewrite. |
| AUD-138 | HAL Output pending transport at EOF | Partial final writes retain program audio while inherited drain completed; generic host/engine paths reject zero-output sinks | Public waveform/backpressure/lifecycle/heap evidence, preserving control recovery, and consuming engine/application integration | Direct-plugin stage accepted by Astra. The explicit bounded serial host route and same-rate/channel ring-size reprepare are implemented; corrected HAL library 71/71, focused reprepare 9/9 and strict all-target HAL Clippy pass. Host production is unchanged since the earlier 551 passed/1 ignored and strict lint checkpoint. Tests cover writer-error cleanup, already-started drain, precommit and priming mutations, deterministic preparation failure, large used storage and shrinking retained backlog under heap guards. Root preserved source/log hashes in `target/audit-artifacts/aud138-preserving-reprepare-r2/` under `crates/sotf-plugins`; Astra accepted the bounded host/recovery/reprepare stage after the additional unfinished multi-chunk tail test and strict lint. Engine/application admission and native playback remain open. No generic queue or manager rewrite. Report: `audit/hal-output-finite-stream.md`; proposal: `audit/proposals/hal-output-host-drain.md`; review: `audit/reviews/AUD138-astra.md`. |
| AUD-139 | Dynamic EQ shelf bands | Both public band-state types have no shape selector; EQ construction/rebuild hardcodes Peak, and detection always uses a bounded bandpass | Dynamic low/high shelf processing with explicit detector and gain laws, unchanged legacy peak behavior, independent response/dynamics references, stable parameter/preset routes and reachable host controls | Astra accepted bounded shelf DSP, legacy compatibility, scalar controls, actual CLAP callbacks, qualified CPU comparisons and the corrected Linux VST3 COM restart checkpoint. The VST3 r2 packet binds 42 selected inputs including factory and opt-in changes; focused 2/2 passes. The corrected aggregate default-sync fixture uses the actual plugin-aware constructor; full NIH passes 118 with one ignored and strict lint. These are historical pre-version-bump gates, not a current full-workspace pass. Packaged native loading, SOTF consuming-host rebuilds, mounted shelf controls/curves/presets, AU and remaining feature dimensions are open. Evidence and limits: `audit/reviews/AUD139-astra.md`. |
| AUD-140 | Channel-changing serial host EOF | Drain shares an ordinary fast-path predicate that rejects every node with unequal input/output width, including nonzero serial channel changes | Independent public host refusal and preserved-tail regression; actual Ambisonics EOF completion; separately validated drain topology/capacity while preserving ordinary fast-path eligibility | Astra accepted the bounded design and the frozen implementation on 2026-09-29; review basis is the four-file source manifest `002b6a4e6f8a52c02780cc3f9a351ffc532c7b3d787f30de332f1d4773d1b244`. The implemented serial drain validator preserves the ordinary fast-path helper and preflights source/intermediate scratch before native drain. Final focused tests pass 13/13 with three manual utilities ignored; explicit saved ordinary-audio replay passes 1/1. Host library tests pass 551/551 (one ignored), Ambisonics library tests 59/59; strict host/facade/Ambisonics all-target lint passes. Root verified terminal logs and matching selected four-file focused manifests. Cases cover actual 64→16 EOF, independent FIR tails, expansion/contraction scratch, two finite producers, bypass, missing identity capability, unequal rates, incompatible adjacent widths and Unknown-tail refusal. Pre-edit captures retain meaningful ordinary audio and a full recovered two-frame 64-channel tail independently checked exactly. Actual engine endpoint integration remains open. Report: `audit/channel-changing-host-eof.md`; proposal: `audit/proposals/channel-changing-host-eof.md`; review: `audit/reviews/AUD140-astra.md`. AUD138 owns terminal-sink backpressure; no generic graph/queue/manager rewrite. |
| AUD-141 | LR24 multiway recombination | Branch selection omits terms from the documented product of all-pass split sums; the existing widely spaced six-tone test tolerates the error | Public overlapping-split regression, independent complex three/four-band response, phase compensation, real split/merge route and unchanged two-way/per-channel/FIR behavior | Public Plugin regression reproduces summed gain 0.8322950965 (about −1.594 dB) for 1,000/1,200 Hz splits at 1,100 Hz/48 kHz; band 0 complex error is 0.1677049016 and the other two bands match the independent decomposition. Terminal red log: `/tmp/sotf-aud141-public-red.log`. Separate four-band prediction of −4.576 dB remains analytical. Existing six-tone maximum predicted gain error is 0.00877, below its 1% tolerance. Astra accepted bounded design `4817094bae37ae5efa1ef27ef72328643a64d1fa0755632a07f6a2a339d94b46`. The correction now passes 104 package tests, including the expanded three/four-band complex-response matrix, lifecycle, automation and heap checks. The original summed magnitude is now 0.999999999061; four captured stereo compatibility controls replay bitwise. The actual Crossover→BandMerge 2→6→2 host test passes 1/1, checking independent complex response, full waveform residual and cross-channel leakage. Final replay and both strict lint gates pass. The reviewer-requested full-vector finite/peak assertions pass their focused test and strict lint; Astra accepted the bounded implementation. AUD142/AUD143 remain separate. Report: `audit/crossover-multiway-recombination.md`; review: `audit/reviews/AUD141-astra.md`. |
| AUD-142 | IIR crossover families and slopes | Crossover parser, DSP banks and visible parameter choices support LR24 and FIR only | Define and implement missing IIR families/orders with explicit cutoff/polarity/sum laws, stable choices, complete control/state routes and independent numerical/chain evidence | Astra accepted the bounded IIR core and qualified CPU evidence, including ten final boundary/automation/lifecycle tests with unchanged numerical bounds and strict lint. FFI state/preset routes are accepted: unchanged public probes cover 30 state and eight full-preset cases. Typed engine route has seven focused cases and seventeen width regressions; native scalar/schema gates are recorded separately. Crossover default-sync regression is corrected and reviewed. Astra accepted the bounded mounted UI/model/persistence correction: four serial tests exercise real listeners, fresh Structural updates, per-channel recovery/edits, dormant values and full restored-settings DSP comparisons. The 39-source packet is verified; unrelated lint failures remain non-green. Native output buses/callback lifecycle and live-manager audio remain open. See `audit/reviews/AUD142-astra.md`, `audit/reviews/AUD142-ffi-astra.md` and the implementation plan. |
| AUD-143 | BandSplit multiband phase compensation | Serial LR24/LR48 carry gives unequal group delay in three/four bands; populated LR48 reset also allocated | Independent per-band/summed complex response, full split/merge waveforms, automation/lifecycle/heap/CPU evidence and complete public control/state/native routes | Astra accepted the bounded phase-compensated DSP, legacy replay, FFI migration, mounted controls/preset, lifecycle/heap and qualified CPU evidence. Refreshed native buffer handling and loaded all-band delivery are accepted; the final exact finite live/twin late-refusal check passes both loaded CLAP/VST3 formats and closes its evidence finding. Astra also accepted the inactive VST3 bus-array correction: every reported nonzero-width bus receives a prepared pointer array. Six actual loaded descriptor cases under allocation/deallocation guards, five loaded waveform/canary cases and strict host lint pass in `audit/artifacts/aud143-inactive-vst3-bus-arrays-r1`; the plugin artifact remains the unchanged accepted r3 bundle. The historical 6,250-pass/three-failure workspace gate has not been replaced by a coherent full green gate. See `audit/reviews/AUD143-astra.md`. |
| AUD-144 | FFI preset envelope identity | Preset import reads state bytes without checking the exported schema version, type marker or plugin family | Actual exported-document mutations, supported alias/legacy compatibility, and rejection preserving populated state and audio | The original real C-ABI probe reproduced nine invalid envelopes resetting populated audio. r2 now passes the unchanged probe: all nine reject with -8 and exact saved state/audio continuation. Full FFI library passes 81 with one manual utility ignored, and strict lint passes. Root then confirmed that genuine FletcherMunson presets, which imported into LoudnessCompensation with exact state/audio before validation, regress to a family-mismatch rejection. Both library revisions, complete vectors and command receipts are preserved. Luna corrected the forward legacy migration in r3: focused tests pass 3/3, full FFI library passes 82 with one ignored, strict lint passes, and both unchanged external probes exit 0. Root verified exact saved-state/audio equivalence, waveform hashes and lengths, artifact identity and 340 matching selected build hashes. Astra medium accepted the bounded sealed r3 checkpoint. See `audit/ffi-preset-envelope-validation.md` and `audit/reviews/AUD144-astra.md`. |

External issue publication was blocked by automatic approval review. Local work
continues under the user's implementation request; publication is awaiting an
optional user decision.

Automatic review also rejected the proposed host branch-queue rewrite, citing
its broad changes to core routing/buffering and risk of silent audio corruption,
and requiring more specific user authorization. That queue integration remains
unapplied. Existing unequal-branch prefix mixing is retained for compatibility
until the replacement is approved and validated; it can discard unmatched data.

## Pending engine transition approval

Automatic approval review rejected the broader manager/output transition
integration, citing production concurrency and playback changes beyond its
recognized authorization for tests and isolated fixes. The incomplete protocol
was removed from the live source while its snapshot and proposed integration
are retained under `/tmp/sotf-engine-transition-review`. The verified AUD-062
command outcomes and AUD-064 independent pause/stream-flush states are retained.
The broader Stop/Play, host-format and bypass barriers remain unimplemented until
specific authorization and deterministic protocol validation.

## Fifth implementation checkpoint

- In-scope workspace: **5,398 tests passed across 258 binaries; 11 skipped**.
  MIDI and IAMF were excluded. Test duration 86.610 seconds; build 28.81 seconds.
  Log: `/tmp/sotf-audit-wave5-nextest.log`.
- Includes scalar getters, AEC/Beamformer startup and finite drain, HAL staging,
  native GUI-state replies, and Matrix/ChannelMuteSolo zero-tail declarations.
- 202 changed Rust files passed rustfmt; scoped `git diff --check` passed.
  Focused all-target warnings-denied Clippy passed for the changed plugin families,
  NIH and portable HAL. The macOS HAL test target compiles and passes Clippy;
  native macOS tests remain unexecuted.
- Dynamic resampler cutoff, ramp sizing and lifecycle fixes remain isolated
  investigations. This checkpoint does not claim those defects are corrected.

## Fourth implementation checkpoint

- In-scope workspace: **5,353 tests passed across 251 binaries; 11 skipped**.
  MIDI and IAMF were excluded. Test duration 85.875 seconds; build 37.42 seconds.
  Log: `/tmp/sotf-audit-wave4-nextest.log`.
- Covers Upmixer startup/EOS, native Gate automation, scalar getter fixes,
  selected-family native tails and parametric f64 dispatch (AUD-049–052).
- 167 changed Rust files passed rustfmt. Scoped `git diff --check` passed.
  Focused all-target Clippy passed for host/Gain/Delay, Convolution and NIH;
  Binaural/Upmixer focused Clippy passed. Vendored NIH formatting is preserved.
- The HAL proposal remains outside the repository. Its isolated baseline and
  revised library compile for x86_64-apple-darwin; this does not validate native
  reader transitions, device deadlines or cross-process behavior.

## Third implementation checkpoint

- In-scope workspace: **5,321 tests passed across 249 binaries; 11 skipped**.
  MIDI and IAMF were excluded. Test duration 85.870 seconds; build 34.12 seconds.
  Log: `/tmp/sotf-audit-wave3-nextest.log`.
- Includes completed AUD-035 through AUD-048, with scope limits retained above
  (Gain native automation only; Binaural drain implemented, Upmixer pending).
- 138 changed Rust files passed rustfmt. Vendored NIH source formatting was
  preserved; its two timing changes were verified against the pinned source.
  Scoped `git diff --check` passed; user MIDI/IAMF work was excluded.
- Focused warnings-denied Clippy passed for engine/host/resampler, NIH,
  LoudnessCompensation, Binaural, bridge/FFI and ChannelMuteSolo.
- This checkpoint precedes Upmixer startup and Gate scalar-getter follow-ups.

## Comparison references

- [FabFilter Pro-Q 4 overview](https://www.fabfilter.com/help/pro-q/using/overview):
  selectable phase modes, dynamic/spectral processing, metering, and control integration.
- [FabFilter Pro-L 2 true peak limiting](https://www.fabfilter.com/help/pro-l/using/truepeaklimiting):
  input true peak detection and correction of peaks introduced by limiting.
- [FabFilter Pro-DS advanced controls](https://www.fabfilter.com/help/pro-ds/using/advancedcontrols):
  adjustable stereo linking. The [manual](https://www.fabfilter.com/help/pro-ds)
  also documents threshold and range controls.

These references establish comparison dimensions, not certification or identical
sound. Numerical acceptance criteria must come from the intended algorithm,
independent reference signals, and applicable standards.

## Verification

The configured target directory resolves outside sandbox writable roots;
normal-host test execution was approved. Initial broad builds encountered
in-progress edits and were retried after focused builds succeeded.

- `cargo test --offline -p driver-common -p driver-hal --lib`: passed 23 driver-common tests; driver-hal runs zero tests on Linux, so macOS verification remains open.
- `cargo test -p sotf-host --lib`: 473 passed after AUD-001.
- `cargo test -p sotf-plugin-limiter --all-targets`: 107 passed after the linked envelope and ISP validation fixes.
- Host and limiter Clippy with `-D warnings`: passed.
- `just --dry-run plugins-test-all`: selects all 45 plugin crates, facade and host in one invocation.
- Diagnostic manifest inspection: all self-contained QA binaries are selected; `qa-aae-validation` requires an external corpus manifest and is documented separately.
- Release diagnostics: all 41 self-contained binaries passed. Full plugin/host nextest passed 3,937 tests across 186 binaries (3 skipped: installed CLAP/VST3/AU allocation tests). These results precede the follow-up ISP and wrapper work below; rerun affected tests after those changes.
- Engine nextest with no default features: 972 passed across 35 binaries; 7 skipped. Hardware/native-platform coverage remains separate.
- Built the local gain binary and ran real CLAP, VST3 and isolated-CLAP tests: exact -6.5 dB steady-state gain and restored preset output pass. The isolated test now includes its fixed transport latency and callback pacing instead of accepting silent output as evidence of attenuation.
- Previously skipped native CLAP and VST3 allocation tests now pass against that binary. Audio Unit remains unverified on Linux.
- Dither: 24 unit tests, one independent final-output moment test, one allocation/deallocation regression and 16 integration tests pass. Limiter release QA also passes after predictive output correction.

- Final in-scope workspace gate: `cargo nextest run --offline --workspace --exclude sotf-midi --exclude sotf-iamf --no-default-features --lib --tests --no-fail-fast --test-threads 8` — **5,230 passed across 245 binaries; 11 skipped**. This includes the cold metering regression and all completed follow-ups above. Log: `/tmp/sotf-audit-nextest-complete.log`.
- Final integration Clippy with warnings denied: engine, NIH, bridge and inference all targets passed. Host/saturation, both compressor crates and FFI focused Clippy also passed.
- Release follow-up diagnostics: `qa-host`, `qa-eq`, `qa-saturation`, `qa-resampler`, and `qa-multiband-compressor` all passed after the final DSP changes (`/tmp/sotf-audit-final-qa/results.json`).
- Formatting: 82 changed Rust files passed rustfmt; the subsequently corrected analog fuzzer was formatted separately. User MIDI/IAMF sources were excluded.

- Second implementation checkpoint: **5,272 tests passed across 248 binaries; 11 skipped**, with MIDI and IAMF excluded. This includes AUD-025 through AUD-034. Log: `/tmp/sotf-audit-wave2-nextest.log`.
- Host all-target Clippy passes after transport, native f64 and drain changes. All 116 changed Rust files passed rustfmt; scoped diff checks pass.
- All eight affected release diagnostics pass: host, ABCompare, Binaural, Gate, MonoToStereo, MultibandExpander, StereoImager and XTC. Multiband Expander's diagnostic initially asserted the old incorrect ratio law (-50 dB instead of -60 dB); its expected value now uses the conventional independent transfer equation, retaining the existing crossover/envelope tolerance. The corrected diagnostic passes, including allocation and performance checks (`/tmp/sotf-audit-wave2-qa/qa-multiband-expander-corrected.log`).

## Remaining scope

- Complete the feature/accuracy comparisons for every inventoried crate.
- Run and inspect all plugin library/integration tests and diagnostic binaries.
- Audit engine, driver transport, wrapper layers, inference, spatial support,
  test infrastructure, and platform-specific paths.
- Extend numerical evidence beyond finite-output and peak-bounded smoke tests.
- Resolve remaining confirmed gaps, including mixed-rate branch retention, native external-key routing and finite-stream support in remaining delayed processors.
- Verify full chain behavior, realtime constraints, automation and preset/UI wiring.

## Filter and restoration comparisons

### AUD-145 — Per-band EQ placement

Confirmed source gap, 2026-09-30; implementation pending an available Luna lane.
`sotf-plugin-eq/src/lib/types.rs` defines filter topology and channel-specific
filter banks, but no per-band Left/Right/Mid/Side placement. The existing
per-channel API does not by itself provide Mid/Side processing within a band
sequence. FabFilter documents per-band Stereo/Left/Right/Mid/Side choices in
[Pro-Q 4's primary manual](https://www.fabfilter.com/help/pro-q/using/stereo).
This establishes a feature comparison, not equivalent sound or certification.

Required result: persisted and reachable placement controls, explicit stereo
pair/layout semantics, independent complete-vector/matrix-response evidence,
legacy compatibility, lifecycle/realtime checks and full consuming routes.
Design and acceptance details: [AUD145 proposal](audit/proposals/eq-band-placement.md).
Dynamic and linear-phase EQ placement remain separate parts of the full parity
audit; implementing static EQ alone will not close those gaps.

These are source-inspected findings. A listed missing feature is a parity gap,
not a claim that every application needs it. Existing test results are distinct
from reference-quality evidence.

| Crate | Implemented | Verified missing features or evidence |
|---|---|---|
| `sotf-plugin-eq` | Biquad/SVF, Orfanidis, warped/Kautz, per-channel filtering and ordered per-filter Stereo/Left/Right/Mid/Side core routing | Placement persistence and consuming app/native/UI routes remain incomplete. Independent SVF shelf/helper accuracy defects await correction; advanced mixed/rate, automation and realtime gates remain open. Integrated dynamic/spectral bands remain a parity gap |
| `sotf-plugin-dynamic-eq` | Eight peak/low-shelf/high-shelf bands, individual dynamics, solo and linking; AUD139 shelf core accepted | Independent held/coupled shelf response and lifecycle gates pass. FFI and actual offline-engine delivery are tested but await Astra review; native/UI control lifecycle and matched CPU remain open. Tilt/spectral processing and per-band channel selection remain feature gaps |
| `sotf-plugin-linear-phase-eq` | Linear/minimum-phase FIR, 1–8k taps, dry alignment, partitioned convolution | Dynamic bands and per-band channel selection missing; independent analytic checks now cover 960 single-filter and 27 multiband cases |
| `sotf-plugin-convolution` | Uniform/nonuniform partitioning, direct head, async IR loading/resampling, optional four-path true stereo with persisted settings | AUD134's independent f64 full-vector, EOF, compatibility and mounted control evidence is accepted for its documented routes. The bounded in-process native resource/persistence callback checkpoint is also accepted (AUD134 review, packet r4); native file selection, host-serviced reactivation and packaged editor/loading evidence remain open; see `audit/true-stereo-convolution.md` |
| `sotf-plugin-crossover` | LR12/24/48, Butterworth orders 1–8, Bessel2, per-channel/multiway splitting and 31–16385-tap FIR | AUD141 LR24 recombination and bounded AUD142 core/qualified CPU, FFI and typed-engine evidence are Astra-accepted. The new explicit band-count contract has initial core 1/1 and engine 9/9 gates, with compatibility review pending. Product UI and actual-rate admission are in progress; native Both-output buses and complete consuming routes remain open |
| `sotf-plugin-resampler` | 64/128/256-tap sinc modes, ratio changes, exact drain | Prepared dynamic cutoffs and exact variable EOF added; fixed/dynamic response has independent evidence, narrow transition-band rejection and smooth cutoff transitions remain gaps |
| `sotf-plugin-denoiser` | IMCRA/Wiener, decision-directed estimates, profiles, masking, dual resolution | No editable frequency reduction curve or residual audition; total-energy decrease alone does not prove wanted-signal preservation |
| `sotf-plugin-declick` | Eight-sample robust median context, linked stereo pairs | No multiband periodic/random modes, frequency skew, repair widening or residual audition |
| `sotf-plugin-hiss-reducer` | Time-domain expansion or WOLA Wiener filtering, tonal protection | No profile capture, per-frequency curve or channel linking; tests do measure >2 dB clean-reference SNR improvement and <1 dB wanted-tone loss |
| `sotf-plugin-speech-denoiser` | RNNoise at 48 kHz, linked 22-band stereo gains, VAD; accepted 960-frame EOF rendering policy for queued program | No suppression strength or model selection; speech-corpus SI-SDR/STOI evidence missing. AUD136 verifies timing, full output and compatibility at 48 kHz, not independent model quality or finite natural response; see `audit/speech-denoiser-enabled-accepted-queue.md` |

Primary comparison sources: [Pro-Q](https://www.fabfilter.com/help/pro-q),
[spectral dynamics](https://www.fabfilter.com/help/pro-q/using/spectral-dynamics),
[RX spectral denoise](https://docs.izotope.com/rx11/en/spectral-de-noise.html),
[RX declick](https://docs.izotope.com/rx11/en/de-click.html),
[Pristine Space](https://www.voxengo.com/doc/pspace/),
and [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet).

### Spatial scope

The Ambisonics decoder's bounded implementation now covers ACN/SN3D orders 1–7,
regularized mode matching, AllRAD/VBAP, max-rE weighting and a dual-band
crossover for existing named layouts. AUD-133's named-layout orders 4–7,
64-channel AIFF→decoder→engine→plugin route, service-PCM admission boundaries,
and graph-model/canvas roundtrip have scoped acceptance and a green coordinated
workspace gate. Native bridge/FFI/NIH routes and arbitrary user-defined
loudspeaker layouts remain open parity gaps against the
[IEM AllRADecoder](https://plugins.iem.at/docs/allradecoder/). Its primary guide
also documents layout/decoder export and imaginary speakers; corresponding
end-to-end SOTF workflows still need inspection before a conclusion.
Custom layouts and those workflows remain separate open work.

Binaural rendering already includes SOFA selection, transactional head
tracking and room reflections. Upmixing includes direct/ambient decomposition
and VBAP. Comparisons with [dearVR PRO 2](https://www.sennheiser.com/de-de/immersive/dear-reality)
and [Halo Upmix](https://nugenaudio.com/haloupmix/) need independent spatial,
dialogue-preservation and downmix-compatibility measurements; feature names
alone do not establish equivalent quality.

## Workspace inventory

All 59 included workspace crates are listed below. The integration-file count is navigation help, not an accuracy verdict; unit tests are not counted. Feature comparison remains open unless noted in the issue table. Platform support and vendored dependencies require separate verification.

| Crate | Responsibility | Integration files | Diagnostic binaries |
|---|---|---:|---|
| [sotf-host](crates/sotf-plugins/crates/sotf-host/Cargo.toml) | Core traits, host, and shared utilities for SOTF audio plugins | 9 | qa-host |
| [sotf-plugin-ambisonics](crates/sotf-plugins/crates/sotf-plugin-ambisonics/Cargo.toml) | SOTF Ambisonics Decoder plugin - selectable mode matching or AllRAD/VBAP from HOA to speaker layouts | 1 | qa-ambisonics |
| [sotf-plugin-ab-compare](crates/sotf-plugins/crates/sotf-plugin-ab-compare/Cargo.toml) | SOTF AB Compare plugin - A/B comparison | 1 | qa-ab-compare |
| [sotf-plugin-aae](crates/sotf-plugins/crates/sotf-plugin-aae/Cargo.toml) | SOTF Active Acoustic Enhancement plugin - LARES-inspired multichannel reverb | 1 | qa-aae, qa-aae-quality, qa-aae-validation |
| [sotf-plugin-aec](crates/sotf-plugins/crates/sotf-plugin-aec/Cargo.toml) | SOTF Acoustic Echo Cancellation plugin - PBFDAF with two-path and post-filter | 1 | qa-aec |
| [sotf-plugin-analog-common](crates/sotf-plugins/crates/sotf-plugin-analog-common/Cargo.toml) | Shared analog coloration stage for the sotf-plugin-analog-* family | 0 | — |
| [sotf-plugin-analog-compressor](crates/sotf-plugins/crates/sotf-plugin-analog-compressor/Cargo.toml) | SOTF Analog Compressor — single-band compressor with analog coloration stage | 1 | — |
| [sotf-plugin-analog-eq](crates/sotf-plugins/crates/sotf-plugin-analog-eq/Cargo.toml) | SOTF Analog EQ — 4-band parametric EQ with analog coloration stage | 1 | — |
| [sotf-plugin-analog-limiter](crates/sotf-plugins/crates/sotf-plugin-analog-limiter/Cargo.toml) | SOTF Analog Limiter — mastering limiter with analog coloration stage | 1 | — |
| [sotf-plugin-band-merge](crates/sotf-plugins/crates/sotf-plugin-band-merge/Cargo.toml) | SOTF Band Merge plugin - merge frequency bands | 3 | qa-band-merge |
| [sotf-plugin-beamformer](crates/sotf-plugins/crates/sotf-plugin-beamformer/Cargo.toml) | SOTF Beamformer plugin - MVDR, superdirective, and GSC beamformers | 1 | qa-beamformer |
| [sotf-plugin-band-split](crates/sotf-plugins/crates/sotf-plugin-band-split/Cargo.toml) | SOTF Band Split plugin - split signal into frequency bands | 3 | qa-band-split |
| [sotf-plugin-binaural](crates/sotf-plugins/crates/sotf-plugin-binaural/Cargo.toml) | SOTF Binaural Decoder plugin - HRTF-based binaural rendering | 3 | qa-binaural |
| [sotf-plugin-channel-mute-solo](crates/sotf-plugins/crates/sotf-plugin-channel-mute-solo/Cargo.toml) | SOTF Channel Mute/Solo plugin - per-channel mute/solo/dim | 1 | qa-channel-mute-solo |
| [sotf-plugin-convolution](crates/sotf-plugins/crates/sotf-plugin-convolution/Cargo.toml) | SOTF Convolution plugin - FFT-based convolution for IR processing | 1 | qa-convolution |
| [sotf-plugin-crossfeed](crates/sotf-plugins/crates/sotf-plugin-crossfeed/Cargo.toml) | SOTF Crossfeed plugin - stereo crossfeed for headphones | 2 | qa-crossfeed |
| [sotf-plugin-crossover](crates/sotf-plugins/crates/sotf-plugin-crossover/Cargo.toml) | SOTF Crossover plugin - frequency band splitting | 3 | qa-crossover |
| [sotf-plugin-delay](crates/sotf-plugins/crates/sotf-plugin-delay/Cargo.toml) | SOTF Delay plugin - audio delay with feedback | 4 | qa-delay |
| [sotf-plugin-de-esser](crates/sotf-plugins/crates/sotf-plugin-de-esser/Cargo.toml) | SOTF De-Esser plugin - sibilance reduction | 2 | qa-de-esser |
| [sotf-plugin-dynamic-eq](crates/sotf-plugins/crates/sotf-plugin-dynamic-eq/Cargo.toml) | SOTF Dynamic EQ plugin - frequency-selective dynamics | 1 | qa-dynamic-eq |
| [sotf-plugin-denoiser](crates/sotf-plugins/crates/sotf-plugin-denoiser/Cargo.toml) | SOTF Denoiser plugin - audio denoising (MCRA/Wiener) | 1 | qa-denoiser |
| [sotf-plugin-speech-denoiser](crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/Cargo.toml) | SOTF speech denoiser plugin - RNNoise voice denoising | 2 | qa-speech-denoiser |
| [sotf-plugin-hiss-reducer](crates/sotf-plugins/crates/sotf-plugin-hiss-reducer/Cargo.toml) | SOTF hiss reducer plugin - stationary high-frequency noise reduction | 3 | qa-hiss-reducer |
| [sotf-plugin-declick](crates/sotf-plugins/crates/sotf-plugin-declick/Cargo.toml) | SOTF declick plugin - time-domain click and transient repair | 2 | qa-declick |
| [sotf-plugin-dither](crates/sotf-plugins/crates/sotf-plugin-dither/Cargo.toml) | SOTF Dither plugin - TPDF dither with F-weighted noise shaping for bit-depth reduction | 1 | qa-dither |
| [sotf-plugin-downmix](crates/sotf-plugins/crates/sotf-plugin-downmix/Cargo.toml) | SOTF Downmix plugin - multichannel to stereo downmixing | 2 | qa-downmix |
| [sotf-plugin-eq](crates/sotf-plugins/crates/sotf-plugin-eq/Cargo.toml) | SOTF EQ plugin - parametric equalizer with biquad filters | 4 | qa-eq |
| [sotf-plugin-gain](crates/sotf-plugins/crates/sotf-plugin-gain/Cargo.toml) | SOTF Gain plugin - simple volume control with per-channel support | 3 | qa-gain |
| [sotf-plugin-gate](crates/sotf-plugins/crates/sotf-plugin-gate/Cargo.toml) | SOTF Gate plugin - noise gate | 4 | qa-gate |
| [sotf-plugin-hal-input](crates/sotf-plugins/crates/sotf-plugin-hal-input/Cargo.toml) | SOTF HAL Input plugin - macOS audio HAL input | 0 | — |
| [sotf-plugin-hal-output](crates/sotf-plugins/crates/sotf-plugin-hal-output/Cargo.toml) | SOTF HAL Output plugin - macOS audio HAL output | 0 | — |
| [sotf-plugin-linear-phase-eq](crates/sotf-plugins/crates/sotf-plugin-linear-phase-eq/Cargo.toml) | SOTF Linear-Phase EQ plugin - parametric EQ with FIR convolution for zero phase distortion | 2 | qa-linear-phase-eq |
| [sotf-plugin-limiter](crates/sotf-plugins/crates/sotf-plugin-limiter/Cargo.toml) | SOTF Limiter plugin - peak limiter | 5 | qa-limiter |
| [sotf-plugin-loudness-compensation](crates/sotf-plugins/crates/sotf-plugin-loudness-compensation/Cargo.toml) | SOTF Loudness Compensation plugin - equal loudness contour compensation | 2 | qa-loudness-compensation |
| [sotf-plugin-matrix](crates/sotf-plugins/crates/sotf-plugin-matrix/Cargo.toml) | SOTF Matrix plugin - channel matrix mixing | 5 | qa-matrix |
| [sotf-plugin-mono-to-stereo](crates/sotf-plugins/crates/sotf-plugin-mono-to-stereo/Cargo.toml) | SOTF Mono to Stereo plugin - mono to stereo widening | 1 | qa-mono-to-stereo |
| [sotf-plugin-multiband-compressor](crates/sotf-plugins/crates/sotf-plugin-multiband-compressor/Cargo.toml) | SOTF Multiband Compressor plugin - multiband dynamic range compression | 2 | qa-multiband-compressor |
| [sotf-plugin-multiband-expander](crates/sotf-plugins/crates/sotf-plugin-multiband-expander/Cargo.toml) | SOTF Multiband Expander plugin - multiband dynamic range expansion | 3 | qa-multiband-expander |
| [sotf-plugin-pnd](crates/sotf-plugins/crates/sotf-plugin-pnd/Cargo.toml) | SOTF duration-preserving pitch-drift correction plugin | 2 | qa-pnd |
| [sotf-plugin-resampler](crates/sotf-plugins/crates/sotf-plugin-resampler/Cargo.toml) | SOTF Resampler plugin - sample rate conversion | 2 | qa-resampler |
| [sotf-plugin-saturation](crates/sotf-plugins/crates/sotf-plugin-saturation/Cargo.toml) | SOTF Saturation / Harmonic Exciter plugin | 1 | qa-saturation |
| [sotf-plugin-spectral-compressor](crates/sotf-plugins/crates/sotf-plugin-spectral-compressor/Cargo.toml) | SOTF Spectral Compressor plugin - per-bin frequency domain dynamics | 2 | qa-spectral-compressor |
| [sotf-plugin-stereo-imager](crates/sotf-plugins/crates/sotf-plugin-stereo-imager/Cargo.toml) | SOTF Stereo Imager plugin - multi-band M/S stereo width control | 1 | qa-stereo-imager |
| [sotf-plugin-transient-shaper](crates/sotf-plugins/crates/sotf-plugin-transient-shaper/Cargo.toml) | SOTF Transient Shaper plugin - SPL Transient Designer approach | 2 | qa-transient-shaper |
| [sotf-plugin-upmixer](crates/sotf-plugins/crates/sotf-plugin-upmixer/Cargo.toml) | SOTF Upmixer plugin - stereo to surround upmixing | 4 | qa-upmixer |
| [sotf-plugin-xtc](crates/sotf-plugins/crates/sotf-plugin-xtc/Cargo.toml) | SOTF XTC plugin - crosstalk cancellation | 4 | qa-xtc |
| [sotf-plugins](crates/sotf-plugins/Cargo.toml) | an audio player and recorder that support audio plugins | 31 | — |
| [sotf-engine](crates/sotf-engine/Cargo.toml) | an audio player and recorder that support audio plugins | 35 | — |
| [sotf-testkit](crates/sotf-testkit/Cargo.toml) | Shared test fixtures and helpers for the SOTF workspaces | 0 | — |
| [sotf-test](crates/sotf-test-macros/Cargo.toml) | Proc-macro test tagging attributes for the SOTF workspace | 0 | — |
| [sotf-streaming](crates/sotf-streaming/Cargo.toml) | HTTP streaming input and live PCM output for SOTF audio engine | 0 | — |
| [plugins-bridge](crates/sotf-plugins/crates/plugins-bridge/Cargo.toml) | Format-agnostic adapter for SOTF audio plugins (AU, VST3, CLAP) | 2 | — |
| [plugins-denoiser](crates/sotf-plugins/crates/plugins-denoiser/Cargo.toml) | Shared denoiser DSP blocks for SOTF plugins | 0 | — |
| [plugins-ffi](crates/sotf-plugins/crates/plugins-ffi/Cargo.toml) | C FFI bindings for SOTF audio plugins (for Audio Unit integration) | 0 | — |
| [plugins-gpui](crates/sotf-plugins/crates/plugins-gpui/Cargo.toml) | Common GPUI rendering infrastructure for SOTF audio plugin UIs | 0 | — |
| [plugins-inference](crates/sotf-plugins/crates/plugins-inference/Cargo.toml) | Shared async ML inference bridge for audio plugins (non-blocking audio thread, worker thread model) | 0 | — |
| [plugins-nih](crates/sotf-plugins/crates/plugins-nih/Cargo.toml) | VST3/CLAP plugin wrappers for SOTF audio plugins via nih-plug | 0 | — |
| [plugins-spatial](crates/sotf-plugins/crates/plugins-spatial/Cargo.toml) | Shared spatial DSP blocks for SOTF plugins | 0 | — |
| [driver-common](crates/driver-common/Cargo.toml) | Platform-agnostic audio driver trait for system-wide audio capture | 0 | — |
| [driver-hal](crates/driver-hal/Cargo.toml) | Shared memory interface for Swift HAL driver communication | 2 | — |

### De-esser integration and precision

The engine settings enum, parameter index accessors, preset defaults and wire
converter now carry range and stereo link through to the DSP factory. Legacy
presets retain independent channels and a 60 dB range. The plugin numerical
tests exposed gain-conversion approximation error, corrected with `exp2`.
The tests cover both modes at 44.1/48/96 kHz and realtime allocation constraints.

### Limiter true-peak follow-up

An independent f64 windowed-sinc reconstruction of 12 kHz bursts at 48 kHz
found output of -5.485 dBTP for a -6 dB ceiling and 6-frame lookahead. A 96 kHz
two-tone burst also exceeded the ceiling by 0.171 dB with 24-frame lookahead.
The new predictive output stage corrects the gain-modulated signal before
emission. It adds 18 frames at 44.1/48/88.2 kHz, 36 at 96 kHz and zero at
192 kHz; ISP switching is structural where it changes latency. Tests measure
the final reconstructed output over 320 rate/lookahead/release/signal/phase
combinations against the unchanged +0.1 dB tolerance. Low-level output remains
bit-exact after delay, and threshold automation and channel-link checks pass.

### Additional feature comparisons

- [Pro-C dynamics controls](https://www.fabfilter.com/help/pro-c/using/dynamicscontrols) and [timing controls](https://www.fabfilter.com/help/pro-c/using/timecontrols) provide comparison points for range and hold.
- De-esser lookahead, linear-phase split, M/S processing and external sidechain remain missing compared with [Pro-DS](https://www.fabfilter.com/help/pro-ds).
- The Linux driver tests cover the fallback/configuration contract; zero driver-hal tests run on this platform. Native macOS HAL accuracy and callback behavior remain unverified.

### Dither precision and inference ownership

The unshaped TPDF test measures final quantized output against the independent
zero-mean and quarter-LSB² second-moment result in
[Lipshitz, Wannamaker and Vanderkooy, JAES 40(5), section 5.3](https://hajim.rochester.edu/ece/sites/zduan/teaching/ece472/reading/Lipshitz_1992.pdf).
At input 0.75 the old f32 addition produced approximately +0.0175 LSB mean
error at 20 bits and +0.2507 LSB at 24 bits. f64 guard arithmetic before
quantization fixes the signal-dependent rounding bias. The oracle covers
eight signed/sub-LSB input levels at 16/20/24 bits, 262,144 samples per
channel, with mean and second-moment errors below 0.008 in LSB units.
Dither release QA still passes with zero allocations and 0.26% measured CPU
for its fixture on this host; that measurement is not a cross-platform bound.

Async inference now provides `try_send` to return a full-queue input unchanged
and `with_latest` to borrow a result without cloning. Reset only invalidates
the generation; the worker disposes of stale output. Regression tests verify
original buffer ownership, contention behavior, no result clone, and disposal
on the worker. Generic `send`/`latest` retain their convenience semantics;
their input destructors/output clones must be realtime safe. Construction,
shutdown and handle destruction belong on the control thread.

### Wrapper and FFI boundaries

The NIH parameter bridge now preserves float/int/bool types independently of
UI step counts. Default construction and parameter synchronization are checked
for all 43 packaged wrappers. Generated wrappers declare actual DSP channel
counts and use an auxiliary input bus where NIH's main buffer cannot retain
extra inputs. Ten layout families are compared with direct DSP output across
1–257-frame blocks and asymmetric channel signals.

Ordinary EQ exports 20 neutral bands, restores structural settings before
activation, and reuses transition/filter/cache storage during live band edits.
The generic NIH constructor restores exposed numeric structural settings and
rejects incompatible layouts or unsupported controls explicitly. Setup fields
retain their existing hidden/non-automatable policy. Hosts must reactivate after
changing structural saved state; the callback rejects mismatched state without
rebuilding the DSP there. Automatic host reactivation is still missing.

The FFI fixed-frame API now accepts success only for the exact requested frame
count, and clears the destination on DSP failure or mismatched count. Injected
partial/overlong/error fixtures cover mono, stereo and surround layouts without
callback allocation. Generic FFI preset restore now stages a candidate with retained construction
configuration and live settings, committing only after validation. Failure
preserves the active DSP envelope, configuration and parameter metadata. Tests
cover partial updates, external IRs, asymmetric routing, Crossfeed preset actions
and all saturation oversampling factors. Missing analog-family parameter maps
were also added so their advertised FFI constructors no longer panic.

### Cold callbacks and sample clocks

The earlier complete workspace run executed 5,194 tests: 5,193 passed, one failed,
and 12 were skipped. The failed isolated EQ allocation test exposed ArcSwap
thread-local allocation during first metering publication. The replacement
cache keeps a producer-local snapshot and bounded spare storage, publishes with
`try_lock`, and skips publication under reader contention. Its shared reader
handle now has type `Arc<SharedCache<T>>` instead of `Arc<ArcSwap<T>>`; `load()`
and `load_full()` remain available. This is a Rust source API change. Five cache
regressions and the cold EQ test pass. The final gate for this implementation
batch passes all 5,230 tests across 245 binaries, with 11 tests skipped.

Host sample positions now advance by actual accepted frames, including fractional
rate conversion and buffered emissions, instead of separately truncating the
transport position every callback. Latency accumulation uses a common integer
clock and rounds at the destination. Reset/seek/rebuild and cold native-f64
processing are covered. Integer compensation still leaves fractional alignment
residue below one destination frame. General DAG drain remains explicitly
unsupported. Oversampling adapters now translate sample/loop positions to the
inner rate and preserve the supplied musical origin and transport flags. Buffered
chunks use the current transport snapshot; metadata changes within a chunk are
not separately scheduled. Seek/loop discontinuities with pending input require
reset to avoid combining samples across the discontinuity. Branch retention
remains awaiting approval.

Saturation's native wrapper now restores all five modes and three oversampling
choices, honors requested oversampling before activation, and reports its
latency. Continuous scalar access no longer constructs maps. Negotiated callback
size tests cover 1, 127 and 8,192 frames without allocation. An independent
oversampler impulse check subsequently found callback-dependent actual delay
(peak frame 511 versus 256 for 1 versus 256-frame callbacks, reported 512 at 2x);
fixed initial buffering now passes impulse, partition, reset and allocation
checks across 2x/4x and mono/stereo/six-channel configurations.

Both oversampling adapters now drain partial input, the upsampling overlap,
the inner plugin's declared tail, the downsampling partial block and overlap,
then retained output. Each call performs at most one internal chunk or inner
drain step. The result includes deterministic chunk padding; empty-stream
drain is a no-op, while a nonempty drained stream requires reset before reuse.
An inner error latches a reset requirement and cannot expose unprocessed queued
audio. Tests compare 960 configurations against zero-padded finite-FIR
references, including variable and oversized inner tail bursts, 1-frame output
capacity, exact frame counts, native host integration, zero-frame progress,
capacity-error retry and reset. Allocation checks and host Clippy pass. This
preserves only the tail each inner plugin declares; plugins without their own
drain implementation still need that contract added. Fractional inner latency
now rounds up conservatively in the integer host clock. Independent impulse
first moments reproduced a reported 512 versus measured 512.5 frames; both
wrappers now bound reporting error below one frame for inner delays 0–7 at
2x/4x. Exact fractional-delay compensation remains separate.

### Compressor range and hold

Single-band, multiband and analog compressors now expose `range_db` and
`hold_ms`, appended to existing parameter indices. The 120 dB range setting
disables the cap to preserve legacy behavior exactly; hold defaults to zero.
Multiband band overrides inherit globals when omitted. Range limits reduction
before makeup/parallel mix, with an exact gain floor despite fast-math error.
Per-channel hold state is preallocated and reset without allocation. Independent
static gain, timed release/hold/retrigger, live range, reset and callback
partition tests pass; both allocations and deallocations measure zero during
new-control automation. Engine presets/accessors/factory conversion cover all
three variants and preserve per-band overrides. The engine converter also now
preserves the previously dropped multiband link amount and sidechain tilt.

## Sixth verification checkpoint

The in-scope workspace run executed **5,479 tests across 265 binaries: 5,478 passed,
one failed, and 10 were skipped**. MIDI and IAMF were excluded. The only failure
was the Gate layout snapshot after adding Mode and Max Boost. All audio, routing,
and numerical tests in that run passed. Log: `/tmp/sotf-audit-wave6-nextest.log`.

The ten Gate snapshots were then reviewed and updated. They add Mode and
conditionally enabled Max Boost; the existing layout solver moves lower-priority
groups into overflow at intermediate widths. No other controls or layout
properties changed. The full 25-test layout snapshot target passes without
snapshot-update mode (`/tmp/sotf-gate-layout-verification.log`). This is serialized
layout verification, not a native GUI rendering check.

All 239 changed in-scope Rust files passed the final rustfmt check; MIDI/IAMF and vendored sources were excluded. Focused final Clippy passes for engine, facade, NIH, bridge and C API. The bridge
and C tests were rerun after equivalent lint-only iteration changes. Gate DSP has
87 passing tests, including independent transfer/timing/partition oracles and
cold allocation/deallocation checks. The iOS feeder has ten portable tests using
the actual feeder and render callback; native AudioUnit/device execution remains
unverified. These results precede the subsequent limiter drain implementation.

- [Gate modes and numerical evidence](audit/gate-modes.md)
- [Gate external wiring checkpoint](audit/gate-external-wiring.md)
- [Native Gate keys and bus boundaries](audit/native-gate-sidechain.md)
- [Gate finite-stream correction](audit/gate-finite-stream.md)
- [iOS feeder and callback evidence](audit/ios-feeder.md)
- [Limiter oversampling measurements](audit/limiter-oversampling.md)
- [Limiter finite-stream correction](audit/limiter-finite-stream.md)
- [Shared RMS detector correction](audit/rms-detector.md)
- [External sidechain adapter correction](audit/sidechain-adapters.md)
- [Limiter finite-stream and oversampling design](audit/proposals/limiter-protection.md)
- [Asymmetric adapter proposal](audit/proposals/sidechain-adapters.md)

## Seventh verification checkpoint

The final in-scope workspace run passed **5,512 tests across 272 binaries**, with
10 skipped. MIDI and IAMF were excluded. Build time was 13.40 seconds and test
time 87.361 seconds. Log: `/tmp/sotf-audit-wave7-final-nextest.log`.

The preceding run had one failure: a facade smoke fixture processed a constructed
limiter without initialization. The factory returns construction-only instances;
host insertion initializes them at the actual input rate. Factory documentation
now states that contract, and all three direct-processing fixtures initialize
explicitly. The limiter fixture independently checks exactly 48 delayed frames
at 48 kHz, followed by bit-exact below-threshold program. All 22 factory tests
and focused warnings-denied Clippy pass. The production initialization guard
was retained.

This checkpoint includes the shared RMS detector correction, native Limiter and
Gate drain, asymmetric Gate adapter support, and actual CLAP/VST3 key and bus
boundary corrections. CLAP keeps legacy stereo configuration ID 0 and adds the
key bus at ID 1. VST3 uses the key layout initially through an additive default
layout hook. The final native Gate build passes 90 library tests and two native
auxiliary-output integration tests, with focused all-target Clippy clean.
Logs: `/tmp/sotf-native-gate-compatibility-tests.log` and
`/tmp/sotf-native-gate-compatibility-clippy.log`.

All 250 changed in-scope non-vendored Rust files passed rustfmt before the final
factory correction; the two factory files also passed their subsequent format
check. Scoped diff checks pass. The vendor RMS suite separately passes 641
tests; its two existing ignored doctests and two baseline-only Clippy exceptions
are documented in [the RMS report](audit/rms-detector.md).

This is a portable workspace and native-format harness checkpoint. Native
macOS/iOS audio-device execution and the two broader rejected host/engine
protocol proposals remain open. AUD-073 tracks further observed finite-stream
loss; passing this suite does not imply those unimplemented drains are correct.

## Eighth verification checkpoint

The in-scope workspace run passed **5,579 tests across 283 binaries**, with
10 skipped. MIDI and IAMF were excluded. Build time was 35.76 seconds and test
time 87.965 seconds. Log: `/tmp/sotf-audit-wave8-nextest.log`.

This checkpoint adds finite-stream preservation for FIR crossover, FIR EQ,
finite Delay, Convolution, Declick, SpectralCompressor, and eligible zero-color,
single-band or settled-dry dynamics states. Recursive feedback/color/crossover
states remain explicitly unsupported by automatic drain. SpectralCompressor now
includes the required negative-origin startup windows at unchanged latency;
7,168 first/final impulse phases and dense unity reconstruction pass.

Independent direct-convolution and delayed-identity oracles cover the new FIR
paths, and a factory-created Gate→FIR EQ→Limiter chain passes 96 waveform runs
across phase modes, bypass combinations, rates, channels and callback partitions.
Convolution's first callback and completion adoption now retain heap ownership
without audio-thread allocation or deallocation. Focused all-target Clippy
passes for all changed production crates. All 272 changed in-scope non-vendored
Rust files pass rustfmt; scoped diff checks pass.

The additive offline tail API passes analytic feedback-echo, exact duration,
progress and source/chain resampling checks. Subsequent independent review
identified two more endpoint-composition cases (AUD082); their follow-up is
outside this checkpoint. Newly reproduced XTC and Downmix startup/acceptance
defects (AUD080/081), other buffered families, the engine drain-call limit,
native-device execution and the two previously rejected protocol proposals
remain open. This checkpoint does not establish complete SOTA parity.

## Ninth verification checkpoint

The in-scope workspace run passed **5,632 tests across 293 binaries**, with
10 skipped. MIDI and IAMF were excluded. Build time was 2.30 seconds and test
time 88.775 seconds. Log: `/tmp/sotf-audit-wave9-final-nextest.log`.

This checkpoint includes XTC's fixed sample clock and full input acceptance,
Downmix and Denoiser startup reconstruction, Denoiser/spectral Hiss finite
tails, tonal-analysis callback invariance, HPSS reset, Denoiser persistent
configuration wiring, empty-bank EQ oversampling drain and cold capacity,
offline endpoint composition, and host drain capacity validation before DSP.
Focused strict Clippy passes for the changed crates and public configuration
routes. Source and evidence reports retain each fix's supported-mode limits.
All 290 changed in-scope non-vendored Rust files pass rustfmt; scoped diff
checks pass. Formatting log: `/tmp/sotf-audit-wave9-format-final.log`.

The first run passed 5,630 tests and failed two older XTC facade expectations
that compared delayed output to the same input index. The fixtures now assert
the declared delay, exact startup zeros and the full corresponding source
prefix, retaining their original accuracy thresholds. Both suites pass all
10 tests and focused strict Clippy; independent review found no blocker.
The first run remains recorded in `/tmp/sotf-audit-wave9-nextest.log`.

PND/Downmix finite drains, SpeechDenoiser dry/wet alignment, the engine's long
tail call budget, A/B variable-rate path composition, native device execution,
and the previously rejected protocol proposals remain outside this checkpoint.
It does not establish complete SOTA parity.

## Tenth verification checkpoint

The in-scope workspace passed **5,702 tests across 301 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 6.54 seconds; test time
91.496 seconds. Log: `/tmp/sotf-audit-wave10-final-nextest.log`.

This checkpoint includes prepared per-stage drain work contracts, monotone
completed-prefix traversal, bounded oversampling preparation, native work bounds,
engine removal of the global 4,096-call cap, PND and eligible Downmix finite
responses, Downmix validation and structural capacity, and SpeechDenoiser dry/wet
alignment. A real 30-second/192 kHz convolution impulse response completes in
5,626 native drain calls with its final marker preserved. Focused strict Clippy
and cold allocation/deallocation checks pass; reports state each supported mode.
All 306 changed in-scope non-vendored Rust files pass formatting, and scoped diff
checks pass. Logs: `/tmp/sotf-audit-wave10-format.log` and
`/tmp/sotf-audit-wave10-diff-check.log`.

The first run passed 5,700 tests and failed two older SpeechDenoiser native-wrapper
fixtures that still expected a 480-frame dry delay. Their independent expected
model-plus-queue delay is now 960 frames. Exact waveform, startup, sentinel and
allocation assertions retain their original strength; independent review found
no blocker. Both focused tests and strict wrapper Clippy pass. The first result
is retained in `/tmp/sotf-audit-wave10-nextest.log`.

The separately reproduced terminal same-format host-replacement defect, XTC
stale-rate publication race, XTC finite response, and MultibandExpander spectral
startup/tail work remain outside this checkpoint. A/B variable-rate composition,
recursive-tail policies, native-device execution and the two approval-blocked
protocol proposals remain open. This checkpoint does not establish complete
SOTA parity.

## Eleventh verification checkpoint

The in-scope workspace passed **5,745 tests across 308 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 21.92 seconds; test time
91.913 seconds. Log: `/tmp/sotf-audit-wave11-nextest.log`.

This checkpoint includes the terminal same-format engine replacement fix, XTC
finite EOS and stale-rate generation invalidation, MultibandExpander spectral
startup and cached finite drain, disabled SpeechDenoiser finite output, and
honest zero-tail declarations for BandMerge, TransientShaper and single-band
Ambisonics. Focused strict Clippy passes throughout; spectral MultibandExpander
also passes release QA. Independent source reviews found no remaining blocker.
All 318 changed in-scope non-vendored Rust files pass formatting; scoped diff
checks pass. Logs: `/tmp/sotf-audit-wave11-format.log` and
`/tmp/sotf-audit-wave11-diff-check.log`.

New real SpeechDenoiser/Downmix/Limiter chains pass 48 independent waveform,
latency, composed-support and bypass/reset runs, alongside the prior 96 Gate/FIR
EQ/Limiter runs. XTC includes independent neutral and nonidentity transform
oracles plus deterministic actual-worker publication tests. Full reports retain
measured errors, supported modes, exact counts and cold heap evidence.

Review caught a new MultibandExpander tail-metadata edge while a fade reached dry
inside a canonical drain refill. A permanent regression now retains full spectral
support metadata for that epoch, including unread cached wet output, and clears
it on reset. DSP sample arithmetic did not change for that correction.

The separately reproduced MultibandExpander upper soft-knee discontinuity
(AUD095) remains outside this checkpoint. A/B variable-rate composition, XTC
hard-disabled latency and ordinary AutoGain cadence, synchronous source-load
error handling, recursive-tail policies, native-device execution and the two
approval-blocked protocol proposals remain open. This checkpoint does not
establish complete SOTA parity.

## Twelfth verification checkpoint

The in-scope workspace passed **5,766 tests across 311 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 17.64 seconds; test time
93.074 seconds. Log: `/tmp/sotf-audit-wave12-nextest.log`.

This checkpoint includes the MultibandExpander centered soft-knee correction,
AEC native finite-tail declaration, transactional XTC source initialization,
and Beamformer GSC/MVDR numerical recovery. Each crate's full focused suite and
strict all-target/all-feature Clippy pass. Expander and Beamformer release QA
also pass. All 324 changed in-scope non-vendored Rust files pass formatting;
scoped diff checks pass. Logs: `/tmp/sotf-audit-wave12-format.log` and
`/tmp/sotf-audit-wave12-diff-check.log`.

Independent expander curves now measure a 0.033 dB settled gain change where
the old code jumped 4.179 dB. AEC ordinary-zero tests cover ongoing adaptation
and post-filter transitions. XTC source failures preserve live and partial-EOS
audio plus actual pending worker results. Beamformer exact silence and f64
oracles cover the previously failing low-energy and finite extreme ranges;
wider GSC state increases measured kernel cost by 9.5–11.7%, with local release
callback times remaining well within their deadline. Reports give fixture
counts, errors, cold allocation/free evidence and untested limits.

AUD099 separately reproduces XTC disabled latency and stale-history replay,
including split impulses in an actual host PDC graph. Its aligned warm-bypass
implementation follows this checkpoint. A/B variable-rate composition, normal
XTC AutoGain callback cadence, recursive-tail policies, native-device execution,
and the two approval-blocked protocol proposals remain open. This checkpoint
does not establish complete SOTA parity.

## Thirteenth verification checkpoint

The in-scope workspace passed **5,787 tests across 315 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 26.21 seconds; test time
93.421 seconds. Log: `/tmp/sotf-audit-wave13-nextest.log`.

This checkpoint includes aligned warm XTC bypass, AEC process-clock rejection,
Limiter engine control forwarding, Beamformer native finite-tail metadata and
the private Limiter kernel extraction. Full focused suites and strict Clippy
pass; XTC release QA passes. All 334 changed in-scope non-vendored Rust files
pass formatting, and scoped diff checks pass. Logs:
`/tmp/sotf-audit-wave13-format.log` and
`/tmp/sotf-audit-wave13-diff-check.log`.

XTC disabled audio now follows its advertised latency and actual host PDC.
Independent enabled comparisons preserve 2,345,760 output samples exactly;
disabled CPU now includes continuously warm wet processing. The limiter's
96 captured native audio/telemetry/EOS baselines remain exact after extraction.
This checkpoint adds no new 2x/4x limiter processing. Beamformer metadata proves
finite audio support; AUD101's separately reproduced covariance poisoning and
transient spectral overflow remain subsequent numerical work.

The pending limiter composition, A/B variable-rate composition, ordinary XTC
AutoGain cadence, recursive-tail policies, native-device execution and the two
approval-blocked protocol proposals remain open. Complete SOTA parity is not
established.

## Fourteenth verification checkpoint

The in-scope workspace passed **5,841 tests across 324 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 19.83 seconds; test time
97.860 seconds. Log: `/tmp/sotf-audit-wave14-nextest.log`.

This checkpoint includes prepared limiter-local 2x/4x audio processing and its
engine, bridge, C ABI, NIH and toolbar routes; protected Beamformer covariance;
rate-correct AEC constructor preparation; and continuous, causal XTC AutoGain
measurement. Limiter's full focused suite passed 150 tests before the final
wire-form regression; the aggregate includes that additional test (151 total).
Strict package lint, release oracles/QA and independent reviewers support the
individual reports. All 354 changed in-scope non-vendored Rust files pass
formatting; scoped diff checks pass. Logs:
`/tmp/sotf-audit-wave14-format.log` and
`/tmp/sotf-audit-wave14-diff-check.log`.

The first build found an exhaustive analog-limiter fixture missing the new
default field. The first completed run then passed 5,838 tests and exposed two
integration omissions: toolbar choice labels and limiter layout snapshots.
The shared choice deserializer now normalizes accepted constructor labels to
the canonical integer; live scalar typing remains unchanged. Ten snapshots
were reviewed, including the existing 844-pixel overflow decision. Both former
failures pass in the final run. Earlier logs are retained as
`/tmp/sotf-audit-wave14-build-first.log` and
`/tmp/sotf-audit-wave14-first-tests.log`.

The limiter preserves native 1x audio/telemetry/EOS baselines and demonstrates
selected alias reductions while retaining final sample/finite-reconstruction
ceilings. It does not establish universal alias improvement or equivalence to
proprietary processors. Its added CPU cost, conservative gain-meter semantics
and structural reactivation requirements are documented. XTC AutoGain audio
intentionally changes from the earlier callback-dependent behavior; measured
CPU cost and exact disabled-path preservation are documented separately.

AUD105 shared AutoGain smoothing and AUD106 standalone correlation sample loss
have been reproduced and planned but are **not implemented at this checkpoint**.
A/B variable-rate composition, recursive-tail policies, native-device execution
and the two approval-blocked protocol proposals remain open. Complete SOTA
parity is not established.

## Fifteenth verification checkpoint

The in-scope workspace passed **5,942 tests across 345 binaries**, with 10
skipped. MIDI and IAMF were excluded. Build time was 52.82 seconds; test time
103.994 seconds. Log: `/tmp/sotf-audit-wave15-nextest.log`.

This checkpoint includes AUD105–116: shared gain smoothing/precision, causal
EQ/Crossfeed/AAE/Upmixer measurement, correlation ingestion and prepared
publication, SOFA integer propagation delays, corrected HRIR resampling, and
spectrum endpoint energy. Focused package suites, strict Clippy, independent
oracles and the individual reports qualify each result. Enabled AutoGain audio
changes deliberately; measured EQ/Crossfeed/XTC CPU costs are documented.
The AAE/Upmixer matched CPU comparison is still pending.

The format pass found an obsolete wrong-directory XTC draft, confirmed by its
author and preserved in `/tmp` before removal. The remaining format differences
were from subsequent, unfinished AUD117 work; they are not claimed as a clean
format checkpoint. Scoped diff checking passed. Later AUD117–120 work is not
covered by this test run.

Remaining work includes Loudness Range, the small-FFT initialization panic,
long-running meter storage, AutoGain's unused metering work, A/B variable-rate
composition, recursive-tail policies, native-device execution and the two
approval-blocked protocol proposals. Complete SOTA parity is not established.

## Sixteenth verification checkpoint

The in-scope workspace passed **5,954 tests across 347 binaries**,
with 10 skipped. MIDI and IAMF were excluded. Build time was 47.38 seconds;
test time 103.822 seconds. Log: `/tmp/sotf-audit-wave16-nextest.log`.

This checkpoint adds optional prepared Loudness Range, complete integrated
history reservation, public EBU synthetic/role/timeline checks, serialization
compatibility, and cold/full-history/retained-reader allocation regressions.
All 399 changed in-scope non-vendored Rust files pass formatting; scoped diff
checks pass. Logs: `/tmp/sotf-audit-wave16-format.log` and
`/tmp/sotf-audit-wave16-diff-check.log`. The vendored one-line reserve change
also passes its 24 existing EBU tests and backend library lint with the two
previously documented upstream exceptions. Host strict all-target Clippy passes.

LRA's default history adds 576,000 prepared bytes per actual analyzer. The
integrated reserve correction adds 240,000 prepared bytes per integrated meter.
The measured maximum-capacity LRA dirty query is approximately 255 µs median
locally, with 335 µs worst observed; no universal callback-deadline claim is
made. Authentic EBU programme files remain unavailable after HTTP 403.

AUD118 small-FFT initialization, AUD120 AutoGain meter cost, matched AAE/Upmixer
CPU timing, A/B variable-rate composition, recursive-tail policies, native-device
execution and the two approval-blocked protocols remain open. No complete SOTA
parity claim is made.

## Seventeenth verification checkpoint

The two in-scope nextest invocations passed **5,969 tests across 350 binaries**,
with 10 skipped. The workspace run excluding FFI, MIDI and IAMF passed 5,903
tests across 349 binaries; the separate FFI run passed all 66 tests in one
binary. MIDI and IAMF package tests remained excluded. Logs:
`/tmp/sotf-audit-wave17-nextest.log` and
`/tmp/sotf-audit-wave17-ffi-nextest.log`.

The workspace rebuilt in 1m25s and tested in 80.954s; FFI built in 19.81s and
tested in 1.320s. The former external Cargo target was deleted outside this
audit. These checks use the ignored workspace-owned
`crates/sotf-plugins/target`, with offline dependency resolution. Earlier logs
remain valid historical evidence, but the old cached executables are gone.

This checkpoint adds the AUD118 small surround FFT correction and AUD120 private
AutoGain meter optimization. The 34 Upmixer compatibility cases and 52 warmed
EQ cases retain exact audio. All 17.28 million captured public gain values and
their telemetry also match. Independent generic-meter, complex DFT, lifecycle,
malformed-input and cold/hour-long heap checks pass. Strict all-target Clippy
passes for the host and all seven direct AutoGain caller crates.

All 405 changed in-scope non-vendored Rust files pass formatting; scoped diff
checks pass. Logs: `/tmp/sotf-audit-wave17-format.log` and
`/tmp/sotf-audit-wave17-diff-check.log`.

Seven matched local CPU trials show native 48 kHz enabled EQ falling from
235.152 to 43.933 ns/frame, with a median paired ratio of 0.1868. Every measured
EQ case improves; the six-channel 192 kHz shared helper changes only slightly.
The detailed report specifies fixtures, profile, host and timing variation.

Historical AAE/Upmixer CPU timing, A/B variable-rate composition, recursive-tail
policies, minimum FFT geometry outside the verified sizes, native-device
execution and the two approval-blocked protocols remain open. Complete SOTA
parity is not established.

## Eighteenth verification checkpoint

The in-scope workspace passed **5,975 tests across 352 binaries**, with ten
skipped. FFI is included; MIDI and IAMF package tests are excluded. Build time
was 1m13s; test time was 79.379s. Log:
`/tmp/sotf-audit-wave18-nextest.log`.

AUD121 corrects both host and backend true-peak interpolation tables. The old
coefficient-copy oracle falsely supported an accuracy claim: actual public
meters overread a known 12 kHz/48 kHz tone by 4.675 dB. The new analytic and
explicit full-convolution regressions fail before and pass after the correction.
Existing peak-query ownership, prepared storage and other meter calculations
are retained. All 7,232 captured snapshots match after excluding the corrected
true-peak array, and fresh-thread processing/query/reset has zero heap activity.

The full host suite and 24 backend EBU tests pass. Host all-target Clippy is
strict; backend library Clippy passes with its two documented preexisting
exceptions. Formatting passes for 408 changed in-scope non-vendored Rust files
and the changed vendored constant file; scoped diff checks pass.

This establishes the published interpolation calculation, not complete external
programme certification or exact arbitrary-phase reconstruction. Unsupported
host true-peak rates, broader metering quality/feature comparisons, historical
AAE/Upmixer CPU timing, A/B variable-rate composition, recursive-tail policies,
minimum FFT geometry outside verified sizes, native-device execution and the
two approval-blocked protocols remain open. The full audit remains active.

## Nineteenth verification checkpoint

The in-scope workspace passed **5,988 tests across 353 binaries**, with ten
skipped. FFI is included; MIDI and IAMF package tests are excluded. Build time
was 36.89s; test time was 80.577s. Log:
`/tmp/sotf-audit-wave19-nextest.log`.

AUD122 completes true-peak interpolation at end of stream without emitting
audio or advancing other measurement clocks. A public final-impulse fixture
previously underread its complete FIR response by 30.455 dB. The low-level host
and backend meters now expose bounded finalization, the plugin publishes its
final peak interval, and the engine refreshes UI data after drain. Direct,
compiled and actual-worker regressions reproduce both original failures and
pass after the correction.

The independent final-position matrix covers 576 runs, including all supported
rates and 1/2/6/24 channels. Other metering fields remain identical in focused
controls. First finalization, retained-reader retries and reset have zero heap
activity in fresh-thread checks, including 96 plugin configurations. Full host
tests, all 68 processing-worker tests, and 24 backend EBU tests pass. Strict
host/engine all-target Clippy passes; backend library Clippy retains its two
preexisting exceptions. All 410 changed in-scope non-vendored Rust files and
the two changed vendored Rust files pass formatting; scoped diff checks pass.

Final snapshot publication remains best effort under retained readers; host
completion does not wait indefinitely for UI ownership. Unsupported true-peak
rates, exact reconstruction/external programme certification, historical
AAE/Upmixer CPU evidence, A/B variable-rate composition, recursive-tail policies,
minimum FFT geometry outside verified sizes, native-device execution and the
two approval-blocked protocols remain open. The full audit remains active.

## Detailed comparison reports

- [Finite-stream true peaks](audit/true-peak-finite-stream.md): complete interpolation response, zero added programme frames, final UI publication and lifecycle/heap evidence.

- [True-peak calibration](audit/true-peak-coefficients.md): corrected ITU FIR coefficients, analytic amplitude and independent convolution evidence, and the earlier test-oracle correction.

- [AutoGain measurement cost](audit/auto-gain-meter-cost.md): reduced private meters, exact gain/audio compatibility and matched CPU evidence.

- [Small surround FFTs](audit/upmixer-small-fft.md): corrected initialization geometry, independent complex response and unchanged ordinary-size waveforms.

- [Loudness Range](audit/loudness-range.md): explicit history/status, published synthetic signals, retained snapshots and measured query cost.

- [Integrated history preparation](audit/loudness-history-capacity.md): long-run allocation proof and complete bounded reserve.

- [EQ AutoGain clock](audit/eq-autogain-clock.md): continuous aligned metering, exact raw baselines and measured cost.

- [Spatial AutoGain clock](audit/spatial-autogain-clock.md): paired measurement, delayed Upmixer reference and exact raw controls.

- [Loudness Range comparison](audit/loudness-range-gap.md): missing feature, primary requirements and official-corpus availability.

- [Spectrum endpoint power](audit/spectrum-endpoint-power.md): inclusive upper-frequency mapping and independently calibrated band energy.

- [Crossfeed AutoGain clock](audit/crossfeed-autogain-clock.md): causal frame-clock measurement, exact raw controls and measured CPU cost.

- [SOFA delay loading](audit/sofa-delay.md): exact integer propagation delays, staged consumer initialization and cross-rate timing.

- [Fractional and signed SOFA delays (AUD-131)](audit/sofa-fractional-delay.md): signed causal rebasing, fractional phase oracle, exact integer preservation, Binaural/XTC lifecycle and honest workspace-gate qualification.

- [Multichannel AutoGain clock proof](audit/spatial-autogain-clock-proof.md): AAE/Upmixer partition and future-suffix errors with exact raw controls.

- [HRIR resampling](audit/sofa-resampling.md): physical-time preservation, delayed-response flushing and checked offline preparation.

- [Finite-stream inventory](audit/finite-stream-inventory.md): all 45 plugin crates, with 19 public suffix-loss probes across 17 families; records the source checkpoint before AUD-074 onward.

- [FIR crossover](audit/fir-crossover-finite-stream.md) and [FIR EQ](audit/fir-eq-finite-stream.md): independent convolution, finite support, lifecycle and real chain evidence.

- [Delay and Convolution](audit/delay-convolution-finite-stream.md): finite EOS, frozen IR adoption and callback ownership corrections.

- [Declick and SpectralCompressor](audit/declick-spectral-finite-stream.md): finite bounds and complete startup reconstruction.

- [Eligible dynamics](audit/dynamics-finite-stream.md): exact delayed program, nonlinear zero continuation, eligibility and recursive-state limits.

- [EQ finite stream](audit/eq-finite-stream.md): internal oversampling drain, parameterized adapter forwarding and maximum-block heap preparation.

- [Offline tail duration](audit/offline-tail-duration.md): explicit recursive export endpoint with unchanged default.

- [XTC stream clock](audit/xtc-stream-clock.md): startup reconstruction and complete callback input acceptance.

- [XTC finite stream and generation](audit/xtc-finite-generation.md): complete fixed-window response, frozen EOF publication and deterministic stale-rate rejection.

- [XTC initialization](audit/xtc-staged-initialize.md): fallible source preparation preserves failed live/drain epochs and pending desired work.

- [XTC aligned bypass](audit/xtc-aligned-bypass.md): fixed dry latency, continuously warm wet history, reversible fades and finite EOF support.

- [XTC AutoGain clock](audit/xtc-autogain-clock.md): continuous aligned measurement, causal sample-clock targets, independent finite-stream equality and measured CPU cost.

- [MultibandExpander spectral stream](audit/multiband-expander-spectral-finite-stream.md): complete startup windows, finite cached continuation and stable tail metadata.

- [MultibandExpander soft knee](audit/multiband-expander-soft-knee.md): continuous centered gain law, preserved hard-knee timing and independent spectral/DC accuracy.

- [Downmix startup](audit/downmix-startup.md): preceding-window reconstruction across layouts and spectral modes.

- [Downmix finite stream](audit/downmix-finite-stream.md): complete finite spectral support, exact LFE eligibility, lifecycle and bounded work.

- [PND finite stream](audit/pnd-finite-stream.md): frozen learned correction at EOF, complete retained synthesis and measured independent neutral accuracy.

- [Spatial drain work bounds](audit/spatial-drain-work-bounds.md): Binaural/Upmixer call-count declarations with actual long render caps and cached-output coverage.

- [Ambisonics tail support](audit/ambisonics-tail-support.md): zero-tail matrix decoding with recursive dual-band negative controls.

- [AEC native tail support](audit/aec-native-tail-support.md): finite ordinary zero-input support while adaptation and post-filter transitions continue.

- [AEC process clock](audit/aec-process-clock.md): wrong-rate ordinary callbacks preserve output, learned state and retained audio.

- [AEC constructor parity](audit/aec-constructor-parity.md): direct construction honors the reported learning rate and requested adaptive clock.

- [SpeechDenoiser latency](audit/speech-denoiser-latency.md): corrected dry alignment and metadata, unchanged wet model waveform and timing.

- [Disabled SpeechDenoiser finite stream](audit/speech-disabled-finite-stream.md): exact retained dry program and bounded fade continuation with an explicit enabled-wet limitation.

- [Denoiser and spectral Hiss](audit/denoiser-hiss-finite-stream.md): finite response, startup, tonal-analysis timing and explicit capture preservation.

- [Denoiser configuration](audit/denoiser-config-wiring.md): harmonic/percussive and spatial controls survive typed, factory, bridge and engine construction routes.

- [A/B variable-rate paths](audit/abcompare-variable-rate.md): public reproduction of an unresolved nested-path composition defect.

- [Host drain preflight](audit/host-drain-preflight.md): invalid output capacity is rejected before consuming retained audio.

- [Drain work contracts](audit/drain-work-contract.md) and [independent review](audit/drain-work-independent-review.md): per-stage quotas, bounded wrapper preparation, long finite tails and retained control responsiveness.

- [Dynamics drain bounds](audit/dynamics-drain-work-bounds.md) and [native facade coverage](audit/native-drain-bounds-facade.md): scalar progress declarations checked against actual full-capacity calls and partial cached output.

- [Utility plugins](audit/utility-plugins.md): eleven scoped capability and accuracy comparisons, with source-backed findings distinguished from executed measurements.

- [Zero audio tails](audit/zero-audio-tail-support.md): exact BandMerge/TransientShaper zero continuation while control/envelope state remains active.

- [Dynamics plugins](audit/dynamics-plugins.md): ratio laws, spectral detector calibration, analog model state and remaining numerical evidence.

- [Limiter engine controls](audit/limiter-engine-controls.md): persisted link/compatibility settings reach the DSP, with independent unlinked-channel audio verification.

- [Limiter native kernel](audit/limiter-native-kernel.md): private extraction preserves exact native audio, telemetry and EOS before adding the proposed oversampling path.

- [Limiter audio oversampling](audit/limiter-oversampling-implementation.md), [independent accuracy](audit/limiter-oversampling-accuracy.md), [telemetry review](audit/limiter-oversampling-telemetry-review.md), [engine and toolbar wiring](audit/limiter-oversampling-wiring.md), and [external routes](audit/limiter-oversampling-propagation.md): prepared 2x/4x processing, final protection, accepted control clocks, finite output, preserved native behavior and quantified limitations.

- [Spatial, adaptive and HAL plugins](audit/spatial-plugins.md): ten source-based comparisons, including finite-stream tails, SOFA delays and cold filter publication.

- [Beamformer numerical recovery](audit/beamformer-numerical-recovery.md): finite extreme-input recovery, prolonged exact silence, independent adaptive/weight oracles and measured CPU cost.

- [Beamformer native tail support](audit/beamformer-native-tail-support.md): finite signal support with ordinary adaptation, all hop phases and explicitly separate covariance-recovery limitations.

- [Beamformer covariance recovery](audit/beamformer-covariance-recovery.md): protected learned state, finite overload output, measured recovery duration and unchanged ordinary output.

- [Loudness compensation and metering](audit/metering-effects.md): published equal-loudness checkpoints, AutoGain feedback and analyzer/meter evidence.

- [Standalone correlation ingestion](audit/correlation-stream.md): complete multichannel measurement, independent centered Pearson accuracy and callback preflight.

- [Correlation realtime storage](audit/correlation-realtime.md): cold processing/reset without heap activity, immutable retained snapshots and conditional publication; embedded LoudnessData remains separate.

- [AutoGain caller regressions](audit/autogain-caller-regressions.md): meaningful smoothing controls in EQ, Crossfeed and XTC, with the intermediate scalar precision failure explicitly retained.

- [EQ AutoGain reset](audit/eq-autogain-reset.md): reset restores measurement phase and reproduces a fresh instance across ordinary and compiled routes.

- [EQ and Crossfeed measurement clocks](audit/autogain-caller-clock-proof.md): independent public callback-partition and future-suffix failures, with exact disabled controls; corrections remain pending.

- [SOFA delay proof](audit/sofa-delay-proof.md): metadata delays are lost in Binaural and XTC while equivalent shifted impulses produce the expected timing and phase.

- [Engine and drivers](audit/engine-drivers.md): offline/live clock domains, transition timing, EOS and HAL recovery contracts.

- [Terminal engine replacement](audit/engine-terminal-replacement.md): same-format replacement during final-tail/EOS backpressure drains the committed host before completion.

- [Support layers](audit/support-layers.md): native transport/tails, bridge types, realtime FFI controls and cross-layer integration.

- [Native transport details](audit/nih-transport.md): signed preroll, fallback clocks and pinned NIH limitations.

- [Spectral detector boundaries](audit/spectral-boundary-design.md): measured phase dependence near DC/Nyquist and the tradeoffs between an energy convention and fitted tone amplitude; production detector changes remain deferred.

### Oversampled precision capability (AUD-030)

The dynamic oversampling adapter advertised native f64 when its inner plugin
supported it, although its resamplers and buffers only process f32. This selected
the trait's allocating f64 fallback. A fresh-thread regression reproduced two
allocations. The adapter now reports its actual f32 capability, selecting the
host's preallocated conversion buffers. The regression passes for 2x/4x,
mono/stereo/six-channel processing and callback sizes from 1 to 8,192 frames,
with output equal to the f32 reference converted to f64.

## Native tail and precision follow-up

AUD-051 adds an explicit conservative response-bound contract and native lifecycle
cache. Selected families have numerical support evidence; other native families
report unknown/infinite and can consume more idle CPU. See
[audit/native-tails.md](audit/native-tails.md) for bounds, native mapping and limits.
AUD-052 fixes a second f64 adapter entry point; the independent cold precision
regression and focused Clippy pass. HAL staging has subsequently been implemented as AUD-056, with portable tests
and macOS target compile/Clippy passing; native runtime validation remains open.
