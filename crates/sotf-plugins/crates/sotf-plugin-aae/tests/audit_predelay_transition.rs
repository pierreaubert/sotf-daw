//! Predelay automation must not boost fully correlated delay taps.
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_aae::{AaePlugin, params::AaePluginParams};

#[test]
fn correlated_predelay_change_preserves_the_steady_wet_signal() {
    for rate in [44_100_u32, 48_000, 96_000] {
        let make = || {
            let mut plugin = AaePlugin::try_from_params(AaePluginParams {
                pre_delay_ms: 5.0,
                dry_level: 0.0,
                er_level: 0.3,
                late_level: 0.0,
                mod_depth: 0.0,
                er_mod_depth: 0.0,
                auto_gain_enabled: false,
                ..Default::default()
            })
            .unwrap();
            plugin.initialize(f64::from(rate)).unwrap();
            plugin
        };
        let mut changed = make();
        let mut reference = make();
        let input = [0.01; 512 * 2];
        let mut actual = vec![0.0_f32; 512 * changed.output_channels()];
        let mut expected = actual.clone();
        let context = ProcessContext::new(rate, 512);
        for _ in 0..(rate as usize * 2).div_ceil(512) {
            changed.process(&input, &mut actual, &context).unwrap();
            reference.process(&input, &mut expected, &context).unwrap();
        }
        changed
            .set_parameter(
                ParameterId::from("pre_delay_ms"),
                ParameterValue::Float(50.0),
            )
            .unwrap();
        let mut max_error = 0.0_f32;
        let mut reference_peak = 0.0_f32;
        for _ in 0..(rate as usize / 2).div_ceil(512) {
            changed.process(&input, &mut actual, &context).unwrap();
            reference.process(&input, &mut expected, &context).unwrap();
            for (&a, &b) in actual.iter().zip(&expected) {
                max_error = max_error.max((a - b).abs());
                reference_peak = reference_peak.max(b.abs());
            }
        }
        assert!(reference_peak > 1e-5, "the wet path must be exercised");
        assert!(
            max_error < 1e-6,
            "rate={rate}: correlated transition error {max_error}, wet peak {reference_peak}"
        );
    }
}
