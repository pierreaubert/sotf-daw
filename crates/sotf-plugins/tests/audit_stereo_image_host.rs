//! Actual factory presets, queued host controls and transparent bypass.

// Rust guideline compliant 2026-02-21
use sotf_plugins::{DawHost, ParameterId, ParameterValue, ProcessContext, create_plugin};

#[test]
fn preset_reload_and_queued_controls_match_the_native_factory_path() {
    for rate in [16_000, 48_000, 192_000] {
        let preset = serde_json::json!({
            "width":1.6,"low_mid_freq":120.0,"mid_high_freq":6000.0,
            "low_width":0.2,"mid_width":1.2,"high_width":0.7,"mono_bass":true,"mix":0.37
        });
        let saved = serde_json::to_string(&preset).unwrap();
        let reloaded = serde_json::from_str(&saved).unwrap();
        let mut direct = create_plugin("stereo_imager", &preset, 2, rate).unwrap();
        direct.initialize(f64::from(rate)).unwrap();
        let mut host = DawHost::new(2, rate);
        host.add_plugin(create_plugin("stereo_imager", &reloaded, 2, rate).unwrap())
            .unwrap();
        host.build().unwrap();
        let input: Vec<f32> = (0..4096)
            .flat_map(|i| {
                let mid = (i as f32 * 0.131).sin() * 0.1;
                let side = (i as f32 * 0.031).cos() * 0.2;
                [mid + side, mid - side]
            })
            .collect();
        let mut expected = vec![0.0; input.len()];
        let mut actual = expected.clone();
        for (key, value) in [
            ("mix", ParameterValue::Float(0.0)),
            ("low_mid_freq", ParameterValue::Float(900.0)),
            ("mono_bass", ParameterValue::Bool(false)),
            ("mix", ParameterValue::Float(1.0)),
            ("width", ParameterValue::Float(0.0)),
            ("width", ParameterValue::Float(1.0)),
        ] {
            // This invalid candidate must leave the accepted preset and filter
            // history untouched on both the native and queued host paths.
            assert!(
                host.set_plugin_parameter_immediate(0, "width", ParameterValue::Float(3.0))
                    .is_err()
            );
            assert!(
                direct
                    .set_parameter(ParameterId::from("width"), ParameterValue::Float(3.0))
                    .is_err()
            );
            host.set_plugin_parameter(0, key, value.clone()).unwrap();
            direct.set_parameter(ParameterId::from(key), value).unwrap();
            direct
                .process(&input, &mut expected, &ProcessContext::new(rate, 4096))
                .unwrap();
            host.process(&input, &mut actual).unwrap();
            let error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                actual == expected,
                "{rate} {key}: host/factory mismatch {error}"
            );
            for (out, src) in actual
                .as_chunks::<2>()
                .0
                .iter()
                .zip(input.as_chunks::<2>().0.iter())
            {
                assert!((out[0] + out[1] - src[0] - src[1]).abs() < 2e-7);
            }
        }
        host.bypass_plugin(0).unwrap();
        host.process(&input, &mut actual).unwrap();
        assert!(actual == input, "host bypass must be exact");
        host.unbypass_plugin(0).unwrap();
        host.process(&input, &mut actual).unwrap();
        assert!(actual.iter().all(|x| x.is_finite() && x.abs() < 1.0));
        for (out, src) in actual
            .as_chunks::<2>()
            .0
            .iter()
            .zip(input.as_chunks::<2>().0.iter())
        {
            assert!((out[0] + out[1] - src[0] - src[1]).abs() < 2e-7);
        }
    }
}
