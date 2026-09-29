use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sotf_host::LoudnessRangeConfig;
use sotf_host::analyzer_loudness_monitor::LoudnessMonitor;
use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
const CALLBACK_FRAMES: usize = 480;
const PREFILL_CALLBACKS: usize = 301;

fn fixture(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|sample| {
            let frame = sample / channels;
            let channel = sample % channels;
            let phase = std::f64::consts::TAU * 997.0 * frame as f64 / SAMPLE_RATE as f64
                + channel as f64 * 0.13;
            (0.2 * phase.sin()) as f32
        })
        .collect()
}

fn make_monitor(channels: usize, layout_id: Option<&str>, lra_enabled: bool) -> LoudnessMonitor {
    let mut monitor = if let Some(layout_id) = layout_id {
        let config = get_speaker_config(layout_id).expect("speaker config");
        let layout = ChannelLayout::from_speaker_config(config).expect("valid 7.1.4 layout");
        LoudnessMonitor::new_with_layout(channels as u32, SAMPLE_RATE, layout)
            .expect("construct explicit loudness monitor")
    } else {
        LoudnessMonitor::new(channels as u32, SAMPLE_RATE).expect("construct loudness monitor")
    };

    if lra_enabled {
        monitor = monitor
            .with_loudness_range(Some(LoudnessRangeConfig::default()))
            .expect("prepare rolling LRA");
    }

    monitor
}

fn benchmark_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    channels: usize,
    layout_id: Option<&str>,
    lra_enabled: bool,
) {
    let audio = fixture(CALLBACK_FRAMES, channels);
    let mut monitor = make_monitor(channels, layout_id, lra_enabled);
    for _ in 0..PREFILL_CALLBACKS {
        monitor
            .add_frames(&audio)
            .expect("prefill loudness monitor");
    }

    let layout = layout_id.unwrap_or("stereo");
    let lra = if lra_enabled { "lra_on" } else { "lra_off" };
    group.throughput(Throughput::Elements((CALLBACK_FRAMES * channels) as u64));
    group.bench_function(BenchmarkId::new(layout, lra), |bencher| {
        bencher.iter_custom(|iterations| {
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                monitor
                    .add_frames(std::hint::black_box(&audio))
                    .expect("process loudness callback");
            }
            std::hint::black_box(&monitor);
            start.elapsed()
        });
    });
}

fn benchmark_paused_case(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    channels: usize,
    layout_id: Option<&str>,
    lra_enabled: bool,
) {
    let audio = fixture(CALLBACK_FRAMES, channels);
    let mut monitor = make_monitor(channels, layout_id, lra_enabled);
    for _ in 0..PREFILL_CALLBACKS {
        monitor
            .add_frames(&audio)
            .expect("prefill loudness monitor");
    }
    monitor.pause_integrated_measurement();

    let layout = layout_id.unwrap_or("stereo");
    let lra = if lra_enabled {
        "lra_on_paused"
    } else {
        "lra_off_paused"
    };
    group.throughput(Throughput::Elements((CALLBACK_FRAMES * channels) as u64));
    group.bench_function(BenchmarkId::new(layout, lra), |bencher| {
        bencher.iter_custom(|iterations| {
            let start = std::time::Instant::now();
            for _ in 0..iterations {
                monitor
                    .add_frames(std::hint::black_box(&audio))
                    .expect("process paused loudness callback");
            }
            std::hint::black_box(&monitor);
            start.elapsed()
        });
    });
}

fn benchmark_aud125_ms_cost(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("aud125_ms_steady_callback");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    benchmark_case(&mut group, 2, None, true);
    benchmark_case(&mut group, 2, None, false);
    benchmark_case(&mut group, 6, Some("5.1"), true);
    benchmark_case(&mut group, 6, Some("5.1"), false);
    benchmark_case(&mut group, 12, Some("7.1.4"), true);
    benchmark_case(&mut group, 12, Some("7.1.4"), false);

    benchmark_paused_case(&mut group, 2, None, true);
    benchmark_paused_case(&mut group, 2, None, false);
    benchmark_paused_case(&mut group, 6, Some("5.1"), true);
    benchmark_paused_case(&mut group, 6, Some("5.1"), false);
    benchmark_paused_case(&mut group, 12, Some("7.1.4"), true);
    benchmark_paused_case(&mut group, 12, Some("7.1.4"), false);

    group.finish();
}

criterion_group!(benches, benchmark_aud125_ms_cost);
criterion_main!(benches);
