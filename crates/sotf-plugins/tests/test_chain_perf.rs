use sotf_plugins::{
    DawHost, LoudnessMonitorPlugin, ParametricInPlacePluginAdapter, ParametricPluginAdapter,
};
use std::time::Instant;

#[sotf_test::slow]
#[test]
fn test_plugin_chain_performance_with_monitor_96khz() {
    let channels = 2;
    let sample_rate = 96000;
    let frame_size = 1024;

    let mut host = DawHost::new(channels, sample_rate);

    // EQ
    let eq = sotf_plugins::EqPlugin::new(channels, vec![]);
    host.add_plugin(Box::new(ParametricPluginAdapter::new(eq)))
        .unwrap();

    // Compressor
    let comp = sotf_plugins::CompressorPlugin::new(channels);
    host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(comp)))
        .unwrap();

    // Monitor
    let monitor = LoudnessMonitorPlugin::new(channels).unwrap();
    host.add_plugin(Box::new(monitor)).unwrap();

    host.build().unwrap();

    let input = vec![0.1; frame_size * channels];
    let mut output = vec![0.0; frame_size * channels];

    // Warm up
    for _ in 0..100 {
        host.process(&input, &mut output).unwrap();
    }

    let iterations = 1000;
    let mut times = Vec::with_capacity(iterations);
    let start = Instant::now();
    for _ in 0..iterations {
        let callback = Instant::now();
        host.process(&input, &mut output).unwrap();
        times.push(callback.elapsed());
        assert!(output.iter().all(|sample| sample.is_finite()));
    }
    let duration = start.elapsed();
    let avg_ms = duration.as_secs_f64() * 1000.0 / iterations as f64;
    let budget_ms = (frame_size as f64 / sample_rate as f64) * 1000.0;

    println!(
        "\nChain Performance Results (96kHz, {} channels):",
        channels
    );
    println!("Average process time: {:.4} ms", avg_ms);
    println!("Real-time budget:     {:.4} ms", budget_ms);
    times.sort_unstable();
    let p99_ms = times[iterations * 99 / 100].as_secs_f64() * 1000.0;
    assert!(
        p99_ms < budget_ms,
        "chain p99 {p99_ms} ms exceeds {budget_ms} ms callback budget"
    );
    println!("P99 process time: {p99_ms:.4} ms");
    println!(
        "Callback budget used (mean): {:.2}%",
        (avg_ms / budget_ms) * 100.0
    );
}

#[test]
fn negotiated_host_matches_default_across_rate_expansion() {
    use sotf_plugins::{DynamicEqPlugin, ResamplerPlugin};
    let make = |negotiated: bool| {
        let mut host = DawHost::new(2, 48_000);
        if negotiated {
            host.set_max_block_frames(128).unwrap();
        }
        host.add_plugin(Box::new(
            ResamplerPlugin::new(2, 48_000, 96_000, 128).unwrap(),
        ))
        .unwrap();
        host.add_plugin(Box::new(ParametricInPlacePluginAdapter::new(
            DynamicEqPlugin::new(2),
        )))
        .unwrap();
        host.build().unwrap();
        host
    };
    let mut actual = make(true);
    let mut reference = make(false);
    for frames in [1, 7, 31, 128, 64].into_iter().cycle().take(128) {
        let input: Vec<f32> = (0..frames * 2)
            .map(|i| (i as f32 * 0.07).sin() * 0.1)
            .collect();
        let capacity = actual
            .output_frames_for_input(frames)
            .max(reference.output_frames_for_input(frames));
        let mut a = vec![0.0; capacity * 2];
        let mut b = a.clone();
        assert_eq!(
            actual.process(&input, &mut a).unwrap(),
            reference.process(&input, &mut b).unwrap()
        );
        assert_eq!(a, b);
    }
}
