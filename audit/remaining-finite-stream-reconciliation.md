# AUD073 finite-stream reconciliation

Status: source and report reconciliation, 2026-09-29. MIDI and IAMF are
excluded. The initial inventory was read-only; the AUD136 follow-up below
records subsequent implementation and executed tests. AUD133 Ambisonics and
AUD134 convolution have separate reports and are not re-audited here.

## Disposition of the older inventory

`audit/finite-stream-inventory.md` records a 2026-09-28 source snapshot before
AUD074 onward. Its 19 public probes established omitted zero-continuation
output in those exact configurations; they did not all prove full response
bounds. The current issue ledger is the disposition source: AUD073 remains
partly open, and the rows and detailed reports below supersede older inventory
entries (`AUDIT.md:85-94`).

The measured finite cases with implemented drains are Delay without feedback,
stable Convolution, Declick, SpectralCompressor, Denoiser, spectral Hiss,
PND, XTC, spectral MultibandExpander, eligible spectral Downmix, disabled
SpeechDenoiser, empty-bank EQ oversampling, eligible AnalogLimiter, and dry or
one-band time-domain MBC/MBE. FIR Crossover and LinearPhaseEQ were closed
separately under AUD074 and AUD078. Binaural/Upmixer and AEC/Beamformer were
already covered by AUD045 and AUD055. See [Delay and Convolution](delay-convolution-finite-stream.md),
[Declick and SpectralCompressor](declick-spectral-finite-stream.md),
[Dynamics](dynamics-finite-stream.md), [EQ](eq-finite-stream.md),
[PND](pnd-finite-stream.md), [Downmix](downmix-finite-stream.md),
[XTC](xtc-finite-generation.md), [spectral MBE](multiband-expander-spectral-finite-stream.md),
and [disabled SpeechDenoiser](speech-disabled-finite-stream.md).

One stale detail needs care: the dynamics report's table predates the later
spectral MBE report. Spectral MBE now drains its finite window/cache support;
time-domain multiband wet modes still contain recursive crossover state and
remain outside the finite drain. Likewise, AUD051 `TailLength` metadata is not
an EOS drain contract: a conservative `Unknown` or `Infinite` report does not
itself emit retained audio, and a finite metadata bound does not prove that a
plugin implements drain. The host drain call-budget work in AUD077 bounds work;
it does not define a recursive-tail endpoint.

## Initial finite audio and transport findings

This table records the source findings before AUD136 production edits. The
follow-up below supersedes the enabled SpeechDenoiser implementation status.

| Path | Current source evidence | Reconciliation |
|---|---|---|
| SpeechDenoiser, enabled | The RNNoise backend frames input in 480-sample blocks, has a partial-frame accumulator, and queues wet and aligned dry output at a fixed 960-frame latency (`crates/sotf-plugins/crates/plugins-denoiser/src/rnnoise.rs:1-70`). The plugin reports `Unknown` while enabled; `drain_call_bound` reports immediate completion and `drain` returns `COMPLETE/0` whenever enabled (`crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/src/lib.rs:303-365`). | **Finite accepted-program queue is not flushed.** This is distinct from the enabled model/high-pass response, which the source and accepted disabled-path report leave `Unknown`. The disabled-only fix explicitly retains enabled immediate completion as a limitation (`speech-disabled-finite-stream.md`). Do not extend `Finite(960)` to enabled mode without an oracle for the wet model. |
| ABCompare | The plugin owns two nested `DawHost`s plus path A/B and dry `DelayLine`s (`crates/sotf-plugins/crates/sotf-plugin-ab-compare/src/lib/abcompare_plugin.rs:28-85`; `lib/delay_line.rs:1-48`). Its `process` advances both hosts and all active alignment rings, but its `Plugin` implementation ends after `latency_samples` without a drain override (`abcompare_plugin.rs:1030-1210`); the trait default returns zero frames (`sotf-host/src/plugin.rs:242-264`). | **Source-confirmed finite composition gap; no dedicated public EOS regression was found in the inventory.** A child with an implemented finite drain or unequal child latencies leaves exact child/alignment samples that the outer default cannot forward. This does not justify broad host branch-queue changes. |
| HAL Output sink | The sink accepts the whole plugin callback while retaining an unwritten suffix in `pending`; `flush_pending` is called only from ordinary `process` (definition `crates/sotf-plugins/crates/sotf-plugin-hal-output/src/lib.rs:411-440`, sole caller at line 599). The sink has no drain override and has zero output channels. | **Separate finite transport-retention risk, not an ordinary output-frame suffix.** If the final write is partial and no later `process` call occurs, the source has no path that retries the pending frames. The inventory records this as a source finding, not an executed EOS regression. Prove it with a portable fake writer before calling it a reproduced loss; native HAL runtime is not required for that logic test and has not been established here. |

These findings use current source slices after TokenSave status/context checks.
The ABCompare and HAL Output entries have not been promoted to executed public
regressions by this research task.

### AUD136 follow-up: reproduced regression and implementation checks

After the read-only inventory, Luna added a public test with 1,513 accepted
frames (three complete model frames plus 73 samples). The pre-edit enabled drain
returned no frames, leaving total output at 1,513 versus 2,473 for an otherwise
identical plugin continued with 960 zero-input frames. Before failing the length
assertion, the tightened test verifies that the actually omitted suffix
`reference[1513..2473]` has peak magnitude greater than `1e-5`. This is an
asserted lower bound, not a recorded peak measurement. The comparison uses a
separate ordinary-process execution of the same DSP; it is a continuation and
endpoint oracle, not an independent RNNoise algorithm implementation.

Root inspected the terminal expected failure in
`/tmp/sotf-aud136-red-suffix-eof.log`, SHA-256
`4f0956e315f63906869b9cd7ef9aafc07a42f96f3a71d97bcc985a03255b1b0f`.
The four-file source/lock manifest is
`/tmp/sotf-aud136-red-suffix-source.sha256`, aggregate SHA-256
`f77b0553183ab85095754f05218a99032c359c0b5b2502a426afdce255cb10f9`.
This evidence precedes production changes and does not prove that the enabled
model's full response ends after 960 frames. Astra subsequently accepted the
[bounded cutoff design](proposals/speech-denoiser-enabled-accepted-queue.md),
with pre-edit preservation and implementation checks still required; see the
[design review](reviews/AUD136-astra.md). The policy renders 960 zero-input
frames, clears residual backend state, and retains enabled `TailLength::Unknown`.

The owner captured actual source copies and audio under
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit`. Root verified the
two 2,473-sample output files and the 1,513-sample input against the reported
hashes, and independently checked that the preserved disabled output is exactly
960 zeros followed by the complete input. That establishes a pre-edit
compatibility reference.

The implemented policy now passes 46 package tests (zero failures, two ignored
baseline utilities), including all 480 terminal residues in mono/stereo,
partition and enable transitions, preflight and reset behavior, allocation and
deallocation guards through terminal backend reset, and a real 48 kHz
`DawHost` endpoint. Direct 44.1 kHz initialization rejects. Root inspected
`/tmp/sotf-aud136-speech-plugin-focused.log`, SHA-256
`6fb3fe4e421544299a4821b5a03f544b8ee03d751573e81a65d14d0ac397bb6e`.

After test-helper lint cleanup, the final package run again passes 46 tests
with two manual utilities ignored. The explicitly run ignored baseline replay
passes 1/1, comparing encoded sample bytes for enabled ordinary zero continuation
and disabled process/drain. Strict plugin/backend Clippy and formatting pass.
Root verified the final test/replay/lint logs and all selected source/lock
entries against identical start/end manifests at
`c7aca58a5882162c229d38e8b8b282b2b131c3501e18486410a1655093e786e9`.
Astra independently reviewed and accepted the scoped implementation on
2026-09-29; its one documentation correction added the required baseline
directory to the manual replay command. See the
[implementation report](speech-denoiser-enabled-accepted-queue.md) and
[review](reviews/AUD136-astra.md) for final log hashes and evidence. These tests
prove the declared accepted-program cutoff and compatibility paths, not a
finite natural RNNoise response or speech-quality accuracy.

### AUD137 follow-up: public finite-child regression reproduced

Luna's public integration regression now constructs ABCompare with the built-in
per-channel no-feedback Delay on path A, empty path B, pure-A mix, AutoGain
disabled and the default inactive full-range band mask. At 48 kHz, the 1 ms
delay shifts the two accepted stereo markers by exactly 48 frames. Eight
accepted frames produce eight ordinary frames and no drain, while the
independent sample-shift oracle expects 72 total frames, including the child's
64-frame prepared-ring drain. The expected suffix contains left=1 at frame 48
and right=0.5 at frame 55. That nonzero suffix is asserted before the length
comparison fails at 16 versus 144 interleaved samples.

Root inspected `/tmp/sotf-aud137-public-red.log`, SHA-256
`18065682d4633bbc3cf0645361b81d7aba902008faeff487a7f2d760afda36a7`.
The separate ignored capture passed 1/1 before production edits; log
`/tmp/sotf-aud137-preedit-capture.log`, SHA-256
`4594cf0294437977bc238cc6c0676a9d3339825923e76c09dce010c163562302`.
Actual source copies and audio are retained under
`crates/sotf-plugins/target/audit-artifacts/aud137/`.

Root independently decoded the preserved input and analytic reference and
confirmed those exact marker positions/amplitudes. The captured ordinary output
has only 16 zero samples and the captured drain is empty. This is strong loss
evidence but not a meaningful nonzero ordinary-processing compatibility
baseline. A separate 128-frame dense stereo recording has now been captured,
and its irregular-callback process control passes. Root independently decoded
the stored input and output and verified the exact 48-frame shift: 96 initial
interleaved zeros followed by the input with its final 96 samples omitted from
this ordinary-processing window. The 256-sample output contains 154 nonzero
samples with peak 0.75. The preserved process and analytic arrays both hash to
`b6cd6798f3adeb63c8fbfcb6c8b2f507eeb9fd6d2de378cd814cbfb3e255184e`.
Capture log: `/tmp/sotf-aud137-preedit-dense-capture.log`, SHA-256
`3a8e19a902ab12c895fbce774239011b39790f9b5d66419ad2af5c37c2c027c5`;
ordinary control: `/tmp/sotf-aud137-preedit-process-control.log`, SHA-256
`621b7bae35a0fd204bc4394daaf24a64d931612502fd962d76b6f756b35bf681`.
Each selects and passes one test. Composition design and implementation review
remain open; production behavior is unchanged.

Before sharing the mixer between process and drain, Luna also captured seven
ordinary-process controls: pure A, pure B, half mix, difference, B phase
inversion, bypass and active AutoGain. Input/output arrays and exact parameters
are under the `pre-edit-audio/mixer-controls` directory. Capture and explicit
replay each pass one selected test; logs are
`/tmp/sotf-aud137-preedit-mixer-controls.log` and
`/tmp/sotf-aud137-preedit-mixer-controls-replay.log`. Root verified both terminal
results and inspected the AutoGain samples: the output/input median falls from
about 1.99526 in frames 0–999 to 1.02418 in frames 8600–9599, so this case
exercises gain movement rather than just an enabled flag. These saved controls
are compatibility evidence, not independent loudness-quality measurements.

Astra's first design review requires explicit reset after drain, reset-required
handling after partial child-operation failures, and frozen controls during
drain. Clearing a child host's drain bookkeeping does not reset terminal child
DSP. The revised proposal also rejects active or previously used recursive
band-mask state before any child advances; that unsupported mode remains an
explicit follow-up. See [proposal](proposals/abcompare-finite-stream.md) and
[review](reviews/AUD137-astra.md). Astra accepted the revised design at
`b10a82638b1caae6bcdf6495d77bedf1aa23e543cc78864ffb27c4b3bc77df00`;
The first implementation now passes the preserved public Delay-child
regression: all 72 frames (144 samples) agree with the independent expected
vector within `1e-6`. Root inspected the one-pass terminal result in
`/tmp/sotf-aud137-first-green.log`, SHA-256
`5c921c6e94fb61f3b670303361abb27d2a85c44ad538f71518cf8a170b2950d1`.
This is an intermediate gate before removal of an unused helper; it is not a
frozen final package or strict lint result. The preserved reproducer allocates
at least the known child ring size, so it does not independently establish that
the newly declared output capacity is sufficient. Exact declared-capacity,
Plugin/Rack/Graph, partition, lifecycle, replay and heap gates remain in work.
Astra implementation acceptance is pending.

A subsequent composition run passes five tests in
`/tmp/sotf-aud137-composition-focused-1.log`, SHA-256
`83ec28c9df77b0394b3537a29d3caebcd8b56e4f940ad637d800b1b7af31cd00`.
Root inspected its successful terminal result and the independent direct f64
FIR convolution reference. Exact declared capacity, unequal child drain chunks,
three-frame path alignment, transactional preflight/reset, partial-child-failure
poisoning, AutoGain ordinary-zero continuation and a real outer DawHost are
exercised. Plugin/Rack/Graph configuration variants currently use one fixture
node per path; they do not establish arbitrary graph support. Multi-stage
serial Rack/Graph coverage and the remaining package/replay/heap gates are in
progress. The run still had a test-only unused-variable warning, so it is not
the final strict lint checkpoint.

The later focused suite passes 11/11 in
`/tmp/sotf-aud137-composition-focused-7.log`, SHA-256
`6c196538904d4e0029b1f15eee8a49699875eabb92a0a25d247c90531b0f1bfb`.
It adds two-stage serial Rack/Graph references, mixer modes, zero-progress
children, nonlinear-graph refusal and full lifecycle heap guards. A real reset
allocation was fixed by reinitializing filter values in retained vectors.
The prior-mask test cannot claim a public structural setter toggles the mask:
that setter rejects the change. Full package/replay/lint and Astra acceptance
are tracked separately from this focused checkpoint.

Astra's implementation review subsequently identified two missing boundaries:
sampled frame-geometry probes cannot establish identity for arbitrary children,
and frame selection can emit extra silence at empty completion or a short
dry-only remainder. The pre-fix regression run
`/tmp/sotf-aud137-review-red-composition.log`, SHA-256
`9ca442091717e571ac07ac337b0b46a0731e7bd46e48185454ad3725b6e2b986`,
passes the preceding 11 cases and fails all three new probes: a size-83
nonidentity child is admitted, an empty no-tail stream emits 17 instead of zero
frames, and a three-frame dry remainder emits 16. Root inspected the terminal
result. These findings prevented acceptance of that 127-test candidate.

The final revision fixes both findings and is accepted by Astra for same-rate,
identity-frame serial children. It requires an explicit conservative geometry
capability at every active node and equal negotiated input/output rates;
production Delay opts in. Unsupported size-83 geometry and compensating
48→96→48 rates reject before either child advances. Completion and final dry
fragments now use their actual remaining frame counts. The final composition
suite passes 15 tests and the package passes 131 tests, with four manual
utilities ignored. Separate saved seven-control byte replay passes 1/1 and
strict ABCompare/host/Delay Clippy passes. The nine-entry source/lock manifest
is `5d35cc3b55868eb1078af9dbabe9a5698211c6134a711f954110d0fa033a0425`.
Final logs are `/tmp/sotf-aud137-review-fix-package.log`,
`/tmp/sotf-aud137-review-fix-replay-final.log` and
`/tmp/sotf-aud137-review-fix-clippy.log`; the report and Astra review record
their hashes. Undeclared child geometry, branching graphs and active/prior
recursive band-mask EOF remain open. This is a scoped gate, not a new
workspace-wide result.

### AUD138 follow-up: HAL writer seam and consuming-host requirement

A subsequent read-only source investigation found an existing private
`HalWriter` trait and portable `FakeWriter` in the HAL Output crate. Its
`partial_write_retries_tail_before_new_audio` test covers retry when another
ordinary block arrives; it does not exercise EOF. A new local AUD138 records a
public `Plugin::process` → `Plugin::drain` regression using this seam, with an
accepted-sample log to distinguish attempted writes from accepted frames.
Two independent public process/drain tests have since produced clean expected
failures. With a ready writer, only the first two of four stereo frames reach
the fake writer's accepted-sample log: `[0, 1, 2, 3]` instead of `[0..7]`, even
though default drain reports completion. The separate backpressure test fails
because default drain reports completion while two frames remain queued. The
accepted-sample log records only the prefix actually accepted by each scripted
write, rather than equating an attempted write with delivery.

Root inspected both failures in `/tmp/sotf-aud138-hal-red-clean.log`, SHA-256
`ed2a9c51949eaa66a710b416fdbf47208e7fe0ef751e99683eaac03bc00bc644`
(zero passed, two failed, 27 filtered). The captured pre-edit HAL source is
`/tmp/sotf-aud138-hal-output-pre-edit.rs`, SHA-256
`4b782ed45bbc5913b13c7e113b1f2c43c9391187e6c3b945af568c42cf242243`.
An earlier test run poisoned its fake mutex during the failing assertion and
panicked again in cleanup; the clean run clones observed samples before
asserting. Production behavior was unchanged at that red checkpoint; direct
plugin implementation has since started.

The investigation also found that `DawHost::drain` rejects zero-output sink
geometry (`sotf-host/src/host/daw_host.rs`, around line 2095). Consequently a
direct HAL plugin fix alone cannot establish whole-chain EOF delivery. AUD138
must trace the actual consuming route and separately review and verify any
necessary sink handling. The existing generic unequal-rate queue and manager
protocol proposals remain outside this work.

The driver writer's `write` reports frames accepted into its ring;
`available_read_frames` reports queued plaintext frames. Neither proves physical
playback. Its `flush_audio` is a quiesced discard operation and cannot be used
as a successful delivery endpoint. The proposed policy must retain pending
data under backpressure, bound work per call, and state whether completion
means handoff to the driver or a stronger consumer acknowledgment.

Astra accepted the refined direct-plugin design at
`3b3d7faea8374821a894b6ec1bd1f862d60f0b98aafe582207744132357252e7`.
It guarantees retained-queue handoff rather than recovery of prior explicitly
counted overflow drops, limits each drain call to two writer attempts, and
keeps the total call bound unknown under external backpressure. Control-thread
recovery must preserve pending samples even if new format/capacity preparation
fails; explicit reinitialization can cancel them with truthful accounting. Astra has since
accepted the implementation at source SHA-256
`4905e63a57c8c20a615dfd6cea5974440b04296ccb32da493a0df0a37a6bcb1c`.
All 37 HAL package tests and strict all-target Clippy pass; root verified the
terminal logs and current hashes. The
[implementation report](hal-output-finite-stream.md) records those gates.
The consuming host/engine route and physical playback remain open. The later
host regressions and accepted next-stage design are recorded below.
See [proposal](proposals/hal-output-eof.md) and
[review](reviews/AUD138-astra.md).

#### Consuming-route source audit, 2026-09-29

Current source identifies separate integration and lifecycle requirements.
Corrected public regressions now reproduce the failures below; the bounded
host design and implementation are still pending.

The normal engine configuration already rejects zero output channels
(`types/config/engine_config.rs:168`), and `PreparedHostUpdate::prepare`
explicitly rejects a host with no output channels (`engine/types.rs:155`).
Therefore the hypothetical processing-worker behavior below does not establish
an admitted application route. A generic `DawHost` sink client and the engine
playback client need distinct, explicit support contracts. Do not remove these
admission guards merely to reach an EOF regression.

Ordinary generic-host processing has an earlier source-level obstacle:
`terminal_output_frames` divides `NodeBuffer.actual_len` by `num_channels`
(`sotf-host/src/host/daw_host.rs:3767`). A terminal sink has zero channels, so
the path can panic after its writer has already accepted audio. The same
division exists in `node_input_frames` for a zero-channel predecessor. A fresh
public-host regression reproduced the terminal panic after the fake writer
accepted 4 of 24 expected samples. The next contract must either
support terminal sink processing explicitly or reject unsupported topology
before processing causes side effects.

1. `DawHost::drain` rejects both zero host output channels and zero native node
   output channels (`sotf-host/src/host/daw_host.rs:2051,2095`). Accepting a
   terminal sink requires both paths to handle empty output storage while
   preserving positive input width, causal upstream-tail processing and
   preflight validation.
2. An unknown plugin call bound becomes `UNKNOWN_DRAIN_CALL_LIMIT = 4096`;
   successful incomplete calls with zero emitted frames consume that quota
   (`daw_host.rs:118,2112-2132`). A transport-dependent sink therefore needs an
   explicit wait/retry contract. Removing the geometry guards alone would
   eventually return a convergence error during an ordinary external stall.
3. The engine EOF loop polls commands between drain calls but immediately
   retries an incomplete zero-frame result
   (`sotf-engine/src/engine/processing_thread/processing_state.rs:1022-1140`).
   A sink route needs scheduled retries that still service Stop/Shutdown and
   never report EOS while retained audio awaits handoff. This does not imply
   reusing the rejected broad manager-transition proposal.
4. HAL ordinary `process` returns the consumed input-frame count with zero
   output channels. Host downstream-tail composition forwards the downstream
   return as `PluginDrainResult.frames` (`daw_host.rs:2160-2200`). The engine
   also creates output frames after ordinary processing, independently of
   whether their sample payload is empty (`processing_state.rs:850-949`).
   `AudioFrame::try_new` checks the sample-count product but accepts a
   positive-frame, zero-channel empty payload (`types/state.rs:24-56`). Thus
   sink consumption must be distinguished from emitted audio throughout the
   ordinary and EOF routes; constructor success does not establish a valid
   playback route.
5. HAL Output does not override `Plugin::reset`; the trait default is a no-op
   (`sotf-host/src/plugin.rs:220`). The accepted direct stage verifies explicit
   reinitialization cancellation. It does not prove that a generic host reset
   clears retained HAL audio. New direct and host regressions reproduce stale
   pending samples surviving reset. The next stage must define allocation-free
   pending-queue cancellation/drop accounting separately
   from any control-thread cancellation of audio already accepted into the ring.

Corrected host red log `/tmp/sotf-aud138-host-drain-red-clean.log` contains
three expected failures, SHA-256
`324473ea7940ca16e83c9b91187572f0c3d272e375f68a802103d742bc5d9d67`.
The reset case replays four stale samples. The drain case is refused by the
equal-width serial predicate before sink capacity preflight; removing the sink
then recovers the complete four-frame upstream tail. Direct reset red log
`/tmp/sotf-aud138-hal-reset-red.log`, SHA-256
`b82021b926313edaac51a75791d526bcbb4e716615781d75fd410c6728f4e7b0`,
confirms two retained pending frames and no drop accounting after reset.
The same predicate also rejects nonzero width changes; see
[AUD140](channel-changing-host-eof.md). Do not relax it globally, because it
also gates ordinary f32 optimized processing.

Astra accepted the bounded host design at
`bedb455fd4ba7abf835d10b537d3a1e93da0e44612a2864a830a0d56f4b3bf9a`;
see [host proposal](proposals/hal-output-host-drain.md). Luna is implementing
explicit sink APIs, exclusive ownership before command-sender extraction,
prepared queue preflight/append, frozen stream controls and truthful reset
cancellation. Host implementation acceptance is still pending. AUD140's
nonzero channel-changing route is assigned separately for public regressions
and design review; it must preserve the ordinary optimized path's eligibility.

Required next evidence is an actual linear host with an upstream finite tail
and an injected portable sink. Its consuming scheduler must block/unblock the
sink, observe exact accepted samples, service commands, and emit EOF only after
handoff. If an engine sink route is proposed, its admission/output contract
must first be designed, then exercised by a real processing-worker test.
These tests and any bounded implementation need their own reviewed contract
after the direct-plugin stage. The current
daemon still strips these legacy HAL graph nodes; this audit does not claim
that the generic plugin is its active playback route.

## Recursive response policy, not a finite-support claim

AAE is the clearest example. Its source has finite pre-delay/early-reflection
storage and a recursive FDN/allpass path; RT60 is a control parameter, not a
hard zero endpoint (`sotf-plugin-aae/src/lib/aae_plugin.rs:42-60,826-1020`,
`sotf-plugin-aae/src/params/consts.rs:10-35`). The old public probe observed
nonzero zero-input continuation, but did not separate early reflections from
the FDN response. Treat full AAE output as a recursive-tail policy gap, not a
proved finite endpoint defect. A bounded early-only configuration can be
investigated separately if all recursive audio paths are disabled and its
tap support is measured.

The same distinction applies to feedback Delay, wet time-domain multiband
dynamics, ordinary IIR EQ and analog/color paths, LR crossover/de-esser paths,
Crossfeed, Dynamic EQ, StereoImager, Saturation, and related recursive filters
listed in the inventory. Their zero-input response may continue after input,
but exact finite support has not been shown. Keep `Unknown`/`Infinite` honest
and choose a render cap or a documented residual policy before claiming an
automatic realtime drain. `render_offline_with_tail` already offers a caller
selected offline continuation duration; it does not change automatic realtime
EOS (`offline-tail-duration.md`).

The Ambisonics dual-band IIR entry is also a recursive-policy item in the old
inventory, but the Ambisonics source and its active AUD133 tail work are owned
elsewhere and were deliberately not re-evaluated. AUD134's true-stereo
Convolution track is likewise active; the older finite Convolution drain
disposition remains separate from that routing work.

## Current bounded follow-ups

1. **HAL consuming-host stage (AUD138).** Implement the accepted sink contract
   and verify exact retained samples, backpressure, preflight, failures and
   reset through a real host. Application scheduling and physical playback
   remain separate from portable ring handoff.
2. **Channel-changing serial EOF (AUD140).** Reproduce nonzero-width refusal,
   preserve ordinary audio and upstream tail, and review a bounded design
   before production edits. Include actual Ambisonics EOF completion.
3. **ABCompare unsupported composition.** The bounded AUD137 case is accepted.
   General child geometry, branching and recursive band-mask response remain
   distinct follow-ups requiring their own contracts and evidence.

AUD136's scoped implementation is accepted; its natural model/high-pass
response remains `Unknown`. AUD073 remains open; AUD137's supported serial
composition is accepted. Keep recursive-tail policy separate from these finite
queue cases. AUD087's
blocked broad unequal-rate branch queues and engine manager-transition work
are outside this bounded handoff.
