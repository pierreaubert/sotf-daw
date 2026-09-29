use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sotf_host::LoudnessData;
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use std::time::{Duration, Instant};

const CALLBACK_FRAMES: usize = 128;
const PROFILE_RATES: [u32; 7] = [8_000, 12_000, 24_000, 44_100, 48_000, 96_000, 192_000];
const PROFILE_CHANNELS: [u32; 2] = [1, 24];

fn fixture(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|index| {
            let time = index as f64 / channels as f64;
            (0.31 * (std::f64::consts::TAU * 0.173 * time).sin()
                + 0.27 * (std::f64::consts::TAU * 0.311 * time + 0.2).cos()
                - 0.19 * (std::f64::consts::TAU * 0.427 * time + 0.7).sin()) as f32
        })
        .collect()
}

fn benchmark_true_peak_processing(criterion: &mut Criterion) {
    let mut callback = criterion.benchmark_group("true_peak_process_callback");
    callback.sample_size(10);
    callback.warm_up_time(Duration::from_millis(200));
    callback.measurement_time(Duration::from_millis(500));

    for rate in PROFILE_RATES {
        for channels in PROFILE_CHANNELS {
            let channel_count = channels as usize;
            let audio = fixture(CALLBACK_FRAMES, channel_count);
            let input_samples = (CALLBACK_FRAMES * channel_count) as u64;
            callback.throughput(Throughput::Elements(input_samples));

            callback.bench_function(
                BenchmarkId::new(format!("{rate}hz"), format!("{channels}ch")),
                |bencher| {
                    let mut monitor = LoudnessMonitor::new(channels, rate).unwrap();
                    let mut data = LoudnessData::new(channel_count);
                    bencher.iter_custom(|iterations| {
                        let mut elapsed = Duration::ZERO;
                        for _ in 0..iterations {
                            monitor.reset().unwrap();
                            let start = Instant::now();
                            monitor.add_frames(std::hint::black_box(&audio)).unwrap();
                            elapsed += start.elapsed();
                            monitor.update_loudness_data(&mut data);
                            std::hint::black_box(&data.true_peaks_dbtp);
                        }
                        elapsed
                    });
                },
            );
        }
    }

    callback.finish();

    let mut drain = criterion.benchmark_group("true_peak_finish_and_publish");
    drain.sample_size(10);
    drain.warm_up_time(Duration::from_millis(200));
    drain.measurement_time(Duration::from_millis(500));
    drain.throughput(Throughput::Elements(1));

    for rate in PROFILE_RATES {
        for channels in PROFILE_CHANNELS {
            let channel_count = channels as usize;
            let audio = fixture(CALLBACK_FRAMES, channel_count);
            drain.bench_function(
                BenchmarkId::new(format!("{rate}hz"), format!("{channels}ch")),
                |bencher| {
                    let mut monitor = LoudnessMonitor::new(channels, rate).unwrap();
                    let mut data = LoudnessData::new(channel_count);
                    bencher.iter_custom(|iterations| {
                        let mut elapsed = Duration::ZERO;
                        for _ in 0..iterations {
                            monitor.reset().unwrap();
                            monitor.add_frames(&audio).unwrap();
                            let start = Instant::now();
                            monitor.finish_true_peak();
                            monitor.update_loudness_data(&mut data);
                            elapsed += start.elapsed();
                            std::hint::black_box(&data.true_peaks_dbtp);
                        }
                        elapsed
                    });
                },
            );
        }
    }

    drain.finish();
}

criterion_group!(benches, benchmark_true_peak_processing);
criterion_main!(benches);
