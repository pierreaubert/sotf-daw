// Rust guideline compliant 2026-02-21

use sotf_host::param_specs::UpdateMode;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};

fn isp_limiter(lookahead_ms: f32, enabled: bool) -> LimiterPlugin {
    let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
        "threshold_db": -6.0,
        "lookahead_ms": lookahead_ms,
        "isp_mode": enabled,
    }))
    .unwrap();
    LimiterPlugin::from_params(2, params)
}

#[test]
fn isp_requires_the_detector_delay_at_the_initialized_sample_rate() {
    for (sample_rate, lookahead_ms, should_initialize) in [
        (48_000, 0.10, false),
        (48_000, 0.13, true),
        (96_000, 0.10, false),
        (96_000, 0.13, true),
        (192_000, 0.0, true),
    ] {
        let mut plugin = isp_limiter(lookahead_ms, true);
        let result = plugin.initialize(sample_rate);
        assert_eq!(
            result.is_ok(),
            should_initialize,
            "rate={sample_rate}, lookahead={lookahead_ms}: {result:?}"
        );
    }
}

#[test]
fn isp_latency_changes_require_a_graph_rebuild() {
    for sample_rate in [44_100, 48_000, 96_000] {
        for enabled in [false, true] {
            let mut plugin = isp_limiter(5.0, enabled);
            plugin.initialize(sample_rate).unwrap();
            let latency = plugin.latency_samples();
            let result = plugin.set_parameter(
                ParameterId::from("isp_mode"),
                ParameterValue::Bool(!enabled),
            );
            assert!(result.is_err());
            assert_eq!(plugin.latency_samples(), latency);
            assert_eq!(
                plugin.get_parameter(&ParameterId::from("isp_mode")),
                Some(ParameterValue::Bool(enabled))
            );
            let schema = plugin.parameter_schema();
            let parameter = schema
                .iter()
                .find(|parameter| parameter.id == ParameterId::from("isp_mode"))
                .unwrap();
            assert_eq!(parameter.update_mode, UpdateMode::Structural);
        }
    }
}

#[test]
fn isp_output_delay_matches_reported_latency_and_reset() {
    for (sample_rate, correction_delay) in [(44_100, 18), (48_000, 18), (96_000, 36), (192_000, 0)]
    {
        let mut plugin = isp_limiter(5.0, true);
        plugin.initialize(sample_rate).unwrap();
        let input_delay = (5.0 * 0.001 * sample_rate as f32) as usize;
        let latency = input_delay + correction_delay;
        assert_eq!(plugin.latency_samples(), latency);
        assert_eq!(plugin.compile_metadata().latency_samples, latency);
        let mut input = vec![0.0; 2 * (latency + 33)];
        input[0] = 0.1;
        input[1] = -0.025;
        let mut expected = vec![0.0; input.len()];
        expected[2 * latency] = input[0];
        expected[2 * latency + 1] = input[1];
        for _ in 0..2 {
            let mut output = input.clone();
            for block in output.chunks_mut(34) {
                plugin
                    .process_in_place(block, &ProcessContext::new(sample_rate, block.len() / 2))
                    .unwrap();
            }
            assert_eq!(
                output, expected,
                "ISP stage must preserve samples below the ceiling with declared delay"
            );
            plugin.reset();
        }
    }
}
