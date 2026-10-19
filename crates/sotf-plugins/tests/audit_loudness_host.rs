//! Factory calibration provenance and host bypass/gain behavior.

// Rust guideline compliant 2026-02-21
use sotf_plugins::plugin_loudness_compensation::LoudnessCompensationData;
use sotf_plugins::{DawHost, ParameterId, ParameterValue, create_plugin};

#[test]
fn legacy_alias_exposes_assumed_reference_and_matches_explicit_auto_contour() {
    for rate in [16_000, 48_000, 192_000] {
        let mut legacy = create_plugin(
            "fletcher_munson",
            &serde_json::json!({
                "enabled":true,"reference_level_db":-10.0,"playback_volume_db":-18.0,
                "auto_gain_enabled":true,"smoothing_ms":10.0
            }),
            2,
            rate,
        )
        .unwrap();
        let mut calibrated = create_plugin(
            "loudness_compensation",
            &serde_json::json!({
                "mode":2,"reference_level_db":73.0,"playback_volume_db":-18.0,"auto_calibrated":true,
                "auto_gain_enabled":true,"auto_gain_smoothing_ms":10.0
            }),
            2,
            rate,
        )
        .unwrap();
        for plugin in [&mut legacy, &mut calibrated] {
            plugin.initialize(f64::from(rate)).unwrap();
        }
        let assumed = |plugin: &dyn sotf_plugins::Plugin| {
            plugin
                .get_data()
                .unwrap()
                .downcast_ref::<LoudnessCompensationData>()
                .unwrap()
                .calibration_is_assumed
        };
        assert!(assumed(legacy.as_ref()));
        assert!(!assumed(calibrated.as_ref()));
        let mut input = vec![0.0; 8192];
        input[0] = 0.1;
        let mut actual = vec![0.0; input.len()];
        let mut expected = actual.clone();
        let context = sotf_plugins::ProcessContext::new(rate, input.len() / 2);
        legacy.process(&input, &mut actual, &context).unwrap();
        calibrated.process(&input, &mut expected, &context).unwrap();
        assert!(
            actual == expected,
            "legacy mapping must preserve the explicit assumed contour"
        );
        assert!(
            assumed(legacy.as_ref()),
            "gain-meter publications preserve calibration provenance"
        );
        legacy.reset();
        assert!(assumed(legacy.as_ref()));
        legacy
            .set_parameter(
                ParameterId::from("auto_calibrated"),
                ParameterValue::Bool(true),
            )
            .unwrap();
        assert!(
            !assumed(legacy.as_ref()),
            "explicit confirmation clears legacy provenance"
        );
    }
}

#[test]
fn host_bypass_is_exact_while_pre_and_post_gain_modes_resume_finitely() {
    for position in ["disabled", "pre", "post"] {
        let mut host = DawHost::new(2, 48_000);
        host.add_plugin(
            create_plugin(
                "loudness_compensation",
                &serde_json::json!({
                    "mode":0,"low_gain":0.0,"high_gain":0.0,"mid_enabled":true,
                    "mid_freq":1000.0,"mid_gain":6.0,
                    "auto_gain_enabled":position!="disabled","auto_gain_position":position
                }),
                2,
                48_000,
            )
            .unwrap(),
        )
        .unwrap();
        host.build().unwrap();
        let input: Vec<_> = (0..48000)
            .flat_map(|i| {
                let sample =
                    (std::f64::consts::TAU * 1000.0 * i as f64 / 48000.0).sin() as f32 * 0.1;
                [sample, sample * 0.7]
            })
            .collect();
        let mut output = vec![0.0; input.len()];
        host.process(&input, &mut output).unwrap();
        host.bypass_plugin(0).unwrap();
        host.process(&input, &mut output).unwrap();
        assert!(
            output == input,
            "{position}: bypass must not apply EQ or AutoGain"
        );
        host.unbypass_plugin(0).unwrap();
        host.process(&input, &mut output).unwrap();
        assert!(output.iter().all(|x| x.is_finite() && x.abs() < 0.5));
        let energy = |samples: &[f32]| samples.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
        let start = 36000 * 2;
        let gain = 10.0 * (energy(&output[start..]) / energy(&input[start..])).log10();
        let expected = if position == "disabled" { 6.0 } else { 0.0 };
        assert!(
            (gain - expected).abs() < 0.05,
            "{position}: resumed EQ/AutoGain level {gain} != {expected}"
        );
    }
}
