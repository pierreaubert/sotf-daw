# HAL staging implementation and remaining recovery work

Date: 2026-09-28. Repository: `/home/pierre/src/all_of_sotf/sotf-daw`.
The staging/guard patch was applied after independent review and macOS target compile/Clippy checks. Eight production helper tests pass on Linux. No wire/header/layout/version or sibling `sotf-systemwide` change was made. MIDI/IAMF excluded. Native macOS runtime execution remains pending; the orphan-recovery section is still a proposal.

## Artifacts and validation status

- `/tmp/sotf-hal-staging-proposed.patch`: concrete unified diff against current checked-out HAL source. **Applied on 2026-09-28.**
- `/tmp/sotf-hal-staging-proposal/crates/driver-hal/src/reader_state.rs`: proposed production helper, also compiled directly with `rustc --edition 2024 --test`; **8/8 tests pass**.
- `/tmp/sotf-hal-staging-proposal/`: formatted source artifacts for the proposed changes. **The library and all test targets cross-check successfully for `x86_64-apple-darwin`; macOS runtime tests are unexecuted.**
- `/tmp/sotf-hal-staging-helper-tests.log`: portable test output.
- `/tmp/make-hal-staging-proposal.py`: proposal generator; reads repository files and writes only `/tmp`. It does not apply changes.
- `/tmp/sotf-hal-staging-reader-tests.rs`: six actual macOS reader/guard regressions appended by the generator, with real mmap and AEAD operations.
- `/tmp/sotf-hal-staging-tests-crosscheck/`: isolated package used for `cargo check --offline -p driver-hal --tests --target x86_64-apple-darwin` and the equivalent Clippy command with `-- -D warnings`; **both pass**. Logs: `/tmp/sotf-hal-staging-macos-tests-check.log` and `/tmp/sotf-hal-staging-macos-tests-clippy.log`.

The cross-check uses the real `sotf-plugins` dependency for existing passthrough integration tests, with its unrelated IAMF default feature disabled. No HAL or plugin implementation is mocked. Cross-compilation checks types and Clippy; it does not link or execute a macOS test binary or establish runtime/concurrency behavior. The parent independently cross-checked baseline and proposed libraries in `/tmp/sotf-hal-crosscheck`; `/tmp/sotf-hal-crosscheck-proposal.log` records that earlier check. The portable tests exercise the same proposed helper source used by production macOS builds.

## 1. Concrete failure and proposed ownership

Before this change, the adapter copied `pending_decrypted_samples` before claiming any shared-memory read bit. It interprets those samples using the channel count at the new call. Reading one stereo frame from an eight-sample record leaves six cached samples; changing the transport to three channels makes the next call emit two frames assembled from the old stereo record. `reload_cipher()` also retains old authenticated plaintext across successful and failed key reloads. This is source-level evidence; the macOS backend reproduction has not been executed here.

The proposed std-only private `StagedPlaintext` owns the sample vector, consumed offset, and identity together. Its setup constructor reserves capacity. Stage, copy, observe, and invalidate never grow it; an oversized or non-frame-aligned suffix is rejected and invalidates any previous suffix. A positive read only writes returned complete frames, preserving the caller-owned destination tail as required by `driver-common` (this also corrects the documented encrypted-partial-read conformance gap).

`reader_state` is compiled under `cfg(any(target_os = "macos", test))`. Other HAL modules remain macOS-only. A Linux helper test does not claim that Linux implements the HAL backend.

### Exact identity

Every staged suffix is tagged with:

| Field | Meaning |
| --- | --- |
| `sample_rate: u32` | Header stream rate |
| `channel_count: u32` | Header interleaved frame width |
| `buffer_frames: u32` | Header transport geometry |
| `encrypted: bool` | Encrypted versus plaintext framing |
| `key_fingerprint: [u8; 8]` | Header's existing public fingerprint, matched to the cached cipher |

Zero rate/channel/frame counts are invalid. This identity is a compatibility check, **not a session epoch**. Neither the fingerprint nor frame counter is a reliable transport-generation identifier. The counter advances per record and must not cause suffix invalidation on every write.

Mapping identity is implicit in the adapter's ownership: a reader owns one immutable mapping, and reconnect constructs a new reader with empty staging. A future method that replaces `buffer` in place must explicitly invalidate staging; this draft does not add such a method.

### Invalidation paths in the draft

| Event | Behavior |
| --- | --- |
| `read()` sees a changed identity | Clear staged length, offset, identity; retain vector capacity |
| `read()` sees plaintext mode | Clear all encrypted leftovers, use guarded plaintext read |
| Reconfiguration/read-commit claim unavailable | Clear leftovers; zero destination and return 0 |
| Cipher absent or fingerprint mismatch | Clear leftovers, increment existing mismatch counter, zero destination and return 0 |
| `reload_cipher()` called | Invalidate before disk I/O; remove cached cipher first; success installs new matching cipher, any failure leaves it unavailable |
| No mapping | Clear leftovers, zero destination and return 0 |
| New reader/reconnect | Empty staging by construction; explicit owner reset required if same instance is reused |
| Invalid/corrupt encrypted record | Clear leftovers; already-copied, independently authenticated prefix may be returned if identity still matches |
| Scratch/suffix capacity failure | Clear leftovers, zero destination and return 0; do not allocate |
| Final identity differs from entry | Clear leftovers, zero destination and return 0; already-consumed ring records are discarded |

`available_read_frames()` takes a guarded identity snapshot, rejects a missing/mismatched cipher, and counts staging only for that exact identity. It is an advisory query and does not mutate staging. It cannot latch a transient identity change that appears only in a query and returns to its old value before `read()`; see the generation limit below.

## 2. Existing read-commit guard integration

`configuring` bit 0 requests reconfiguration, bit 1 is READ_COMMIT, bit 2 is WRITE_COMMIT. The draft retains these bits and existing atomic orderings.

1. `SharedAudioBuffer::try_read_commit()` first checks reconfiguration (setting the existing compatibility acknowledgement on that path), then calls the existing CAS claim for bit 1. It returns an RAII `ReadCommit` only after a successful claim.
2. `HalInputReader::read()` acquires the guard **before** sampling identity or choosing encrypted/plaintext framing.
3. While guarded: take identity; validate cipher; copy pending plaintext; read/decrypt/publish whole records; store leftover suffix; recheck identity.
4. `ReadCommit` delegates directly to `read_audio_under_commit` and `read_next_encrypted_record_under_commit`. It never calls the public wrappers that would recursively claim the same bit. Consuming and cursor-repair methods require `&mut self`; the staged helper takes `&mut ReadCommit`. Read/availability callers hold mutable local guards. Identity observation alone retains `&self`.
5. Drop clears only READ_COMMIT using the existing `fetch_and(!bit, Release)`, preserving a reconfiguration request and any writer bit. Early error/empty/invalid-key returns all drop the guard. Ordinary unwind also releases it; process abort/crash is not recoverable by RAII.
6. A reconfiguration can set bit 0 during the batch but waits for bit 1 before changing geometry. The current batch may finish under the old geometry; the subsequent batch sees the new geometry. This is a bounded buffer transaction, not cancellation at the instant of request.

The guarded availability scan is included because the existing scan reads record headers/geometry without a commit guard and attempts repair through `commit_read_position()`. Calling that under an outer guard would reject its own nested claim. The draft factors `available_read_frames_under_commit()` and its encrypted scan, then publishes their repair cursor directly while already guarded. Public availability queries also use the guard. This preserves the old malformed-record/overrun flush policy; it does not add a new recovery algorithm.

The old adapter-only per-record guarded wrapper becomes unused and is removed; the new RAII method calls the existing private record implementation. Existing convenience encrypted/plaintext readers retain their interfaces.

### Independent guard review correction (2026-09-27)

The initial draft exposed cursor-consuming and repair operations through
`&ReadCommit`. Because the guard contains only `&SharedAudioBuffer`, and its mmap
owner is `Sync`, the guard was also automatically `Sync`. A single successfully
claimed read bit could therefore be shared between scoped threads and used for
concurrent under-commit reads or repair scans. One reader could publish a cursor
and let the producer recycle slots while the other still copied those slots,
violating the single-reader raw-copy invariant. Current adapter calls were
sequential, but the safe guard API did not enforce that requirement.

The isolated proposal now requires `&mut self` for `read_audio`,
`read_next_encrypted_record_into`, and `available_read_frames` (which can repair
the cursor). The helper and all local callers pass an exclusive mutable guard;
`identity` remains a shared observation. This makes simultaneous cursor use
through a shared guard impossible in safe Rust while retaining normal guard
movement and RAII release. The generator, formatted artifacts, and unified patch
were refreshed together. No additional actionable integration fault was found
in the independent review beyond the separately documented protocol limits.
`rustfmt` parses the revised artifacts and `git apply --check` validates the
unapplied patch. The subsequent macOS test-target cross-check and Clippy pass;
macOS runtime integration still requires the listed platform gate.

### Remaining race/latency limits

- Header v6 has no persistent generation. Same-format ring flush, same-key restart, or A→B→A change entirely between read transactions is invisible unless the owner explicitly resets/recreates the reader. This patch must not be represented as a complete session-boundary solution. A later cross-language epoch is a separate protocol change.
- Geometry setters cooperate with the commit bits. `set_encrypted` and `set_key_fingerprint` are currently independent atomic stores and do not take the geometry guard. The final recheck detects a changed endpoint identity, but cannot rule out intermediate ABA changes or a transition after the final load. The draft does **not** claim an atomic key-rotation transaction. Such a guarantee requires the key/mode publisher to use a coordinated quiescence protocol.
- Holding READ_COMMIT for a full requested read (including several decryptions) can extend the reconfiguration wait compared with the old per-record guard. Work is bounded by the output request and protocol limits, but this is not a measured macOS deadline guarantee. Existing timeout behavior must remain unchanged and must be exercised on macOS.
- Existing lower-level malformed/corrupt-record paths log warnings. The helper is allocation-free by construction and reuses its storage; this proposal does not prove all error-path logging is real-time safe.
- `available_read_frames()`, the format getters, and `read()` remain separate calls. A caller can fetch a format, then read after reconfiguration and label the result using the earlier format. A read result carrying its guarded format, or an expected-format read API that rejects a mismatch, is needed to close that separate consumer race. The draft does not claim to fix it.
- Clearing staging when a reconfiguration attempt is observed may discard a valid suffix even if reconfiguration later times out. This is the proposed conservative discontinuity policy; it avoids replaying old-format data after a possibly completed transition.
- Explicit key reload discards the cached cipher even if loading fails. This favors silence over resuming cached state across an intended boundary; control-thread retry already exists in the engine.
- Caller-visible positive-read tail preservation is intentional. Existing tests that expected zero-filled trailing samples on a positive result should be updated to the documented driver contract, not silently weakened.

## 3. Portable evidence and required macOS acceptance

Executed helper tests (8): exact bit-preserving resume/canary tail; stereo→three-channel suffix rejection; every identity field; explicit owner invalidation under unchanged identity; invalid geometry/plain/misaligned suffix rejection; capacity failure; 1,000 reserve-reuse cycles; sub-frame read preservation. Pointer/capacity invariance is checked. These checks are not a whole-reader allocation-counter proof.

### Actual reader regressions added (2026-09-28)

These six tests are in the proposed `shared_memory/tests/misc.rs`; they use the actual macOS reader, encrypted records and mapped header. All six compile and pass Clippy for the macOS target; none has been executed on macOS here.

| Test suffix (`hal_reader_…`) | Concrete assertion |
| --- | --- |
| `staging_rejects_old_channels_after_quiesced_change` | Read one frame from a four-frame stereo encrypted record; retain three staged frames; quiesce from 48 kHz / 512 frames / two channels to 96 kHz / 256 frames / three channels; read three new uniquely marked frames. Availability excludes old staging, returned samples contain only the new marker, the destination suffix stays untouched, and final ring cursors agree. |
| `positive_reads_preserve_caller_suffix_in_both_modes` | In plaintext and encrypted modes, read one frame into three samples, then four frames into eleven samples. Every unwritten sample retains its sentinel. Encrypted mode crosses from a pending suffix to another real record; signed-zero sample bits survive. |
| `matching_key_reload_discards_staged_plaintext` | Stage one old frame, call the public `reload_cipher()` with the same valid key, observe zero availability, then receive only a newly written marker and preserve the caller suffix. |
| `failed_key_reload_discards_staged_plaintext` | Start with a matching cached key and staged frame; public reload from an isolated missing path returns `NotFound`, immediately clears staging and cached cipher, and subsequent reads return zero/silence without leaking the read bit. The immediate staging assertion occurs before any read can hide a missed reload invalidation. |
| `commit_drop_preserves_other_bits_and_geometry` | Claim the real READ_COMMIT guard, publish reconfiguration and writer bits, then release normally and by panic unwinding. Both paths preserve the request/writer bits, geometry and cursors; a subsequent claim is blocked and acknowledges the request. |
| `commit_prevents_geometry_change_until_release` | Hold a real reader guard through a controller reconfiguration attempt. Its timeout retains original geometry/cursors and the read bit. Release and retry; the new geometry installs and cursors reset. |

Reload tests launch the exact same test in a child process using `Command::env("SOTF_HAL_SESSION_KEY_PATH", temporary_path)`. The success fixture creates a private 0600 key inside a temporary directory; the failure fixture deliberately leaves that temporary path absent. No global environment mutation, real user key path, daemon file or default key loader path is used. The parent asserts one child test actually passed, preventing an incorrect exact-test selector from silently passing with zero tests.

The former standalone `commit_read_position` helper now has only existing test callers because production repair is performed inside the guard. The proposal marks it `#[cfg(test)]`, preserving those regressions without leaving dead production code. No repository file was changed.

Before integration is accepted:

1. macOS compile, focused driver-hal tests and Clippy with warnings denied.
2. Real encrypted record → partial read → quiesced rate/channel/frame transition → new uniquely marked record. Assert no old marker is returned, available counts use the new format, and cursor arithmetic stays valid.
3. Encryption/plain switches, matching-key reload, new-key reload, missing/mismatched key reload, and explicit new reader on the same format. Both reload success and failure must discard old staging.
4. Hold/contend the real read bit while requesting reconfiguration. Assert geometry remains old until guard release, request bit survives Drop, timeout leaves geometry intact, all early returns release the bit, and no nested acquisition suppresses a legitimate read/repair.
5. Availability overrun/corrupt-header paths exercise direct guarded repair and do not move a cursor after a completed reconfiguration.
6. Short positive reads preserve sentinel suffixes; zero-frame returns may zero the destination. Use `driver-common` conformance checks against the actual adapter.
7. Install the real allocation/deallocation counter and measure warmed and cold-thread reads, pending-only reads, maximum record sizes, failure paths, and repeated transitions. Audit logging separately if counters fire.
8. Run true producer/consumer process tests for the coordinated geometry transition. Tests must state that uncoordinated key/mode stores and missing epochs remain outside the proven guarantee.

## 4. Orphaned inode reconnect — separate, not in staging patch

### Verified source behavior

- `SharedAudioBuffer::backing_file_is_current()` already compares the regular path entry's `st_dev`/`st_ino` to the mapping's stored values using `symlink_metadata`. Missing path, symlink, wrong inode/device return false.
- `HalDriver::status()` uses that check and can correctly report stale mapping readiness as false.
- `HalInputReader::is_connected()` checks only `driver_ready` in its mapped header. It does no filesystem I/O, which is appropriate for an audio-path status check.
- `DecoderState::try_reconnect_hal_reader(force)` returns early for that stale ready bit **before** checking `force` or its retry timer. An orphan with ready=1 is never reopened.
- `run_engine_heartbeat()` reopens only when its mapping is None, so it keeps updating an old inode.
- `HalDriver::ensure_config_buffer()` also reopens only when None. Refreshing just this handle would make control/heartbeat healthy while audio still uses an orphan.

Concrete reproduction to execute on macOS: map file A and leave A.driver_ready=1; atomically replace the path with valid file B; enqueue distinct samples only in B. Status sees A stale, but decoder polls A forever and heartbeat updates A. This is an inspected scenario, not executed backend evidence.

### Proposed control-thread recovery changes

1. Add an explicitly non-real-time `HalInputReader::backing_file_is_current()` query delegating to the existing mapping identity check. Keep `is_connected()` and `read()` filesystem-free.
2. In the decoder's existing control loop, move the reconnect interval check before a filesystem health check. At each permitted poll, keep a reader only if it is ready **and** its path identity is current. `force` must bypass the interval/ready short-circuit. On stale identity drop the old reader (and its staging) before opening the new path. Failure leaves no active reader and retries at the normal interval.
3. A successful replacement is a stream discontinuity even if rate/channels are equal: reset/recreate the HAL resampler, clear its buffered output and cached rate/channel fields, and discard the old HAL input/send scratch contents. A downstream `Flush` or equivalent generation boundary is needed if queued old pipeline frames must not play after recovery. The exact owner/caller handshake must be chosen before implementation; replacing only the reader does not prove end-to-end freshness.
4. On the heartbeat thread, check `backing_file_is_current()` each existing timer tick; discard a stale mapping, reopen an **existing** path via `SharedAudioBuffer::open`, then refresh that mapping. Never create files, reset ring/nonce counters, rotate keys, or claim a new daemon session just to reconnect. Do not call `initialize()`/`reset_new_daemon_mapping()` for this recovery.
5. For `HalDriver`, add an explicit non-real-time refresh operation on the owning control path. Prepare replacement config and reader handles together and require the same `(device,inode)` for both, then install them under exclusive `&mut self`. Verify the path still names that pair before publishing them; reject mismatched/open failures. `status(&self)` can report the problem but cannot perform this replacement. The existing `ensure_config_buffer()` must not refresh only config while leaving the reader stale.
6. Missing path, symlink replacement, wrong owner, malformed header, or permission failure leaves the transport disconnected. Use the existing secure open/owner/geometry validation. Do not open/create a fallback file or continue reporting orphan readiness.

### Exact mapping identity and concurrency limits

The mapping identity is `(st_dev: u64, st_ino: u64)`, captured from the opened descriptor and checked against a regular path entry; this is distinct from the staging's format/key identity. Neither mtime nor byte length is an inode identity. The old mapping keeps its inode alive, so normal unlink/recreate cannot reuse that live inode unnoticed. Same-inode truncation/content rewrite is not detected by this check and remains prohibited while mapped; a persistent generation/owner protocol is needed for same-inode session changes.

A path can be replaced again immediately after any filesystem check. Recovery provides eventual detection at the bounded health poll interval, not an atomic guarantee that a path continues to name the selected inode for all future reads. Opening config/reader separately also needs the same-identity check above. No filesystem access belongs inside the real-time callback.

Decoder and `HalDriver` have different owners. The decoder can run its non-RT check in the existing worker; `HalDriver` needs its actual control owner to call refresh. That may ultimately require an `AudioDriver` lifecycle hook and a caller change outside this workspace. Those ownership changes are **not** included or presumed authorized in this staging diff. `sotf-systemwide` is not edited.

### Portable and platform verification

A separate std/Unix file-identity helper can share production equality/path-validation logic with Linux tests using real files: retain A's descriptor, atomically rename B onto its path, assert different `(dev,ino)`; test unlink, missing path, symlink, recreation, unchanged handle/path, and failed opens. A pure recovery-decision test can assert forced checks and retry timing. These tests prove filesystem/state decisions only.

macOS acceptance must use two real mapped transports and marker audio: after recovery only B is consumed/heartbeated; A's ready bit may remain 1; all config/reader handles name B; old staged plaintext and resampler history are absent; a failed replacement is silent/disconnected; recovery does not reset a live mapping's nonce/cursors or modify session ownership. Cross-process reconfiguration/key behavior remains a separate gate.

Production helper test log: `/tmp/sotf-hal-staging-portable.log` (8 passed).
