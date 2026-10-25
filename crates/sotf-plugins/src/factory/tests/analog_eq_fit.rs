//! Reopen a fitted Analog EQ preset through the production host factory.

// Rust guideline compliant 2026-02-21
use super::create_plugin;
use sotf_host::{ParametricInPlacePluginAdapter, Plugin, ProcessContext};
use sotf_plugin_analog_eq::{AnalogEqPlugin, AnalogEqPluginParams};

#[test]
fn analog_eq_fitted_export_reopens_through_factory_with_identical_streaming_output() {
    let exported: serde_json::Value = serde_json::from_str(include_str!(
        "../../../models/analog-eq-fit/preset.synthetic.json"
    ))
    .unwrap();
    let reopened: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&exported).unwrap()).unwrap();
    assert_eq!(reopened["plugin_type"], "analog_eq");
    let parameters = &reopened["parameters"];
    assert_eq!(parameters.as_object().unwrap().len(), 15);
    assert_eq!(parameters["analog_model"], "Static");
    let state: AnalogEqPluginParams = serde_json::from_value(parameters.clone()).unwrap();
    for rate in [44_100, 48_000, 96_000, 192_000] {
        let mut reference = ParametricInPlacePluginAdapter::new(
            AnalogEqPlugin::from_params(2, state.clone()).unwrap(),
        );
        let mut hosted = create_plugin("analog_eq", parameters, 2, rate).unwrap();
        reference.initialize(f64::from(rate)).unwrap();
        hosted.initialize(f64::from(rate)).unwrap();
        assert_eq!((hosted.input_channels(), hosted.output_channels()), (2, 2));
        assert_eq!(hosted.latency_samples(), 0);
        // An odd active segment and explicit zero suffix exercise state and final callbacks.
        let frames = 12_289;
        let mut input = vec![0.0_f32; frames * 2];
        for (i, value) in input[..8193 * 2].iter_mut().enumerate() {
            *value = ((i * 13 % 79) as f32 - 39.0) / 2048.0;
        }
        let mut expected = vec![f32::NAN; input.len()];
        reference
            .process(&input, &mut expected, &ProcessContext::new(rate, frames))
            .unwrap();
        let mut actual = vec![f32::NAN; input.len()];
        for start in (0..frames).step_by(17) {
            let count = (frames - start).min(17);
            assert_eq!(
                hosted
                    .process(
                        &input[start * 2..(start + count) * 2],
                        &mut actual[start * 2..(start + count) * 2],
                        &ProcessContext::new(rate, count).with_sample_position(start as u64),
                    )
                    .unwrap(),
                count
            );
        }
        assert!(actual.iter().all(|value| value.is_finite()));
        assert_eq!(actual, expected);
        assert!(actual.iter().zip(&input).any(|(a, b)| (a - b).abs() > 1e-4));
    }
}
