use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use sotf_audio::engine::playback_runtime_harness::{
    FrameWriterHarness, PlaybackCallbackHarness, XorShift64, generated_frame,
};

criterion_group!(
    benches,
    benchmark_playback_frame_writer,
    benchmark_playback_callback
);
criterion_main!(benches);

fn benchmark_playback_frame_writer(c: &mut Criterion) {
    let mut group = c.benchmark_group("playback_frame_writer");

    for frames in [512, 1024, 4096] {
        group.bench_with_input(
            BenchmarkId::new("direct_2ch", frames),
            &frames,
            |b, &frames| {
                b.iter_batched(
                    || setup_case(frames, 2, 2, frames * 2 * 4, 0),
                    |(mut harness, frame)| harness.write(std::hint::black_box(frame)),
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.bench_function("upmix_2_to_8_1024", |b| {
        b.iter_batched(
            || setup_case(1024, 2, 8, 1024 * 8 * 4, 0),
            |(mut harness, frame)| harness.write(std::hint::black_box(frame)),
            BatchSize::SmallInput,
        );
    });

    for input_channels in [6, 8, 10] {
        group.bench_with_input(
            BenchmarkId::new("downmix_to_2_1024", input_channels),
            &input_channels,
            |b, &input_channels| {
                b.iter_batched(
                    || setup_case(1024, input_channels, 2, 1024 * input_channels * 4, 0),
                    |(mut harness, frame)| harness.write(std::hint::black_box(frame)),
                    BatchSize::SmallInput,
                );
            },
        );
    }

    group.bench_function("fallback_4_to_6_1024", |b| {
        b.iter_batched(
            || setup_case(1024, 4, 6, 1024 * 6 * 4, 0),
            |(mut harness, frame)| harness.write(std::hint::black_box(frame)),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("full_buffer_drop_1024", |b| {
        b.iter_batched(
            || setup_case(1024, 2, 2, 1024, 1024),
            |(mut harness, frame)| harness.write(std::hint::black_box(frame)),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

/// End-to-end consumer-side cost per callback: ring read + volume ramp +
/// output metering + clamp. This is the true per-callback budget the
/// producer-side writer bench does not cover.
fn benchmark_playback_callback(c: &mut Criterion) {
    let mut group = c.benchmark_group("playback_callback");

    for channels in [2usize, 8] {
        for frames in [512usize, 4096] {
            group.bench_with_input(
                BenchmarkId::new(format!("{channels}ch"), frames),
                &(channels, frames),
                |b, &(channels, frames)| {
                    let mut harness = PlaybackCallbackHarness::new(
                        frames * channels * 4,
                        frames * channels,
                        channels,
                        48_000,
                    );
                    // Prime the ring so every measured iteration reads full.
                    let prime = vec![0.5f32; frames * channels];
                    harness.process(&prime);
                    b.iter_batched(
                        || vec![0.5f32; frames * channels],
                        |input| {
                            std::hint::black_box(harness.process(&input));
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}

fn setup_case(
    frames: usize,
    input_channels: usize,
    output_channels: usize,
    ring_capacity: usize,
    prefill_samples: usize,
) -> (FrameWriterHarness, sotf_audio::engine::AudioFrame) {
    let mut rng = XorShift64::new(
        ((frames as u64) << 32) ^ ((input_channels as u64) << 16) ^ output_channels as u64,
    );
    let samples = frames * input_channels;
    let frame = generated_frame(frames, input_channels, samples, &mut rng);
    let harness =
        FrameWriterHarness::for_frame(ring_capacity, output_channels, &frame, prefill_samples);
    (harness, frame)
}
