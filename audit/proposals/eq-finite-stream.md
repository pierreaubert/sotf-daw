# EQ internal oversampler: scoped finite-drain proposal

2026-09-28. Initial read-only investigation, followed by parent approval of the implementation below and an isolated public cold-callback probe. Scope is AUD-073, not host branch queues or the manager protocol.

## Existing implementation

- `sotf-plugin-eq/src/lib/eq_plugin.rs:108` owns an optional `Oversampler`. The non-SVF oversampled path at line 1524 runs its prepared FFT up/down pipeline and then ordinary EQ output metering and AutoGain compensation.
- `sotf-host/src/parametric_plugin.rs:41` lacks `drain` and `drain_output_frames_max`. Its adapter therefore inherits immediate completion even if an EQ implementation could emit retained samples. Production implementors are only EQ and Gain.
- `sotf-host/src/oversampling/oversampler.rs:322` already has a private finite state machine: remaining output, padded partial input, one up-filter overlap, inner tail, down partial, one down-filter overlap. It performs at most one internal chunk per call. Generic wrappers use it; EQ cannot call this `pub(super)` method.
- Existing `Oversampler::tail_length(Finite(0))` at line 206 gives a conservative 1024 native-frame response bound. It includes the fixed 256-frame startup queue, residual input phase/padding, and one chunk of overlap in each FFT resampler. Group delay alone is insufficient.

## Smallest proposed implementation

### Additive parameterized-plugin hooks

In `parametric_plugin.rs`, add the same default hooks already present in `ParametricInPlacePlugin`:

```rust
fn drain_output_frames_max(&self) -> usize { 0 }
fn drain(&mut self, output: &mut [f32], context: &ProcessContext)
    -> PluginResult<PluginDrainResult>;
```

The default implementation returns `PluginDrainResult::COMPLETE`. The existing `Plugin` adapter forwards both methods. Gain keeps these defaults, its existing finite-zero tail metadata, and its existing process behavior.

### Narrow finite eligibility

An EQ state is eligible only when **all channels** have empty biquad banks, empty SVF banks, empty advanced-filter banks, and no active coefficient transitions. Checking only the first channel, zero band gain, current silence, or identity-looking coefficients is insufficient.

- Eligible 1x state has no retained audio: finite zero and existing immediate completion.
- Eligible 2x/4x state with a prepared oversampler has finite buffered response.
- Any remaining filter bank retains the existing unsupported recursive drain behavior (`COMPLETE`), with conservative `Unknown` metadata. No threshold, guessed time cutoff, new render error, or claim that recursive EOS is repaired.
- Removing all filters on the control thread discards their recursive states; retained oversampler queues remain finite and may then be drained. Tests must cover this history.

AutoGain can remain eligible: it multiplies existing output and does not generate audio from zero input. Preserve its current measurement cadence and compensation kernel rather than bypassing it during drain.

### Reuse ordinary zero continuation, not a new general flush API

The smallest implementation does not expose the generic closure-based `drain_with` method. Add one documented scalar accessor to `Oversampler`, tentatively `passthrough_tail_frames() -> usize`, returning the existing conservative `4 * OS_CHUNK_SIZE` support bound for a memoryless inner operation. Reuse that constant/helper in the existing private tail calculation to keep one source of truth.

EQ drains into a prepared 256-frame cache, always running exactly four canonical 256-frame zero-input blocks for the 1024-frame bound. It serves arbitrary frame-aligned destination capacities from this cache, at most 256 frames per call. This preserves input-zero measurements, metering, compensation, residual queues and FFT state as ordinary continuation with that canonical partition. Destination capacity cannot retime AutoGain or its metering cadence. One cache refill is the maximum DSP work per drain call. It can append conservative zeros at the endpoint, as documented; it does not claim minimal output length.

Each call supplies at most 256 new native frames. With fewer than 256 residual input frames, that causes at most one up/down chunk. Existing allocations cover this drain work. The bound is independent of oversampling factor because all terms are expressed in the native clock.

### Lifecycle and transactional preflight

- Add private accepted-input and finite-drain state. Empty streams complete without freezing; recursive defaults retain existing behavior. Eligible 1x states accept nonempty EOS and freeze consistently even though they have no retained samples.
- First nonempty eligible drain validates matching positive prepared rate, frame-aligned nonzero capacity, and valid oversampler state before touching output or state. Freeze only after successful acceptance.
- Retain the original per-call `ProcessContext` rate and bounded frame count. No fabricated external transport data is needed because the eligible kernel is context-independent except for dimensions/rate.
- After finite drain starts, reject nonempty processing before copying the input to output or updating metering. Check both ordinary and compiled entry points even though oversampling normally disables the compiled bank.
- Guard scalar/batch controls and `set_filters`/`set_channel_filters` after accepted drain. Known unchanged primitive writes may succeed without invoking setters that rebuild state; changed updates require reset. Validate a whole batch before mutation.
- Reset clears counters and histories. Successful initialization re-arms the endpoint along with prepared oversampler state. Preserve unrelated existing initialization behavior for recursive filter banks.
- Repeated completed drain returns zero complete without modifying destination. Only the emitted prefix is written.

## Concrete preparation concern found during review

EQ documents a prepared maximum of 4096 input frames. It constructs `Oversampler::new` directly. That constructor initially prepares residual output for `(256 + latency) * 4` frames, normally 3072; ordinary `process` accumulates all chunk results before reading output. A first 4096-frame callback can require the existing 256 startup frames plus 4096 queued output frames, causing `ensure_residual_out_capacity` to resize. Generic host wrappers separately reserve their negotiated maximum; EQ cannot call that private method.

Confirmed by the parent-authorized `/tmp/sotf-eq-cold4096-probe.rs`, linked against current public EQ and matching host artifacts. Every 2x/4x × mono/stereo fresh-thread case returned `Ok(4096)` while counting one allocation and one deallocation. Log: `/tmp/sotf-eq-cold4096-red.log`. Setup, input storage, thread creation and teardown were outside counting.

Approved correction: make the existing `reserve_for_max_frames` method public with an explicit control-thread/preparation contract, and call it when EQ prepares an oversampler for its advertised 4096-frame limit. Do not inflate every primitive constructor or alter other callers. Cover initialization and structural oversampling selection. Existing overflow validation remains intact.

## Focused red/green verification plan

1. Public EQ adapter: empty-bank final impulse/signed stereo marker with 2x and 4x, input lengths around 1/17/255/256/257/1023/4096. Capture current missing suffix through default drain.
2. Independent endpoint oracle: ordinary processing of a substantially longer zero-padded stream, using the same prepared configuration and no drain. Compare the candidate's program plus 1024-frame continuation, then verify later reference samples contain no meaningful retained input. No silence threshold determines the candidate endpoint.
3. Partition comparisons: irregular processing blocks and drain capacities 1/17/256/257/4097; output canaries, maximum 256 frames per call, exact finite count and bounded completion.
4. AutoGain on/off and a call crossing the measurement throttle: candidate drain versus four canonical 256-frame ordinary zero blocks; destination partition changes must not change output. Normal callback AutoGain cadence remains a separate existing behavior.
5. Eligibility: per-channel active bank, SVF, advanced topology, neutral-gain recursive bank, transitions, and filter removal after prior recursive processing. No finite claim based on a neutral snapshot.
6. Lifecycle: invalid capacity/rate leaves state/output unchanged; empty/repeated drain; unchanged control writes; rejected changed scalar/batch/replacement controls; new processing rejection before output copy; reset/reinitialize replay.
7. Cold thread allocation/deallocation guard for first small and maximum accepted process callback, first drain, complete drain and reset. Include 4096 input frames to attribute the preparation concern above.
8. Host synthetic `ParametricPlugin` forwarding/default tests. Actual Gain adapter must still return zero complete without writes. No native f64 drain is promised because the shared drain interface is f32.

## Affected files if approved

- `sotf-host/src/parametric_plugin.rs`: two defaults and forwarding.
- `sotf-host/src/oversampling/oversampler.rs`: scalar support accessor and public documented existing reserve method.
- `sotf-plugin-eq/src/lib/eq_plugin.rs`: eligibility, finite lifecycle, shared kernel and bounded zero continuation.
- New focused host/EQ tests and EQ documentation.

No native wrapper, bridge parameter schema, preset IDs, Gain implementation, engine scheduling, DAG queues, or manager transitions need to change.
