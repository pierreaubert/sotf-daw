//! Expander factories validate before publication and preserve route identity.

// Rust guideline compliant 2026-02-21
use serde_json::json;
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext};
use sotf_plugin_multiband_expander::{MultibandExpanderPlugin, MultibandExpanderPluginParams};

#[test]
fn bridge_expander_routes_match_explicit_native_audio_and_tail_contracts() {
    let presets = [
        ("Expander", json!({})),
        (
            "expander",
            json!({"num_bands":5,"processing_mode":"spectral",
            "detection_mode":"RMS","lookahead_ms":5.0,"sidechain_hpf_hz":0.0}),
        ),
        (
            "expander",
            json!({"detection_mode":"RMS","mix":0.5,"sidechain_hpf_hz":120.0}),
        ),
        ("MultibandExpander", json!({})),
        ("multiband_expander", json!({"num_bands":5,"ratio":1.0})),
        (
            "multiband_expander",
            json!({"num_bands":2,"threshold_db":-48.0,
            "hysteresis_db":3.0,"hold_ms":10.0,"range_db":12.0}),
        ),
        (
            "multiband_expander",
            json!({"num_bands":3,"processing_mode":"Spectral",
            "sidechain_hpf_hz":0.0,"detection_mode":"Peak","link_channels":true}),
        ),
    ];
    let mut cases = 0;
    for rate in [44_100, 96_000, 192_000] {
        for channels in [1, 2, 6] {
            for (kind, config) in &presets {
                let broadband = kind.eq_ignore_ascii_case("expander");
                let mut params: MultibandExpanderPluginParams =
                    serde_json::from_value(config.clone()).unwrap();
                if broadband {
                    params.num_bands = 1;
                    params.processing_mode = "time_domain".into();
                }
                let native =
                    MultibandExpanderPlugin::try_from_params(channels, params, rate).unwrap();
                let mut native = ParametricInPlacePluginAdapter::new(native);
                let mut bridge = plugins_bridge::create_plugin(
                    kind,
                    channels,
                    f64::from(rate),
                    &config.to_string(),
                )
                .unwrap();
                native.initialize(f64::from(rate)).unwrap();
                bridge.initialize(f64::from(rate)).unwrap();
                let expected_name = if broadband {
                    "Expander"
                } else {
                    "Multiband Expander"
                };
                assert_eq!(bridge.info().name, expected_name);
                assert_eq!(bridge.latency_samples(), native.latency_samples());
                assert_eq!(bridge.tail_length(), native.tail_length());
                let has_bands = bridge
                    .parameters()
                    .iter()
                    .any(|p| p.id.as_str() == "num_bands");
                assert_eq!(has_bands, !broadband);
                let mut cursor = 0;
                for frames in [1, 17, 137, 1024, 3, 257, 1024, 1024, 1024] {
                    let input: Vec<_> = (0..frames * channels)
                        .map(|i| {
                            let n = cursor + i / channels;
                            let phase = std::f64::consts::TAU * n as f64 / f64::from(rate);
                            ((0.01 * (phase * 110.0).sin() + 0.002 * (phase * 6000.0).cos())
                                / (i % channels + 1) as f64) as f32
                        })
                        .collect();
                    let mut actual = vec![f32::NAN; input.len()];
                    let mut expected = actual.clone();
                    let context = ProcessContext::new(rate, frames);
                    assert_eq!(
                        bridge.process(&input, &mut actual, &context).unwrap(),
                        frames
                    );
                    assert_eq!(
                        native.process(&input, &mut expected, &context).unwrap(),
                        frames
                    );
                    assert_eq!(actual, expected, "{kind}/{rate}/{channels}/{config}");
                    cursor += frames;
                }
                let capacity = bridge.drain_output_frames_max();
                assert_eq!(capacity, native.drain_output_frames_max());
                let mut actual = vec![f32::NAN; capacity * channels];
                let mut expected = actual.clone();
                let mut complete = false;
                for _ in 0..32 {
                    let context = ProcessContext::new(rate, 0);
                    let a = bridge.drain(&mut actual, &context).unwrap();
                    let b = native.drain(&mut expected, &context).unwrap();
                    assert_eq!((a.frames, a.complete), (b.frames, b.complete));
                    assert_eq!(
                        &actual[..a.frames * channels],
                        &expected[..b.frames * channels]
                    );
                    if a.complete {
                        complete = true;
                        break;
                    }
                }
                assert!(complete);
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 63);
}

#[test]
fn bridge_refuses_invalid_expander_presets_and_unsupported_spectral_controls() {
    for kind in [
        "Expander",
        "expander",
        "MultibandExpander",
        "multiband_expander",
    ] {
        for config in [
            json!({"ratio":0.9}),
            json!({"attack_ms":0.0}),
            json!({"range_db":-1.0}),
            json!({"knee_db":30.0}),
            json!({"hold_ms":-1.0}),
        ] {
            assert!(
                plugins_bridge::create_plugin(kind, 2, 48_000.0, &config.to_string()).is_err(),
                "{kind}/{config}"
            );
        }
    }
    for config in [
        json!({"num_bands":1}),
        json!({"num_bands":6}),
        json!({"processing_mode":"spectral","sidechain_hpf_hz":0.0,"detection_mode":"RMS"}),
        json!({"processing_mode":"spectral","sidechain_hpf_hz":0.0,"lookahead_ms":1.0}),
        json!({"processing_mode":"spectral","sidechain_hpf_hz":80.0}),
        json!({"processing_mode":"spectral","sidechain_hpf_hz":0.0,"link_channels":false}),
    ] {
        assert!(
            plugins_bridge::create_plugin("multiband_expander", 2, 48_000.0, &config.to_string())
                .is_err(),
            "{config}"
        );
    }
}

#[test]
fn native_and_bridge_reject_invalid_expander_construction_geometry() {
    for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let p = MultibandExpanderPluginParams {
            num_bands: 1,
            ..Default::default()
        };
        assert!(
            MultibandExpanderPlugin::try_from_params(2, p, rate).is_err(),
            "native rate{rate}"
        );
        assert!(
            plugins_bridge::create_plugin("expander", 2, rate, "{}").is_err(),
            "bridge rate{rate}"
        );
    }
    assert!(plugins_bridge::create_plugin("expander", 0, 48_000.0, "{}").is_err());
}
