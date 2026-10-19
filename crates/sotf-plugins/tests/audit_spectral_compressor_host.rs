//! Queued controls and STFT clocks through actual prepared factory hosts.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, ParameterId, ParameterValue, ProcessContext, create_plugin};

#[test]
fn live_controls_keep_one_hop_clock_across_native_and_host_partitions() {
    for rate in [44_100, 48_000, 96_000, 192_000] {
        for (index, n) in [1024, 2048, 4096].into_iter().enumerate() {
            for target in 0..3 {
                for adaptive in [false, true] {
                    let preset = serde_json::json!({"fft_size_index":index,"threshold_db":-30.0,"ratio":8.0,
                        "attack_ms":3.0,"release_ms":80.0,"spectral_smoothing":0.75,
                        "target_mode":target,"adaptive_threshold":adaptive,"adaptive_offset_db":-6.0,
                        "channel_link":0.0,"mix":1.0});
                    let reloaded =
                        serde_json::from_str(&serde_json::to_string(&preset).unwrap()).unwrap();
                    let mut host = DawHost::new(2, rate);
                    host.set_max_block_frames(8192).unwrap();
                    host.add_plugin(
                        create_plugin("spectral_compressor", &reloaded, 2, rate).unwrap(),
                    )
                    .unwrap();
                    host.build().unwrap();
                    let mut direct =
                        create_plugin("spectral_compressor", &preset, 2, rate).unwrap();
                    direct.initialize(f64::from(rate)).unwrap();
                    assert_eq!(host.total_latency_samples(), n);
                    assert_eq!(direct.latency_samples(), n);
                    let mut time = 0;
                    for (frames, key, value) in [
                        (n / 3, "threshold", ParameterValue::Float(-36.0)),
                        (n + 17, "spectral_smoothing", ParameterValue::Float(1.0)),
                        (3 * n + 11, "channel_link", ParameterValue::Float(1.0)),
                        (n * 2 + 7, "mix", ParameterValue::Float(0.0)),
                        (n * 2 + 19, "mix", ParameterValue::Float(1.0)),
                    ] {
                        host.set_plugin_parameter(0, key, value.clone()).unwrap();
                        direct.set_parameter(ParameterId::from(key), value).unwrap();
                        let input: Vec<f32> = (time..time + frames)
                            .flat_map(|i| {
                                let tone = (i as f64 * 0.137).sin() as f32;
                                let burst = if i % (n + 17) < 31 {
                                    0.2 * (i as f64 * 2.71).sin() as f32
                                } else {
                                    0.0
                                };
                                [0.4 * tone + burst, -0.04 * tone + burst]
                            })
                            .collect();
                        let mut actual = vec![0.0; input.len()];
                        let mut expected = actual.clone();
                        for (src, dst) in input.chunks(31 * 2).zip(actual.chunks_mut(31 * 2)) {
                            host.process(src, dst).unwrap();
                        }
                        for (src, dst) in input.chunks(8192 * 2).zip(expected.chunks_mut(8192 * 2))
                        {
                            direct
                                .process(src, dst, &ProcessContext::new(rate, src.len() / 2))
                                .unwrap();
                        }
                        assert!(
                            actual == expected,
                            "{rate} FFT={n} target={target} adaptive={adaptive} control={key}: host or callback changed output"
                        );
                        time += frames;
                    }
                }
            }
        }
    }
}
