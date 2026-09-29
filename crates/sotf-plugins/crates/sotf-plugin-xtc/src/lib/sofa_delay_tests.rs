// Rust guideline compliant 2026-02-21
use crate::XtcPluginParams;
use crate::filters::{compute_geometry_cache, compute_xtc_filters_full_with_cache_and_hrtf};
use crate::load::load_hrtf_for_xtc;
use rustfft::num_complex::Complex;
use sofa_reader::SofaWriter;
use std::path::Path;

const RATE: u32 = 48_000;
const FFT_SIZE: usize = 128;
const IR_LENGTH: usize = 16;

fn write_fractional_sofa(path: &Path) {
    let mut writer = SofaWriter::new();
    for (key, value) in [
        ("Conventions", "SOFA"),
        ("Version", "1.0"),
        ("SOFAConventions", "SimpleFreeFieldHRIR"),
        ("SOFAConventionsVersion", "1.0"),
        ("DataType", "FIR"),
    ] {
        writer.add_attribute_str(key, value);
    }
    for (name, length) in [("I", 1), ("M", 3), ("R", 2), ("N", IR_LENGTH), ("C", 3)] {
        writer.add_dimension(name, length);
    }
    writer.add_variable_f64("Data.SamplingRate", &["I"]);
    writer
        .write_f64("Data.SamplingRate", &[f64::from(RATE)])
        .unwrap();
    writer.add_variable_f64("SourcePosition", &["M", "C"]);
    writer
        .write_f64(
            "SourcePosition",
            &[30.0, 0.0, 2.0, -30.0, 0.0, 2.0, 0.0, 0.0, 2.0],
        )
        .unwrap();
    writer
        .add_variable_attribute_str("SourcePosition", "Type", "spherical")
        .unwrap();

    let amplitudes = [[1.0, 0.35], [0.25, 1.0], [0.5, 0.6]];
    let mut impulse_responses = vec![0.0_f64; 3 * 2 * IR_LENGTH];
    for (measurement, ears) in amplitudes.iter().enumerate() {
        for (ear, &amplitude) in ears.iter().enumerate() {
            impulse_responses[(measurement * 2 + ear) * IR_LENGTH] = amplitude;
        }
    }
    writer.add_variable_f64("Data.IR", &["M", "R", "N"]);
    writer.write_f64("Data.IR", &impulse_responses).unwrap();
    writer.add_variable_f64("Data.Delay", &["I", "R"]);
    writer.write_f64("Data.Delay", &[0.25, 1.5]).unwrap();
    writer.finish(path).unwrap();
}

fn assert_analytic_delay_response(response: &[Complex<f32>], amplitude: f64, delay_samples: f64) {
    let dc = response[0];
    assert!((f64::from(dc.re) - amplitude).abs() < 2.0e-6);
    assert!(f64::from(dc.im).abs() < 2.0e-6);

    let mut previous_phase = 0.0;
    for (bin, value) in response.iter().enumerate().take(58).skip(1) {
        let frequency = bin as f64 / FFT_SIZE as f64;
        let magnitude = f64::from(value.norm());
        let magnitude_error_db = 20.0 * (magnitude / amplitude).log10();
        assert!(
            magnitude_error_db.abs() <= 0.03,
            "bin={bin}, magnitude error={magnitude_error_db} dB"
        );
        let raw_phase = f64::from(value.im).atan2(f64::from(value.re));
        let unwrapped_phase = raw_phase
            + std::f64::consts::TAU
                * ((previous_phase - raw_phase) / std::f64::consts::TAU).round();
        previous_phase = unwrapped_phase;
        let ideal_phase = -std::f64::consts::TAU * frequency * delay_samples;
        let phase_delay_error =
            (unwrapped_phase - ideal_phase).abs() / (std::f64::consts::TAU * frequency);
        assert!(
            phase_delay_error <= 0.001,
            "bin={bin}, phase-delay error={phase_delay_error} samples"
        );
    }
}

fn expected_cascade_with_band_crossfade(
    plant: [[Complex<f32>; 2]; 2],
    bin: usize,
) -> [[Complex<f32>; 2]; 2] {
    let frequency = f64::from(RATE) * bin as f64 / FFT_SIZE as f64;
    let sigmoid = |x: f64, width: f64| 1.0 / (1.0 + (-x / width).exp());
    let alpha =
        (1.0 - sigmoid(100.0 - frequency, 30.0)) * (1.0 - sigmoid(frequency - 12_000.0, 1_500.0));
    let alpha = alpha as f32;
    let identity_blend = 1.0 - alpha;

    [
        [
            plant[0][0] * identity_blend + Complex::new(alpha, 0.0),
            plant[0][1] * identity_blend,
        ],
        [
            plant[1][0] * identity_blend,
            plant[1][1] * identity_blend + Complex::new(alpha, 0.0),
        ],
    ]
}

#[test]
fn xtc_plant_delay_matches_analytic_response_and_inverse_cascade() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fractional-plant.sofa");
    write_fractional_sofa(&path);

    let mut params = XtcPluginParams::default();
    params.fft_size = FFT_SIZE;
    params.source_mode = "hrtf_file".to_owned();
    params.hrtf_file = Some(path.to_string_lossy().into_owned());
    params.spectral_normalization = false;
    params.bypass_spectral_normalization = true;
    params.bypass_neumann_refinement = true;
    params.beta_base = 1.0e-10;
    params.kappa_target = 1.0e6;
    params.max_gain_db = 40.0;

    let plant = load_hrtf_for_xtc(&path.to_string_lossy(), &params, RATE, FFT_SIZE / 2 + 1)
        .unwrap()
        .unwrap();
    let offset = 32.0;
    assert_eq!(plant.delay_rebase_seconds, offset / f64::from(RATE));
    assert_analytic_delay_response(&plant.h_ll, 1.0, 0.25 + offset);
    assert_analytic_delay_response(&plant.h_rl, 0.35, 1.5 + offset);
    assert_analytic_delay_response(&plant.h_lr, 0.25, 0.25 + offset);
    assert_analytic_delay_response(&plant.h_rr, 1.0, 1.5 + offset);

    let geometry = compute_geometry_cache(&params, RATE, FFT_SIZE / 2 + 1);
    let inverse = compute_xtc_filters_full_with_cache_and_hrtf(
        &params,
        RATE,
        FFT_SIZE / 2 + 1,
        &geometry,
        None,
        Some(&plant),
    );
    let w_rl = inverse.filter_rl.as_ref().unwrap();
    let w_rr = inverse.filter_rr.as_ref().unwrap();
    for bin in 2..=18 {
        let plant_matrix = [
            [plant.h_ll[bin], plant.h_lr[bin]],
            [plant.h_rl[bin], plant.h_rr[bin]],
        ];
        let expected = expected_cascade_with_band_crossfade(plant_matrix, bin);
        let c00 = plant_matrix[0][0];
        let c01 = plant_matrix[0][1];
        let c10 = plant_matrix[1][0];
        let c11 = plant_matrix[1][1];
        let r00 = c00 * inverse.filter_ll[bin] + c01 * w_rl[bin];
        let r01 = c00 * inverse.filter_lr[bin] + c01 * w_rr[bin];
        let r10 = c10 * inverse.filter_ll[bin] + c11 * w_rl[bin];
        let r11 = c10 * inverse.filter_lr[bin] + c11 * w_rr[bin];
        assert!(
            (r00 - expected[0][0]).norm() < 2.0e-3,
            "bin={bin} r00={r00:?}, expected={:?}",
            expected[0][0]
        );
        assert!(
            (r01 - expected[0][1]).norm() < 2.0e-3,
            "bin={bin} r01={r01:?}, expected={:?}",
            expected[0][1]
        );
        assert!(
            (r10 - expected[1][0]).norm() < 2.0e-3,
            "bin={bin} r10={r10:?}, expected={:?}",
            expected[1][0]
        );
        assert!(
            (r11 - expected[1][1]).norm() < 2.0e-3,
            "bin={bin} r11={r11:?}, expected={:?}",
            expected[1][1]
        );
    }
}
