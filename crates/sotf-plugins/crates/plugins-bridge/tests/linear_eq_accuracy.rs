//! Ordinary factory resolves automatic FIR support at the actual device clock.

// Rust guideline compliant 2026-02-21
use plugins_bridge::create_plugin;
use sotf_host::{ParameterId, ParameterValue, ProcessContext};
use sotf_plugin_linear_phase_eq::LinearPhaseEqDynamicPlugin;

#[test]
fn ordinary_factory_resolves_96k_automatic_support_before_reporting_latency() {
    for phase in [0, 1] {
        let config = serde_json::json!({
            "num_filters":3,"fir_length_index":0,"accuracy_mode":true,"phase_mode_index":phase,
            "filters":[
                {"filter_type":"Peak","frequency":500.0,"q":2.0,"gain_db":9.0},
                {"filter_type":"Peak","frequency":2000.0,"q":1.5,"gain_db":-6.0},
                {"filter_type":"Lowshelf","frequency":150.0,"gain_db":3.0}
            ]
        });
        let mut plugin = create_plugin("LinearPhaseEQ", 2, 96_000.0, &config.to_string()).unwrap();
        let dynamic = plugin
            .as_any()
            .unwrap()
            .downcast_ref::<LinearPhaseEqDynamicPlugin>()
            .unwrap();
        let report = dynamic.inner().accuracy_report().unwrap();
        assert_eq!(report.sample_rate, 96_000.0);
        assert_eq!(
            report.manual_latency_samples,
            if phase == 0 { 544 } else { 32 }
        );
        assert_eq!(
            report.additional_latency_samples,
            report
                .latency_samples
                .saturating_sub(report.manual_latency_samples)
        );
        if phase == 0 {
            assert!(report.additional_latency_samples > 0);
        }
        println!(
            "automatic_factory rate={} phase={phase} taps={} total_latency={} manual_latency={} additional_latency={}",
            report.sample_rate,
            report.selected_taps,
            report.latency_samples,
            report.manual_latency_samples,
            report.additional_latency_samples
        );
        assert_eq!(plugin.latency_samples(), report.latency_samples);
        assert_eq!(plugin.signal_delay_samples(), report.latency_samples as f64);
        assert_eq!(
            dynamic.snapshot_for_update().fir_length_index,
            report.selected_taps.ilog2() as usize - 10
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("fir_length_index")),
            Some(ParameterValue::Int(0))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("accuracy_mode")),
            Some(ParameterValue::Bool(true))
        );
        plugin.initialize(96_000.0).unwrap();
        assert_eq!(plugin.latency_samples(), report.latency_samples);
        let input = vec![0.0; 2 * 127];
        let mut output = vec![1.0; input.len()];
        assert_eq!(
            plugin
                .process(&input, &mut output, &ProcessContext::new(96_000, 127))
                .unwrap(),
            127
        );
        assert!(output.iter().all(|value| value.is_finite()));
        let before = plugin.latency_samples();
        assert!(plugin.initialize(48_000.0).unwrap_err().contains("rebuild"));
        assert_eq!(plugin.latency_samples(), before);
    }
}
