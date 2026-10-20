//! Broadband and multiband identities agree across bridge and A/B fallback paths.

// Rust guideline compliant 2026-02-21
use serde_json::{Value, json};
use sotf_host::{ParametricInPlacePlugin, Plugin, ProcessContext};
use sotf_plugin_ab_compare::{
    GraphNodeConfig, PathConfig, PluginInRack, build_path_from_config_with_factory,
};
use sotf_plugin_multiband_compressor::{
    MultibandCompressorPlugin, MultibandCompressorPluginParams,
};

fn paths(kind: &str, params: &Value) -> [PathConfig; 3] {
    [
        PathConfig::Plugin {
            plugin_type: kind.into(),
            parameters: params.clone(),
        },
        PathConfig::Rack {
            plugins: vec![PluginInRack {
                plugin_type: kind.into(),
                parameters: params.clone(),
            }],
        },
        PathConfig::Graph {
            nodes: vec![GraphNodeConfig {
                id: "dynamics".into(),
                plugin_type: kind.into(),
                parameters: params.clone(),
            }],
            edges: vec![],
        },
    ]
}

#[test]
fn compressor_factory_audio_and_identity_match_explicit_band_construction() {
    let presets = [
        json!({}),
        json!({"num_bands": 5, "ratio": 1.0}),
        json!({"num_bands": 3, "ratio": 1.0, "per_band_lookahead_ms": 5.0}),
        json!({"num_bands": 3, "threshold_db": -24.0, "ratio": 4.0,
            "detection_mode": "RMS", "knee_db": 6.0, "link_amount": 0.5,
            "attack_ms": 1.0, "release_ms": 40.0}),
    ];
    let mut cases = 0;
    for rate in [44_100, 96_000] {
        for channels in [1, 2, 6] {
            for kind in ["compressor", "multiband_compressor"] {
                for params in &presets {
                    let mut native_params: MultibandCompressorPluginParams =
                        serde_json::from_value(params.clone()).unwrap();
                    if kind == "compressor" {
                        native_params.num_bands = 1;
                    }
                    let bands = native_params.num_bands;
                    for path in paths(kind, params) {
                        let mut host =
                            build_path_from_config_with_factory(&path, channels, rate, None)
                                .unwrap();
                        let mut bridge = plugins_bridge::create_plugin(
                            kind,
                            channels,
                            f64::from(rate),
                            &params.to_string(),
                        )
                        .unwrap();
                        bridge.initialize(f64::from(rate)).unwrap();
                        let mut native = MultibandCompressorPlugin::try_from_params(
                            channels,
                            native_params.clone(),
                            rate,
                        )
                        .unwrap();
                        native.initialize(f64::from(rate)).unwrap();
                        let expected_name = if bands == 1 {
                            "Compressor"
                        } else {
                            "Multiband Compressor"
                        };
                        assert_eq!(bridge.info().name, expected_name);
                        assert_eq!(host.get_plugin(0).unwrap().info().name, expected_name);
                        for plugin in [bridge.as_ref(), host.get_plugin(0).unwrap()] {
                            assert_eq!(plugin.latency_samples(), native.latency_samples());
                            let has_bands = plugin
                                .parameters()
                                .iter()
                                .any(|p| p.id.as_str() == "num_bands");
                            assert_eq!(has_bands, bands > 1);
                        }
                        let mut cursor = 0;
                        for frames in [1, 17, 257, 1024, 3, 257, 1024, 1024, 492] {
                            let input: Vec<_> = (0..frames * channels)
                                .map(|i| {
                                    let n = cursor + i / channels;
                                    let ch = i % channels;
                                    let phase = n as f64 / f64::from(rate) * std::f64::consts::TAU;
                                    ((0.25 * (phase * 110.0).sin() + 0.1 * (phase * 6000.0).cos())
                                        / (ch + 1) as f64)
                                        as f32
                                })
                                .collect();
                            let context = ProcessContext::new(rate, frames);
                            let mut expected = input.clone();
                            native.process_in_place(&mut expected, &context).unwrap();
                            let mut actual = vec![f32::NAN; input.len()];
                            assert_eq!(
                                bridge.process(&input, &mut actual, &context).unwrap(),
                                frames
                            );
                            assert_eq!(actual, expected, "bridge {kind}/{rate}/{channels}");
                            assert_eq!(host.process(&input, &mut actual).unwrap(), frames);
                            assert_eq!(actual, expected, "fallback {kind}/{rate}/{channels}");
                            assert!(actual.iter().all(|x| x.is_finite()));
                            cursor += frames;
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 144);
}

#[test]
fn compressor_factories_reject_invalid_dynamics_before_path_publication() {
    for kind in ["compressor", "multiband_compressor"] {
        for params in [
            json!({"ratio": 0.0}),
            json!({"program_dependent_release": true}),
        ] {
            assert!(plugins_bridge::create_plugin(kind, 2, 48_000.0, &params.to_string()).is_err());
            for path in paths(kind, &params) {
                assert!(build_path_from_config_with_factory(&path, 2, 48_000, None).is_err());
            }
        }
    }
}

fn external_override(
    _: &str,
    _: &Value,
    channels: usize,
    rate: f64,
) -> Result<Box<dyn Plugin>, String> {
    plugins_bridge::create_plugin("gain", channels, rate, "{\"gain_db\":0.0}")
}

#[test]
fn injected_ab_factory_retains_control_over_compressor_identity() {
    for path in paths("compressor", &json!({"ratio": 0.0})) {
        let mut host =
            build_path_from_config_with_factory(&path, 2, 48_000, Some(external_override)).unwrap();
        assert_eq!(host.get_plugin(0).unwrap().info().name, "Gain");
        let input = [0.125, -0.25, 0.5, -0.5];
        let mut output = [f32::NAN; 4];
        assert_eq!(host.process(&input, &mut output).unwrap(), 2);
        assert_eq!(input, output);
    }
}
