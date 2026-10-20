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
