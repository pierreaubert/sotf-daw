# AUD136: enabled SpeechDenoiser accepted-program drain

**Status:** Astra accepted the bounded design; production behavior, focused
package tests, pre-edit baseline replay, and strict plugin/backend lint pass.
Final implementation review is pending. Executed evidence is recorded in
[`audit/speech-denoiser-enabled-accepted-queue.md`](../speech-denoiser-enabled-accepted-queue.md).

## Source finding and pre-edit evidence

`plugins-denoiser/src/rnnoise.rs` accumulates input in 480-frame blocks. The
backend reports 960 frames of latency: one 480-frame RNNoise model delay plus
the adapter's 480-frame startup/output queue. A public SpeechDenoiser call can
end at any sample in that model block, leaving accepted samples in the
accumulator and output queued in the backend. `SpeechDenoiserPlugin::drain`
currently returns `COMPLETE` with zero frames whenever enabled, and its
`drain_call_bound` reports one call. Enabled `tail_length` is `Unknown`.

The existing disabled-path contract is separate and accepted under AUD083:
disabled processing returns a finite 960-frame delayed dry stream. This
proposal keeps that behavior, its timing, and its tests unchanged.

The test-only source baseline was captured before adding the AUD136 regression:

| File | Pre-test SHA-256 |
|---|---|
| `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/src/lib.rs` | `e73e28a2f3f33ae812d33fedd99b1f9027fa54f9d01def62248fba66340dd774` |
| `crates/sotf-plugins/crates/plugins-denoiser/src/rnnoise.rs` | `cc4369c0a77bd575cc739c70bfd9e5249a93e6892def10fd4b166cbaae23b344` |
| `crates/sotf-plugins/crates/sotf-plugin-speech-denoiser/tests/finite_stream.rs` | `5b07f3227eca9fcafb955be012ceb5c66e5b408cf3916830c1ae0ec6d5d1c77e` |
| `Cargo.lock` | `c161c74af23f42ac8efe4540eac59a7961d38ac1d12fa659d95587cb9699c3e5` |

Complete pre-production source copies, including the host completion contract,
SpeechDenoiser/backend sources and tests, adapter source, manifests, and lock,
are retained in
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/source`. Its
`pre-edit-source.sha256` manifest SHA-256 is
`002b0398f71dfedb57a853528b2e9582e9b681f2de97cb420a537b62173ba6a4`.
An ignored capture test preserved the 1,513-frame voiced input plus two
2,473-frame f32le outputs at
`crates/sotf-plugins/target/audit-artifacts/aud136/pre-edit/audio`:

| Captured samples | SHA-256 |
|---|---|
| `input_mono.f32le` | `f9d289b128c03b3d2a4ce8e6701a6e283c6a969db6a81fc16184d17f62040bad` |
| `enabled_ordinary_process_zero_continuation.f32le` | `6b0b1c5c8fd26c2d4114a22619085affb9c98c6406536581eeab488b08113e07` |
| `disabled_process_and_drain.f32le` | `00ed1602cb69e77eed28af50bd93fa0e64c9aa9adb94e37105f298d87dd76b32` |

Capture log `/tmp/sotf-aud136-preedit-audio-capture.log` has SHA-256
`cb7815b6e17de111a6bb52543a59ce9be7906dc36e14059485f7d2db51531d76` and
records one ignored capture test passing.

The public expected-red case is
`enabled_partial_eof_preserves_accepted_program_like_zero_continuation` in
`tests/finite_stream.rs`. It submits 1,513 mono frames, ending 73 frames into a
model block, then compares ordinary process-plus-drain with a separately
initialized enabled instance given the same input followed by 960 zero frames.
The test requires the reference's omitted 960-frame suffix to exceed a
`1e-5` peak floor before checking the complete vector. It failed because drain
returned 1,513 total frames versus 2,473 for the zero-continuation twin. The
twin uses the same RNNoise implementation through the ordinary process route:
this is an independent execution/continuation oracle, not an independent
denoising algorithm or speech-quality reference. The threshold is asserted,
not reported as a measured peak.

- Terminal expected-red log:
  `/tmp/sotf-aud136-red-suffix-eof.log`, SHA-256
  `4f0956e315f63906869b9cd7ef9aafc07a42f96f3a71d97bcc985a03255b1b0f`.
- Tested source/lock manifest:
  `/tmp/sotf-aud136-red-suffix-source.sha256`, aggregate SHA-256
  `f77b0553183ab85095754f05218a99032c359c0b5b2502a426afdce255cb10f9`.
- Command used the shared absolute target and `TMPDIR` under
  `crates/sotf-plugins/target/audit-tmp`:
  `cargo test --offline --locked -p sotf-plugin-speech-denoiser --test finite_stream enabled_partial_eof_preserves_accepted_program_like_zero_continuation -- --exact --nocapture`.

## Proposed drain policy

For an initialized, enabled SpeechDenoiser with accepted input, drain feeds
exactly 960 zero frames through the existing backend and emits the resulting
960 frames in chunks of at most 480. This preserves the final accepted partial
model block and releases its fixed processing queue just as normal zero input
does. At 48 kHz this is exactly 960 host frames.

Enabled `TailLength` remains `Unknown`. The 960-frame drain is an explicit
render cutoff for the fixed accepted-program queue; it is not evidence that the
RNNoise or high-pass recursive response has finite support or naturally ends
at 960 frames. Immediately after the last emitted sample, the plugin calls the
existing allocation-free backend reset to clear remaining accumulator, model,
and output-ring state. It records terminal drain state (`Some(0)`) until public
reset or successful initialization. Thus `PluginDrainResult.complete` is
truthful at return: no later plugin output remains under this declared policy.
The natural recursive response after the cutoff is intentionally discarded.
The plugin's final published analyzer snapshot remains available; the backend
reset must not replace the published cache with defaults.

An empty stream still completes immediately and remains unfrozen. A valid
nonempty drain freezes new input and changed `enabled` values until reset, as
the existing disabled path does. Sample-rate mismatch, malformed output
geometry, and zero output capacity must fail before starting unfinished
nonempty drain work or mutating output/state. Empty streams and already
completed streams may return `COMPLETE` with zero capacity without freezing the
plugin. Repeated terminal drain calls return complete with zero frames.
Existing disabled behavior, including its 960-frame alignment, is preserved.

`drain_output_frames_max` stays 480. With a 480-frame destination, the plugin
emits two output calls and marks the second complete; the full-capacity call
bound is two. Smaller direct destinations may require more calls, while the
host uses the declared 480-frame maximum. No queue, manager, or host processing
behavior change is proposed.

### Drain completion wording

The current shared `PluginDrainResult.complete` field comment says that no
buffered input or filter tail remains. The proposed plugin reset makes that
true at completion while documenting that the recursive tail was cut off. A
narrow doc clarification is proposed: completion means the plugin's declared
drain policy will emit no further output for the finalized stream; a plugin
may explicitly discard residual state for an `Unknown` response, and this does
not make `TailLength` finite. There is no host API or queue protocol change.

## Sample-rate and host-path scope

The plugin and backend accept only 48 kHz. `DawHost::build` initializes a node
at its actual input rate. The inspected automatic wrapping path handles
`preferred_oversampling`; no `requires_sample_rate` contract or automatic
fixed-rate resampler was found. Therefore the host endpoint test will prove a
48 kHz factory/adapter chain emits the accepted input plus 960 drain frames,
and will verify that a direct 44.1 kHz chain fails initialization. This work
will not claim that 960 model frames convert to 960 frames at another host
rate. An explicitly configured resampler graph is outside this bounded issue.

## Acceptance evidence planned

1. Compare the full emitted vector with an independently initialized plugin
   that processes the same input and then 960 zero frames. Require exact frame
   count, exact samples, and a nonzero peak in the 960-frame suffix. Label this
   as same-algorithm continuation/timing evidence, not an independent RNNoise
   quality oracle.
2. Cover mono and stereo, every terminal accumulator residue from 1 through
   479, exact-block endings, and multiple accepted blocks. Compare one-frame,
   17-frame, irregular, 480-frame, oversized, and 8193-frame callback
   partitions. Preserve existing 960-frame model/backend latency tests.
3. Compare the zero-continuation oracle through enabled/bypass transitions
   before EOF. Verify empty input, full and partial drains, repeated complete,
   reset versus fresh-instance replay, failed drain preflight followed by
   successful processing, and input/changed-control rejection after drain
   begins.
4. Verify `TailLength::Unknown` before and after enabled drain, a two-call
   maximum-capacity bound, and the 480-frame output maximum. Preserve AUD083's
   disabled `Finite(960)` semantics.
5. Guard the complete process and drain paths, including the terminal backend
   reset, for zero allocation and zero deallocation after initialization and
   cold preparation. Verify final analyzer telemetry remains published.
6. Run an actual `DawHost` chain containing the SpeechDenoiser adapter at
   48 kHz through process and drain, checking endpoint samples and total frame
   count. Assert a direct 44.1 kHz chain is rejected rather than implying
   transparent conversion.

No corpus-level speech-quality, model-response finite-support, broad sample
rate conversion, host-queue, or manager-protocol claim is part of AUD136.
