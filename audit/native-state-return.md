# AUD-057: native GUI state response ownership and realtime correction

Date: 2026-09-28. Scoped to vendored NIH CLAP/VST3 response delivery plus NIH tests. AUD-053 scalar getters are separately recorded in `/tmp/sotf-native-scalar-allocation-inventory.md`.

## Confirmed defect

The existing queued GUI-state lifecycle regression intermittently aborted in an isolated process. GDB captured the precise audio-thread path:

`clap/wrapper.rs:2322` → `crossbeam_channel::Sender::send` → zero-capacity `Channel::send` → thread-local `Context::new` → 48-byte `Arc<Context::Inner>` allocation.

Evidence: `/tmp/sotf-native-tail-gui-repeat.log`, `/tmp/sotf-native-tail-gui-backtrace-deep.log` (frames20–34). The GUI may not have entered `recv()` after its request rendezvous completes. Returning the state through that same zero-capacity channel then waits for the GUI and initializes the audio thread's wait context. VST3 had the same response-send pattern.

## Implemented scope

`crates/3rdparties/nih-plug/src/wrapper/gui_state_return.rs` owns a prepared `ArrayQueue<T>` of capacity1 and a control-only exchange mutex. Both native wrappers:

1. Serialize public GUI state exchanges before submitting a request.
2. Retain the existing zero-capacity request and `send_timeout` behavior.
3. Apply an accepted state at the existing callback-end restoration point.
4. Move that state into the prepared response queue from audio, without blocking, allocating, dropping the state, or taking the GUI exchange mutex.
5. Poll/pop the response on the GUI thread with1ms sleeps, destroy it there, and release the exchange mutex before host notifications.

A Crossbeam `bounded(1)` reply was considered but rejected during source review: its `try_send` still takes a receiver-waker mutex when a GUI receiver is registered. `ArrayQueue` avoids that response wake path. GUI polling adds up to one polling interval plus scheduler delay to control acknowledgement. It does not delay audio publication.

The optional scalar tail cache and earlier two automation/transport corrections remain intact. The original ISC license and commit provenance are unchanged. Vendor README documents this separate behavioral patch. The NIH test crate adds only dev dependencies on already-resolved Crossbeam and parking_lot; unrelated Cargo.lock changes were preserved.

## Ownership and lifecycle proof

- The GUI exchange mutex is held from before request submission until response consumption/destruction. Only one request can be accepted at a time. The previous response was popped before the next exchange begins, so the one-element response queue is empty at every valid audio publication.
- An accepted request has **no response timeout or cancellation path**. Its synchronous GUI caller retains the wrapper/context and mutex until the response arrives. A slow GUI therefore leaves one owned response in the queue; it cannot submit another request until that response is consumed.
- An unaccepted `send_timeout` returns ownership of the original request. The caller retries or applies it on the control thread after deactivation. There can be no response for that timed-out request because its rendezvous did not complete. A deterministic test verifies it cannot supply a stale response to the next accepted request.
- The queue is part of the same wrapper; unlike a channel pair it has no disconnection state. A live GUI context retains the wrapper during an outstanding request. Normal teardown therefore cannot destroy a queued response on audio. The actual VST3 test closes its view, releases GUI contexts and interface references, then terminates the component.
- Full queue is an internal programming error, not a normal backpressure path: the control serialization and accepted-request protocol exclude it. The helper fails on this invariant violation rather than blocking/retrying. It does not provide an API for abandoned accepted requests.
- The GUI mutex is released before host parameter-rescan notifications, so synchronous reentrant control calls cannot deadlock on the exchange that triggered the notification.

## Verification

- Full NIH library suite: **69 passed,0 failed,0 ignored**, `/tmp/sotf-native-state-return-full.log`.
- All-target NIH Clippy with warnings denied: passed, `/tmp/sotf-native-state-return-clippy.log`.
- Actual VST3 `IEditController::create_view`/`IPlugView::attached` captures a real `GuiContext`; four concurrent GUI state restores complete through allocation-guarded native process callbacks, before processing is stopped. Tail queries, reset, deactivate/reactivate and teardown remain checked: `/tmp/sotf-native-state-return-vst3.log`.
- Actual CLAP queued state restore still changes scheduling status and publishes the new bound in the same callback; synchronous host tail queries observe the updated cache.
- The exact private shared helper source is compiled into the normal NIH test crate (without creating a public vendor API). Deterministic cases prove: first reply on a fresh thread completes before the GUI receives; the owned state is destroyed on its originating control thread; eight concurrent control callers receive their own responses; unaccepted request timeout cannot create a stale later reply. Callback reply operations retain allocation/deallocation guards.
- `git diff --check` passed. No ignored tests or allocator exemptions were added.

## Limits

This correction proves the **reply path** is nonblocking and allocation-free. Existing request-side rendezvous synchronization, state parsing, plugin reinitialization, and the old `permit_alloc` scopes are unchanged. Arbitrary plugin state restoration is not newly promised to be realtime-safe. Tests ran on Linux; macOS and Windows native runs were not performed. The reply path has no native mutex initialization on audio because the only new mutex is used exclusively by control callers.
