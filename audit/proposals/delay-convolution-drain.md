# Delay and Convolution finite-stream drain proposal

Status: read-only design. No Rust edits or test runs made for this task. MIDI/IAMF, host DAG queues and manager protocol are excluded. Parent owns approval and the audit ledger.

## Contracts and measured-source evidence

- `sotf-host/src/plugin.rs:246-264`: `drain_output_frames_max` is per-call capacity, not total tail. `drain` must be allocation-free, transactional on capacity errors and eventually complete. Both parametric adapters already forward these methods.
- Gate `gate_plugin.rs:1127-1196` and Limiter `limiter_plugin.rs:1051-1111` use a shared normal/zero-continuation kernel, `has_input`, an optional remaining count, exact output prefix, reset before later input, and no control mutation once EOS has begun.
- Delay `delay_plugin.rs:517-528` advertises `Finite(max_samples)` without recursive history and `Infinite` after any nonzero feedback until reset/initialize. Its processing kernel at872-1028 retains channel rings, Lagrange interpolation reads, LFO phase and clean read-head transitions; there is no drain override.
- Convolution `convolution_plugin.rs:1130-1153` advertises latency + prepared IR length - 1, conservatively extended through replacements; pending asynchronous work is `Unknown`. Its UPC/NUPC/inactive dry processing at1156-1422 retains real output. There is no drain override.
- Host `daw_host.rs:2010` propagates drain errors. Engine `processing_state.rs:1038-1060` stops the processing worker on a drain error or after 4096 drain calls. A new recursive-Delay error would therefore regress ordinary playback/render completion. Returning incomplete indefinitely also stops the worker at the limit.

## Shared implementation pattern (local to each plugin)

1. Extract the existing DSP body as a private normal/zero-continuation kernel without changing its sample arithmetic. The public process method tracks whether a valid nonempty input has been accepted, and rejects new nonempty input after accepted EOS until reset/initialize.
2. Add `has_input: bool` and `drain_remaining: Option<usize>`. `None` means accepting input, `Some(n)` is frozen EOS, `Some(0)` is completed. A failed or empty-capacity attempt while output remains must not enter EOS.
3. Preflight rate/preparation, nonzero channel count, whole-frame destination shape and at least one destination frame when a tail is pending. Validate before modifying destination, stream state, pending IR publication, or controls. `context.num_frames` is not destination capacity; replace it only in the private continuation context. These DSP kernels ignore transport/events, so retain all other caller metadata unchanged.
4. Compute `frames = min(destination_frames, remaining, quantum)`, fill only that prefix with zero and run the same in-place kernel. Leave destination suffix sentinels untouched. Decrement only after successful processing. Return the final nonempty block with `complete=true`; subsequent calls return COMPLETE without mutation.
5. Use the caller output prefix as zero-input scratch: both plugins have equal input/output widths. No new heap scratch, no temporary Vec, no generic prepare API. Existing prepared FFT/ring storage suffices. Advertise a stable 1024-frame maximum per call (or smaller finite bound); any positive frame-aligned capacity, including 1, remains accepted. This is an internal work quantum, not a process callback maximum.
6. Freeze all parameter-changing routes after accepted EOS, including bulk apply, primitive setters and Convolution's public load_ir/clear path. Empty streams return COMPLETE without freezing future input. Reset clears EOS and signal history. Successful initialize must also rearm EOS only together with clearing old histories; do not merely clear the counter while retained audio survives. Preserve normal constructor/setup behavior (Convolution constructors already prepare usable rate/channel storage; do not newly require a separate initialize for existing callers).

### Quantum / engine limit tradeoff

1024 frames is one UPC partition, gives bounded per-call work, and emits a 30-second IR at48k in about1408 calls, or at96k in about2814 calls. 256 frames would already exceed4096 calls at48k. The maximum5-second Delay ring at768k is4194304 frames: exactly4096 calls at1024 frames before any other chain tail. A 30-second Convolution at192k needs about5626 calls. Small caller capacities or multiple serial stateful plugins also exceed the global budget. This is a concrete existing engine-policy gap, not a plugin tail-length error. Do not silently truncate, choose a quantum dynamically solely to evade the limit, or edit the engine under this task. Independent plugin drain tests may legitimately exceed4096 calls. A separate host/renderer proposal should replace a fixed call-count limit with a progress/explicit render-budget policy.

## Delay: provable finite configurations

### Bound

For `recursive_tail == false`, use the already advertised `max_samples` ring length, snapshotted at first valid drain. It covers every legal old/new delay tap, per-channel delay, automation smoother, LFO modulation and the four interpolation taps; all writes under zero continuation are zero because feedback and allpass history have been zero since reset. After one complete ring traversal, no input-derived audio remains. Do not use just the current delay_ms: smoothing and transitions can retain older, longer taps. Do not infer a shorter bound from mix=0. The extra trailing zeros are an explicit conservative bound, not threshold trimming.

For modulation or pitch-preserving changes already in progress, continue all smoothers, phase, and transitions exactly as normal zero-input processing. Freeze new control writes at EOS; existing trajectories continue.

### Recursive feedback: separate unresolved policy

A nonzero-feedback latch means recursive history may survive even after the target becomes0 (feedback smoothing and allpass state matter). Allpass recursion and modulated/fractional delay do not supply a simple exact finite support proof. TailLength must stay Infinite; neither a guessed seconds cutoff nor a numerical silence threshold proves exhaustion.

Do not add a new drain error to these existing presets without host/renderer policy approval. The smallest compatibility-preserving finite-only patch would retain the current COMPLETE behavior specifically for recursive history and explicitly document that this configuration remains unsupported; it must not claim recursive EOS is fixed. This branch violates the aspirational complete=no-retained-audio contract just as today. Preferred staged delivery: implement/test finite support, keep the recursive policy as a separately approved integration decision before declaring Delay EOS support complete. A renderer-controlled maximum duration or explicit truncation result needs a distinct contract and is outside these two plugin crates.

### Tests

- Red: short final impulse with delay longer than the input, drain through public ParametricInPlacePlugin/Plugin adapter; current default loses it.
- Independent analytic integer and fractional delay sums with signed/channel-distinct impulses; integer, subframe and near ring-limit taps; final impulse is at the final accepted input frame.
- Exact raw zero-padded reference for modulation, pending scalar/per-channel delay changes, mix ramps, pitch-preserving transitions, allpass enabled with zero feedback. Include44.1/48/96/192k plus a small allocation-bounded high-rate ring case.
- Process partitions1/17/255/1023/1024/1025/17003; drain capacities1/17/255/1024/4097; exact total emitted finite bound; monotonic progress; repeated COMPLETE; empty stream; reset and reinitialize equivalence.
- Invalid rate, partial frame and zero capacity preserve output and next waveform; rejected mid-drain control writes preserve targets and history. Fresh thread first drain and reset count both allocation and deallocation, with no warmup on that thread.

## Convolution: stable/frozen active response

### Bound

At the first valid drain, freeze the active backend and compute `max(latency_samples + max_ir_frames.saturating_sub(1), transition_remaining)`, checked for representability. This is the current signal state's support: old backends are already discarded during replacement; their only surviving signal is the scalar transition_from fade. The existing retained_tail_frames getter may remain conservatively larger for native-host scheduling; it need not force excess drain zeros after every historical swap. No-IR/loading/failed/cleared states still have the declared inactive dry delay. Head mode has zero scheduling latency but still needs IR_len−1 response samples. Never replace this support by latency alone or rounded FFT partition count.

Feed exact zero continuation through the same UPC/NUPC/inactive path. UPC naturally finishes the partial input partition, FDL response, overlap and output ring. NUPC naturally advances its internal partitions/direct head. Dry alignment and mix/gain smoother trajectories remain identical to a normal zero-padded stream. No additional FFT-specific tail flushing algorithm is needed.

### Asynchronous ownership and EOS freeze

Move mailbox polling/installation into a small method invoked by normal processing only after normal preflight. The drain kernel must not poll or install completions. First valid drain freezes whichever backend was active before the call; even an already-ready but unconsumed completion stays pending. This gives deterministic active-stream semantics instead of depending on worker timing.

The worker request at37-47 owns only generation/configuration and a result sender; it never holds a plugin-state reference. It builds a complete owned IrLoadResult and posts it. Leaving the receiver and keepalive sender untouched during drain means the callback cannot destroy a completed backend, receiver allocation, or error String. At most the existing single desired completion remains retained. A pending job may finish into that mailbox while the frozen stream drains, without changing the active DSP. Reset/initialize rearms ordinary publication; stale-generation/error retirement retains existing behavior. Plugin teardown still owns and destroys pending state on its existing control-thread lifetime path. Freeze new load_ir/clear/bulk/scalar mutations while EOS is active; do not cancel by dropping the receiver on the callback. Existing tail_length may remain Unknown while a result is pending; this is conservative and distinct from the explicitly frozen drain operation.

### Cold ArcSwap blocker and minimal ownership correction proposal

Current kernel line1209 calls `self.state.load()`. Pinned arc-swap1.9.2 `strategy/hybrid.rs:189-190` enters LocalNode; `debt/list.rs:149-169` may allocate a new Box on a cold thread when no unused node exists. Current long-IR RT test (`src/lib/tests.rs:745+`) explicitly warms processing before counting. Existing fresh-thread tests cover only scalar/tail getters. Thus allocation-free cold drain cannot be claimed by reusing this kernel unchanged.

First add an isolated CountingAlloc red test: construct/load and feed input on the control thread, keep that thread alive, move the plugin and prepared destination to a new thread, count only its first drain/zero-continuation call. Also exercise a cold first process. Count alloc AND dealloc; do not warm or permit allocation. Root should approve the ownership change after the red evidence.

Smallest ownership change if confirmed: `IrRuntimeState.state` is private-to-crate and has no shared production reader or external handle. All production reads/writes are one process load and control/audio installation/clear; workers send owned results. Replace `Arc<ArcSwap<Option<ConvolutionState>>>` with audio-owned `Arc<Option<ConvolutionState>>`. Read with an Arc clone for the duration of a DSP call (atomic refcount only, never last because the plugin still owns the active Arc), or a safe split-field borrow. Install/clear with `mem::replace` and move the old Arc into existing RetiredIrState before enqueueing; never drop a last active Arc in the callback. No mutex, no TLS, no new shared cache and no change to loader/reclaimer queues. Keep test observer Arcs alive as before; adapt private tests that directly used load/store. Review every clear/install/failure/backpressure path before applying. Update stale crate docs describing ArcSwap. This is a local private ownership correction, not a publication API redesign.

### Tests

- Red: current native drain loses last response for no-IR delayed dry, UPC, NUPC and direct-head mode; use real public file loading.
- Independent f64 direct convolution, cyclic mapping of two unequal IR channels onto three program channels, mixes0/.375/1 and nonzero gain. Dense and final-only signed IR taps at lengths1/97/127/128/129/1023/1024/1025/2053/8195; input ending at an awkward partition position with a final impulse. Assert exact returned count, last nonzero tap, no duplicated startup delay and post-bound zero continuation on a reference clone.
- Existing zero-padded stream reference for live gain/mix ramps and already-started replacement/clear fade. Do not use tested drain as its own oracle.
- Deterministic prepared completion mailbox: inject replacement before first drain and during drain, including longer/shorter IR, stale generation and error. Assert frozen waveform, pending ownership, reset publication, and no callback last-Arc/error/receiver destruction. Reclaimer saturation fixture remains valid.
- Same lifecycle/capacity/partition/transactionality tests as Delay. Test cold first drain on a different thread from prior processing, first process, ready completion, retirement backpressure, repeated COMPLETE/reset, and allocation/deallocation counts.

## Proposed files and order

1. New finite_stream integration tests and limited private deterministic mailbox fixtures under the two plugin crates; capture independent red logs first.
2. Delay local stream/drain state and kernel extraction for finite configurations, pending explicit recursive compatibility policy.
3. Convolution cold ownership red and approved private Arc correction, with all existing lifetime/replacement tests.
4. Convolution kernel/publication split, frozen EOS and exact finite bound.
5. Per-crate full tests and all-target Clippy; parent aggregate later. No host trait/adapter/engine/wrapper parameter or public preset changes.

Remaining approval points: recursive Delay compatibility branch; Convolution private Arc ownership correction after red measurement. Engine4096-call policy is a recorded separate integration limit, not included in implementation scope.
