//! Transactional color/tail metadata and allocation-free limiter automation.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::{assert_no_allocs, assert_no_allocs_or_deallocs};
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_analog_common::MODEL_NAMES;
use sotf_plugin_analog_limiter::{AnalogLimiterPlugin, AnalogLimiterPluginParams};
#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

fn make(rate: u32, model: &str) -> AnalogLimiterPlugin {
    let mut p = AnalogLimiterPlugin::from_params(
        2,
        AnalogLimiterPluginParams {
            threshold: -12.0,
            lookahead: 0.37,
            analog_model: model.into(),
            analog_color: 0.0,
            ..Default::default()
        },
    )
    .unwrap();
    p.initialize(f64::from(rate)).unwrap();
    p
}
#[test]
fn rejected_structural_batches_preserve_zero_color_eof_proof_and_audio() {
    for model in MODEL_NAMES {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            for invalid in [
                vec![
                    ("analog_color", ParameterValue::Float(0.7)),
                    ("lookahead", ParameterValue::Float(1.0)),
                ],
                vec![
                    ("analog_drive", ParameterValue::Float(12.0)),
                    ("lookahead", ParameterValue::Float(1.0)),
                ],
                vec![
                    ("analog_color", ParameterValue::Float(0.7)),
                    ("threshold", ParameterValue::Float(f32::NAN)),
                ],
                vec![
                    ("analog_color", ParameterValue::Float(0.7)),
                    ("z_unknown", ParameterValue::Float(1.0)),
                ],
            ] {
                let mut a = make(rate, model);
                let mut b = make(rate, model);
                let source: Vec<f32> = (0..1026)
                    .map(|i| 0.3 * (i as f64 * 0.137).sin() as f32)
                    .collect();
                for p in [&mut a, &mut b] {
                    let mut warm = source.clone();
                    p.process_in_place(&mut warm, &ProcessContext::new(rate, 513))
                        .unwrap();
                }
                let before = a.current_values();
                let tail = a.tail_length();
                let latency = a.latency_samples();
                let batch = invalid
                    .into_iter()
                    .map(|(id, value)| (ParameterId::from(id), value))
                    .collect();
                assert!(a.apply_values(batch).is_err());
                assert_eq!(
                    a.current_values(),
                    before,
                    "{rate} {model}: rejected batch changed controls"
                );
                assert_eq!(
                    a.tail_length(),
                    tail,
                    "rejected batch invalidated finite color epoch"
                );
                assert_eq!(a.latency_samples(), latency);
                let mut actual = source.clone();
                let mut expected = source.clone();
                a.process_in_place(&mut actual, &ProcessContext::new(rate, 513))
                    .unwrap();
                b.process_in_place(&mut expected, &ProcessContext::new(rate, 513))
                    .unwrap();
                assert!(
                    actual == expected,
                    "{rate} {model}: rejected batch changed populated audio"
                );
            }
        }
    }
}
#[test]
fn scalar_validation_updates_and_current_model_snapshots_reuse_storage() {
    for model in MODEL_NAMES {
        let mut p = make(48_000, model);
        let mut updates: Vec<_> = [
            ("threshold", -18.0),
            ("release", 80.0),
            ("mix", 0.7),
            ("analog_drive", 12.0),
            ("analog_color", 0.7),
            ("analog_character", 0.3),
            ("analog_trim", -3.0),
        ]
        .into_iter()
        .map(|(id, value)| (ParameterId::from(id), ParameterValue::Float(value)))
        .collect();
        updates.push((ParameterId::from("soft"), ParameterValue::Bool(true)));
        updates.push((ParameterId::from("true_peak"), ParameterValue::Bool(true)));
        for (id, value) in &updates {
            assert_no_allocs_or_deallocs("Analog Limiter live validation", || {
                p.parametric_validate_parameter(id, value).unwrap()
            });
            assert_no_allocs_or_deallocs("Analog Limiter live scalar setter", || {
                p.set_parameter(id.clone(), value.clone()).unwrap()
            });
        }
        let values = updates.into_iter().collect();
        assert_no_allocs("Analog Limiter bulk numeric/bool setter", || {
            p.apply_values(values).unwrap()
        });
        let mut values = p.current_values();
        values.remove(&ParameterId::from("lookahead"));
        assert_no_allocs("Analog Limiter canonical live snapshot", || {
            p.apply_values(values).unwrap()
        });
        let mut audio = [0.1; 514];
        assert_no_allocs_or_deallocs("Analog Limiter populated process/reset", || {
            p.process_in_place(&mut audio, &ProcessContext::new(48_000, 257))
                .unwrap();
            p.reset();
        });
    }
}
