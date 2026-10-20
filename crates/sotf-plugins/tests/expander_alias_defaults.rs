//! Compatibility aliases preserve direct defaults; the named factory is broadband.

// Rust guideline compliant 2026-02-21
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
use sotf_plugins::{ExpanderPlugin, ExpanderPluginParams, create_plugin};

#[test]
fn direct_alias_retains_multiband_defaults_and_explicit_one_band_is_broadband() {
    assert_eq!(ExpanderPluginParams::default().num_bands, 3);
    assert_eq!(ExpanderPlugin::new(2).info().name, "Multiband Expander");
    for rate in [44_100, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for config in [
                serde_json::json!({}),
                serde_json::json!({"num_bands": 5, "ratio": 1.0}),
                serde_json::json!({"num_bands": 3, "detection_mode": "RMS",
                    "threshold_db": -24.0, "knee_db": 6.0, "processing_mode": "spectral",
                    "lookahead_ms": 5.0, "sidechain_hpf_hz": 0.0}),
            ] {
                let mut params: ExpanderPluginParams =
                    serde_json::from_value(config.clone()).unwrap();
                params.num_bands = 1;
                params.processing_mode = "time_domain".into();
                let mut native = ExpanderPlugin::try_from_params(channels, params, rate).unwrap();
                native.initialize(f64::from(rate)).unwrap();
                let mut factory = create_plugin("expander", &config, channels, rate).unwrap();
                factory.initialize(f64::from(rate)).unwrap();
                assert_eq!(native.info().name, "Expander");
                assert_eq!(factory.info().name, "Expander");
                assert_eq!(factory.latency_samples(), native.latency_samples());
                assert!(factory.parameters().iter().all(
                    |p| p.id.as_str() != "num_bands" && !p.id.as_str().starts_with("crossover")
                ));
                let mut cursor = 0;
                for frames in [1, 17, 257, 1024, 3, 257, 1024] {
                    let input: Vec<_> = (0..frames * channels)
                        .map(|i| {
                            let frame = cursor + i / channels;
                            let ch = i % channels;
                            (0.25
                                * (std::f64::consts::TAU * 110.0 * frame as f64 / f64::from(rate))
                                    .sin()
                                / (ch + 1) as f64) as f32
                        })
                        .collect();
                    let mut expected = input.clone();
                    let mut actual = vec![f32::NAN; input.len()];
                    let context = ProcessContext::new(rate, frames);
                    assert_eq!(
                        native.process_in_place(&mut expected, &context).unwrap(),
                        frames
                    );
                    assert_eq!(
                        factory.process(&input, &mut actual, &context).unwrap(),
                        frames
                    );
                    assert_eq!(actual, expected, "{rate}/{channels}/{config}");
                    cursor += frames;
                }
            }
        }
    }
}
