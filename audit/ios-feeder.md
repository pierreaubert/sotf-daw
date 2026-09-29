# RemoteIO feeder accuracy and control ordering (AUD-066)

Status: implementation and portable regressions complete, 2026-09-28. Native
AudioUnit/device execution is not available on this Linux host. MIDI and IAMF
are excluded.

The existing RemoteIO feeder loop is extracted from `audio_unit_handle.rs` into
`feeder.rs`. The actual loop, command handle, ring helpers and render callback
are compiled for portable library tests. AudioUnit construction remains iOS-only.
The broader acknowledged manager/output transition proposal was removed from
production after automatic approval review; its artifacts remain separate under
`/tmp/sotf-engine-transition-review`.

## Corrected behavior

- A FIFO Flush initiates an asynchronous callback drain. Following fresh audio
  stays pending until the flush completes, so the callback cannot erase it.
- Empty-ring flush completion also handles a backend that stops requesting
  buffers. An atomic callback-active guard avoids treating an in-flight callback
  containing old audio as quiescent for this check.
- Stream-flush and pause state remain independent: Resume cannot end Stop's
  drop-until-FIFO-Flush requirement, and stream Flush cannot resume a pause.
- Disconnected-input EOS returns to the command loop between drain waits;
  Shutdown no longer waits for the entire two-second audio drain timeout.
- Source rate, channel count and checked frame dimensions are validated before
  publication. Samples are never relabeled as a different hardware format.
- Frames larger than the ring are written in bounded pieces. Ownership and a
  sample offset remain pending between iterations; each piece contains whole
  interleaved frames. Odd ring capacity therefore cannot shift channel order.
  Stop/Pause/Shutdown recycle the retained frame and discard its unplayed suffix.

Normal ring writes and the callback require no new dynamic storage. The feeder
already has non-realtime error reporting and buffer-recycle fallback paths;
zero-allocation claims here apply to the explicitly tested callback paths.

## Evidence

The new tests call the actual feeder on a worker thread, use its actual bounded
ring and invoke the production callback with the same interleaved AudioBufferList
ABI as RemoteIO. Recycle replies, command replies, atomics and visible ring counts
provide barriers; no physical device is opened and no protocol implementation is
copied into the tests.

Initial ordinary matrix: eight tests passed. The added oversized-frame tests then
failed on the original whole-block writer because it never published any samples:
`/tmp/sotf-ios-oversized-red.log`.

After the partial-write fix, **10 tests pass, zero ignored**, in
`/tmp/sotf-ios-feeder-green.log`:

- fresh post-Flush audio, Stop/Resume versus FIFO Flush, and Pause/Resume with
  both callback completion orderings;
- disconnected EOS and saturated/oversized-ring command responsiveness;
- unsupported source rate/channels, malformed public dimensions and multiplication
  overflow, plus unchanged/cancelled/unsupported reconfiguration replies;
- exact 37-frame asymmetric stereo sequence through capacities 16 and 17 and
  variable callback sizes, with EOS only after the final samples;
- Stop after an exact played prefix, suppression of old EOS and fresh stereo
  sentinels after the next FIFO Flush;
- cold actual callbacks for 1/2/8 channels, covering audio, flush and underrun:
  **zero allocations and zero frees**, measured by explicit TLS counters that
  also preserve the engine's existing CountingAlloc hooks.

Engine library/test Clippy also passes with warnings denied:
`/tmp/sotf-ios-feeder-clippy.log`.

Commands:

```sh
cargo test --offline -p sotf-engine --no-default-features --lib playback_thread_stub::feeder::tests
cargo clippy --offline -p sotf-engine --no-default-features --lib --tests -- -D warnings
```

These checks do not establish the pending manager-wide Stop/Play or output-format
barriers, physical hardware drain completion, or native AudioUnit deadlines.
