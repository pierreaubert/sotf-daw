// Rust guideline compliant 2026-02-21
use criterion::BenchmarkGroup;
use criterion::measurement::WallTime;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::{CrossoverPlugin, CrossoverPluginParams, PerChannelOpMode};
use std::hint::black_box;

const SAMPLE_RATE: u32 = 48_000;

fn bench_setup(
    group: &mut BenchmarkGroup<'_, WallTime>,
    id: &str,
    create: impl Fn() -> CrossoverPlugin,
) {
    group.bench_function(id, |bencher| {
        bencher.iter(|| {
            let mut plugin = create();
            plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
            black_box(plugin)
        });
    });
}

fn benchmark_setup(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_setup_and_initialize");
    group.sample_size(30);
    bench_setup(&mut group, "lr24_2ch_2band", || {
        CrossoverPlugin::new(2, "LR24", 1_000.0, "both").unwrap()
    });
    bench_setup(&mut group, "lr24_2ch_4band", || {
        CrossoverPlugin::new_multiway(2, "LR24", 200.0, "both", &[1_200.0, 6_000.0]).unwrap()
    });
    bench_setup(&mut group, "lr24_8ch_4band", || {
        CrossoverPlugin::new_multiway(8, "LR24", 200.0, "both", &[1_200.0, 6_000.0]).unwrap()
    });
    bench_setup(&mut group, "lr24_8ch_per_channel", || {
        CrossoverPlugin::new_per_channel(
            "LR24",
            vec![
                120.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 6_000.0, 8_000.0,
            ],
            vec![
                PerChannelOpMode::Lowpass,
                PerChannelOpMode::Highpass,
                PerChannelOpMode::Mute,
                PerChannelOpMode::Passthrough,
                PerChannelOpMode::Lowpass,
                PerChannelOpMode::Highpass,
                PerChannelOpMode::Mute,
                PerChannelOpMode::Passthrough,
            ],
        )
        .unwrap()
    });
    for taps in [63, 511, 1_025] {
        bench_setup(&mut group, &format!("fir_2ch_4band_{taps}taps"), || {
            fir_plugin(2, 4, taps)
        });
    }
    group.finish();
}

fn bench_plugin(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    id: &str,
    frames: usize,
    mut plugin: CrossoverPlugin,
) {
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input: Vec<f32> = (0..frames * plugin.input_channels())
        .map(|index| ((index % 101) as f32 - 50.0) / 101.0)
        .collect();
    let mut output = vec![0.0; frames * plugin.output_channels()];
    let context = ProcessContext::new(SAMPLE_RATE, frames);
    group.throughput(Throughput::Elements(input.len() as u64));
    group.bench_with_input(BenchmarkId::new(id, frames), &frames, |bencher, _| {
        bencher.iter(|| {
            plugin
                .process(black_box(&input), black_box(&mut output), &context)
                .unwrap()
        });
    });
}

fn benchmark_lr_blocks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_lr_interleaved_blocks");
    for frames in [32, 512, 2_048] {
        bench_plugin(
            &mut group,
            "2ch_2band_both",
            frames,
            CrossoverPlugin::new(2, "LR24", 1_000.0, "both").unwrap(),
        );
        bench_plugin(
            &mut group,
            "2ch_4band_both",
            frames,
            CrossoverPlugin::new_multiway(2, "LR24", 200.0, "both", &[1_200.0, 6_000.0]).unwrap(),
        );
        bench_plugin(
            &mut group,
            "8ch_4band_both",
            frames,
            CrossoverPlugin::new_multiway(8, "LR24", 200.0, "both", &[1_200.0, 6_000.0]).unwrap(),
        );
        bench_plugin(
            &mut group,
            "8ch_per_channel_mixed",
            frames,
            CrossoverPlugin::new_per_channel(
                "LR24",
                vec![
                    120.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 6_000.0, 8_000.0,
                ],
                vec![
                    PerChannelOpMode::Lowpass,
                    PerChannelOpMode::Highpass,
                    PerChannelOpMode::Mute,
                    PerChannelOpMode::Passthrough,
                    PerChannelOpMode::Lowpass,
                    PerChannelOpMode::Highpass,
                    PerChannelOpMode::Mute,
                    PerChannelOpMode::Passthrough,
                ],
            )
            .unwrap(),
        );
    }
    group.finish();
}

fn fir_plugin(channels: usize, bands: usize, taps: usize) -> CrossoverPlugin {
    let extra_frequencies = match bands {
        2 => vec![],
        4 => vec![1_200.0, 6_000.0],
        _ => unreachable!(),
    };
    CrossoverPlugin::from_params(
        channels,
        &CrossoverPluginParams {
            crossover_type: "FIR".into(),
            frequency: if bands == 2 { 1_000.0 } else { 200.0 },
            output: "both".into(),
            extra_frequencies,
            fir_taps: Some(taps),
            channel_frequencies_hz: vec![],
            channel_modes: Some(vec![]),
            topology: None,
            band_count: None,
        },
    )
    .unwrap()
}

fn benchmark_fir_blocks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_fir_interleaved_blocks");
    for frames in [32, 512, 2_048] {
        for taps in [63, 511] {
            bench_plugin(
                &mut group,
                &format!("2ch_2band_{taps}taps"),
                frames,
                fir_plugin(2, 2, taps),
            );
            bench_plugin(
                &mut group,
                &format!("2ch_4band_{taps}taps"),
                frames,
                fir_plugin(2, 4, taps),
            );
        }
    }
    group.finish();
}

fn benchmark_new_iir_setup(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_new_iir_setup");
    group.sample_size(30);

    for kind in ["LR12", "LR48", "BW6", "BW42", "BW48", "Bessel12"] {
        let label = format!("{}_2ch_2way", kind.to_ascii_lowercase());
        bench_setup(&mut group, &label, || {
            CrossoverPlugin::new(2, kind, 1_000.0, "both").unwrap()
        });
    }
    for kind in ["LR12", "LR48", "BW42", "BW48", "Bessel12"] {
        let label = format!("{}_2ch_4way", kind.to_ascii_lowercase());
        bench_setup(&mut group, &label, || {
            CrossoverPlugin::new_multiway(2, kind, 200.0, "both", &[1_200.0, 6_000.0]).unwrap()
        });
    }
    for kind in ["LR12", "LR48", "BW42", "BW48", "Bessel12"] {
        let label = format!("{}_2ch_per_channel", kind.to_ascii_lowercase());
        bench_setup(&mut group, &label, || {
            CrossoverPlugin::new_per_channel(
                kind,
                vec![500.0, 1_500.0],
                vec![PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
            )
            .unwrap()
        });
    }
    group.finish();
}

fn benchmark_new_iir_blocks(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("crossover_new_iir_interleaved_blocks");
    for frames in [32, 512, 2_048] {
        for kind in ["LR12", "LR48", "BW6", "BW42", "BW48", "Bessel12"] {
            let label = format!("{}_2ch_2way_both", kind.to_ascii_lowercase());
            bench_plugin(
                &mut group,
                &label,
                frames,
                CrossoverPlugin::new(2, kind, 1_000.0, "both").unwrap(),
            );
        }
        for kind in ["LR12", "LR48", "BW42", "BW48", "Bessel12"] {
            let label = format!("{}_2ch_4way_both", kind.to_ascii_lowercase());
            bench_plugin(
                &mut group,
                &label,
                frames,
                CrossoverPlugin::new_multiway(2, kind, 200.0, "both", &[1_200.0, 6_000.0]).unwrap(),
            );
        }
        for kind in ["LR12", "LR48", "BW42", "BW48", "Bessel12"] {
            let label = format!("{}_2ch_per_channel_mixed", kind.to_ascii_lowercase());
            bench_plugin(
                &mut group,
                &label,
                frames,
                CrossoverPlugin::new_per_channel(
                    kind,
                    vec![500.0, 1_500.0],
                    vec![PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
                )
                .unwrap(),
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    benchmark_setup,
    benchmark_lr_blocks,
    benchmark_fir_blocks,
    benchmark_new_iir_setup,
    benchmark_new_iir_blocks
);
criterion_main!(benches);
