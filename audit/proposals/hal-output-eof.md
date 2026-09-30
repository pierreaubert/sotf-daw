# AUD138: HAL Output finite-stream EOF

## Finding and pre-edit evidence

`HalOutputPlugin::process` can accept an input block while `HalWriter` accepts only a prefix. The plugin retains the frame-aligned unwritten suffix in `pending`; `flush_pending` retries it only at the start of a later ordinary `process` call (`crates/sotf-plugins/crates/sotf-plugin-hal-output/src/lib.rs`). `HalOutputPlugin` has no `drain` override, so the default `Plugin::drain` returns `PluginDrainResult::COMPLETE` with zero frames (`crates/sotf-plugins/crates/sotf-host/src/plugin.rs`). EOF can therefore report complete with pending samples.

The plugin is a sink (`output_channels() == 0`). Its injected `HalWriter` and existing test `FakeWriter` let public `Plugin::process`→`Plugin::drain` behavior run on any platform without HAL hardware.

Test-only fake-writer accounting and two independent public regressions were added in `crates/sotf-plugins/crates/sotf-plugin-hal-output/src/lib.rs`:

- `eof_drain_flushes_the_final_partial_write_to_the_writer` scripts a short write, then checks accepted interleaved samples after EOF drain. It fails because only 4 of the original 8 samples reach the fake writer while drain returns complete.
- `eof_drain_reports_backpressure_and_retries_without_new_input` scripts a short write followed by zero writable frames. It fails because drain returns complete with 2 pending frames. Its later retry assertion is the intended green oracle; it is not reached in this red run.

Run: `cargo test --offline --locked -p sotf-plugin-hal-output eof_drain --lib`, with the shared warm target and offline environment. Exit status 101; 0 passed, 2 expected failures, 27 filtered. Log: `/tmp/sotf-aud138-hal-red-clean.log`, SHA-256 `ed2a9c51949eaa66a710b416fdbf47208e7fe0ef751e99683eaac03bc00bc644`.

Pre-test source snapshot: `/tmp/sotf-aud138-hal-output-pre-edit.rs`, SHA-256 `4b782ed45bbc5913b13c7e113b1f2c43c9391187e6c3b945af568c42cf242243` (1168 lines). No production code was changed for the capture or red run.

## Refined plugin contract

### What EOF promises

The EOF guarantee covers only samples still retained in `pending`. `process` already drops newest complete frames when that bounded queue is full and records them in `dropped_frames`; EOF cannot recover those earlier losses. Drain adds no silent drop. `flush_audio` or transport quiescing must never be reported as successful delivery.

At the plugin boundary, complete means the retained queue is empty because the writer accepted those samples into the shared-memory ring. It does not mean that CoreAudio physically played them. A sink always returns `frames == 0`, even when its writer accepts ring frames.

### Work, progress, and backpressure

- A call with no pending samples returns complete with zero frames, including an initial/empty call and repeated calls after completion. Its `drain_call_bound` may be one.
- While samples are pending, `drain_call_bound` remains `None`: external transport availability has no finite call bound. It becomes a finite empty-state bound only after the queue empties. Never invent a convergence bound by dropping samples.
- A ready-writer call makes at most two `write` attempts, matching the existing `flush_pending` budget. A zero write stops the call. Short and wrapped writes preserve logical frame order. Each valid accepted prefix is removed exactly once; every unaccepted sample stays queued. `drain_output_frames_max()` remains zero.
- If the writer is disconnected, the key is not ready, configuration changed, service is in progress, priming failed, or the negotiated format is invalid, pending data remains unchanged and drain reports incomplete with zero frames. Drain never reconnects, reloads a key, primes, flushes, allocates storage, or waits.
- If the writer is absent or violates its accepted-frame contract by over-reporting, return an error without consuming the affected unwritten queue portion. If a prior attempt in the same call validly accepted a prefix, retain that progress and preserve the remaining suffix. Invalid context and nonempty sink destination errors are also transactional.

The shared `Plugin::drain` documentation currently says implementations eventually return complete. That is not an unconditional promise a disconnected sink can make. Keep the bound unknown while pending and resolve the host/engine behavior for a sink that stays unavailable; do not hide that liveness limit with a finite fallback or sample loss.

### Validation, lifecycle, and telemetry

Before changing queue or transport state, validate that initialization succeeded, `context.sample_rate` is nonzero and matches the initialized rate, and the sink destination is empty. The host must prevalidate the destination before `begin_drain`; the plugin repeats the checks in `drain`. Validation errors preserve pending samples and the caller's buffer.

Drain is a point-in-time queue operation and needs no terminal latch. If `process` arrives while drain is incomplete, ordinary process ordering applies: old pending samples stay ahead of new audio, and any queue-capacity overflow continues to count newest dropped complete frames. A drain that returned complete closes only the current empty queue; later process input starts more work for a subsequent EOF drain.

Keep counters lifetime-monotonic: `requested_frames` is process input only; `written_frames` increases by each valid writer-accepted pending frame; `dropped_frames` preserves existing process-overflow accounting and also records pending frames explicitly cancelled by reinitialization. Drain retries never increase drops. A writer zero-write counts as backpressure; transport gates update state without pretending a writer attempt occurred.

`service_transport` is control-thread recovery. Refine its quiesce path so it may quiesce/flush the shared ring and re-prime it, but does not clear the plugin's pending queue. On reconnect, key, configuration, format, or priming failure the queue remains available for a later recovery and drain. Do not call this control operation from drain. `initialize`/reinitialize is an explicit epoch-cancellation boundary: after validating its new nonzero rate and compatible format, it may cancel pending samples, add their frame count to `dropped_frames`, and perform the existing transport reset. Failed preflight leaves the old queue and telemetry intact. This explicit lifecycle cancellation is not EOF success. Teardown ends the plugin lifetime; it does not certify pending delivery.

### Bounded storage and errors

`pending` is bounded by the negotiated transport capacity. For frame-safe traversal across `VecDeque` physical wrap, prepare a drain staging buffer on the control thread to at least that capacity; gather the logical pending prefix into it without allocation, then make at most two writer calls. Remove from `pending` only the valid accepted prefix. Do not replace or shrink this storage on first, repeated, blocked, or terminal drain calls. Successful and incomplete/backpressured calls must allocate and deallocate nothing. The existing owned-`String` plugin error API may allocate on invalid-argument or writer-contract-violation errors; those exceptional errors must still leave every unaccepted frame queued. No broader error-type change is proposed here.

## Consuming route and boundary

The factory exposes `hal_output` only on macOS with the `hal` feature (`crates/sotf-plugins/src/factory/create.rs`, `create_plugin`); the catalog marks it systemwide platform I/O (`factory/catalog.rs`). The current `sotf-systemwide` daemon strips and rejects `hal_output`/`hal_input` graph nodes because the decoder thread handles driver I/O directly (`../sotf-systemwide/crates/daemon/bin/sotf_daemon/misc.rs`). The active daemon is therefore not this plugin's consumer; the concrete current entry is factory-created generic-host client configuration.

That generic EOF route remains required and unimplemented. `DawHost::drain` rejects zero-output host/plugin layouts (`crates/sotf-plugins/crates/sotf-host/src/host/daw_host.rs`), while the engine drain loop retries incomplete zero-frame results without sending output or waiting (`crates/sotf-engine/src/engine/processing_thread/processing_state.rs`). A sink-specific host call path and scheduled non-busy retry/recovery policy require their own concrete review. Do not change generic unequal-rate queues or manager protocol under this proposal.

`HalOutputWriter::write` accepts samples into shared memory (`crates/driver-hal/src/shared_memory/hal_output_writer.rs`). The Rust `HalDriver` reads through `HalInputReader`, and current Swift `driverDoIOOperation` comments that its input callback does not consume that ring as playback (`../sotf-systemwide/swift/driver-hal/Sources/SotFHALDriver.swift`). The fake-writer regression establishes loss before ring acceptance only. Physical playback remains unproven and outside this plugin-level guarantee.

## Acceptance tests for a later implementation

- Keep both portable public regressions; make the ready-writer test assert exact accepted sample order and the blocked/retry test assert retained frames, unknown call bound while blocked, and completion only after retry.
- Cover empty, repeated, and terminal drains; new input during an incomplete drain and after a completed drain; explicit reinitialize cancellation; unchanged lifetime counters and prior overflow accounting.
- Exercise a physically wrapped queue, short/zero/alternating writes, the two-attempt ceiling, writer over-report, missing/unavailable writer, every state gate, and control-thread recovery. Check that errors and gates preserve all unaccepted queue frames and that recovery can later submit them.
- Use allocator instrumentation to prove zero allocations and deallocations on first, repeated, blocked, and empty/terminal success paths after initialization has prepared storage.
- Separately design and verify zero-output `DawHost` draining plus a scheduled engine retry path. Whole-chain EOF parity remains open until that route is tested without busy waiting and while retaining data under backpressure.
