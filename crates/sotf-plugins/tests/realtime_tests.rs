// ============================================================================
// Real-Time Safety Tests
// ============================================================================
//
// Synthetic timing smoke checks. These do not establish device deadline safety.
// A callback budget is frames / sample_rate; allocation claims belong to the
// instrumented realtime_allocation_tests suite.

use sotf_plugins::{
    GainPlugin, LimiterPlugin, ParametricInPlacePlugin, ParametricPlugin, Plugin, ProcessContext,
    UpmixerPlugin,
};
use std::hint::black_box;
use std::time::{Duration, Instant};

const MAX_ALLOWED_LATENCY_US: u64 = TEST_BUFFER_SIZE as u64 * 1_000_000 / TEST_SAMPLE_RATE as u64;
const ITERATIONS: usize = 100;
const TEST_SAMPLE_RATE: u32 = 48000;
const TEST_BUFFER_SIZE: usize = 512;

fn percentile(times: &[Duration], p: f64) -> Duration {
    let mut sorted: Vec<_> = times.iter().collect();
    sorted.sort();
    let idx = (p / 100.0 * sorted.len() as f64) as usize;
    *sorted[idx.min(sorted.len() - 1)]
}

#[test]
fn test_gain_plugin_timing() {
    let mut gain = GainPlugin::new(2, 3.0);
    gain.plugin_initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];
    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let mut times = Vec::with_capacity(ITERATIONS);

    let mut output = input.clone();
    for _ in 0..10 {
        gain.process(&input, &mut output, &context).unwrap();
    }

    for _ in 0..ITERATIONS {
        let start = Instant::now();
        black_box(gain.process(&input, &mut output, &context)).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
    }

    let min = times.iter().min().unwrap();
    let max = times.iter().max().unwrap();
    let mean: Duration = times.iter().sum::<Duration>() / ITERATIONS as u32;
    let p50 = percentile(&times, 50.0);
    let p95 = percentile(&times, 95.0);
    let p99 = percentile(&times, 99.0);

    println!("\n=== GainPlugin ===");
    println!("  Min:     {:.3} µs", min.as_secs_f64() * 1e6);
    println!("  Mean:    {:.3} µs", mean.as_secs_f64() * 1e6);
    println!("  P50:     {:.3} µs", p50.as_secs_f64() * 1e6);
    println!("  P95:     {:.3} µs", p95.as_secs_f64() * 1e6);
    println!("  P99:     {:.3} µs", p99.as_secs_f64() * 1e6);
    println!("  Max:     {:.3} µs", max.as_secs_f64() * 1e6);
    println!("  Budget:  {} µs", MAX_ALLOWED_LATENCY_US);

    let passed = p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US;
    println!("  Status:  {}", if passed { "PASS" } else { "FAIL" });

    assert!(
        p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US,
        "GainPlugin P99 latency ({:.3} µs) exceeds budget ({} µs)",
        p99.as_secs_f64() * 1e6,
        MAX_ALLOWED_LATENCY_US
    );
}

#[test]
fn test_gain_plugin_chain_timing() {
    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];
    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let mut gains = [
        GainPlugin::new(2, 1.0),
        GainPlugin::new(2, -2.0),
        GainPlugin::new(2, 1.0),
    ];
    for gain in &mut gains {
        gain.plugin_initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();
    }
    let mut first = vec![0.0; input.len()];
    let mut second = vec![0.0; input.len()];
    let mut output = vec![0.0; input.len()];
    let mut times = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        gains[0]
            .process(black_box(&input), &mut first, &context)
            .unwrap();
        gains[1].process(&first, &mut second, &context).unwrap();
        gains[2]
            .process(&second, black_box(&mut output), &context)
            .unwrap();
        times.push(start.elapsed());
    }
    for (&actual, &expected) in output.iter().zip(&input) {
        assert!((actual - expected).abs() < 1e-5, "three gains must cancel");
    }

    let p99 = percentile(&times, 99.0);

    println!("\n=== GainPlugin x3 chain ===");
    println!("  P99:  {:.3} µs", p99.as_secs_f64() * 1e6);

    assert!(
        p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US,
        "GainPlugin chain P99 latency ({:.3} µs) too high",
        p99.as_secs_f64() * 1e6
    );
}

#[test]
fn test_limiter_plugin_timing() {
    let mut plugin = LimiterPlugin::new(2, -3.0, 50.0, 5.0, false);
    plugin.initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let input = vec![0.8f32; TEST_BUFFER_SIZE * 2];
    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let mut times = Vec::with_capacity(ITERATIONS);

    let mut output = input.clone();
    for _ in 0..10 {
        output.copy_from_slice(&input);
        plugin.process_in_place(&mut output, &context).unwrap();
    }

    for _ in 0..ITERATIONS {
        output.copy_from_slice(&input);
        let start = Instant::now();
        black_box(plugin.process_in_place(&mut output, &context)).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
    }

    let p99 = percentile(&times, 99.0);

    println!("\n=== LimiterPlugin ===");
    println!("  P99:  {:.3} µs", p99.as_secs_f64() * 1e6);

    assert!(
        p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US,
        "LimiterPlugin P99 latency ({:.3} µs) too high",
        p99.as_secs_f64() * 1e6
    );
}

#[test]
fn test_upmixer_plugin_timing() {
    let mut plugin = UpmixerPlugin::new(
        4096, "9.1.6", 1.0, 0.5, 0.3, 80.0, 0.5, 250.0, 1.0, 1.0, false, 1.0,
    );
    plugin.initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];
    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let output_channels = plugin.output_channels();
    let mut times = Vec::with_capacity(ITERATIONS);

    let mut output = vec![0.0f32; TEST_BUFFER_SIZE * output_channels];
    for _ in 0..10 {
        plugin.process(&input, &mut output, &context).unwrap();
    }

    for _ in 0..ITERATIONS {
        let start = Instant::now();
        black_box(plugin.process(&input, &mut output, &context)).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
    }

    let p99 = percentile(&times, 99.0);

    println!("\n=== UpmixerPlugin (9.1.6, FFT 4096) ===");
    println!("  P99:  {:.3} µs", p99.as_secs_f64() * 1e6);

    assert!(
        p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US,
        "UpmixerPlugin P99 latency ({:.3} µs) too high",
        p99.as_secs_f64() * 1e6
    );
}

#[test]
fn test_gain_repeated_processing() {
    let mut gain = GainPlugin::new(2, 0.0);
    gain.plugin_initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];

    for _ in 0..100 {
        let mut output = input.clone();
        gain.process(&input, &mut output, &context).unwrap();
        assert_eq!(output, input);
    }
}

#[test]
fn test_gain_long_running_identity() {
    let mut gain = GainPlugin::new(2, 0.0);
    gain.plugin_initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];

    for _ in 0..1000 {
        let mut output = input.clone();
        gain.process(&input, &mut output, &context).unwrap();
        assert_eq!(output, input);
    }
}

#[test]
fn test_gain_steady_state_timing() {
    let mut times = Vec::with_capacity(ITERATIONS);

    let mut gain = GainPlugin::new(2, 0.0);
    gain.plugin_initialize(f64::from(TEST_SAMPLE_RATE)).unwrap();

    let context = ProcessContext::new(TEST_SAMPLE_RATE, TEST_BUFFER_SIZE);

    let input = vec![0.5f32; TEST_BUFFER_SIZE * 2];

    let mut output = input.clone();
    for _ in 0..10 {
        gain.process(&input, &mut output, &context).unwrap();
    }

    for _ in 0..ITERATIONS {
        let start = Instant::now();
        black_box(gain.process(&input, &mut output, &context)).unwrap();
        let elapsed = start.elapsed();
        times.push(elapsed);
    }

    let p99 = percentile(&times, 99.0);
    let mean: Duration = times.iter().sum::<Duration>() / ITERATIONS as u32;

    println!("\n=== Gain steady state (isolated) ===");
    println!("  Mean: {:.3} µs", mean.as_secs_f64() * 1e6);
    println!("  P99:  {:.3} µs", p99.as_secs_f64() * 1e6);

    assert!(
        p99.as_micros() as u64 <= MAX_ALLOWED_LATENCY_US,
        "Gain steady-state P99 latency ({:.3} µs) too high",
        p99.as_secs_f64() * 1e6
    );
}
