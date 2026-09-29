# Delay and Convolution native finite EOS: verified checkpoint

Production and tests are frozen after the approved local implementation. No host/engine, DAG queue, manager, MIDI, IAMF, or native wrapper edits were made. Existing unrelated worktree changes are preserved.

## Delay

- Extracted the existing sample kernel without changing DSP arithmetic. Finite native drain uses zero-input continuation for one prepared ring traversal, covering per-channel delays, fractional interpolation, modulation, and clean-transition read heads.
- Each call writes at most 1024 frames, accepts any positive whole-frame capacity, writes only the returned prefix, and completes with the last nonempty result. Invalid rate/shape/capacity attempts preserve signal state and output.
- Empty streams complete without freezing. Accepted finite EOS rejects new nonempty input and changed controls until reset/initialize; unchanged scalar/batch writes remain accepted. Continuation and reset allocate and free no storage on cold audio threads.
- Recursive feedback remains the explicitly unsupported legacy immediate-completion branch, with `TailLength::Infinite`. No cutoff or new render error was introduced, and recursive EOS is not claimed fixed.
- Existing parameter IDs, schema, presets, signal arithmetic and feedback behavior are unchanged.

Evidence: the initial final-impulse regression returned only 34 samples instead of the required 2082. It now passes. Independent static taps use the Lagrange polynomial's defining product; modulation and pending automation use a separately zero-padded normal-processing reference. Tests cover input partitions 1–17003, drain capacities 1–4097, 44.1/48/96/192 kHz, cold process/drain/reset allocation **and deallocation**, unchanged/rejected control writes, reset/reinitialize, and the public Plugin adapter.

Delay verification: 96 distinct tests pass (full 95-test suite followed by the final seven-test finite suite containing one additional adapter test). All-target/all-feature Clippy passes.

- `target/audit-delay-drain-red.log`
- `target/audit-delay-drain-green.log`
- `target/audit-delay-drain-focused.log`
- `target/audit-delay-drain-clippy.log`

## Convolution finite drain

- Native drain covers UPC, NUPC, zero-latency heads, inactive delayed dry audio, and held-output fades. Frozen support is `max(latency + longest_IR_length - 1, remaining_transition_frames)`; the same processing kernel preserves FFT partial blocks, overlap, output queues, dry alignment and smoother trajectories.
- Work is bounded by one 1024-frame partition per call. Any positive whole-frame capacity is supported. Preflight precedes destination mutation and EOS entry; suffix sentinels remain unchanged.
- The first accepted drain freezes the active backend. Pending completions, including already-ready ones, remain in the mailbox until reset permits ordinary processing to accept them. Worker requests never hold a plugin-state reference. Pending native tail metadata remains Unknown.
- Reset rearms input and clears histories without dropping mailboxes. Initialize preserves its ordinary historical behavior; initialize after accepted EOS clears/rearms history together. Invalid zero-rate initialization leaves EOS intact.

Evidence: 26 of 28 initial backend/IR cases lost their final response; only one-tap zero-latency heads had no tail to lose. The final oracle covers signed dense and last-only IR taps, cyclic IR mapping, mix 0/.375/1, gain, partial and oversized input blocks, capacities 1/17/255/1024/4097, automation before EOS, exact last-tap timing, and other prepared rates through 192 kHz. Async fixtures deterministically deliver ready/late/stale/failure completions and verify frozen waveform, metadata, reset adoption, and retirement saturation.

## Convolution callback ownership corrections

1. Cold active-state load originally allocated one ArcSwap reader node. The isolated first audio callback measured `(1 allocation, 0 frees)`. Private storage is now an audio-owned `Arc<Option<ConvolutionState>>`. A per-call Arc clone cannot be the final owner because the plugin retains the active root; replacement moves the prior root into the existing retirement payload before queueing. No mutex or publication API was added.
2. Completion adoption originally freed one channel message block when `ir_load_result_rx = None` dropped the consumed receiver. An isolated standard-library probe measured receive `(0,0)` and receiver drop `(0,1)` while the sender remained alive. A sender keepalive alone was insufficient.
3. Both completion endpoints now survive callback processing and reset. `completion_pending` separately drives polling and tail metadata, and flips once for success, stale/error completion, or enqueue failure. The next control-thread load/clear/teardown releases consumed mailbox storage. Each loader request retains its existing separate one-result mailbox; workers cannot publish an older result into a newer request's mailbox.
4. New lifetime fixtures hold only Weak observers of the old/new states. They verify sole pending ownership, old final-Arc retention when both retirement queues are full, deferred release after the test reclaimer receives the payload, and zero callback allocation/free across reset→adoption. The unit allocator delegates to the existing host CountingAlloc so older allocation assertions remain operative too.

All existing channels, reclaimer queues and payload types are unchanged. Public native ABI/preset/schema behavior is unchanged. Cargo.lock changed by exactly one line versus the immediate pre-change snapshot: removal of `arc-swap` from `sotf-plugin-convolution`'s dependency list. The workspace still retains arc-swap for other users; unrelated lock changes were preserved.

Convolution verification: **71 tests pass** (50 unit + 4 direct convolution + 7 finite-stream integration + 10 existing integration). All-target/all-feature Clippy passes. The final 50-unit rerun also passes with the combined allocator instrumentation.

- `target/audit-convolution-drain-red.log`
- `target/audit-convolution-inactive-drain-red.log`
- `target/audit-convolution-cold-red.log`
- `target/audit-convolution-cold-green.log`
- `target/audit-convolution-owned-state-tests.log`
- `target/audit-convolution-drain-green.log`
- `target/audit-convolution-drain-unit-final.log`
- `target/audit-convolution-drain-clippy.log`
- Attribution probe: `/tmp/sotf-convolution-receiver-probe.rs`

## Scope and remaining integration limits

The engine still aborts after 4096 drain calls (`processing_state.rs:1038`). A 30-second IR at 192 kHz takes about 5626 calls at the 1024-frame quantum; multiple serial tails or smaller caller capacity can also exceed that policy. The plugin response is not shortened to fit it. This is separate host/renderer work, and no engine source was changed.

No guessed infinite-tail truncation, host DAG drain expansion, broad preparation API, or manager protocol work is included. Native drain remains an f32 output contract as defined by the existing host trait; no new native f64 drain API is claimed. Allocation/free observations are deterministic on the current Linux test environment; no new native macOS runtime verification was performed.
