# AUD010: limiter-local oversampling integration plan

Date: 2026-09-28. **Approved staged plan, now implemented.** Phase 1 native-kernel
extraction and phase 2 prepared 2x/4x processing are covered by the
[implementation report](../limiter-oversampling-implementation.md).
The plan was originally prepared read-only before implementation approval.
This resumes `audit/limiter-oversampling.md` and
`audit/proposals/limiter-protection.md`. Native finite EOS (AUD069) and the shared
prepared-drain/budget work (AUD077) are already implemented and are dependencies,
not work to repeat. MIDI/IAMF and manager/branch queues are excluded.

## Decision and scope

Use the existing public `OversampledPlugin<PrivateWetCore>` inside LimiterPlugin,
with a private native final protector and one delayed dry path. The existing
`inner_mut()` accessor permits a bounded control timeline without a host API
change. No preferred-oversampling request is emitted: direct, bridge, FFI, NIH
and engine construction must all use the same limiter-owned rate path.

Choose an explicit **processing-clock automation contract** for the new 2x/4x
paths: a control accepted before native input frame `s` governs the wet core
starting at high-rate processing frame `F*s`. It must never govern preceding
buffered processing frames. This does not promise that the change follows the
peak of source impulse `s` through the upsampling FIR: that filter delays and
spreads audio. The existing native limiter also applies current controls to
currently processed delayed audio rather than attaching controls to individual
source samples. Final output protection and mix use the native output clock.

If source-impulse alignment of wet-core controls is required instead, this is a
different contract: delay the control stream by the upsampler's physical delay.
The public generic wrapper exposes total round-trip latency, not the individual
up-filter delay. A narrow read-only delay accessor plus a correspondingly larger
prepared timeline would then be appropriate; deriving that delay by subtracting
assumed equal halves or duplicating Rubato internals would be brittle. It is not
needed for the recommended processing-clock contract.

## Current source facts

| Source | Relevant fact |
| --- | --- |
| `sotf-plugin-limiter/src/lib/limiter_plugin.rs:601-852` | Current kernel owns detection, gain/release, lookahead, mix and optional native ISP correction. Threshold/mix smoothers advance once per frame. |
| Same file `:510-598, :958-1162` | Scalar setters validate; lookahead/ISP changes are structural where they affect latency. Native EOS freezes changed settings, emits exactly active audio-ring support, and permits identical resends. |
| `sotf-host/src/oversampling/oversampled_plugin.rs:55-89, :144-214` | Prepared generic constructor and `inner_mut()` are public. Each core call has `256*F` frames. Current setters merely forward latest values, so direct forwarding loses within-chunk control timing. |
| Same file `:229-335` | `begin_drain` finishes residual input and up-filter overlap, then prepares the child. A valid prepared bound can be queried afterward. Child zero-output progress is preserved. |
| `oversampling/oversampler.rs:260-334, :492-604` | Wrapper consumes whole 256-frame input chunks, retains one fixed 256-frame startup queue, and supports bounded prepared EOS. Begin performs at most two existing input chunks. |
| `oversampling/misc.rs:1-40` | Public `OS_CHUNK_SIZE=256`; context mapping uses the processing clock and explicitly does not schedule historical metadata changes. |

The generic wrapper has grow fallbacks and unchecked `sample_rate*factor` in its
initializer. A limiter-owned maximum submission of 256 frames and checked rate/
dimension preflight make those paths unreachable here; no shared fix is needed
to integrate the feature safely.

## 1. Preserve the public owner and the exact 1x path

- Keep constructor signatures, the ten legacy indices/IDs, defaults, getter
  types and JSON fields. Append index 10, `oversampling`, as a structural choice:
  index 0/1/2 selects 1x/2x/4x, default 0. Use one documented numeric choice
  representation across Params, LimiterPluginParams and native schemas; do not
  ambiguously interpret numeric 2 as both an index and a physical factor.
- The existing `new(...)` constructor selects 1x. Old JSON without the new key
  selects 1x. Add a prepared path only during initialize/control-thread rebuild.
  A changed live factor is rejected before mutation. Same-value resends remain
  harmless, including after EOS.
- Extract private DSP state and kernel operations, keeping the Native branch's
  arithmetic, operation order, smoothing, denormal handling and metering cadence
  unchanged. The public owner retains IDs/schema/canonical scalar settings/cache.
  Private cores have no separate public registry or preset state. Avoid a pair
  of publicly parameterized LimiterPlugin instances in production.
- Capture current 1x audio/telemetry/latency/EOS fixtures before extraction and
  require exact before/after equality for the same build platform. Preserve the
  existing independent ceiling and timing tests and their tolerances. Subsequent
  1x public processing must directly take this native branch, with no FFT,
  added guard, control queue, dry-ring copy or altered rounding.
- The new path should disable the zero-latency compiled-op shortcut; native
  metadata remains unchanged. Structural capacity/latency must be stable before
  processing and through EOS.

## 2. Private wet path and final protection

At initialization let native rate be `R`, factor `F`, public lookahead quantize
to the existing native integer `L`, and let the native true-peak detector delay
be `D(R)` (currently 6 below 96 kHz, 12 below 192 kHz, zero thereafter).

1. Wet core: run the existing gain law at `F*R`, with exact integer delay `F*L`,
   mix fixed to 1, and output ISP correction off. Its input true-peak detector
   is enabled for `true_peak || isp_mode`, as in the successful prototype. Use
   the public soft/link/dual-release choices and a high-rate threshold smoother.
2. Downsample through the existing wrapper.
3. Native final protector: reuse the extracted native kernel, hard mode and
   fully wet, with the current native threshold, release and link controls.
   Its dual-release is off, matching the verified prototype. In sample mode use
   sample detection and zero lookahead/ISP delay. In ISP mode use exactly `D`
   pre-lookahead plus the existing `3D` output correction, for support/delay
   `G=4D`. Retain the existing ISP requirement of hard limiting, fully wet mix,
   and adequate public native lookahead; do not silently relax it in this task.
4. Mix once at native output using a prepared dry delay of
   `L_total = wrapper.latency_samples() + G`. Because the private core uses
   `F*L`, no fractional native lookahead is rounded away. With the current FFT
   pair this is `512 + L + G`; use actual wrapper metadata, not that hardcoded
   sum in production. Low-level impulse tests independently verify it.

The final wet branch obeys its current smoothed sample ceiling; ISP uses the
existing finite reconstruction convention, not an ideal continuous-time
supremum. Mixing dry audio afterward intentionally removes the ceiling
guarantee for `mix<1`, as in the current product. Putting a mandatory protector
after the mix would change the meaning of dry mix and is not proposed. The ISP
mode continues to reject `mix<1`, so its guarantee always covers final output.

Original dry samples should receive the existing nonfinite-input sanitization
before both the dry ring and FFT. A pure delay is not the same transfer function
as the FFT wet branch: exact dry-only timing and weighted mix are promised,
not an all-frequency wet/dry null. General extreme-input FFT robustness is not
silently established by the limiter's output sample clamp.

## 3. Bounded accepted-input control timeline

Store a Copy scalar snapshot of wet-core controls, not ParameterIds, maps,
strings or heap-owned events. Relevant fields are threshold target, release,
soft, input true-peak choice, dual release and link. Feed-forward remains its
documented compatibility scalar; active nonzero lookahead is already predictive.
Structural settings and mix are not per-core timeline fields.

Maintain a limiter-owned input phase `p in 0..256`. For each public processing
call:

1. Validate the entire caller buffer, checked dimensions, initialized rate and
   open lifecycle before changing history or accepting any frame.
2. Submit at most `256-p` native frames. Before submission append one current
   canonical snapshot per accepted frame to the core's prepared 256-slot ring.
   Advance/copy the original dry input into the matched dry path.
3. Call the generic wrapper on this bounded slice, then final-protect and mix
   exactly its returned native frames. Update phase modulo 256 and continue.

Proof of capacity: before submission the control ring contains exactly `p`
unprocessed input frames. Submitting `m<=256-p` yields at most 256 snapshots.
If `p+m=256`, the wrapper executes exactly one `256*F` core chunk and consumes
exactly 256 snapshots; otherwise it executes no core chunk. Thus no callback
length, no callback partition, and no number of setters can overflow storage.
Large public callbacks are streamed through the same bounded loop.

Inside a core chunk, consume snapshot `j` before high-rate frame `j*F`; hold it
for `F` samples. Apply only changed scalar fields, preserving continuous kernel
histories and rate-correct smoothing. Adjacent equal snapshots may be processed
as one run. Worst case is 256 bounded runs per chunk, not an unbounded event
queue. Repeated setters without intervening input only replace the canonical
snapshot; rejected setters alter neither that snapshot nor queued controls.

At native returned-output index `n`, the final guard's threshold/release/link
and the outer mix use current accepted canonical targets. Their native-rate
threshold/mix smoothers advance once per emitted frame, including ordinary
startup zeros. The guard protects already queued wet samples with the current
smoothed ceiling, rather than waiting another FFT latency for an input event.
This intentionally creates two derived clocks under one parameter owner.

Use local monotone accepted/core/output frame counts for these semantics;
transport seeks must not replay/discard control records. Preserve sample/PPQ
offsets when subdividing ProcessContext, but do not infer historical controls
from the wrapper's transported metadata. The limiter has no musical-time DSP.

## 4. Prepared EOS, clocks and useful call bounds

Use an explicit outer phase: Open, WetTail, GuardTail, DryTail, Complete, Failed.
For nonempty streams the first valid begin/drain freezes canonical targets and
allows only identical setters until reset. Empty EOS is a no-op. Wrong rate,
shape or zero output capacity is rejected before begin/history mutation.

- **Begin:** preflight all fixed capacities; mark the core's control timeline
  as ending with the frozen canonical snapshot; then call wrapper.begin_drain.
  Queued records are consumed first. Synthetic residual/up-overlap padding uses
  the frozen snapshot after those records end; it is not an underflow or new
  accepted input. A setter accepted after the last real sample still defines
  the padding/output target. Prepare the private child only after this work.
  Repeated begin is idempotent. Partial internal DSP failure enters Failed and
  requires reset, rather than promising impossible rollback.
- **WetTail:** one bounded wrapper drain step per public call. Feed each emitted
  frame through the final guard and dry/mix paths. A zero-frame incomplete step
  does not advance guard, mix smoothing or dry-ring clocks. A nonempty complete
  wrapper result is delivered once before transitioning.
- **GuardTail:** after all wet output has been delivered, emit the final guard's
  remaining finite `G`-frame audio support in <=256-frame steps. Advance dry/mix
  only for those emitted frames. Do not snapshot the guard's current remaining
  state before the upstream wet tail has finished feeding it.
- **DryTail:** emit any still-retained original dry frames with wet zero,
  decrementing the dry support already consumed by previous emitted tail
  frames. This is usually empty but remains necessary for a proof independent
  of a particular resampler's emitted padding. Complete/reset are stable.

No outer cache is necessary: both the wrapper and native kernels are already
streaming, and controls are frozen during drain. Advertise a structural maximum
of 256 native output frames for the new path and accept smaller whole-frame
destinations. Forward only real emitted slices; preserve trailing canaries.

Support metadata is conservatively
`max(wrapper.tail_length() + G, L_total)` using checked arithmetic. Current
wrapper support is `1024 + 256*ceil(L/256)`, not its 512-frame group/queue delay.
Detector/release recurrences cannot produce nonzero audio after the finite audio
rings empty. This metadata remains conservative throughout the EOS epoch.

For full advertised capacity let `W` be the prepared wrapper's remaining call
bound and `R_d` the outer remaining dry support. A checked conservative bound
in WetTail is `W + ceil(G/256) + ceil(R_d/256) + 2`: the extra two calls cover
zero-length phase completions. G is fixed future guard support, not a premature
snapshot of guard state. In GuardTail use its actual remaining native bound
plus `ceil(R_d/256)+1`; in DryTail use `max(1,ceil(R_d/256))`; Complete returns 1.
Before nonempty preparation return None, following the existing wrapper. Tests
must count actual successful full-capacity calls, including zero-output calls,
after prior partial service and repeated queries; queries never advance DSP.

Reset clears both DSP histories, FFT queues, dry ring, timeline, cursors and EOS
phase while preserving canonical settings. Reinitialize prepares the selected
factor/rate off callback. Successful reset installs current canonical targets
in both private cores, not the last historically consumed queued snapshot.

## 5. Required independent verification

1. **Native preservation:** capture before-extraction output/telemetry and keep
   exact 1x comparisons, all existing limiter tests, latency, scalar setters,
   presets and AUD069 EOS tests unchanged in expectation.
2. **Automation oracle:** construct a test-only offline composition from the
   complete input and absolute event list. In an Oversampler callback apply
   controls from absolute high-rate index `q` using event time `floor(q/F)`,
   without the production pending ring/phase code. Native final controls use
   output index `n`; dry reference is an independent indexed delay. Compare
   public waveform exactly across phase 0..255, factors 2/4, event fields,
   repeated same-position writes, invalid writes, reset and irregular/large
   partitions. Plain partition equality alone is insufficient: forwarding the
   latest target can be wrong in every partition and still agree.
3. **Ceiling and alias:** preserve the 840 burst fixtures and independent f64
   Hann-sinc reconstruction oracle from the executed prototype. Extend with
   threshold steps at all chunk phases, release/link/dual-release changes and
   shortest valid lookahead. Verify each final sample against its independently
   tracked smoothed output ceiling, and reconstruction under the documented
   finite-kernel criterion. Keep the prior 0.00001 dB sample/0.1 dB reconstructed
   burst bounds; retain coherent alias measurements without promising monotonic
   factor improvement. Include soft mode/sample mode and ISP restrictions.
4. **Signal clocks/mix:** first/final low-level markers, all residual phases,
   44.1/48/96/192 kHz, 1/2/6 channels, fractional-ms lookahead; independent exact
   dry delay and weighted-mix reference. Native-rate case must stay unchanged.
5. **EOS:** public process+drain versus independently explicit zero continuation
   with matching frozen controls, including non-multiple input lengths, no/full
   residual input, last-frame controls, partial drain capacities, call bounds,
   zero-output progress, invalid-call retry, repeated begin/complete/reset and
   same-value setter resends. Do not trim a mismatching waveform to make it fit.
6. **Realtime:** explicit fresh-thread 0 allocations and 0 deallocations for
   first process, all valid scalar writes, every timeline phase, cold begin,
   partial/final drain, bound queries and reset. Exercise 32-channel capacity
   if accepted; reject >32 for new paths before initialization mutation. Measure
   release CPU on the same input/settings with maximum event density, rather
   than extrapolating from constant-control prototype timings.

## Integration handoff and unresolved scope

The DSP implementation can remain limiter-local: kernel/path modules, Params/
LimiterPluginParams, metadata, docs and focused tests. External wiring is a
separate small handoff: append index 10 in engine settings/defaults/accessors/
converter, preserve the new field through bridge/NIH/FFI construction and
transactional preset restoration, and update explicit struct literals/fuzzer
fixtures. Verify native stable parameter IDs and old JSON default construction.
Do not request another generic host wrapper around the internally oversampled
limiter.

The existing engine converter omission of `link_amount` and `feed_forward` is
tracked separately as **AUD102**, owned by root. It is not part of this rate-path
design or a reason to modify its DSP ownership.

The previous prototype is at `/tmp/sotf-limiter-final-protection/protection_probe.rs`
and `.log`; it proves constant-control composition, 840 burst ceilings, 24
partition/reset/latency/heap combinations and the recorded limited alias gains.
It does not prove the proposed control timeline, final-control automation or
composed EOS. Those remain the implementation's concrete acceptance gates.

## Phase 2 telemetry proposal (superseded)

The initial energy-ratio proposal was rejected because filter attenuation and
phase cancellation can falsely indicate limiter activity. The approved
[contributing-gain telemetry addendum](limiter-oversampling-telemetry.md)
supersedes that section. No public energy diagnostic is added. Native 1x
telemetry remains exact; the prepared paths track actual limiting gains over
the finite resampler contributors and aligned final-guard stages.
