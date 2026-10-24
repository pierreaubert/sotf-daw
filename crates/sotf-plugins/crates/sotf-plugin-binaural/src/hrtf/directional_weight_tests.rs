//! Independent directional-simplex interpolation regressions.

// Rust guideline compliant 2026-02-21
use super::calculate_vbap_gains;
use sotf_host::sofa::{SofaFile, SourcePosition};

fn fixture(positions: Vec<SourcePosition>) -> SofaFile {
    let count = positions.len();
    SofaFile {
        sample_rate: 48_000.0,
        num_measurements: count,
        ir_length: 1,
        positions,
        impulse_responses: vec![1.0; count * 2],
        convention: "SimpleFreeFieldHRIR".into(),
        data_sample_rate: Some(48_000.0),
    }
}

fn position(v: [f64; 3]) -> SourcePosition {
    SourcePosition::new(
        v[1].atan2(v[0]).to_degrees() as f32,
        v[2].atan2(v[0].hypot(v[1])).to_degrees() as f32,
        1.0,
    )
}

#[test]
fn directional_weights_reconstruct_unequal_full_sphere_simplexes() {
    let mut maximum = 0.0_f32;
    let mut cases = 0;
    for yaw in [-170.0_f64, -75.0, 0.0, 45.0, 160.0] {
        for pitch in [-60.0_f64, 0.0, 60.0] {
            let (sy, cy) = yaw.to_radians().sin_cos();
            let (sp, cp) = pitch.to_radians().sin_cos();
            let vertices = [
                [cy * cp, sy * cp, sp],
                [-sy, cy, 0.0],
                [-cy * sp, -sy * sp, cp],
            ];
            let sofa = fixture(vertices.map(position).to_vec());
            for expected in [
                [0.2_f64, 0.3, 0.5],
                [0.05, 0.8, 0.15],
                [0.7, 0.25, 0.05],
                [0.0, 0.4, 0.6],
            ] {
                let target = position(std::array::from_fn(|axis| {
                    (0..3).map(|i| expected[i] * vertices[i][axis]).sum()
                }));
                let actual = calculate_vbap_gains(&target, &[(0, 0.0), (1, 0.0), (2, 0.0)], &sofa);
                for (a, b) in actual.into_iter().zip(expected) {
                    maximum = maximum.max((a - b as f32).abs());
                }
                assert!(
                    maximum < 2e-6,
                    "directional interpolation {yaw}/{pitch}: {actual:?} versus {expected:?}, max={maximum}"
                );
                cases += 1;
            }
        }
    }
    eprintln!("DIRECTIONAL_WEIGHTS cases={cases} maximum_error={maximum:e}");
}

#[test]
fn outside_simplex_weights_are_nonnegative_and_preserve_constant_fields() {
    let sofa = fixture(vec![
        SourcePosition::new(0.0, 0.0, 1.0),
        SourcePosition::new(90.0, 0.0, 1.0),
        SourcePosition::new(0.0, 90.0, 1.0),
    ]);
    for azimuth in [90.0001, 90.001, 90.01, 90.1, 120.0, -45.0] {
        let actual = calculate_vbap_gains(
            &SourcePosition::new(azimuth, 0.1, 1.0),
            &[(0, 0.0), (1, 0.0), (2, 0.0)],
            &sofa,
        );
        assert!(
            actual.iter().all(|x| x.is_finite() && *x >= 0.0),
            "{azimuth}: {actual:?}"
        );
        let used: f32 = actual.into_iter().filter(|w| *w >= 1e-6).sum();
        assert!(
            (used - 1.0).abs() <= 2e-6,
            "renderer effective weights: {actual:?}, sum={used}"
        );
    }
}

#[test]
fn dense_measurement_triangles_do_not_snap_to_one_neighbor() {
    for spacing in [0.01_f32, 0.1, 1.0, 5.0] {
        let sofa = fixture(vec![
            SourcePosition::new(0.0, 0.0, 1.0),
            SourcePosition::new(spacing, 0.0, 1.0),
            SourcePosition::new(0.0, spacing, 1.0),
        ]);
        let vertices = sofa
            .positions
            .iter()
            .map(|p| p.to_cartesian_unit_vector().map(f64::from))
            .collect::<Vec<_>>();
        let expected = [0.2_f64, 0.3, 0.5];
        let target = position(std::array::from_fn(|axis| {
            (0..3).map(|i| expected[i] * vertices[i][axis]).sum()
        }));
        let actual = calculate_vbap_gains(&target, &[(0, 0.0), (1, 0.0), (2, 0.0)], &sofa);
        for (value, expected) in actual.into_iter().zip(expected) {
            assert!(
                (f64::from(value) - expected).abs() < 2e-6,
                "dense spacing={spacing}: {actual:?}"
            );
        }
    }
}
