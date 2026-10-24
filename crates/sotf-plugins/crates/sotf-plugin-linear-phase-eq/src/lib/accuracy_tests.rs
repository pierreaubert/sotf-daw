//! Independent response checks and automatic-support lifecycle regressions.

// Rust guideline compliant 2026-02-21
use super::*;
use crate::{BandConfig, LinearPhaseEqPlugin, LinearPhaseEqPluginParams};
use sotf_host::{ParameterId, ParameterValue, ParametricInPlacePlugin, ProcessContext};

fn params(bands: serde_json::Value, phase: usize) -> LinearPhaseEqPluginParams {
    serde_json::from_value(serde_json::json!({
        "num_filters": bands.as_array().unwrap().len().max(1),
        "fir_length_index": 3,
        "phase_mode_index": phase,
        "accuracy_mode": true,
        "filters": bands,
    }))
    .unwrap()
}

fn dft(taps: &[f32], frequency: f64, rate: f64) -> C {
    taps.iter()
        .enumerate()
        .map(|(n, &tap)| {
            C::from_polar(
                f64::from(tap),
                -std::f64::consts::TAU * frequency * n as f64 / rate,
            )
        })
        .sum()
}

#[test]
fn null_policy_and_matrix_operator_do_not_hide_residuals_or_cancellation() {
    // A dual-clipping rule would call both -100 and -81 dB "zero error".
    let expected = 1e-5;
    let actual = 10.0_f64.powf(-81.0 / 20.0);
    assert!(deviation_bound_db(actual - expected, expected) > ACCURACY_TOLERANCE_DB);
    let epsilon = 1.0 - 10.0_f64.powf(-ACCURACY_TOLERANCE_DB / 20.0);
    assert!(
        deviation_bound_db(0.99 * epsilon * ACCURACY_REFERENCE_FLOOR, 0.0) < ACCURACY_TOLERANCE_DB
    );
    assert!(
        deviation_bound_db(1.01 * epsilon * ACCURACY_REFERENCE_FLOOR, 0.0) > ACCURACY_TOLERANCE_DB
    );
    let matrix = [
        [C::new(2.0, 0.0), C::new(1.0, 0.0)],
        [C::new(1.0, 0.0), C::new(2.0, 0.0)],
    ];
    let (small, large) = singular_values(matrix);
    assert!((small - 1.0).abs() < 1e-14 && (large - 3.0).abs() < 1e-14);
    // Rank-one perturbation has norm two although each entry has norm one.
    assert!((singular_values([[ONE; 2]; 2]).1 - 2.0).abs() < 1e-14);
}

#[test]
fn automatic_selection_preserves_manual_choice_defaults_and_lifecycle() {
    let original: LinearPhaseEqPluginParams = serde_json::from_str("{}").unwrap();
    assert!(!original.accuracy_mode);
    assert_eq!(original.fir_length_index, 1);
    for phase in [0, 1] {
        let saved = params(serde_json::json!([]), phase);
        let mut plugin = LinearPhaseEqPlugin::from_params(2, 96_000, saved.clone()).unwrap();
        assert_eq!(plugin.selected_fir_taps(), 1024);
        let report = plugin.accuracy_report().unwrap();
        assert_eq!(report.latency_samples, if phase == 0 { 544 } else { 32 });
        assert_eq!(report.sample_rate, 96_000.0);
        assert_eq!(report.latency_samples, plugin.latency_samples());
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("fir_length_index")),
            Some(ParameterValue::Int(3))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("accuracy_mode")),
            Some(ParameterValue::Bool(true))
        );
        let base = plugin.snapshot_config();
        assert!(base.accuracy_mode);
        assert_eq!(base.fir_length_index, 0);
        let parameters = plugin.parameters();
        assert_eq!(parameters[5].id.as_str(), "band_0_type");
        assert_eq!(parameters.last().unwrap().id.as_str(), "accuracy_mode");
        plugin.initialize(96_000.0).unwrap();
        plugin.reset();
        assert_eq!(plugin.accuracy_report(), Some(report));
        let before = plugin.snapshot_config();
        assert!(plugin.initialize(48_000.0).unwrap_err().contains("rebuild"));
        assert_eq!(plugin.snapshot_config(), before);
        assert_eq!(plugin.accuracy_report(), Some(report));
        assert!(
            plugin
                .set_parameter(
                    ParameterId::from("accuracy_mode"),
                    ParameterValue::Bool(false)
                )
                .unwrap_err()
                .contains("structural")
        );
        let mut manual = saved;
        manual.accuracy_mode = false;
        let manual = LinearPhaseEqPlugin::from_params(2, 96_000, manual).unwrap();
        assert_eq!(manual.selected_fir_taps(), 8192);
        assert!(manual.accuracy_report().is_none());
    }
}

#[test]
fn automatic_gain_uses_ideal_endpoint_reference_and_final_f32_support() {
    for phase in [0, 1] {
        for shape in ["Peak", "Lowshelf", "Highshelf", "Lowpass", "Highpass"] {
            let mut settings = params(
                serde_json::json!([
                    {"filter_type":shape,"frequency":6000.0,"q":0.7,"gain_db":6.0}
                ]),
                phase,
            );
            settings.auto_gain = true;
            let plugin = LinearPhaseEqPlugin::from_params(2, 48_000, settings).unwrap();
            let report = plugin.accuracy_report().unwrap();
            assert!(report.maximum_error_db <= 0.05);
            let taps = plugin.stage_fir(0).unwrap();
            if shape == "Highpass" {
                assert!(dft(taps, 0.0, 48_000.0).norm() < 5.75e-7);
                assert!((dft(taps, 24_000.0, 48_000.0).norm() - 1.0).abs() < 1e-5);
            } else {
                assert!((dft(taps, 0.0, 48_000.0).norm() - 1.0).abs() < 1e-5);
            }
        }
    }
}

#[test]
fn final_f32_response_meets_independent_biquad_and_direct_dft_checks() {
    let bands = serde_json::json!([
        {"filter_type":"Peak", "frequency":500.0, "q":2.0, "gain_db":9.0},
        {"filter_type":"Peak", "frequency":2000.0, "q":1.5, "gain_db":-6.0},
        {"filter_type":"Lowshelf", "frequency":150.0, "q":1.0, "gain_db":3.0}
    ]);
    for phase in [0, 1] {
        let settings = params(bands.clone(), phase);
        let plugin = LinearPhaseEqPlugin::from_params(2, 96_000, settings).unwrap();
        let report = plugin.accuracy_report().unwrap();
        println!(
            "automatic_accuracy rate=96000 phase={phase} taps={} latency={} manual_latency={} additional_latency={} maximum_error_db={} worst_hz={} points={}",
            report.selected_taps,
            report.latency_samples,
            report.manual_latency_samples,
            report.additional_latency_samples,
            report.maximum_error_db,
            report.worst_frequency_hz,
            report.frequency_points
        );
        assert!(report.maximum_error_db <= ACCURACY_TOLERANCE_DB);
        assert!(report.frequency_points >= 16385);
        let taps = plugin.stage_fir(0).unwrap();
        let mut maximum = 0.0_f64;
        // Different grid, direct trigonometric DFT and the independent library
        // biquad target; no production qualifier/interpolation call here.
        for i in 0..=2048 {
            let frequency = i as f64 * 48_000.0 / 2048.0;
            let expected = 10.0_f64.powf(
                plugin
                    .bands
                    .iter()
                    .map(|b| b.biquad.log_result(frequency))
                    .sum::<f64>()
                    / 20.0,
            );
            let actual = dft(taps, frequency, 96_000.0).norm();
            maximum = maximum.max(20.0 * (actual / expected).log10().abs());
        }
        assert!(
            maximum <= 0.05,
            "phase={phase} taps={} error={maximum}",
            taps.len()
        );
        // The historical fixed-1024 FIR-M1 limitation remains observable.
        let fixed = LinearPhaseEqPlugin::from_params(
            2,
            96_000,
            serde_json::from_value(serde_json::json!({
                "num_filters":3, "fir_length_index":0, "phase_mode_index":phase, "filters":bands,
            }))
            .unwrap(),
        )
        .unwrap();
        let fixed_qualified = crate::accuracy::measure(
            &[fixed.stage_fir(0).unwrap().to_vec()],
            &fixed.bands,
            false,
            phase,
            false,
            2,
            &[[0, 1]],
            96_000.0,
        )
        .unwrap();
        if fixed_qualified.maximum_error_db > 0.05 {
            assert!(plugin.selected_fir_taps() > 1024);
        }
        for shorter in [1024_usize, 2048, 4096, 8192]
            .into_iter()
            .take_while(|n| *n < plugin.selected_fir_taps())
        {
            let index = shorter.ilog2() as usize - 10;
            let fixed = LinearPhaseEqPlugin::from_params(2, 96_000, serde_json::from_value(serde_json::json!({
                "num_filters":3, "fir_length_index":index, "phase_mode_index":phase, "filters":bands,
            })).unwrap()).unwrap();
            assert!(
                crate::accuracy::measure(
                    &[fixed.stage_fir(0).unwrap().to_vec()],
                    &fixed.bands,
                    false,
                    phase,
                    false,
                    2,
                    &[[0, 1]],
                    96_000.0
                )
                .unwrap()
                .maximum_error_db
                    > 0.05
            );
        }
    }
}

#[test]
fn max_support_failure_and_fixed_length_dynamic_refusal_are_explicit() {
    let impossible = params(
        serde_json::json!([
            {"filter_type":"Peak", "frequency":20.0, "q":10.0, "gain_db":24.0}
        ]),
        0,
    );
    let failure = LinearPhaseEqPlugin::from_params(2, 192_000, impossible)
        .err()
        .unwrap();
    assert!(failure.contains("8192") && failure.contains("0.05"));
    for phase in [0, 1] {
        let mut plugin =
            LinearPhaseEqPlugin::from_params(2, 96_000, params(serde_json::json!([]), phase))
                .unwrap();
        let base = plugin.snapshot_config();
        let latency = plugin.latency_samples();
        let demanding: BandConfig = serde_json::from_value(serde_json::json!({
            "filter_type":"Peak", "frequency":20.0, "q":10.0, "gain_db":24.0,
        }))
        .unwrap();
        let failure = LinearPhaseEqPlugin::prepare_band_update(&base, 0, demanding)
            .err()
            .unwrap();
        assert!(failure.contains("fixed 1024") && failure.contains("rebuild"));
        assert_eq!(plugin.snapshot_config(), base);
        assert_eq!(plugin.latency_samples(), latency);
        let gentle: BandConfig = serde_json::from_value(serde_json::json!({
            "filter_type":"Peak", "frequency":6000.0, "q":0.7, "gain_db":3.0,
        }))
        .unwrap();
        let prepared = LinearPhaseEqPlugin::prepare_band_update(&base, 0, gentle).unwrap();
        plugin.commit_prepared_update(prepared).unwrap();
        assert_eq!(plugin.latency_samples(), latency);
        assert_eq!(plugin.selected_fir_taps(), 1024);
        assert!(plugin.accuracy_report().unwrap().maximum_error_db <= 0.05);
    }
}

#[test]
fn accepted_update_keeps_resolved_long_support_when_new_target_could_be_shorter() {
    for phase in [0, 1] {
        let original = params(
            serde_json::json!([
                {"filter_type":"Peak","frequency":125.0,"q":2.0,"gain_db":9.0}
            ]),
            phase,
        );
        let mut plugin = LinearPhaseEqPlugin::from_params(2, 96_000, original.clone()).unwrap();
        let taps = plugin.selected_fir_taps();
        assert!(taps > 1024);
        let latency = plugin.latency_samples();
        let mut prefix = vec![0.125; 257 * 2];
        let (counts, processed) = crate::drain_tests::count_allocs(|| {
            plugin.process_in_place(&mut prefix, &ProcessContext::new(96_000, 257))
        });
        assert_eq!(counts, (0, 0));
        processed.unwrap();
        let base = plugin.snapshot_config();
        let mut new_band = original.filters[0].clone();
        new_band.gain_db = 0.0;
        let mut fresh_params = original;
        fresh_params.filters[0] = new_band.clone();
        let fresh = LinearPhaseEqPlugin::from_params(2, 96_000, fresh_params).unwrap();
        assert_eq!(fresh.selected_fir_taps(), 1024);
        let update = LinearPhaseEqPlugin::prepare_band_update(&base, 0, new_band).unwrap();
        let mut prepared = Some(update);
        let (counts, committed) =
            crate::drain_tests::count_allocs(|| plugin.try_commit_prepared_update(&mut prepared));
        assert_eq!(counts, (0, 0));
        committed.unwrap();
        assert!(prepared.is_none());
        assert_eq!(plugin.selected_fir_taps(), taps);
        assert_eq!(plugin.latency_samples(), latency);
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("fir_length_index")),
            Some(ParameterValue::Int(3))
        );
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("accuracy_mode")),
            Some(ParameterValue::Bool(true))
        );
        let accepted = plugin.snapshot_config();
        assert!(accepted.accuracy_mode);
        assert_eq!(accepted.fir_length_index, base.fir_length_index);
        let (counts, ()) = crate::drain_tests::count_allocs(|| plugin.reset());
        assert_eq!(counts, (0, 0));
        assert_eq!(plugin.snapshot_config(), accepted);
        assert_eq!(plugin.accuracy_report().unwrap().selected_taps, taps);
        assert_eq!(plugin.latency_samples(), latency);
    }
}

#[test]
fn placed_route_qualifies_actual_paired_cascade_and_full_latency() {
    for phase in [0, 1] {
        let mut settings = params(
            serde_json::json!([
                {"filter_type":"Peak","frequency":4000.0,"q":0.7,"gain_db":2.0,"placement":"left"},
                {"filter_type":"Peak","frequency":6000.0,"q":0.7,"gain_db":-2.0,"placement":"mid"}
            ]),
            phase,
        );
        settings.stereo_pairs = Some(vec![[0, 1]]);
        let mut plugin = LinearPhaseEqPlugin::from_params(3, 48_000, settings).unwrap();
        let report = plugin.accuracy_report().unwrap();
        assert!(report.paired_operator_qualified);
        assert_eq!(report.stage_count, 2);
        assert_eq!(
            report.latency_samples,
            2 * (if phase == 0 {
                report.selected_taps / 2 + 32
            } else {
                32
            })
        );
        let mut silence = vec![0.0; 3 * 127];
        plugin
            .process_in_place(&mut silence, &ProcessContext::new(48_000, 127))
            .unwrap();
        plugin.reset();
        assert_eq!(plugin.accuracy_report(), Some(report));
    }
}
