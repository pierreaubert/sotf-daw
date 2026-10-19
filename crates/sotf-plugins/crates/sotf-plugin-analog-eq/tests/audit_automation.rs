//! Allocation and state preservation across the public Analog EQ control routes.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
    assert_no_allocs,
};
use sotf_plugin_analog_eq::AnalogEqPlugin;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn valid_live_scalar_and_bulk_updates_reuse_prepared_filters_and_metadata() {
    for channels in [1, 2, 8] {
        let mut p = AnalogEqPlugin::new(channels);
        p.initialize(48_000.0).unwrap();
        let updates: Vec<_> = [
            ("low_freq", 230.0),
            ("low_gain", 12.0),
            ("mid1_freq", 1100.0),
            ("mid1_gain", -9.0),
            ("mid1_q", 3.0),
            ("mid2_freq", 5700.0),
            ("mid2_gain", 8.0),
            ("mid2_q", 0.3),
            ("high_freq", 13000.0),
            ("high_gain", -6.0),
            ("analog_drive", 9.0),
            ("analog_color", 0.7),
            ("analog_character", 0.3),
            ("analog_trim", -3.0),
        ]
        .into_iter()
        .map(|(key, value)| (ParameterId::from(key), ParameterValue::Float(value)))
        .collect();
        for (id, value) in &updates {
            assert_no_allocs_or_deallocs("Analog EQ live scalar validation", || {
                p.parametric_validate_parameter(id, value).unwrap()
            });
            assert_no_allocs_or_deallocs("Analog EQ live scalar setter", || {
                p.set_parameter(id.clone(), value.clone()).unwrap()
            });
        }
        let batch = updates.iter().cloned().collect();
        assert_no_allocs("Analog EQ live bulk setter", || {
            p.apply_values(batch).unwrap()
        });
        let schema = p.parameter_schema();
        for (id, value) in &updates {
            assert_eq!(p.get_parameter(id).as_ref(), Some(value));
            assert_eq!(
                &schema.iter().find(|p| &p.id == id).unwrap().default_value,
                value
            );
        }
        let mut audio = vec![0.1; 257 * channels];
        assert_no_allocs_or_deallocs("Analog EQ processing after automation", || {
            p.process_in_place(&mut audio, &ProcessContext::new(48_000, 257))
                .unwrap();
        });
        assert!(audio.iter().all(|x| x.is_finite()));
        assert_no_allocs_or_deallocs("Analog EQ populated reset", || p.reset());
    }
}

#[test]
fn malformed_batches_preserve_current_values_metadata_and_populated_audio_history() {
    for model in sotf_plugin_analog_common::MODEL_NAMES {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for invalid in [
                vec![
                    ("analog_color", ParameterValue::Float(0.6)),
                    ("z_unknown", ParameterValue::Float(1.0)),
                ],
                vec![
                    ("low_gain", ParameterValue::Float(9.0)),
                    ("mid1_q", ParameterValue::Float(0.0)),
                ],
                vec![
                    ("low_gain", ParameterValue::Float(9.0)),
                    ("mid2_freq", ParameterValue::Float(f32::NAN)),
                ],
                vec![
                    ("analog_color", ParameterValue::Float(0.6)),
                    ("mid1_gain", ParameterValue::String("bad".into())),
                ],
                vec![
                    ("low_gain", ParameterValue::Float(9.0)),
                    (
                        "analog_model",
                        ParameterValue::String("No such model".into()),
                    ),
                ],
                vec![
                    ("low_gain", ParameterValue::Float(9.0)),
                    (
                        "analog_model",
                        ParameterValue::String(
                            if *model == "Tape" {
                                "Harmonics"
                            } else {
                                "Tape"
                            }
                            .into(),
                        ),
                    ),
                ],
            ] {
                let params = sotf_plugin_analog_eq::AnalogEqPluginParams {
                    analog_model: (*model).into(),
                    analog_color: 0.7,
                    analog_drive: 12.0,
                    mid1_gain: 9.0,
                    ..Default::default()
                };
                let mut p = AnalogEqPlugin::from_params(2, params.clone()).unwrap();
                let mut baseline = AnalogEqPlugin::from_params(2, params).unwrap();
                p.initialize(f64::from(rate)).unwrap();
                baseline.initialize(f64::from(rate)).unwrap();
                let source: Vec<f32> = (0..1026)
                    .map(|i| 0.3 * (i as f64 * 0.137).sin() as f32)
                    .collect();
                for plugin in [&mut p, &mut baseline] {
                    let mut warm = source.clone();
                    plugin
                        .process_in_place(&mut warm, &ProcessContext::new(rate, 513))
                        .unwrap();
                }
                let before = p.current_values();
                let schema = serde_json::to_value(p.parameter_schema()).unwrap();
                let batch = invalid
                    .into_iter()
                    .map(|(id, value)| (ParameterId::from(id), value))
                    .collect();
                assert!(p.apply_values(batch).is_err());
                assert_eq!(p.current_values(), before);
                assert_eq!(serde_json::to_value(p.parameter_schema()).unwrap(), schema);
                let mut a = source.clone();
                let mut b = source.clone();
                p.process_in_place(&mut a, &ProcessContext::new(rate, 513))
                    .unwrap();
                baseline
                    .process_in_place(&mut b, &ProcessContext::new(rate, 513))
                    .unwrap();
                assert!(
                    a == b,
                    "{rate} {model}: rejected batch changed filter/color history"
                );
            }
        }
    }
}

#[test]
fn full_live_snapshot_with_same_model_requires_no_new_allocation() {
    for model in sotf_plugin_analog_common::MODEL_NAMES {
        let params = sotf_plugin_analog_eq::AnalogEqPluginParams {
            analog_model: (*model).into(),
            analog_color: 0.7,
            ..Default::default()
        };
        let mut p = AnalogEqPlugin::from_params(2, params).unwrap();
        p.initialize(48_000.0).unwrap();
        let values = p.current_values();
        assert_no_allocs("Analog EQ unchanged live snapshot", || {
            p.apply_values(values).unwrap()
        });
    }
}

#[test]
fn repreparing_discards_populated_eq_and_color_history_at_each_rate() {
    for model in sotf_plugin_analog_common::MODEL_NAMES {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let params = sotf_plugin_analog_eq::AnalogEqPluginParams {
                analog_model: (*model).into(),
                analog_color: 0.7,
                analog_drive: 12.0,
                mid1_gain: 9.0,
                ..Default::default()
            };
            let mut p = AnalogEqPlugin::from_params(2, params.clone()).unwrap();
            p.initialize(48_000.0).unwrap();
            let mut history = vec![0.2; 4096];
            p.process_in_place(&mut history, &ProcessContext::new(48_000, 2048))
                .unwrap();
            p.initialize(f64::from(rate)).unwrap();
            let mut fresh = AnalogEqPlugin::from_params(2, params).unwrap();
            fresh.initialize(f64::from(rate)).unwrap();
            let mut a = vec![0.0; 514];
            a[0] = 0.1;
            a[3] = -0.037;
            let mut b = a.clone();
            p.process_in_place(&mut a, &ProcessContext::new(rate, 257))
                .unwrap();
            fresh
                .process_in_place(&mut b, &ProcessContext::new(rate, 257))
                .unwrap();
            let peak = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                a == b,
                "{model} {rate}: reinitialization retained old filter history, peak error {peak:e}"
            );
        }
    }
}
