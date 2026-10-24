# driver-hal

Shared memory interface for Swift HAL driver communication.

Rust side of the shared memory bridge used to exchange audio data with the macOS CoreAudio HAL driver. `/tmp/sotf-{uid}/audio.shm` is a 32-byte rendezvous record. Audio lives in an allocated, unlinked Darwin POSIX shared-memory object whose extent cannot be changed after allocation. An authenticated Unix socket transfers its descriptor; production attachment refuses ordinary files.

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
encryption mode, public key fingerprint, and ring generation. A changed identity or a key reload
attempt discards them, including a failed reload. Storage retains its prepared
capacity. Positive reads preserve destination samples after the returned complete
frames; callers own that suffix. Zero-frame reads may clear the destination.

Every geometry reset, queue flush, and encryption transition advances the
ring generation under the IO exclusion gate. This also invalidates staged
plaintext after a same-format reset or A→B→A transition between reads. The
daemon uses checked `configure_encryption` publication for mode, fingerprint,
and optional flush as one transaction. Identity exhaustion requires transport
replacement rather than wrapping.

The daemon publishes a new immutable object and rendezvous on restart. Each
broker owns a random endpoint and unlinks only that socket. Existing mmap leases
keep old pages alive after the marker is replaced or truncated. The surviving
HAL restores cached volume before publishing readiness unless a volume writer
has already initialized the new tuple. Native tests cover descriptor validation,
truncation, replacement, encrypted staging and both shipping languages.
Installed-HAL sandbox/UID and device deadline qualification remain separate.

This crate is the macOS HAL bridge. Other platforms use `driver-common` and its
`NullDriver` fallback until native drivers are implemented.

### Sequence worker pacing diagnostics

The test-only `hal-capture-stress` consumer preserves a nominal 100-microsecond
clock across scheduler oversleep. It retains fragmented reads, 32 ring lengths
and the normal 15-second liveness bound. Read/verification/wait times and
partial verified progress are emitted on failure as well as success. Debug and
release host fixtures remain separate from native device/deadline acceptance.
