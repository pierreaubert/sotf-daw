# AUD083 SpeechDenoiser latency correction — verified

2026-09-28. Source frozen after focused tests and strict Clippy. Implementation
matches `audit/proposals/speech-denoiser-latency.md`. Scope: RNNoise backend and
SpeechDenoiser docs/tests only. No vendored model, dependencies, host, native
wrapper, transport, drain, MIDI or IAMF changes.

## Corrected contract

- Model frame size remains480. Wet analysis/synthesis contributes480 frames;
  the existing causal adapter queue contributes480 more. Total reported latency
  is now **960 frames /20 ms at48 kHz**, in every enabled/reset/init state.
- The wet write cursor remains480 on initialize/reset. Every existing wet model
  frame, including the nonempty first synthesis frame, is emitted at its prior
  position. There is no added wet delay or discarded model result.
- Raw dry blocks are written480 frames farther ahead in their existing separate
  ring. This makes dry and wet refer to the same nominal source time during the
  existing480-sample output-clock crossfade. Disabled output is now an exact
  sanitized/clamped source shift of960 frames.
- Existing1920-sample rings suffice: bounded processing chunks are at most480,
  and the new dry frontier is at most1440 frames ahead of the read head. No
  new storage, resource replacement, queue, lock or allocation was introduced.
- Zero-frame calls return before choosing the initial bypass state. They no
  longer change the waveform of a subsequent early enable/disable sequence.
- Settings, model warm-bypass policy, sample-rate/layout validation and input
  sanitization remain intact. Initialization/reset clear the existing histories.

RNNoise may emit pre-ringing before its nominal960-frame delay: only the first
480 adapter-queue outputs are guaranteed zero in wet mode. Dry is an exact pure
960-frame delay; wet is nonlinear filtered audio with that nominal source origin.
The documentation distinguishes frame size, queue size and total signal delay.

## Red → green evidence

Permanent tests were run before the production correction:

1. Backend closed-form transition oracle (direct model wet + independent960 dry
   shift): maximum observed red difference **0.44921875**.
2. Backend empty-call + early second enable switch: red difference
   **0.06474553**. The test includes137 real frames between changes, making the
   wrongly latched mix audible after the first wet model frame. Merely checking
   startup zeros would not expose it. The corrected comparison is bit exact.
3. Public dry waveform: first failure at output480, where old output was
   **-0.22851563** instead of the required startup zero.
4. The wet full-waveform oracle already passed before the change, and still
   passes bit exactly afterward. This establishes preservation rather than only
   agreement between two callbacks of a modified scheduler.

Logs:

- `/tmp/sotf-speech-timing-red.log`
- `/tmp/sotf-speech-public-red.log`
- `/tmp/sotf-speech-timing-green.log`
- `/tmp/sotf-speech-public-green.log`

## Independent and lifecycle evidence

New backend integration file `plugins-denoiser/tests/rnnoise_timing.rs`:

- **24 wet waveform fixtures**: mono, correlated stereo, opposite-polarity
  stereo ×8 callback partitions, including1-frame and8193-frame calls. Reference
  invokes nnnoiseless directly on complete ordered480-frame input blocks, keeps
  every model result and prepends480 zeros; it has no backend streaming ring.
  Known stereo fixture algebra gives the detector directly from L without
  invoking the production detector helper. Full output equality is exact.
- **12 transition fixtures**: three layouts ×4 callback partitions, changes at
  absolute output frames37/479/480/481/1001/1203/1921/2600/3109/4801, including
  unfinished fades and ring wrap. Reference combines the direct wet waveform
  and independently shifted dry using a closed-form f64 linear ramp. Error is
  below1e-5, allowing existing repeated-f32-ramp rounding. Different callback
  partitions themselves must be exactly equal. Model generations remain exact.
- Four empty-call/early-switch cases compare with a fresh state exactly.
- Two failed-initialization cases verify unsupported rate and zero/wide layouts
  preserve prepared audio/model history and analyzer state against untouched twins.

New public file `sotf-plugin-speech-denoiser/tests/timing.rs`:

- **16 dense dry fixtures** cover mono/stereo ×8 partitions, ring wraps, every
  accepted sample, final marker, nonfinite sanitization and finite clipping.
  Source is dyadic; every returned dry sample equals the exact960-frame oracle.
- **960 marker fixtures** cover every first480-frame phase in mono/stereo, with
  first and final markers and one-frame/mixed callbacks.
- **10 wet impulse cases** at source0/1/72/479/480 verify principal peak at
  source+960 and agreement with declared latency. This complements rather than
  replaces the direct-model full-waveform test.
- Public empty calls, pre-init errors, bad rate/short buffer canaries, invalid
  initialization retry, warmed reset/reinitialize and active fade reset all
  preserve the documented state contract. Fresh equivalence is sample exact.
- **Eight fresh-thread cold memory fixtures** (mono/stereo × callbacks1/137/480/
  8193) cover first call/model, multiple wraps, both live toggles, analyzer loads,
  scalar latency/tail queries, reset and replay. Explicit allocator AND
  deallocator counts are **0/0 in every case**. These counts cover new-thread
  callback work without a warmup on that thread.

Existing480-frame dry/latency expectations were updated to the independently
verified960 contract; existing wet startup/model output assertions retain480.
Model golden-reference tests and old stereo/linking/partition tests remain green.

## Commands and totals

- `cargo test -p plugins-denoiser -p sotf-plugin-speech-denoiser --all-features`
  — **106 passed, 0 failed, 0 ignored** (backend75, Speech31).
- `cargo clippy -p plugins-denoiser -p sotf-plugin-speech-denoiser --all-targets --all-features -- -D warnings`
  — clean.
- Scoped rustfmt check and diff whitespace checks — clean.
- Full logs: `/tmp/sotf-speech-timing-full.log`,
  `/tmp/sotf-speech-timing-clippy.log`.

## Compatibility / remaining scope

Enabled wet waveform/timing is unchanged in direct processing. Disabled audio
moves480 frames later, crossfades become aligned, and hosts now compensate the
existing960-frame wet delay. Sessions may therefore realign relative to other
tracks after latency recalculation. Preset IDs, fields/defaults and the public
480-frame size constant are unchanged. README and both crate changelogs record
this behavior explicitly.

Tail metadata remains Unknown and native drain remains its prior default.
The recursive RNNoise input highpass prevents treating latency as a generic
finite tail bound. This correction does not claim EOF preservation or solve
that separate policy.

## Workspace wrapper-fixture follow-up

The tenth workspace run exposed two older native-wrapper fixtures that still
expected a 480-frame dry delay. Their independent expected delay is now 960
frames (480 model + 480 queue); exact dry comparisons, startup zeros, sentinels,
allocation checks and existing numerical tolerance are unchanged. Both focused
fixtures and strict native-wrapper Clippy pass:
`/tmp/sotf-audit-wave10-speech-fixtures.log` and
`/tmp/sotf-audit-wave10-speech-fixtures-clippy.log`.
The first aggregate result is retained in `/tmp/sotf-audit-wave10-nextest.log`:
5,700 passed, two stale timing expectations failed, 10 skipped.
