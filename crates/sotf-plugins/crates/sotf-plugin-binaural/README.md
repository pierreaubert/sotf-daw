# sotf-plugin-binaural

SOTF Binaural Decoder plugin for HRTF-based binaural rendering.

Renders supported multichannel layouts to binaural stereo using causal,
partitioned overlap-add convolution with SOFA HRTFs. The renderer has a fixed
`fft_size` host latency, strict speaker-layout admission, transactional SOFA
replacement, generation-tagged asynchronous head tracking, source-owned broadband room reflections, optional
diffuse-field EQ, and late reverb.

## SOFA delay support

SOFA `Data.Delay` uses source-rate sample counts. Shared `[I,R]` and
measurement-specific `[M,R]` nonnegative integer delays are materialized exactly
once into the HRIR before filter preparation. Missing/all-zero delay preserves
raw samples and length; existing SQLite caches are loaded unchanged. Fractional
or negative nonzero delay returns an explicit unsupported-capability error.
Newly materialized IR storage is limited to 256 MiB for the entire dataset,
separately from the renderer's per-IR support limit. Same-rate integer timing is
verified; this change alone does not establish cross-rate phase accuracy.

The existing linear-convolution IR-length check includes the materialized delay.
Initialization stages file loading, resampling and support validation before
changing live clocks/history; rejected preparation preserves active/pending
filter owners and partial drain state. Later unrelated preparation errors retain
their existing behavior.

## Head-orientation source clock

Orientation advances in per-sample steps of accepted source audio, and HRTF
requests are sampled at completed source-hop boundaries. Host callback sizes and
partial-hop parameter changes therefore produce identical request sequences.
Empty callbacks and synthetic EOF padding do not advance orientation; reset
clears the pending input-hop phase. The fixed FFT transport latency is unchanged.

`src/lib/head_clock_tests.rs` verifies 36 request schedules across 44.1/48/96 kHz,
FFT sizes 64/256/1024, four callback patterns and interrupted three-axis motion.
It checks the first request against independent 10 ms one-pole timing and covers
empty, partial, reset and drain boundaries. A deterministic request sink isolates
this clock test from background-thread scheduling. Worker publication freshness is covered by the gated handoff tests below.
End-to-end motion-to-audio latency, coordinate/delay boundaries and recursive
tail-cutoff audibility remain separate audit work.

## Asynchronous pose preparation

A bounded atomic mailbox retains the latest desired three-axis pose. The
capacity-one channel only wakes the worker; a full channel still retains the
newest pose without an audio-thread retry. The worker coalesces queued poses,
uses an immutable dataset/configuration snapshot, and publishes completed poses
in increasing generation order. It can publish a result while a newer desired
pose is waiting: rejecting every in-flight result would starve tracking during
continuous motion. This policy bounds queue backlog, not preparation latency.

Reset and resource/worker changes invalidate the preparation epoch. Both worker
publication and audio adoption reject old epochs, including a reset occurring
after preparation but before adoption. Resource commits retire the old worker
before installing replacement filters. EOF finishes the current filter
transition without adopting later background publications. Discarded results
and displaced publications are reclaimed off the audio thread.

Ordinary processing and drain require the exact prepared sample rate. A
mismatch, including empty/completed calls, preserves output, controls and stream
state. `head_worker_tests.rs` gates worker preparation with rendezvous channels
to verify coalescing, motion progress, ordering, lifecycle cancellation and EOF
ownership; it also stresses coherent snapshots during 100,000 pose writes.
Cold empty/full-wakeup callback writes have zero allocations and frees. These
handoff tests use controlled prepared filters and do not establish head-turn
accuracy or a hardware motion-to-audio deadline.

## Circular head angles and prepared spatial cues

Yaw, pitch and roll smooth along the shorter component arc across ±180°;
antipodal direction follows the signed component request. Reported parameter
targets retain the requested canonical values. Rebase control updates to keep
internal f32 angles within one extra turn, and use circular distance for the
0.5° request threshold. This is per-axis Euler smoothing, not a promise of a
quaternion geodesic for simultaneous multi-axis motion.

SOFA replacement and rate reinitialization prepare direct filters at the current
smoothed head pose, preventing a temporary zero-pose spatial image. Positive
yaw looks left, positive pitch looks up and positive roll raises the left ear.
The inverse transform is checked against an independent forward/left/up head
basis in 675 direction/orientation combinations.

`head_cue_tests.rs` checks 36 seam/timing cases and real mono SOFA cues at held
−180/−90/0/90/180° yaw and 44.1/48/96 kHz. Two datasets have different grids and
directional level/onset cues; shared `[I,R]` and genuinely varying `[M,R]`
positive integer delays must be applied exactly once. It verifies prepared DC
and complex transfer values, then 30 direct impulse responses after supported
10 ms resource crossfades, including a short partial final hop and drain.
These held-pose synthetic fixtures do not establish continuously moving
non-grid HRTF interpolation accuracy, a hardware motion-to-audio deadline,
maximum dataset cost or recursive-tail cutoff audibility.

### Repeated HRTF transitions

Linear fades retarget from the effective complex transfer of the active fade,
including each side's diffuse-field EQ. Two setup-allocated per-channel buffers
retain that transfer transactionally: a full retirement queue leaves both the
current owners and the audible transfer unchanged. This adds two stereo complex
spectra per input channel (32 × channels × frequency bins bytes); retarget work
is bounded by those dimensions. The fade gain advances once per source hop.
This preserves the scheduled linear transfer; it does not establish a subjective
click threshold for arbitrary moving HRTFs.

Spectral RTPGHI fades finish before adopting the latest published state. This
preserves their phase history and coalesces intermediate publications. Pending
updates can wait the remaining rounded fade duration (the public setting ranges
from 10 to 500 ms), plus publication/retirement contention; this is a documented
tracking-latency tradeoff, not a low-latency head-motion guarantee. EOF continues
the audible transition while excluding later publications.
