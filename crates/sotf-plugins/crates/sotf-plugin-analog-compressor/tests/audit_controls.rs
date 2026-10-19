//! Live control allocation and stream preparation for the analog compressor.

// Rust guideline compliant 2026-02-21
use sotf_host::test_utils::{assert_no_allocs, assert_no_allocs_or_deallocs};
use sotf_host::{
    CountingAlloc, ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext,
};
use sotf_plugin_analog_common::MODEL_NAMES;
use sotf_plugin_analog_compressor::{AnalogCompressorPlugin, AnalogCompressorPluginParams};
#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[test]
fn every_numeric_and_boolean_live_control_reuses_prepared_state() {
    for model in MODEL_NAMES {
        let mut p = AnalogCompressorPlugin::from_params(
            2,
            AnalogCompressorPluginParams {
                analog_model: (*model).into(),
                ..Default::default()
            },
        )
        .unwrap();
        p.initialize(48_000.0).unwrap();
        let mut updates: Vec<_> = [
            ("threshold", -24.0),
            ("ratio", 8.0),
            ("attack", 3.0),
            ("release", 80.0),
            ("knee", 9.0),
            ("range_db", 12.0),
            ("hold_ms", 7.5),
            ("makeup", 2.0),
            ("mix", 0.6),
            ("analog_drive", 12.0),
            ("analog_color", 0.7),
            ("analog_character", 0.3),
            ("analog_trim", -3.0),
        ]
        .into_iter()
        .map(|(id, value)| (ParameterId::from(id), ParameterValue::Float(value)))
        .collect();
        updates.push((ParameterId::from("auto_makeup"), ParameterValue::Bool(true)));
        for (id, value) in &updates {
            assert_no_allocs_or_deallocs("Analog Compressor live validation", || {
                p.parametric_validate_parameter(id, value).unwrap()
            });
            assert_no_allocs_or_deallocs("Analog Compressor live scalar control", || {
                p.set_parameter(id.clone(), value.clone()).unwrap()
            });
        }
        let values = updates.iter().cloned().collect();
        assert_no_allocs("Analog Compressor bulk control", || {
            p.apply_values(values).unwrap()
        });
        let schema = p.parameter_schema();
        for (id, value) in &updates {
            assert_eq!(p.get_parameter(id).as_ref(), Some(value));
            assert_eq!(
                &schema.iter().find(|p| p.id == *id).unwrap().default_value,
                value
            );
        }
        let snapshot = p.current_values();
        assert_no_allocs("Analog Compressor canonical snapshot", || {
            p.apply_values(snapshot).unwrap()
        });
        let mut audio = [0.1; 514];
        assert_no_allocs_or_deallocs("Analog Compressor processed controls/reset", || {
            p.process_in_place(&mut audio, &ProcessContext::new(48_000, 257))
                .unwrap();
            p.reset();
        });
    }
}
#[test]
fn preparation_clears_the_previous_detector_and_color_epoch() {
    for model in MODEL_NAMES {
        for rate in [44_100, 48_000, 96_000, 192_000] {
            let params = AnalogCompressorPluginParams {
                analog_model: (*model).into(),
                analog_color: 0.7,
                analog_drive: 12.0,
                auto_makeup: true,
                threshold: -30.0,
                attack: 1.0,
                release: 100.0,
                ..Default::default()
            };
            let mut used = AnalogCompressorPlugin::from_params(2, params.clone()).unwrap();
            used.initialize(48_000.0).unwrap();
            let mut history = vec![0.8; 8192];
            used.process_in_place(&mut history, &ProcessContext::new(48_000, 4096))
                .unwrap();
            used.initialize(f64::from(rate)).unwrap();
            let mut fresh = AnalogCompressorPlugin::from_params(2, params).unwrap();
            fresh.initialize(f64::from(rate)).unwrap();
            let mut a = vec![0.02; 514];
            let mut b = a.clone();
            used.process_in_place(&mut a, &ProcessContext::new(rate, 257))
                .unwrap();
            fresh
                .process_in_place(&mut b, &ProcessContext::new(rate, 257))
                .unwrap();
            let error = a
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(
                a == b,
                "{rate} {model}: preparation retained previous detector state, error {error:e}"
            );
        }
    }
}
