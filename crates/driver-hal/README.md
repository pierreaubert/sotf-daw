# driver-hal

Shared memory interface for Swift HAL driver communication.

Rust side of the shared memory bridge used to exchange audio data with the macOS CoreAudio HAL driver. Communicates via memory-mapped file at `/tmp/sotf-{uid}/audio.shm`.

## Shared-memory contract

- Rust and Swift share a versioned C-layout header. Cross-process fields use
  matching acquire/release atomics, and tests pin the header size and offsets.
- The daemon owns geometry changes and raises the `configuring` gate while ring
  positions are reset. Swift re-checks that gate before publishing a position.
- Ring capacity is derived from the current header geometry, bounded by the
  mapped capacity.
- Encrypted IO uses ChaCha20-Poly1305 records and staging buffers preallocated
  for the maximum HAL geometry. Real-time entry points do not grow them.
- The daemon owns session-key rotation; shared-memory open/reinitialization
  never changes key files independently.

## Reader transactions and staged plaintext

`HalInputReader` holds the existing read-commit bit while it snapshots the
format, delivers cached plaintext, and consumes encrypted records. The guard's
consuming methods require exclusive mutable access; drop releases only its read
bit, preserving any writer or reconfiguration request. Reconfiguration can wait
for the whole bounded read, including its decryptions.

Cached suffixes carry their sample rate, channel count, buffer geometry,
encryption mode, and public key fingerprint. A changed identity or a key reload
attempt discards them, including a failed reload. Storage retains its prepared
capacity. Positive reads preserve destination samples after the returned complete
frames; callers own that suffix. Zero-frame reads may clear the destination.

This format identity is not a session generation. Protocol v6 cannot identify
a same-format restart or an A→B→A transition entirely between reads. Key/mode
stores are not coordinated by the geometry guard; the final identity check is
not an atomic key-rotation guarantee. Format queries and reads are separate,
and replacing an orphaned mapping remains a control-owner recovery task.

Portable staging tests pass. The macOS library and test targets compile and
pass Clippy for x86_64-apple-darwin; native reader/reconfiguration tests and
device deadline measurements still require execution on macOS.

This crate is the macOS HAL bridge. Other platforms use `driver-common` and its
`NullDriver` fallback until native drivers are implemented.
