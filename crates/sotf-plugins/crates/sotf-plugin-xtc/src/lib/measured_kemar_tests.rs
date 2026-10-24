//! Measured KEMAR transfer-matrix ownership across listener poses.
//!
//! Gardner and Martin, MIT Media Laboratory (1994); fixture attribution lives in
//! sotf-plugin-binaural/tests/fixtures/kemar. These are mannequin measurements.

// Rust guideline compliant 2026-02-21
use crate::XtcPluginParams;
use crate::load::load_hrtf_for_xtc;
use rustfft::num_complex::Complex;
use sotf_host::sofa::{SourcePosition, load_sofa};

fn dft(ir: &[f32], bin: usize) -> Complex<f64> {
    ir.iter()
        .enumerate()
        .map(|(i, &sample)| {
            let phase = -std::f64::consts::TAU * bin as f64 * i as f64 / 2048.0;
            Complex::new(phase.cos(), phase.sin()) * f64::from(sample)
        })
        .sum()
}

#[test]
fn measured_sofa_lookup_tracks_yaw_and_head_translation() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../sotf-plugin-binaural/tests/fixtures/kemar/mit_kemar_normal_pinna.sofa");
    let sofa = load_sofa(&path).unwrap().data;
    let mut cases = 0;
    let mut failures = 0;
    let mut maximum = 0.0_f64;
    for (yaw, x, z) in [
        (0.0_f32, 0.0, 0.0),
        (45.0, 0.0, 0.0),
        (-45.0, 0.0, 0.0),
        (179.0, 0.0, 0.0),
        (-179.0, 0.0, 0.0),
        (0.0, 0.2, 0.0),
        (0.0, -0.2, 0.0),
        (0.0, 0.0, 0.3),
        (30.0, 0.1, 0.2),
    ] {
        let params = XtcPluginParams {
            head_yaw_deg: yaw,
            head_offset_x: x,
            head_offset_z: z,
            ..Default::default()
        };
        let actual = load_hrtf_for_xtc(path.to_str().unwrap(), &params, 44_100.0, 1025)
            .unwrap()
            .unwrap();
        let mut case_error = 0.0_f64;
        // Independent right/forward head basis. Positive yaw turns right.
        let (sy, cy) = f64::from(yaw).to_radians().sin_cos();
        for speaker in 0..2 {
            let angle = f64::from(params.speaker_angle_deg).to_radians();
            let world_x =
                f64::from(params.distance_m) * angle.sin() * if speaker == 0 { -1.0 } else { 1.0 };
            let world_y = f64::from(params.distance_m) * angle.cos();
            let delta = [world_x - f64::from(x), world_y - f64::from(z)];
            let right = delta[0] * cy - delta[1] * sy;
            let forward = delta[0] * sy + delta[1] * cy;
            let position = SourcePosition::new(
                (-right).atan2(forward).to_degrees() as f32,
                0.0,
                delta[0].hypot(delta[1]) as f32,
            );
            let reference = sofa.get_hrtf_at_position(&position).unwrap();
            let actual_ears = if speaker == 0 {
                [&actual.h_ll, &actual.h_rl]
            } else {
                [&actual.h_lr, &actual.h_rr]
            };
            for (spectrum, ir) in actual_ears
                .into_iter()
                .zip([reference.ir_left, reference.ir_right])
            {
                for bin in [0, 4, 22, 43, 85, 171, 341] {
                    let expected = dft(&ir, bin);
                    let actual =
                        Complex::new(f64::from(spectrum[bin].re), f64::from(spectrum[bin].im));
                    case_error = case_error.max((actual - expected).norm());
                }
            }
        }
        maximum = maximum.max(case_error);
        failures += usize::from(case_error >= 2e-5);
        cases += 1;
        println!(
            "MEASURED_XTC_POSE yaw={yaw} head_x={x} head_z={z} maximum_dft_error={case_error:e}"
        );
    }
    println!(
        "MEASURED_XTC_POSE_SUMMARY cases={cases} failures={failures} maximum_error={maximum:e}"
    );
    assert_eq!(
        failures, 0,
        "measured SOFA lookup ignores or misroutes listener pose"
    );
}
