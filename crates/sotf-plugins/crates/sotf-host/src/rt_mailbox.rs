//! Lock-free bounded mailbox for realtime control handoff.
//!
//! Opt-in helper for plugins that prepare resources on control or worker
//! threads and consume them on audio (or return retired resources the other
//! way). Backed by the existing `rtrb` dependency, so no new manifest edge is
//! needed. Control allocates the ring once at construction; audio
//! `push`/`pop` calls are lock-free, bounded, and allocation-free.
//!
//! Producers and consumers are single-owner (`Send`, not `Sync`). Share a
//! control-side endpoint through `Mutex` (control may `try_lock`; audio never
//! locks) and keep the audio-side endpoint behind the plugin `&mut`.

// Rust guideline compliant 2026-02-21

/// Single-consumer endpoint of a realtime mailbox.
///
/// `Send` but not `Sync`; keep behind plugin `&mut` on audio.
pub use rtrb::Consumer as RtConsumer;
/// Single-producer endpoint of a realtime mailbox.
///
/// `Send` but not `Sync`; wrap in `Mutex` for shared control handles.
pub use rtrb::Producer as RtProducer;
/// Full-mailbox error carrying the refused value intact.
///
/// Match `RtPushError::Full(value)` to recover ownership without dropping:
/// audio stashes back to a holding slot, control drops off audio.
pub use rtrb::PushError as RtPushError;

/// Build a bounded single-producer/single-consumer mailbox.
///
/// Allocates the ring; call on control at construction. Audio `push`/`pop`
/// calls are lock-free and allocation-free. A full `push` returns
/// `RtPushError::Full(value)` with the value intact for retention or
/// off-audio handling; an empty `pop` reports empty with no wait.
///
/// # Panics
///
/// Panics when `capacity` is zero (`rtrb` requires a nonzero ring).
pub fn rt_mailbox<T>(capacity: usize) -> (RtProducer<T>, RtConsumer<T>) {
    rtrb::RingBuffer::new(capacity)
}
