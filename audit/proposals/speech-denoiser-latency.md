# AUD083 SpeechDenoiser latency alignment proposal

2026-09-28. Read-only source review; no production/test/dependency changes and no
new Cargo runs. Scope is SpeechDenoiser and its RNNoise adapter, not Denoiser,
Hiss, host scheduling, native wrappers, MIDI or IAMF. TokenSave status/search/read
were used first; current source slices were checked because aggregate changes
postdate some graph entries. DSP/MS Rust skill guidance applied.

## Finding and existing executed evidence

The current wet signal contains two distinct 480-frame delays, but the dry path
and declared latency contain only one. The backend's comment claiming identical
wet/dry delay is incorrect.

The prior public probe is `/tmp/sotf-denoiser-tail-probe.rs/.log`, summarized in
`/tmp/sotf-denoiser-followup-plan.md`. It measured mono and stereo impulses at
source0/72/479/480:

- Disabled main peak: source+480, exact input amplitude0.75.
- Enabled main peak: source+960, amplitude about0.0988 mono /0.1445 stereo.
- Public declared latency:480 in every case.
- Direct vendored unit-band-gain processing has its principal impulse peak at480
  before the adapter queue. Its preceding 480-frame output is not exactly zero
  (peak0.1202717 in the existing probe), so discarding that model frame is an
  audio change, not removal of empty initialization padding.

This review inspected those retained results and the source; it did not rerun
or generalize that small matrix into a complete waveform proof. A correction
needs the independent waveform and single-frame callback tests below.

## Source-grounded timing

Let F=480, X_k=input[kF..(k+1)F), and M_k be the complete 480-frame model result
computed from X_k.

The vendored core (`crates/3rdparties/nnnoiseless/src/denoise.rs:213-226`) analyzes
`[previous input frame, current input frame]` in its 960-sample window.
`frame_synthesis` at368-381 returns the first 480 inverse-transform samples plus
prior synthesis overlap and saves the second half. M_k therefore has nominal
source origin `(k-1)F`: the model itself contributes F frames of signal delay.
Its filters, gains and pitch processing can spread energy around that nominal
origin; neither M_0 nor arbitrary wet audio is a pure delayed identity.

The adapter (`crates/sotf-plugins/crates/plugins-denoiser/src/rnnoise.rs`):

- Initialize/reset sets `output_write_pos=F`, `output_read_pos=0` (116-121,
  313-317), priming a separate F-frame output queue with zeros.
- At a completed source frame it places M_k at `(k+1)F` (178-247).
- It currently places X_k in the dry ring at the same position (178-188).
- Output reading (252-276) advances every accepted sample, and mixes wet/dry
  at one shared output-clock position.

Thus wet nominal origin `(k-1)F` appears at output `(k+1)F`: delay2F=960. Raw dry
origin kF appears at output `(k+1)F`: delayF=480.

### Causality with one-frame callbacks

| Last accepted source index | Model work completed | Output index / content |
| --- | --- | --- |
| 0..478 | none | 0..478: queue zeros |
| 479 | M_0 computed from all X_0 | 479: last queue zero |
| 480..958 | no M_1 yet | 480..958: M_0[0..478] |
| 959 | M_1 computed from all X_1 | 959: M_0[479] |
| 960 | M_1 ready | 960: M_1[0], nominal source0 |

Removing the queue cannot keep this timeline: M_0[0] depends on an entire
future source frame relative to output index0. A scheduler could trade the
current whole-frame queue for a 479-frame convention, but that would change all
wet timing to959 and require a distinct compatibility decision. It is not the
minimal correction. Dropping M_0 likewise loses legitimate synthesized audio
and cannot satisfy arbitrary one-frame callbacks without new buffering.

## Recommended minimal correction

Preserve wet arithmetic, neural/stereo decisions, model-frame schedule, existing
queue priming, and every wet output sample at its existing output index.

1. Define explicit constants for F-frame model delay, F-frame adapter queue,
   and total delay2F. Preserve public `SPEECH_DENOISER_FRAME_SIZE=480`: model
   frame size and total signal latency are distinct quantities.
2. Queue each raw dry block X_k at `output_write_pos + F`, one frame ahead of
   its current location in the already separate dry ring. Then X_k appears at
   `(k+2)F`, aligning its source origin with M_(k+1).
3. Return960 from backend latency and therefore the plugin metadata/compiled
   metadata in both enabled states, before/after initialization and reset.
   Do not initialize `output_write_pos` to960: that would also add another
   frame to wet and turn its actual nominal delay into1440.
4. Correct comments/README/changelog and tests that confuse startup queue size,
   model frame size, total wet delay, and bypass delay. Existing assertions about
   the first480 wet samples being queue zeros remain valid. Do not strengthen
   them to 'first960 wet samples are zero': M_0 can contain model pre-ringing.

### Existing ring capacity is sufficient

Both rings already have4F=1920 scalar slots per channel. Calls are subdivided
into chunks C<=F before queued output is read. At a chunk's start let R be the
number of already emitted/accepted frames; after ingestion S=R+C. The completed
wet write frontier is `W=F+floor(S/F)*F`. The new dry frontier is W+F. Its distance
from the current read head is at most `2F+C<=3F`, strictly within the4F ring.
Dry writes cannot overwrite unread output, even with partial accumulators,
oversized callbacks or wraparound. Existing normalization subtracts whole ring
turns from both cursors; adding constant F for dry addressing preserves modulo
mapping. No new buffer or per-sample allocation is necessary.

## Initialization, reset and enable transitions

- Initialize already validates rate48k and mono/stereo before replacing state;
  it builds fresh model states, clears rings and resets analyzer data. Preserve
  this order. Failed rate/layout initialization must retain the prior valid
  stream, verified against a twin.
- Reset already resets models/stereo detector, accumulation, both rings,
  scratch, cursors and bypass initialization in place. Keep W=F, not2F. The
  extra empty dry frame is supplied by the zeroed ring before offset writes.
- The enabled parameter remains live. Model processing continues while dry,
  so re-enabling does not resume stale neural/filter history. Preserve current
  480-sample output-clock crossfade; it blends dry and wet for the same nominal
  source origin after this correction.
- Parameter changes apply at the current callback's first output frame, as
  today. They select already-delayed audio at the output clock; this proposal
  does not reinterpret enable automation as source-clock events or introduce
  an automation FIFO. Tests must schedule changes at fixed absolute callback
  boundaries and vary subdivisions between them.
- A source-only edge found during review: a valid zero-frame backend call sets
  `bypass_initialized=true` and latches the then-current target despite accepting
  no audio. A later parameter change before the first real sample therefore
  starts a fade instead of honoring the initial state directly. Proposed narrow
  companion: after existing shape/backend checks, return0 on num_frames0 before
  updating bypass fields. Test empty enabled call, set disabled, then real input
  against a fresh initially-disabled instance, and the reverse. This finding is
  source-inspected, not yet executed; capture a red regression before changing it.

## Independent numerical and lifecycle verification plan

1. Red public metadata/impulse mismatch: fixed impulse positions0/1/72/479/480
   and every model-frame phase, mono/stereo, one-frame callbacks included.
   Disabled path must exactly equal sanitized/clamped source shifted by960.
   Use a dyadic dense multichannel sequence and extra ordinary zeros to cover
   first sample, last marker and many ring wraps. Include1/7/137/479/480/481/4096
   plus mixed partitions. Exact delay and sample sequence, not a peak tolerance.
2. Wet waveform oracle independent of adapter queues: invoke the vendored model
   directly on ordered fixed480-frame input blocks, keep EVERY model result,
   scale as specified, then prepend exactly480 zeros. Compare the entire backend
   wet stream to that sequence under every callback partition. A backend unit
   test can use its existing nnnoiseless dependency without adding one to the
   public plugin. This proves preserved model output/order and queue causality;
   it does not falsely claim the denoiser is a linear pure delay.
3. Mono direct model plus correlated/anti-phase stereo fixtures using common
   gains on both direct channel states. For those known detector relationships,
   construct the reference detector from the algebraic linked policy without
   calling the adapter helper. Preserve existing recorded model reference tests.
4. Independent crossfade combination: always-wet model oracle plus exact
   960-frame dry shift, with prescribed output-clock mix evolution. Exercise
   enabled→disabled→enabled before a full model frame, at479/480/481, during an
   unfinished fade and after ring wrap. Check samplewise output and warm neural
   analyzer state, not only maximum jump or declared latency.
5. Public reset and reinitialize after warm audio and during a fade must match
   fresh instances using the final enabled setting. Include zero-sized calls,
   invalid format/rate/shape retry against an untouched twin, output suffix
   canaries, and stable total latency. Keep frame size480 assertions separate.
6. Explicit allocation AND deallocation counts on fresh threads for first
   callback, first model call at1/479/480, wrap, both toggles, reset/replay and
   scalar metadata. Existing prepared storage suffices; no realtime resource
   ownership changes are intended.
7. Run focused SpeechDenoiser tests and RNNoise backend tests, then their full
   crates/Clippy. No aggregate gate or host/wrapper edits are needed for this
   local implementation; callers receive new metadata through existing APIs.

## Compatibility and deferred tails

This corrects a user-visible contract: hosts previously compensated only480
while enabled speech was nominally delayed960. They should now compensate960.
The steady enabled wet waveform/timeline remains unchanged in direct processing;
initially-disabled output moves480 frames later, and transition waveforms change
because the two paths become aligned. Preset fields/IDs/defaults and rate/layout
support remain unchanged. Existing saved sessions may realign relative to other
tracks after host latency recalculation; document this in the release note.

The fix does not provide finite EOF draining. RNNoise's input highpass has
recursive audio state, and mono pitch history contributes additional stored
signal. Retain Unknown/default drain until a separate policy is reviewed; do
not label the960-frame latency a general tail bound. Even initially-disabled
would require separate eligibility/transition reasoning before advertising a
finite dry-only tail, because live bypass fades can still expose the wet path.

No implementation has started. Proposed touched files are the RNNoise backend,
SpeechDenoiser latency docs/tests and README/CHANGELOG only; the vendored model,
host, native wrapper and transport/wiring remain unchanged.
