//! Independent physical-geometry reconstruction diagnostics near rank loss.

// Rust guideline compliant 2026-02-21
use sotf_plugin_ambisonics::custom_layout::{CustomLayout, CustomSpeaker};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;

fn foa(s: &CustomSpeaker) -> [f64; 4] {
    let az = f64::from(s.azimuth_deg).to_radians();
    let el = f64::from(s.elevation_deg).to_radians();
    [1.0, el.cos() * az.sin(), el.sin(), el.cos() * az.cos()]
}
#[test]
fn near_coincident_geometry_is_rejected_or_has_truthful_bounded_mode_matching_diagnostics() {
    let (mut accepted, mut rejected, mut max_error) = (0, 0, 0.0_f64);
    for width in [0.0_f32, 0.000001, 0.001, 0.1, 1.0, 10.0, 30.0, 90.0] {
        let layout = CustomLayout {
            name: format!("collapse-{width}"),
            speakers: (0..6)
                .map(|i| CustomSpeaker {
                    label: format!("S{i}"),
                    azimuth_deg: ((i % 3) as f32 - 1.0) * width,
                    elevation_deg: if i < 3 { -width } else { width },
                    is_lfe: false,
                })
                .collect(),
        };
        match DecodeMatrix::build_for_custom(1, &layout, false) {
            Err(error) => {
                rejected += 1;
                assert!(
                    error.contains("ill-conditioned") || error.contains("rank"),
                    "{error}"
                );
            }
            Ok(matrix) => {
                accepted += 1;
                let q = matrix.quality();
                assert!((1..=4).contains(&q.rank));
                assert!(q.condition_number.is_finite() && q.condition_number >= 1.0);
                assert!(
                    matrix
                        .matrix
                        .iter()
                        .all(|x| x.is_finite() && x.abs() <= 8.0)
                );
                if width == 0.0 {
                    assert_eq!(q.rank, 1);
                }
                // Independently evaluate ||Y^T D-I||_F/sqrt(4), using only
                // closed-form physical FOA directions and stored f32 gains.
                let mut residual = 0.0;
                for row in 0..4 {
                    for col in 0..4 {
                        let actual = layout
                            .speakers
                            .iter()
                            .enumerate()
                            .map(|(s, speaker)| {
                                foa(speaker)[row] * f64::from(matrix.matrix[s * 4 + col])
                            })
                            .sum::<f64>();
                        residual += (actual - if row == col { 1.0 } else { 0.0 }).powi(2);
                    }
                }
                let measured = (residual / 4.0).sqrt();
                let error = (measured - q.reconstruction_error).abs();
                max_error = max_error.max(error);
                assert!(
                    error < 5e-6,
                    "width={width}: stored reconstruction {measured}, diagnostic {}",
                    q.reconstruction_error
                );
                let input = [0.2_f32, -0.1, 0.3, 0.05];
                let mut output = [0.0; 6];
                matrix.decode_frame(&input, &mut output);
                let input_energy = input.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                let frobenius = matrix
                    .matrix
                    .iter()
                    .map(|&x| f64::from(x).powi(2))
                    .sum::<f64>();
                let output_energy = output.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
                assert!(output_energy <= frobenius * input_energy + 1e-5);
            }
        }
        // AllRAD's design rank belongs to its full virtual sphere, not the
        // physical array. Its finite composed matrix must still obey the same
        // coefficient bound even when all physical directions coincide.
        let allrad = DecodeMatrix::build_allrad_for_custom(1, &layout, false).unwrap();
        assert_eq!(allrad.quality().rank, 4);
        assert!(
            allrad
                .matrix
                .iter()
                .all(|x| x.is_finite() && x.abs() <= 8.0)
        );
        let mut gains = [0.0; 6];
        allrad.decode_frame(&[1.0, 0.0, 0.0, 0.0], &mut gains);
        assert!(gains.iter().all(|x| x.is_finite()));
    }
    assert!(
        accepted >= 2 && rejected >= 1,
        "must exercise both bounded rank loss and peak-gain rejection"
    );
    eprintln!(
        "conditioning: accepted {accepted}, rejected {rejected}, max diagnostic error {max_error}"
    );
}
