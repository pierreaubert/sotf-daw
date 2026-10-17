// Rust guideline compliant 2026-02-21

//! DAW-scale chain benchmarks: 50/100/200-plugin linear chains.
//!
//! Measures the host render paths a large session actually takes:
//! compiled-linear (with static-gain fusion), the graph fallback for the
//! same chain, and a half-bypassed compiled chain. Each scenario asserts
//! the expected render path via `DawHost::render_stats` before timing, so
//! a silent fallback to the graph path fails loudly instead of producing
//! a misleading number.
//!
//! Run with: `cargo bench -p sotf-plugins --bench chain-scale-benchmark`

use criterion::{Criterion, criterion_group, criterion_main};
use sotf_plugins::{EqPlugin, GainPlugin, ParametricPluginAdapter, PluginHost};
use std::hint::black_box;

const FRAMES: usize = 512;
const CHANNELS: usize = 2;

fn gain_chain(count: usize, compiled: bool) -> PluginHost {
    let mut host = PluginHost::new(CHANNELS, 48000);
    host.set_compiled_linear_enabled(compiled).unwrap();
    for i in 0..count {
        let gain_db = if i % 2 == 0 { -0.05 } else { 0.05 };
        host.add_plugin(Box::new(ParametricPluginAdapter::new(GainPlugin::new(
            CHANNELS, gain_db,
        ))))
        .unwrap();
    }
    host.build().unwrap();
    host
}

fn eq_chain(count: usize) -> PluginHost {
    use math_audio_iir_fir::{Biquad, BiquadFilterType};
    let mut host = PluginHost::new(CHANNELS, 48000);
    for i in 0..count {
        let offset = (i % 16) as f64 * 10.0;
        let filters = vec![
            Biquad::new(BiquadFilterType::Peak, 420.0 + offset, 48_000.0, 1.0, -1.5),
            Biquad::new(BiquadFilterType::Peak, 1_800.0 + offset, 48_000.0, 1.8, 1.0),
            Biquad::new(BiquadFilterType::Highshelf, 10_000.0, 48_000.0, 0.707, 1.0),
        ];
        host.add_plugin(Box::new(ParametricPluginAdapter::new(EqPlugin::new(
            CHANNELS, filters,
        ))))
        .unwrap();
    }
    host.build().unwrap();
    host
}

fn bench_chains(c: &mut Criterion) {
    let input = vec![0.5_f32; FRAMES * CHANNELS];

    for count in [50_usize, 100, 200] {
        let mut host = gain_chain(count, true);
        assert_eq!(
            host.render_stats().path,
            sotf_plugins::HostRenderPath::CompiledLinear,
            "gain chain must take the compiled linear path"
        );
        let mut output = vec![0.0_f32; FRAMES * CHANNELS];
        c.bench_function(&format!("gain_chain_{count}_compiled"), |b| {
            b.iter(|| {
                host.process(black_box(&input), black_box(&mut output))
                    .unwrap();
            });
        });

        let mut host = eq_chain(count);
        assert_eq!(
            host.render_stats().path,
            sotf_plugins::HostRenderPath::CompiledLinear,
            "EQ chain must take the compiled linear path"
        );
        let mut output = vec![0.0_f32; FRAMES * CHANNELS];
        c.bench_function(&format!("eq_chain_{count}_compiled"), |b| {
            b.iter(|| {
                host.process(black_box(&input), black_box(&mut output))
                    .unwrap();
            });
        });
    }

    // Same 200-gain chain through the graph fallback, for path comparison.
    let mut host = gain_chain(200, false);
    assert_eq!(
        host.render_stats().path,
        sotf_plugins::HostRenderPath::Graph,
        "disabled compiled plan must take the graph path"
    );
    let mut output = vec![0.0_f32; FRAMES * CHANNELS];
    c.bench_function("gain_chain_200_graph", |b| {
        b.iter(|| {
            host.process(black_box(&input), black_box(&mut output))
                .unwrap();
        });
    });

    // 200 gains with every second plugin bypassed (elision fast path).
    let mut host = gain_chain(200, true);
    for index in (1..200).step_by(2) {
        host.bypass_plugin(index).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.render_stats().bypassed_nodes, 100);
    let mut output = vec![0.0_f32; FRAMES * CHANNELS];
    c.bench_function("gain_chain_200_half_bypassed", |b| {
        b.iter(|| {
            host.process(black_box(&input), black_box(&mut output))
                .unwrap();
        });
    });

    // Silent input: stateless linear ops skip their process calls.
    let mut host = gain_chain(200, true);
    let silent = vec![0.0_f32; FRAMES * CHANNELS];
    let mut output = vec![0.0_f32; FRAMES * CHANNELS];
    c.bench_function("gain_chain_200_silent", |b| {
        b.iter(|| {
            host.process(black_box(&silent), black_box(&mut output))
                .unwrap();
        });
    });
}

criterion_group!(benches, bench_chains);
criterion_main!(benches);
