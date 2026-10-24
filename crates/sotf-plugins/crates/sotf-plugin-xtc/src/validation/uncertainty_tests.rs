//! Adversarial checks for supplied row-error cancellation envelopes.

// Rust guideline compliant 2026-02-21
use super::{ComplexTransferMatrix, bound_cancellation};
use rustfft::num_complex::Complex;

fn unpack(m: ComplexTransferMatrix) -> [[Complex<f64>; 2]; 2] {
    m.map(|r| r.map(|[re, im]| Complex::new(re, im)))
}
fn pack(m: [[Complex<f64>; 2]; 2]) -> ComplexTransferMatrix {
    m.map(|r| r.map(|v| [v.re, v.im]))
}
fn response(
    h: [[Complex<f64>; 2]; 2],
    w: [[Complex<f64>; 2]; 2],
    ear: usize,
    cue: usize,
) -> Complex<f64> {
    (0..2)
        .map(|speaker| h[ear][speaker] * w[speaker][cue])
        .sum()
}

#[test]
fn row_bounded_adversaries_attain_response_envelope_edges() {
    let plants = [
        [[[1.0, 0.1], [0.2, -0.3]], [[0.1, 0.4], [0.8, -0.2]]],
        [[[0.1, 0.0], [0.1, 0.0]], [[0.1, 0.0], [0.1, 0.0]]],
        [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]],
    ];
    let filters = [
        [[[1.0, -0.2], [-0.3, 0.4]], [[-0.5, 0.1], [1.2, 0.3]]],
        [[[0.01, 0.0], [0.02, 0.0]], [[0.03, 0.0], [0.04, 0.0]]],
        [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]],
    ];
    let mut cases = 0;
    let mut max_error = 0.0_f64;
    for plant in plants {
        for filter in filters {
            let h = unpack(plant);
            let w = unpack(filter);
            for radius in [0.0_f64, 0.001, 0.05, 0.5, 2.0] {
                let errors = [radius, radius * 0.7];
                let envelope = bound_cancellation(plant, filter, errors).unwrap();
                for cue in 0..2 {
                    let norm_sq: f64 = (0..2).map(|i| w[i][cue].norm_sqr()).sum();
                    let norm = norm_sq.sqrt();
                    let mut perturbed = h;
                    for ear in 0..2 {
                        let original = response(h, w, ear, cue);
                        let amplitude = original.norm();
                        let phase = if amplitude > 0.0 {
                            original / amplitude
                        } else {
                            Complex::new(1.0, 0.0)
                        };
                        let delta = if ear == cue {
                            -phase * amplitude.min(errors[ear] * norm)
                        } else {
                            phase * (errors[ear] * norm)
                        };
                        for speaker in 0..2 {
                            perturbed[ear][speaker] += delta * w[speaker][cue].conj() / norm_sq;
                        }
                        let actual_row_error = (0..2)
                            .map(|i| (perturbed[ear][i] - h[ear][i]).norm_sqr())
                            .sum::<f64>()
                            .sqrt();
                        assert!(actual_row_error <= errors[ear] + 2e-15);
                    }
                    let desired = response(perturbed, w, cue, cue).norm();
                    let unwanted = response(perturbed, w, 1 - cue, cue).norm();
                    max_error = max_error.max((desired - envelope.desired_amplitude[cue][0]).abs());
                    max_error =
                        max_error.max((unwanted - envelope.undesired_amplitude[cue][1]).abs());
                    assert!(max_error < 2e-14, "adversary boundary error {max_error}");
                    assert!(bound_cancellation(pack(perturbed), filter, [0.0; 2]).is_some());
                    cases += 1;
                }
            }
        }
    }
    println!("XTC_UNCERTAINTY_BOUNDARY cases={cases} maximum_amplitude_error={max_error:e}");
}

#[test]
fn invalid_error_models_refuse_and_muting_is_not_infinite_separation() {
    let identity = [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]];
    for error in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(bound_cancellation(identity, identity, [error, 0.0]).is_none());
    }
    let mut invalid = identity;
    invalid[0][1][1] = f64::NAN;
    assert!(bound_cancellation(invalid, identity, [0.0; 2]).is_none());
    assert!(bound_cancellation(identity, invalid, [0.0; 2]).is_none());
    let huge = [[[f64::MAX, f64::MAX]; 2]; 2];
    assert!(bound_cancellation(huge, huge, [0.0; 2]).is_none());
    let muted = bound_cancellation(identity, [[[0.0; 2]; 2]; 2], [1.0; 2]).unwrap();
    assert!(muted.separation_db.into_iter().flatten().all(f64::is_nan));
    let exact = bound_cancellation(identity, identity, [0.0; 2]).unwrap();
    assert!(
        exact
            .separation_db
            .into_iter()
            .flatten()
            .all(|v| v == f64::INFINITY)
    );
}
