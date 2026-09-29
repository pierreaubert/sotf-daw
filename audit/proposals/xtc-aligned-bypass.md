# AUD-099 proposal: XTC bypass on the fixed output clock

2026-09-28. Reviewed and approved by the parent after aggregate gate 5766/311. The following records the pre-implementation reproduction and approved design. XTC initialization ownership was released before implementation. This proposal does not alter filter design, asynchronous publication, host graph queues, or the approval-blocked engine transition protocol. MIDI/IAMF excluded.

## Executed evidence

Artifacts:

- `/tmp/sotf-xtc-bypass-probe.rs`: actual public `XtcPlugin` and `DawHost` APIs.
- `/tmp/sotf-xtc-bypass-probe.log`: all observations.
- `/tmp/sotf-xtc-bypass-build.txt`: exact rustc invocation.
- `target/audit-tmp/xtc-bypass-probe/`: executable and immutable copies of existing XTC/host rlibs. The initialized source changes affect only preparation/commit; the bypass/audio code under review is unchanged in those artifacts.

The diagnostic uses neutral XTC filters (`bypass_xtc_filters=true`) and disables AutoGain, isolating timing from filter coloration and the known ordinary callback-dependent AutoGain cadence. It covers FFT sizes 128/512/2048, rates 44.1/48/96 kHz, and callback patterns `[1]`, `[137,511]`, `[8193]`: 54 impulse cases, 27 disable/resume cases, plus six actual host graphs. The executable reports observed behavior and exits normally; it is a diagnostic reproduction, not a passing assertion of the proposed corrected contract.

### Declared versus emitted latency

In every disabled case the first 0.25 impulse is emitted at frame **0**, while `latency_samples()` declares **N**. Enabled neutral output emits the same impulse at frame **N**. At 48 kHz:

| FFT size N | Disabled actual delay | Declared delay | Timing error |
| ---: | ---: | ---: | ---: |
| 128 | 0 frames | 128 frames | 2.667 ms |
| 512 | 0 frames | 512 frames | 10.667 ms |
| 2048 | 0 frames | 2048 frames | 42.667 ms |

### Re-enabling replays stale program and omits disabled input from wet history

The probe supplies a +0.25 marker on the final frame of an enabled prefix of `N+17` frames, then disables XTC for `G=5N+1001` frames. A distinct −0.0625 marker is supplied `N/2` frames before re-enable. It then enables XTC and supplies zeros.

- Disabled output emits the new −0.0625 marker immediately during the gap.
- Re-enabled output emits the **old +0.25 marker at offset N−1 after resume**, delayed by the entire disabled interval beyond its proper source time.
- The new marker never entered the wet processor and is absent at its independently expected delayed position `N/2` after resume.
- Reset clears this replay; the following zero input is exactly silent.

For N2048 at 48 kHz, the old marker appears **11,241 frames / 234.1875 ms late**, at resumed offset 2047 with amplitude 0.25000003. For N128 the replay is 1641 frames late, at offset 127 with amplitude 0.24999997. Results and positions agree across the three callback patterns and all tested rates.

For neutral filters an aligned dry/wet bypass has a particularly strong independent oracle: every enable history must preserve the same `[N zeros] + input` waveform, because both paths represent the same source samples and a linear crossfade sums to unity. The present behavior violates this without relying on a nonlinear/filter approximation.

### Actual host PDC consequence

Each graph has two root branches (XTC and a zero-latency identity) feeding a summing identity node. The public `DawHost` builds compensation from declared latency. With a +0.25 input impulse:

| XTC state | Actual summed output for every tested N |
| --- | --- |
| Enabled neutral | One +0.5 peak at frame N |
| Disabled | +0.25 at frame 0 **and** +0.25 at frame N |

The host correctly reports N and delays the identity branch. The XTC declaration disagrees with its own disabled waveform, so the host cannot align the branches. This is an executed graph result, not an inferred possibility.

## Source cause and existing contracts

`src/lib/xtc_plugin.rs`:

- `process_audio` around 1323–1377 increments the diagnostic cadence and may measure AutoGain input, then `enabled=false` copies current L/R into the first two outputs, clears extra outputs, and returns before STFT staging/OLA, output metering, compensation, or limiter progress.
- The enabled STFT scheduler advances once per accepted input frame and emits N startup frames. The disabled return pauses input staging, output reading, filter crossfade and limiter history while real source time continues.
- `process` around 1903–1910 advances `enabled_phase` only for enabled callbacks.
- `remaining_tail_frames` around 1471 and `tail_length` around 1933 declare zero observable disabled support. `drain` freezes a nonempty disabled epoch but returns no audio. Those declarations match the old immediate bypass, and must change together with any delayed bypass.
- `latency_samples` around 2002 always reports N.
- `set_parameter("enabled")` changes the bool and rebuilds the complete cached parameter vector. This is an inspected allocation/deallocation path (not measured by this probe). A sample-clock crossfade is intended for live automation, so the bool setter should instead update its existing cached bool in place and avoid restarting a transition on an identical snapshot.

The `enabled` control differs from `bypass_xtc_filters`: the latter keeps the STFT path, delay and neutral reconstruction active. Tests currently pin immediate disabled routing explicitly; those tests document a known old behavior, not a physically consistent PDC contract.

## Minimal proposed implementation

### One fixed delay and continuous wet history

Keep declared latency **N** for every enabled state. Prepare a stereo dry delay ring of exactly `2*N` samples and a frame cursor on the control thread. Every accepted frame enters both that ring and the existing wet STFT scheduler, even when the audible wet mix is zero. The dry output mapping remains L/R to the first two output channels and zero to any extra outputs, delayed by N frames.

Keep the existing wet filter/AutoGain/limiter sequence. Use its current output buffer as the wet buffer; a final per-frame pass reads the prepared dry delay and blends the two outputs. No extra callback-sized wet scratch or new worker/queue is required. This also keeps active filter crossfades and bounded ownership retirement progressing while bypassed.

Default enabled initialization starts with wet coefficient exactly one, and the all-wet fast branch leaves the existing output arithmetic untouched. Disabled initialization starts at exactly zero, yielding exact delayed dry output. Both states still advance the two timelines. Failed initialization must retain the new ring/ramp state along with the existing epoch; integrate its preparation into the staged initializer rather than allocate/reset live fields before commit.

### Explicit finite transition

Propose a **10 ms linear output crossfade**. Use a rounded positive integer frame count, calculated at initialization, and a scalar remaining-frame counter. An enabled change starts a ramp from the current coefficient to the new 0/1 target over that count. Reversing during a ramp starts from the current coefficient; identical bool snapshots do nothing. Assign the endpoint exactly when the counter expires.

The ramp advances once per emitted frame, including zero continuation. A setter affects the next validated output frame; its control event is not additionally delayed by N. Both audio inputs to that ramp already refer to the same source time. Linear complementary weights preserve unity for identical paths; an equal-power blend would raise coherent neutral output in the middle of the fade.

At coefficient zero, copy delayed dry exactly. At coefficient one, retain wet exactly. Between endpoints use complementary linear weights. Keep the wet limiter before the blend, and do not begin clipping bypassed dry input: the existing bypass route preserves overrange dry samples. Extra outputs fade smoothly to zero. The new ramp changes the old instantaneous toggle intentionally and must be documented.

### AutoGain and diagnostics

Run both existing input and **uncompensated wet-output** measurements at the existing cadence while bypassed; continue wet compensation and limiter state. Apply neither AutoGain compensation nor the wet limiter to the dry branch. This avoids the current input-only meter advancement and produces a ready wet path on re-enable. Diagnostics should describe the continuously prepared wet processor; a bypass indication should not falsely reset its actual limiter state.

The normal once-per-ten-callback AutoGain cadence is already partition dependent and remains a separate issue. This proposal does not claim to solve it. Strict waveform/partition oracles use AutoGain off; AutoGain-on tests compare identical accepted callback/control histories and vary only drain destination capacities. Always processing wet audio increases disabled CPU cost; that is the explicit tradeoff of a continuously ready fixed-latency bypass.

### Reset, EOS and native bounds

Replace the enabled-only phase with the **all-accepted-input phase modulo H**, because the wet processor now always advances. Existing invalid-call preflight must precede dry ring, wet state, ramp or phase changes. Reset clears dry/wet history and EOS/cache, sets the ramp directly to the configured enabled target, and preserves the existing pending-publication policy.

With controls fixed at accepted EOF:

- No accepted input: immediate complete, reusable as before.
- Settled disabled (`wet_mix=0` and target zero): exact dry support **N**. Continue the shared kernel with zeros until N output frames are returned; the hidden wet state may stop unfinished because further input/control changes require reset.
- Enabled or transitioning: retain the existing full wet bound using total accepted phase,
  `R = 2N-H + ((H-S%H)%H)`; it already exceeds N and covers the delayed dry branch. The ramp itself cannot create audio after both paths are zero.
- Conservative tail metadata can be N for settled dry and `2N−1` for observable wet/transition state. Latch the full declaration for an accepted full-support EOS epoch through cached output/completion, so a canonical refill that finishes a fade cannot prematurely shrink the bound. Reset clears this epoch latch.
- Retain prepared canonical H-frame continuation and its unread-cache accounting. Dry full-capacity work is four H-block calls; wet/transition work is at most eight, with the existing extra call for a partially consumed cache. Scalar `drain_call_bound`, stable maximum H capacity, rate/shape/capacity transactionality, no-heap operation, identical snapshots and reset-required continuation stay intact.
- Preserve EOF's pending-publication freeze; currently active filter fades can continue during the rendered support. Do not introduce new asynchronous adoption or resource destruction on the callback.

## Why dynamic latency metadata alone is insufficient

Returning zero latency when disabled would describe the immediate path, but a live toggle would change topology compensation by N frames. Current hosts/wrappers build PDC from prepared metadata; a bool setter cannot safely rebuild every branch or make native hosts reconfigure synchronously. Abruptly adding/removing N frames also requires an explicit output-timeline policy to prevent gaps, repeats or misalignment, and it does not fix the paused wet history by itself.

Resetting wet state on enable avoids one replay but discards the recent disabled input and restarts N frames of startup silence. Keeping fixed N with a warm aligned dry path lets direct/factory/native APIs use the same stable PDC and preserves source order across arbitrary toggles. No host lifecycle changes are needed.

## Required permanent regression scope

1. Convert the public disabled impulse and actual host parallel graph into red tests; require one correctly aligned peak and constant declared N through every state.
2. Neutral full-waveform oracle `[N zeros]+source` across all supported FFT sizes, rates, callback partitions, ring wraps, oversized calls and repeated interrupted toggles; cover first/final markers and the reproduced stale/fresh marker pair.
3. Nonidentity two/four-output recommended matrices: independently verify delayed dry routing, ramp endpoint/count/reversal and extra-channel fade. For all-enabled input retain the existing waveform/latency oracle.
4. EOS while initially disabled, settled disabled, fading both ways and recently re-enabled. Compare with same-history ordinary zero continuation, exact finite lengths, metadata latching, partial-cache quotas and post-EOF controls. Keep no-input behavior and failed capacity/rate/shape calls transactional.
5. Cold fresh-thread allocation **and deallocation** assertions for bool automation, first dry-ring use, enabled/disabled callbacks, active filter fade/retirement, canonical drain refills, reset and reuse. Retain caller-owned ParameterId storage so final caller-ID destruction is not mistaken for plugin activity.
6. Staged initialization failure/reset evidence must include the new dry ring/ramp/accepted phase. Existing generation-race and source-load transaction tests stay green.
7. AutoGain-on paired histories must prove both meters, smoother and limiter continue while dry remains unmodified; do not conflate that with solving normal callback cadence.

## Compatibility and approval boundary

This changes an audible old behavior: disabled XTC gains its already advertised N-frame delay, toggles crossfade over 10 ms, and disabled mode consumes wet-processing CPU. It removes delayed replay and fixes host PDC without changing parameter IDs/defaults, output geometry or filter preferences. The default continuously enabled waveform should remain unchanged. The 10 ms duration is a proposed internal policy for review, not an implemented or previously documented promise.

Approved implementation scope is the XTC crate only: prepared dry/ramp state and staged initialization/reset, final output mix, enabled scalar cache update, accepted-phase/EOS metadata accounting, focused tests/docs. Implementation and permanent regression verification are in progress; see the final AUD099 report for results.
