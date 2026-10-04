//! Per-frequency reduction curve and channel-link tests.
//!
//! Curve interpolation is checked against a closed-form log-frequency
//! oracle (worst-case error below 1e-6); link and trigger registry behavior
//! is asserted exactly.

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_host::plugin_params::PluginParamDef;
use sotf_plugin_hiss_reducer::params::{PARAMS, Params};
use sotf_plugin_hiss_reducer::profile::{
    CURVE_ANCHOR_HZ, LINK_INDEPENDENT, LINK_LINKED, ReductionCurve,
};
use sotf_plugin_hiss_reducer::{HissReducerPlugin, HissReducerPluginParams};

const RATE: u32 = 48_000;

fn closed_form_gain(curve: &ReductionCurve, freq_hz: f64) -> f64 {
    let anchors = [
        f64::from(CURVE_ANCHOR_HZ[0]),
        f64::from(CURVE_ANCHOR_HZ[1]),
        f64::from(CURVE_ANCHOR_HZ[2]),
    ];
    let gains = [
        f64::from(curve.low),
        f64::from(curve.mid),
        f64::from(curve.high),
    ];
    if freq_hz <= anchors[0] {
        return gains[0];
    }
    if freq_hz >= anchors[2] {
        return gains[2];
    }
    let segment = if freq_hz < anchors[1] { 0 } else { 1 };
    let position = (freq_hz / anchors[segment]).ln() / (anchors[segment + 1] / anchors[segment]).ln();
    gains[segment] + position * (gains[segment + 1] - gains[segment])
}

#[test]
fn registry_appends_profile_curve_link_after_legacy_params() {
    let keys: Vec<&str> = PARAMS.iter().map(|spec| spec.engine_key).collect();
    assert_eq!(
        keys,
        [
            "enabled",
            "threshold_db",
            "frequency_hz",
            "strength",
            "spectral_mode",
            "learn_noise",
            "use_captured_profile",
            "clear_profile",
            "curve_low",
            "curve_mid",
            "curve_high",
            "link_mode",
            "transient_guard",
        ]
    );
    assert_eq!(Params::VERSION, 2, "append-only growth keeps schema v2");
    assert_eq!(Params::PLUGIN_TYPE_KEY, "hiss_reducer");
}

#[test]
fn default_curve_is_flat_and_anchors_reproduce_exactly() {
    let curve = ReductionCurve::default();
    assert!(curve.is_flat());
    for anchor in CURVE_ANCHOR_HZ {
        assert_eq!(curve.gain_at(anchor), 1.0);
    }
    let shaped = ReductionCurve {
        low: 0.0,
        mid: 0.5,
        high: 1.0,
    };
    assert!(!shaped.is_flat());
    assert_eq!(shaped.gain_at(CURVE_ANCHOR_HZ[0]), 0.0);
    assert_eq!(shaped.gain_at(CURVE_ANCHOR_HZ[1]), 0.5);
    assert_eq!(shaped.gain_at(CURVE_ANCHOR_HZ[2]), 1.0);
}

#[test]
fn log_interpolation_matches_closed_form() {
    let curve = ReductionCurve {
        low: 0.1,
        mid: 0.7,
        high: 0.35,
    };
    let mut worst = 0.0f64;
    let mut worst_freq = 0.0;
    for step in 0..400 {
        let freq = 20.0 * (20000.0f64 / 20.0).powf(step as f64 / 399.0);
        let error = (f64::from(curve.gain_at(freq as f32)) - closed_form_gain(&curve, freq)).abs();
        if error > worst {
            worst = error;
            worst_freq = freq;
        }
    }
    assert!(
        worst < 1e-6,
        "worst interpolation error {worst} at {worst_freq} Hz"
    );
}

#[test]
fn curve_clamps_outside_anchors_and_handles_degenerate_input() {
    let curve = ReductionCurve {
        low: 0.2,
        mid: 0.6,
        high: 0.9,
    };
    assert_eq!(curve.gain_at(20.0), 0.2);
    assert_eq!(curve.gain_at(999.0), 0.2);
    assert_eq!(curve.gain_at(12_001.0), 0.9);
    assert_eq!(curve.gain_at(96_000.0), 0.9);
    assert_eq!(curve.gain_at(0.0), 0.2);
    assert_eq!(curve.gain_at(-100.0), 0.2);
    assert_eq!(curve.gain_at(f32::NAN), 0.2);
    assert_eq!(curve.gain_at(f32::INFINITY), 0.2);
}

#[test]
fn curve_canonicalize_clamps_and_repairs() {
    assert_eq!(ReductionCurve::canonicalize(0.3), 0.3);
    assert_eq!(ReductionCurve::canonicalize(-0.5), 0.0);
    assert_eq!(ReductionCurve::canonicalize(1.5), 1.0);
    assert_eq!(ReductionCurve::canonicalize(f32::NAN), 1.0);
    assert_eq!(ReductionCurve::canonicalize(f32::INFINITY), 1.0);
}

#[test]
fn preset_struct_covers_triggers_curve_and_link() {
    let mut params = Params::default();
    assert_eq!(params.param_value(5), Some(0.0));
    assert_eq!(params.param_value(6), Some(0.0));
    assert_eq!(params.param_value(7), Some(0.0));
    assert_eq!(params.param_value(8), Some(1.0));
    assert_eq!(params.param_value(11), Some(0.0));
    assert_eq!(params.param_value(12), Some(0.0));
    assert_eq!(params.param_value(13), None);

    params.set_param_value(5, 1.0);
    params.set_param_value(6, 1.0);
    params.set_param_value(7, 1.0);
    params.set_param_value(8, 0.25);
    params.set_param_value(9, -2.0);
    params.set_param_value(10, 9.0);
    params.set_param_value(11, 9.0);
    params.set_param_value(12, 1.0);
    // Trigger fire state is stored for UI but clear always reads 0.0 and
    // link clamps to the valid choice range.
    assert_eq!(params.param_value(5), Some(1.0));
    assert_eq!(params.param_value(6), Some(1.0));
    assert_eq!(params.param_value(7), Some(0.0));
    assert_eq!(params.param_value(8), Some(0.25));
    assert_eq!(params.param_value(9), Some(0.0));
    assert_eq!(params.param_value(10), Some(1.0));
    assert_eq!(params.param_value(11), Some(1.0));
    assert_eq!(params.param_value(12), Some(1.0));
    assert!(params.transient_guard);
    assert_eq!(params.link_mode, LINK_LINKED);

    // Serde defaults keep old presets compatible.
    let legacy: Params = serde_json::from_str(
        r#"{"enabled":true,"threshold_db":-30.0,"frequency_hz":4000.0,"strength":0.5,"spectral_mode":false}"#,
    )
    .unwrap();
    assert!(!legacy.learn_noise);
    assert!(!legacy.use_captured_profile);
    assert!(!legacy.clear_profile);
    assert_eq!((legacy.curve_low, legacy.curve_mid, legacy.curve_high), (1.0, 1.0, 1.0));
    assert_eq!(legacy.link_mode, LINK_INDEPENDENT);
    assert!(!legacy.transient_guard);
}

#[test]
fn curve_round_trips_and_leaves_time_domain_bit_exact() {
    let mut plugin = HissReducerPlugin::new(1);
    plugin.initialize(f64::from(RATE)).unwrap();
    assert!(plugin.reduction_curve().is_flat());
    plugin
        .set_parameter(ParameterId::from("curve_low"), ParameterValue::Float(0.0))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("curve_mid"), ParameterValue::Float(0.5))
        .unwrap();
    plugin
        .set_parameter(ParameterId::from("curve_high"), ParameterValue::Float(0.25))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("curve_mid")),
        Some(ParameterValue::Float(0.5))
    );
    assert_eq!(
        plugin.reduction_curve(),
        ReductionCurve {
            low: 0.0,
            mid: 0.5,
            high: 0.25,
        }
    );

    // The curve is spectral-only: time-domain output is bit-identical with
    // flat and extreme curves.
    let input: Vec<f32> = (0..16_384)
        .map(|i| {
            0.15 * (2.0 * std::f32::consts::PI * 750.0 * i as f32 / RATE as f32).sin()
                + 0.02 * ((i * 7919 % 1021) as f32 / 510.5 - 1.0)
        })
        .collect();
    let mut shaped = input.clone();
    let frames = shaped.len();
    plugin
        .process_in_place(&mut shaped, &ProcessContext::new(RATE, frames))
        .unwrap();
    let mut flat = HissReducerPlugin::new(1);
    flat.initialize(f64::from(RATE)).unwrap();
    let mut expected = input.clone();
    let expected_frames = expected.len();
    flat.process_in_place(&mut expected, &ProcessContext::new(RATE, expected_frames))
        .unwrap();
    assert_eq!(shaped, expected);

    // Out-of-range and mistyped curve updates are rejected, keeping state.
    let error = plugin
        .set_parameter(ParameterId::from("curve_low"), ParameterValue::Float(2.0))
        .unwrap_err();
    assert!(error.contains("maximum"), "unexpected error: {error}");
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("curve_low")),
        Some(ParameterValue::Float(0.0))
    );
}

#[test]
fn link_mode_round_trip_validation_and_persistence() {
    let mut plugin = HissReducerPlugin::new(2);
    plugin.initialize(f64::from(RATE)).unwrap();
    assert_eq!(plugin.link_mode(), LINK_INDEPENDENT);
    plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Int(1))
        .unwrap();
    assert_eq!(plugin.link_mode(), LINK_LINKED);
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("link_mode")),
        Some(ParameterValue::Int(1))
    );

    let error = plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Int(5))
        .unwrap_err();
    assert!(error.contains("maximum"), "unexpected error: {error}");
    assert_eq!(plugin.link_mode(), LINK_LINKED, "rejection keeps state");
    let error = plugin
        .set_parameter(ParameterId::from("link_mode"), ParameterValue::Float(1.0))
        .unwrap_err();
    assert!(error.contains("mismatch"), "unexpected error: {error}");

    let json = serde_json::to_string(&plugin.persisted_params()).unwrap();
    let restored: HissReducerPluginParams = serde_json::from_str(&json).unwrap();
    let reloaded = HissReducerPlugin::from_params(2, restored);
    assert_eq!(reloaded.link_mode(), LINK_LINKED);
}
