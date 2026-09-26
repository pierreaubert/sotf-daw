use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use sotf_audio::engine::playback_runtime_harness::VolumeRampHarness;

criterion_group!(benches, benchmark_volume_ramp);
criterion_main!(benches);

/// Baseline for the per-callback output gain ramp (`VolumeRampState::apply`).
///
/// The target alternates every callback so the ramp stays engaged instead of
/// settling at unity gain. Future SIMD work must beat these numbers.
fn benchmark_volume_ramp(c: &mut Criterion) {
    let mut group = c.benchmark_group("volume_ramp");

    for channels in [2usize, 8] {
        for frames in [512usize, 4096] {
            group.bench_with_input(
                BenchmarkId::new(format!("{channels}ch"), frames),
                &(channels, frames),
                |b, &(channels, frames)| {
                    let mut harness = VolumeRampHarness::new(channels, 48_000, 0.0);
                    let mut high = false;
                    b.iter_batched(
                        || vec![0.5f32; frames * channels],
                        |mut buffer| {
                            high = !high;
                            let target = if high { 1.0 } else { 0.0 };
                            harness.apply(std::hint::black_box(&mut buffer), target);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
            // Steady state: gain already at target, the pure-multiply path
            // every static-gain callback takes.
            group.bench_with_input(
                BenchmarkId::new(format!("{channels}ch_steady"), frames),
                &(channels, frames),
                |b, &(channels, frames)| {
                    let mut harness = VolumeRampHarness::new(channels, 48_000, 1.0);
                    b.iter_batched(
                        || vec![0.5f32; frames * channels],
                        |mut buffer| {
                            harness.apply(std::hint::black_box(&mut buffer), 1.0);
                        },
                        BatchSize::SmallInput,
                    );
                },
            );
        }
    }

    group.finish();
}
