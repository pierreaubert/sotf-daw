// Rust guideline compliant 2026-02-21
use std::time::Instant;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use sotf_host::Plugin;
use sotf_host::ProcessContext;
use sotf_host::speaker_config::get_speaker_config;
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use sotf_plugin_ambisonics::spherical_harmonics::MAX_ORDER;
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

const SAMPLE_RATE: u32 = 48_000;
const LAYOUT: &str = "9.1.6";
const BLOCK_FRAMES: usize = 512;
const CALLBACK_OBSERVATIONS: usize = 2_048;

fn supported_benchmark_orders() -> Vec<usize> {
    let mut orders = vec![1, 3];
    if MAX_ORDER >= 7 {
        orders.push(7);
    }
    orders
}

fn make_config(order: usize, algorithm: &str, dual_band: bool) -> AmbisonicsDecoderConfig {
    AmbisonicsDecoderConfig {
        order,
        target_layout: LAYOUT.to_owned(),
        max_re_weighting: true,
        dual_band,
        algorithm: algorithm.to_owned(),
    }
}

fn benchmark_matrix_construction(criterion: &mut Criterion) {
    let speaker_config = get_speaker_config(LAYOUT).expect("benchmark layout exists");
    let mut group = criterion.benchmark_group("AUD133/decode_matrix");

    for order in supported_benchmark_orders() {
        for algorithm in ["mode_matching", "allrad"] {
            for max_re_weighting in [false, true] {
                let id = BenchmarkId::new(
                    format!("order{order}_{algorithm}_maxre{max_re_weighting}"),
                    LAYOUT,
                );
                group.bench_function(id, |bencher| {
                    bencher.iter(|| {
                        let matrix = match algorithm {
                            "mode_matching" => {
                                DecodeMatrix::build(order, speaker_config, max_re_weighting)
                            }
                            "allrad" => {
                                DecodeMatrix::build_allrad(order, speaker_config, max_re_weighting)
                            }
                            _ => unreachable!("benchmark algorithm is fixed"),
                        }
                        .unwrap();
                        std::hint::black_box(matrix);
                    });
                });
            }
        }
    }
    group.finish();
}

fn benchmark_plugin_construction(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("AUD133/plugin_setup");

    for order in supported_benchmark_orders() {
        for algorithm in ["mode_matching", "allrad"] {
            let config = make_config(order, algorithm, false);
            group.bench_function(
                BenchmarkId::new(format!("construct_order{order}_{algorithm}"), LAYOUT),
                |bencher| {
                    bencher.iter(|| {
                        let plugin =
                            AmbisonicsDecoderPlugin::new(std::hint::black_box(&config)).unwrap();
                        std::hint::black_box(plugin);
                    });
                },
            );

            let dual_config = make_config(order, algorithm, true);
            group.bench_function(
                BenchmarkId::new(
                    format!("construct_initialize_order{order}_{algorithm}"),
                    LAYOUT,
                ),
                |bencher| {
                    bencher.iter(|| {
                        let mut plugin =
                            AmbisonicsDecoderPlugin::new(std::hint::black_box(&dual_config))
                                .unwrap();
                        plugin.initialize(SAMPLE_RATE).unwrap();
                        std::hint::black_box(plugin);
                    });
                },
            );
        }
    }
    group.finish();
}

fn callback_input(input_channels: usize) -> Vec<f32> {
    (0..BLOCK_FRAMES * input_channels)
        .map(|sample| {
            let centered = (sample.wrapping_mul(73).wrapping_add(19) % 509) as i32 - 254;
            centered as f32 / 8_192.0
        })
        .collect()
}

fn measure_observed_callback_times(
    order: usize,
    algorithm: &str,
    dual_band: bool,
    input: &[f32],
    output: &mut [f32],
    context: &ProcessContext<'_>,
) {
    let mut plugin =
        AmbisonicsDecoderPlugin::new(&make_config(order, algorithm, dual_band)).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();

    let mut elapsed_ns = Vec::with_capacity(CALLBACK_OBSERVATIONS);
    for _ in 0..CALLBACK_OBSERVATIONS {
        let started = Instant::now();
        plugin
            .process(
                std::hint::black_box(input),
                std::hint::black_box(output),
                std::hint::black_box(context),
            )
            .unwrap();
        elapsed_ns.push(started.elapsed().as_nanos());
    }
    elapsed_ns.sort_unstable();
    let percentile = |percent: usize| elapsed_ns[((elapsed_ns.len() - 1) * percent) / 100];
    println!(
        "AUD133_CALLBACK_OBS order={order} algorithm={algorithm} dual_band={dual_band} frames={BLOCK_FRAMES} n={} p50_ns={} p95_ns={} p99_ns={} max_ns={}",
        elapsed_ns.len(),
        percentile(50),
        percentile(95),
        percentile(99),
        elapsed_ns[elapsed_ns.len() - 1],
    );
}

fn benchmark_callback(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("AUD133/process_512");

    for order in supported_benchmark_orders() {
        for algorithm in ["mode_matching", "allrad"] {
            for dual_band in [false, true] {
                let mut plugin =
                    AmbisonicsDecoderPlugin::new(&make_config(order, algorithm, dual_band))
                        .unwrap();
                plugin.initialize(SAMPLE_RATE).unwrap();
                let input = callback_input(plugin.input_channels());
                let mut output = vec![0.0_f32; BLOCK_FRAMES * plugin.output_channels()];
                let context = ProcessContext::new(SAMPLE_RATE, BLOCK_FRAMES);

                measure_observed_callback_times(
                    order,
                    algorithm,
                    dual_band,
                    &input,
                    &mut output,
                    &context,
                );
                group.bench_function(
                    BenchmarkId::new(format!("order{order}_{algorithm}_dual{dual_band}"), LAYOUT),
                    |bencher| {
                        bencher.iter(|| {
                            plugin
                                .process(
                                    std::hint::black_box(&input),
                                    std::hint::black_box(&mut output),
                                    std::hint::black_box(&context),
                                )
                                .unwrap();
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    benchmark_matrix_construction,
    benchmark_plugin_construction,
    benchmark_callback
);
criterion_main!(benches);
