# Engine and driver read-only audit

Date: 2026-09-27. Scope: engine chain integration, driver-common, driver-hal.
MIDI, IAMF, and sibling repositories excluded. Existing AUDIT.md through AUD-034 was reviewed first.

## Evidence status

This pass inspected live source and existing test bodies, and consulted the primary references below. It did not execute Cargo, audio measurements, or macOS driver tests. Numerical examples below are deductions from the inspected code, not measured results. Parent subsequently reports the separate aggregate checkpoint: 5,272 tests passed, 11 skipped, release QA 8/8. Those results do not establish coverage of the new cases identified here.

Priority implementation candidates are the offline output clock (now assigned AUD-037), variable-block crossfade clock (AUD-038), and HAL pending-plaintext epoch handling. No source edits were made during this audit.

## 1. Offline renderer uses the pre-chain clock for rate-changing output — AUD-037

**Source:** crates/sotf-engine/src/offline_renderer/render.rs:48-87,183 onward,258 onward; engine/processing_thread/build.rs:46 onward.

The renderer builds a configurable DawHost at output_rate, but constructs the WAV header with that input rate without consulting host.output_sample_rate(output_rate). The writer receives the actual terminal-domain frames from host.process. Its final programme-length target and progress total also use the pre-host clock.

**Concrete consequence:** a valid 48→96 kHz Resampler inside config.plugins emits 96 kHz-domain samples into a WAV marked 48 kHz. Pitch is approximately halved and programme duration approximately doubled. The ordinary process/write path is not capped to the final pre-host programme-frame target, so later drain cannot repair already-written excess frames.

**Existing coverage:** offline_renderer/tests.rs exercises source→requested-rate conversion outside the configurable chain and convolution latency at an unchanged rate. Neither establishes the clock of a rate-changing plugin chain.

**Bounded correction:** choose and document the output-rate policy. Either convert the terminal clock to the explicitly requested writer clock, or label and size output using the actual terminal clock. Align header, duration, progress, latency trimming, and final programme-frame target to that decision.

**Independent oracle:** known-frequency tone plus final impulse, parse the actual WAV header, count frames, estimate frequency using ordinary arithmetic/DFT, compare duration at 48↔96 and 44.1↔48 kHz across irregular callback sizes. Validate a source converter plus a chain converter together.

**Primary criterion:** WAVEFORMATEX defines nSamplesPerSec as the sample rate and nBlockAlign as the atomic complete-frame unit: https://learn.microsoft.com/en-us/previous-versions/dd757713(v=vs.85).

## 2. Crossfade duration depends on the first callback size — AUD-038

**Source:** engine/processing_thread/processing_state.rs:153-169,449-500.

compute_crossfade_step derives a callback-sized fraction of the intended 50 ms transition. process_frame calculates this only when crossfade_step is zero and reuses it for subsequent callbacks regardless of their input_frames. A zero-frame first call returns a full transition step. Per-block interpolation includes both endpoints, creating duplicated phase positions at block boundaries.

**Concrete consequence:** at 48 kHz, first callback 128 frames gives a step of 128/2400; subsequent one-frame callbacks advance by the same fraction. The transition finishes after about 146 input frames rather than 2400. Conversely small-first/large-later callbacks stretch it.

**Existing coverage:** zero-size helper behavior, equal-power coefficient identity, and fixed-size DC rate-change continuity. None independently measures elapsed transition time under variable partitions.

**Bounded correction:** track accepted input time or an equivalent sample-based transition cursor; derive each emitted sample's phase from that cursor and current output clock, preserving progress over varying callback sizes and rate changes. Zero-frame processing must not advance musical time. Reset/start of a new transition must restart the cursor.

**Independent oracle:** exact 50 ms transition boundaries and sampled gain trajectory for 1/127/128/513/irregular callback partitions at 44.1/48/96 kHz. Test terminal rate changes separately because emitted frames differ from accepted input frames.

**Related evidence gaps:** rate-changing old-path mapping is callback-local linear interpolation and is tested only with DC, which cannot reveal frequency/phase artifacts. Equal-power mixing of two coherent identical paths produces a +3.01 dB midpoint; this appears an explicit design choice, not a demonstrated defect.

## 3. HAL decrypted staging is not invalidated by transport/geometry/session changes

**Source:** driver-hal/src/shared_memory/types.rs:23-129; hal_input_reader.rs:81-100,125-165; shared_audio_buffer.rs reconfigure_quiesced and read_next_encrypted_record_into.

A reader caches the unused suffix of a successfully decrypted record. The staging helper reads the current channel count, then copies that pending suffix before accessing the shared-memory record reader/reconfiguration gate. reload_cipher replaces the cipher but does not clear staged plaintext. The header has no persistent epoch that can distinguish a same-format flush from uninterrupted transport.

**Concrete source-level reproduction:** read one stereo frame from an eight-sample encrypted record, leaving six staged samples. Reconfigure to three channels and clear the ring. The next six-sample destination receives two three-channel frames assembled from old stereo samples even with no new ring record. A key reload can likewise release old-session staged audio after the new cipher becomes valid. A same-format flush can replay pre-flush staging.

**Impact:** stale audio, channel regrouping, and a reader-side bypass of the ring's otherwise explicit quiescence boundary.

**Correction direction:** coordinated control-thread reader recreation/reset, or a versioned transport generation checked before staged delivery. A generation wire change requires corresponding Swift producer coordination outside this repository; do not add an incompatible local header field. The reader must observe reconfiguration before copying any pending samples.

**Missing test:** partial-record staging followed by channel changes, same-format flush, encrypted/plain mode changes, and key replacement. Assert no stale sample marker crosses the transition and no allocation occurs on the read path. Existing low-level ring/quiescence tests do not cover reader-private staging. Requires macOS execution for this crate.

## 4. EOS drain does not propagate interruption outcomes

**Source:** engine/processing_thread/processing_state.rs:987-1090; handle_processing_command Shutdown arm near 694.

The drain send loop handles Shutdown with break from only the innermost pending-send loop. The surrounding drain loop and later EOS-send path continue. Stop/rebuild commands can also leave a pending frame from the previous stream, while output_channels and output_sample_rate were cached before draining began.

**Source-level consequence:** shutdown can continue waiting on a full downstream queue; rebuild can make drain metadata stale; old tail frames can survive a stop. A runtime deadlock was not executed in this audit.

**Correction direction:** propagate a typed stop/shutdown/reconfigured/disconnected outcome to the outer processing loop, cancel/recycle stale pending messages, and define the stream generation for any continued drain.

**Missing tests:** block downstream sends, inject Shutdown/Stop/host replacement during drain, assert bounded termination, no old-generation audio, and valid channel/rate metadata.

## 5. Global processing bypass still drains the active host

**Source:** processing_state.rs:395-420,638 onward,995-1009.

The normal global-bypass path copies input frames and reports the input clock. Enabling bypass retires a previous transition host but does not reset the active host. EOS directly calls state.host.output_sample_rate and state.host.drain, ignoring bypass.

**Consequence:** a previously primed delay/resampler can emit stale processed tail after a bypassed stream; the tail can carry a different sample rate from the bypassed frames.

**Missing test:** prime a stateful/rate-changing host, enable global bypass, process more input, send EOS, and assert the chosen bypass tail policy and consistent clock. This is separate from already-tracked host-node bypass behavior. Crossfade-at-EOS also needs an explicit old-host tail policy.

## 6. Orphaned mapping detection is not integrated with automatic reconnection

**Source:** driver-hal/src/driver/hal_driver.rs:55,196-205; driver/misc.rs:24-39; shared_memory/hal_input_reader.rs:125; engine/decoder_thread/decoder_state.rs:908-925.

HalDriver::status checks backing_file_is_current and detects an unlinked/replaced mmap. HalInputReader::is_connected only checks driver_ready in the old header. try_reconnect_hal_reader returns immediately on that value even when force is true. Heartbeat keeps its original Some(mapping) indefinitely; ensure_config_buffer likewise opens only when absent.

**Concrete source-level reproduction:** replace the backing path while the old header retains driver_ready=1. Status can report disconnected, while the decoder keeps polling an orphaned ring and the heartbeat updates the old mapping.

**Correction direction:** control-thread identity/generation polling and coordinated replacement of reader, config, and heartbeat handles. Do not add filesystem calls to the audio read callback.

**Existing coverage:** backing identity detection and disconnected status, not end-to-end reconnection followed by an identifiable fresh sample. Engine cipher reload exists and is called before read; this review does not claim otherwise.

## 7. Positive encrypted partial reads violate the common tail-preservation contract

**Source:** driver-common/src/lib.rs:379-423; driver-hal/src/shared_memory/types.rs:42-46,127; plaintext shared_audio_buffer.rs:1110 onward.

The conformance helper requires a positive return to modify exactly frames*channels samples and leave all trailing destination samples untouched. Encrypted staging zeros both incomplete-frame endings and the unread remainder even when it returns positive frames. Plaintext reads preserve the tail.

**Reproduction:** stereo destination of seven sentinel samples, only one encrypted frame available. Return value is one, but samples 2..7 are zeroed instead of preserved.

**Correction direction:** preserve the unwritten tail for positive returns, or deliberately revise the public contract and all callers. Preserve the documented zero-return silence option.

**Missing test:** populated encrypted and plaintext read conformance with positive partial reads, odd destination lengths, exact-bit payloads, and sentinels. The current HAL conformance fixture starts empty and does not demonstrate this path.

## 8. Fallback downmix infers unsupported speaker layouts from channel count

**Source:** engine/playback_thread/frame_writer.rs:95-162; engine/types/state.rs AudioFrame.

The fallback matrix assumes every multichannel layout except five channels has an LFE at index 3. It gives coefficients only through index 9. AudioFrame carries a channel count but no speaker-position mask.

**Concrete consequence:** conventional four-channel FL/FR/BL/BR input treats BL as centre and drops BR as assumed LFE. A 12-channel 7.1.4 input drops the final two channels. Valid four-channel layouts cannot be inferred from count alone.

**Primary criterion:** WAVEFORMATEXTENSIBLE explicitly associates channel positions with a channel mask and gives a four-channel FL/FR/BL/BR example: https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ksmedia/ns-ksmedia-waveformatextensible.

**Correction direction:** carry an explicit layout or reject unsupported fallback mappings. Add isolated-channel impulse matrix tests for 4/5/6/8/10/12-channel layouts. Allocation/finite-output tests for 6→2 and 10→2 are insufficient evidence.

## Remaining feature and realtime evidence gaps

- Engine analysis publication still uses ArcSwap (engine/types.rs PluginDataCache; processing_state.rs update_plugin_data_cache). Existing tests exercise custom fallback counters and ownership, but do not establish zero allocation/deallocation on a fresh processing thread. A cold-thread allocator/deallocator oracle is needed; this review did not measure a callback allocation.
- CPAL callbacks execute on a high-priority audio thread, while decoder/DSP workers deliberately use a softer realtime class. Keep those two guarantees explicit. Official pinned-version reference: https://docs.rs/cpal/0.17.1/cpal/.
- HAL malformed-record paths log while reading; add worst-case/corrupt-record callback cost and allocation evidence. The normal reader correctly caches cipher/scratch state and avoids routine filesystem access.
- driver-hal implementation/tests are macOS-gated. A Linux aggregate compiling zero HAL tests is not hardware/platform evidence. Validate separate processes, encrypted exact-bit transfer, concurrent reconfiguration, restarts, and session transitions on macOS.
- Embedded engine uses f32 and lacks an explicit bounded drain API. Treat higher precision and finite-stream tail export as feature gaps unless a stronger public guarantee is specified.
- Offline export intentionally targets programme duration; a selectable post-programme decay/tail export policy remains a feature gap. Do not describe all discarded reverb decay as a regression without selecting that policy.
- CPAL output selection supports a subset of the sample formats offered by the pinned dependency; unsupported formats return an error. Wider format support is a compatibility feature gap, not silent numerical corruption.
- HAL output geometry setters return true when a mapping exists although underlying void setters may reject invalid sizes/rates or fail to quiesce. Clarify whether true means mapping present or request accepted, then test invalid and timed-out requests.
- The HAL decoder target rate is immutable for the lifetime of its worker. Its resampler cache not comparing target_sample_rate is therefore not established as a live reconfiguration bug in the inspected entry point.

## Proposed next verification sequence

1. AUD-037 file-header/frame-count/frequency regression, then a narrow renderer suite.
2. AUD-038 independent elapsed-time and partition regression, then processing-thread tests.
3. Engine drain interruption and global bypass tail regressions with bounded channels.
4. HAL staging/session/reconnect tests on macOS before any shared-wire change.
5. Real processing-thread cold allocation and supported-layout/sample-format matrices.

No broad graph queue patch, driver wire change, or sibling repository modification was applied.

## Authorized follow-up implementation: AUD-037 and AUD-038

The parent subsequently authorized local implementation of these two engine defects. HAL staging/recovery, EOS interruption/bypass, and layout findings above remain read-only findings.

### AUD-037: export clock and physical signal alignment

- Original regression measured 9,637 source frames at 48 kHz producing 18,942 file frames after a configured 48→96 kHz converter, still labeled 48 kHz.
- The renderer now appends a final converter only when the configured chain ends at a different rate from the requested export. Explicit output_sample_rate and the default source-rate export policy are preserved.
- A second regression showed that trimming realtime scheduling latency deleted the first impulse. Resampler chunk waiting does not emit startup zero samples, and rubato's integer output_delay convention also differs from the observed sinc center.
- Added Plugin::signal_delay_samples() -> f64, measured in output-rate frames. Its default is latency_samples() as f64. The only override is ResamplerPlugin; the only new consumer is offline serial rendering. Realtime graph/PDC and its latency reports are unchanged.
- The offline renderer retains source-converter prefix samples, combines all fractional delays in the final clock, and rounds only once for final trimming. NaN, infinite, negative, and unaddressable declarations return errors. Frame/progress duration conversion uses integer rational arithmetic.
- Pinned rubato 1.0.1 physical center model: max(0, (sinc_len / 2 - 1 / table_phases) * ratio - 1). Derivation inspected the installed primary source: asynchro_sinc.rs initializes the sample index to -(N-1) and advances before emitting; sinc.rs centers phase zero at N/2-1+1/table_phases. The public output_delay implementation instead truncates N*ratio/2.
- Source package: /home/pierre/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rubato-1.0.1/src/{asynchro_sinc.rs,sinc.rs,asynchro.rs}. The docs.rs source URLs were attempted but unavailable through the browser, so the derivation relies on the installed pinned source, not fetched web contents.
- Independent peak measurement covered 756 cases: Fast/Medium/High, four rate pairs (48→96,96→48,44.1→48,48→44.1 kHz), chunks 64/127/513, callbacks 1/127/257, and impulse offsets 0/1/2/17/63/127/997. Maximum measured distance between integer sample peak and the predicted group center was 0.498046875 output frames, below the 0.52-frame bound. This is a fractional group-delay estimate, not a claim that the largest sample lies at an exact fractional position.
- Twelve real file renders (four source/export/internal-rate combinations, three callback sizes) assert exact header/rate/count/progress, independent 1 kHz zero-crossing frequency within 0.1 Hz, preserved first/final impulses, and boundary peak positions within one output frame. Final trimming is nearest-sample alignment and does not implement a fractional-delay filter.

### AUD-038: emitted-frame transition clock

- Removed the cached callback-sized crossfade step. An integer cursor now counts emitted final-rate frames; phase is derived from that cursor and the output sample rate for a 50 ms transition. Zero-input/output callbacks do not advance the fade.
- New updates restart the clock. Stop/Flush reset both hosts, the transition cursor, and the preallocated delay histories without resizing them.
- Independent analytical gains and retirement times pass at 44.1/48/96 kHz, across seven input/output rate pairs, callbacks 1/127/513 and irregular partitions with empty calls, plus 37-frame buffered rate-changing fixtures. The gain timeline is bit-identical between callback partitions. Buffered same-rate fixtures are excluded from this synthetic matrix because DawHost explicitly pads short same-rate results; production same-rate DSP has a full-block contract. The existing rate-change DC continuity test also passes.
- Existing per-block old-path interpolation remains unchanged. This result validates the gain clock, not general phase-perfect resampling between independently buffered old/new chains.

### Executed focused verification

- cargo test -p sotf-engine --no-default-features --lib offline_renderer: 15 passed; /tmp/sotf-offline-renderer-final.log.
- cargo test -p sotf-engine --no-default-features --lib engine::processing_thread: 41 passed; /tmp/sotf-processing-clock-final.log.
- cargo test -p sotf-engine --no-default-features --lib prepared_host_update_tests: 2 passed; /tmp/sotf-transition-delay-reset-final.log.
- cargo test -p sotf-plugin-resampler --lib: 49 passed; /tmp/sotf-resampler-signal-final.log. The separate --nocapture matrix measurement is /tmp/sotf-resampler-physical-delay-matrix.log.
- Clippy for the touched engine/host/resampler library and test targets is recorded in /tmp/sotf-engine-clock-clippy-final.log.

These focused runs exercise engine and resampler contracts; MIDI and IAMF source/tests were not investigated or modified. The initial engine red run used package defaults, while subsequent focused engine verification disabled default features.

## Authorized follow-up implementation: AUD-046

The parent subsequently authorized the EOS bypass/shutdown defects and related failure termination guards.

- Red evidence: ordinary complete-tail ordering passed, while retained-tail bypass, shutdown during pending tail delivery, shutdown during pending EOS, drain-error termination, and disconnected-receiver termination all failed. Four cases timed out waiting for the worker to terminate. Log: /tmp/sotf-engine-eos-red.log.
- Global bypass now skips host drain and resets frozen DSP history at EOS. If bypass arrives while a tail frame is blocked, the unsent tail is recycled, remaining host history is reset, and only ordinary EOS is delivered.
- Shutdown exits the entire processing loop from either EOS send loop. Commands are also checked between drain calls that emit no frames.
- Drain failure, nonconvergence, invalid drain output metadata, and EOS send failure publish ProcessingError and terminate the worker. The output sender is dropped, explicitly disconnecting the consumer instead of leaving it waiting or emitting a false success EOS.
- Normal completion still sends every returned tail frame in causal order before exactly one EOS. Output metadata is refreshed at each drain step.
- Ten deterministic real-worker tests use a fake stateful 48→96 kHz plugin, drain/reset counters, and a rendezvous output channel. Commands injected by the first drain are guaranteed to be observed during blocked sends. Coverage includes initial/mid-send bypass, shutdown at pending tail/pending EOS/zero-output progress, plugin failure, a 4,096-step nonconvergent drain, receiver disconnection, bounded receiver backpressure timeout, and successful ordered tail completion. No sleep-based race synchronization is used by the harness.
- Verification: cargo test -p sotf-engine --no-default-features --lib engine::processing_thread: 51 passed (41 existing plus 10 new). Log: /tmp/sotf-engine-eos-final.log. Clippy: /tmp/sotf-engine-eos-clippy.log.
- Remaining separate follow-up: Stop or host replacement during a pending tail send still needs an explicit stream-generation cancellation policy. The ordinary Frame/Flush send loops' interruption behavior was not changed by this EOS-scoped patch. HAL source remains untouched.

## Normal frame interruption follow-up (AUD062)

The real worker now retires an unsent normal output frame when an explicit
ProcessingCommand::Stop or Shutdown interrupts output backpressure. Stop clears
the active-stream scheduling flag and recycles the pending buffer. Shutdown
exits the outer processing loop. The prior code retried the stopped frame and
only exited the inner send loop on Shutdown.

Deterministic rendezvous tests use decoder-buffer recycling as a barrier: the
frame has finished DSP before the command is submitted. Previously the next
stream received the old 0.5 marker instead of 0.75, and Shutdown timed out while
both data endpoints stayed alive. Both regressions now pass, along with all 52
processing-thread tests and focused engine Clippy. Logs:
`/tmp/sotf-engine-pending-frame-{red,green,clippy}.log`.

This is the worker's explicit Stop command contract. The manager currently uses
a separate FIFO Flush protocol originating in the decoder. Already-queued frames,
rapid Stop/Play ordering, and host-generation changes require a separate full
pipeline review; this local fix does not establish end-to-end epoch isolation.

### Accepted processing commands and pending output formats (AUD062/063)

A follow-up exposed cancellation and nested-loop defects: an unclaimed/cancelled
Stop discarded a normal pending frame, accepted Stop could retry an old tail or
publish its EOS, and Shutdown during a blocked Flush left the worker alive.
The private command handler now returns Continue/Stopped/Shutdown only after its
request ticket is claimed. All send/drain paths act on that result. Pending
normal and drained audio also compare their actual channel count and sample rate
with the accepted host output format; a same-channel rate change drops the old
pending frame. These local guards do not coordinate already queued playback data.

Three new real-worker red regressions reproduced the failures before the fix:
`/tmp/sotf-engine-command-outcome-red.log`. Expanded Stop cases cover pending
tail, empty EOF, zero-output drain progress and cancelled requests; Shutdown also
covers pending Flush with live channels. A separate normal/tail rate-only host
replacement test verifies old-rate audio is not retried after acknowledgement.
The focused processing suite passes57 tests, with the two explicitly open broader
protocol repros excluded, and warnings-denied Clippy passes:
`/tmp/sotf-engine-command-outcome-green.log`,
`/tmp/sotf-engine-command-outcome-clippy.log`.

Remaining: quiesced decoder/playback Stop barriers, already-queued frame acceptance
across rate changes, and global bypass preserving/renegotiating the output clock.
The source review and concrete schedules are in the pending-frame protocol report;
no claim of a complete manager transaction is made by the local fixes.
