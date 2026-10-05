//! Independent decoder-direction accuracy tests (A1).
//!
//! The AUD133 suite owns matrix-element oracles (test-owned Jacobi SVD and
//! Rodrigues harmonics). These tests cover the remaining A1 dimensions with
//! independent references: closed-form FOA encoding (textbook SN3D
//! definition, not the production recurrence), Gerzon energy/velocity vectors
//! computed from gains plus documented speaker geometry, and conditioning
//! bounds from spherical-design theory.
//!
//! Tolerances fixed a priori: velocity/energy cosine >= 0.9999 on the
//! octahedron for mode-matching basic and max-rE (exact by symmetry: decode
//! rows are `[c0, c1*unit_vector]`, so both vectors lie exactly along the
//! source direction); AllRAD pairwise front/back inequalities only;
//! conditioning uses the AUD133 condition bound 4.0.

// Rust guideline compliant 2026-02-21

use sotf_plugin_ambisonics::custom_layout::{CustomLayout, CustomSpeaker};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use std::f64::consts::PI;

/// Independent FOA encoder from the AmbiX/SN3D definition.
///
/// W = 1, Y = sin(az)*cos(el), Z = sin(el), X = cos(az)*cos(el), in ACN
/// order [W, Y, Z, X]. This closed form does not share code with the
/// production associated-Legendre recurrence.
fn foa_encode(azimuth: f64, elevation: f64) -> [f32; 4] {
    let (sin_az, cos_az) = azimuth.sin_cos();
    let (sin_el, cos_el) = elevation.sin_cos();
    [
        1.0,
        (sin_az * cos_el) as f32,
        sin_el as f32,
        (cos_az * cos_el) as f32,
    ]
}

fn speaker(label: &str, azimuth_deg: f64, elevation_deg: f64) -> CustomSpeaker {
    CustomSpeaker {
        label: label.to_owned(),
        azimuth_deg: azimuth_deg as f32,
        elevation_deg: elevation_deg as f32,
        is_lfe: false,
    }
}

/// Regular octahedron: the minimal symmetric full-rank FOA layout.
fn octahedron() -> CustomLayout {
    CustomLayout {
        name: "octahedron".to_owned(),
        speakers: vec![
            speaker("+X", 90.0, 0.0),
            speaker("-X", -90.0, 0.0),
            speaker("+Y", 0.0, 0.0),
            speaker("-Y", 180.0, 0.0),
            speaker("+Z", 0.0, 90.0),
            speaker("-Z", 0.0, -90.0),
        ],
    }
}

/// Icosahedron vertices: a spherical 5-design, full rank for order 2.
fn icosahedron() -> CustomLayout {
    let phi = (1.0 + 5.0_f64.sqrt()) / 2.0;
    let raw = [
        (0.0, 1.0, phi),
        (0.0, 1.0, -phi),
        (0.0, -1.0, phi),
        (0.0, -1.0, -phi),
        (1.0, phi, 0.0),
        (1.0, -phi, 0.0),
        (-1.0, phi, 0.0),
        (-1.0, -phi, 0.0),
        (phi, 0.0, 1.0),
        (phi, 0.0, -1.0),
        (-phi, 0.0, 1.0),
        (-phi, 0.0, -1.0),
    ];
    CustomLayout {
        name: "icosahedron".to_owned(),
        speakers: raw
            .iter()
            .enumerate()
            .map(|(index, (x, y, z))| {
                let norm = (x * x + y * y + z * z).sqrt();
                let (x, y, z) = (x / norm, y / norm, z / norm);
                speaker(
                    &format!("V{index:02}"),
                    x.atan2(y).to_degrees(),
                    z.asin().to_degrees(),
                )
            })
            .collect(),
    }
}

fn cartesian(azimuth: f64, elevation: f64) -> [f64; 3] {
    let (sin_az, cos_az) = azimuth.sin_cos();
    let (sin_el, cos_el) = elevation.sin_cos();
    [cos_el * sin_az, cos_el * cos_az, sin_el]
}

/// Gerzon energy vector: sum(g^2 * u) / sum(g^2).
fn energy_vector(gains: &[f32], layout: &CustomLayout) -> [f64; 3] {
    let mut vector = [0.0; 3];
    let mut energy = 0.0;
    for (gain, speaker) in gains.iter().zip(layout.speakers.iter()) {
        let weight = f64::from(*gain) * f64::from(*gain);
        let direction = cartesian(
            f64::from(speaker.azimuth_deg).to_radians(),
            f64::from(speaker.elevation_deg).to_radians(),
        );
        for axis in 0..3 {
            vector[axis] += weight * direction[axis];
        }
        energy += weight;
    }
    vector.map(|component| component / energy)
}

/// Gerzon velocity vector: sum(g * u) / sum(g).
fn velocity_vector(gains: &[f32], layout: &CustomLayout) -> [f64; 3] {
    let mut vector = [0.0; 3];
    let mut pressure = 0.0;
    for (gain, speaker) in gains.iter().zip(layout.speakers.iter()) {
        let direction = cartesian(
            f64::from(speaker.azimuth_deg).to_radians(),
            f64::from(speaker.elevation_deg).to_radians(),
        );
        for axis in 0..3 {
            vector[axis] += f64::from(*gain) * direction[axis];
        }
        pressure += f64::from(*gain);
    }
    vector.map(|component| component / pressure)
}

fn cosine(left: [f64; 3], right: [f64; 3]) -> f64 {
    let dot = left[0] * right[0] + left[1] * right[1] + left[2] * right[2];
    let norm = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    dot / (norm(left) * norm(right))
}

#[test]
fn mode_matching_preserves_vectors_over_full_sphere() {
    let layout = octahedron();
    for max_re in [false, true] {
        let matrix = DecodeMatrix::build_for_custom(1, &layout, max_re).unwrap();
        let mut worst_energy = 1.0_f64;
        let mut worst_velocity = 1.0_f64;
        let mut output = [0.0_f32; 6];
        // 5-degree full-sphere sweep including both poles.
        for azimuth_step in 0..72 {
            for elevation_step in 0..=36 {
                let azimuth = azimuth_step as f64 * 5.0 * PI / 180.0;
                let elevation = elevation_step as f64 * 5.0 * PI / 180.0 - PI / 2.0;
                let input = foa_encode(azimuth, elevation);
                matrix.decode_frame(&input, &mut output);
                assert!(output.iter().all(|sample| sample.is_finite()));
                let energy: f32 = output.iter().map(|value| value * value).sum();
                assert!(energy < 100.0, "unbounded energy {energy}");

                let direction = cartesian(azimuth, elevation);
                let energy_cosine = cosine(energy_vector(&output, &layout), direction);
                let velocity_cosine = cosine(velocity_vector(&output, &layout), direction);
                worst_energy = worst_energy.min(energy_cosine);
                worst_velocity = worst_velocity.min(velocity_cosine);
            }
        }
        assert!(
            worst_energy >= 0.9999,
            "max_re={max_re}: worst energy cosine {worst_energy}"
        );
        assert!(
            worst_velocity >= 0.9999,
            "max_re={max_re}: worst velocity cosine {worst_velocity}"
        );
    }
}

#[test]
fn allrad_prefers_source_hemisphere_on_octahedron() {
    let layout = octahedron();
    let matrix = DecodeMatrix::build_allrad_for_custom(1, &layout, false).unwrap();
    // (axis direction, source speaker, opposite speaker)
    let axes = [
        (90.0_f64, 0.0_f64, 0_usize, 1_usize),
        (-90.0, 0.0, 1, 0),
        (0.0, 0.0, 2, 3),
        (180.0, 0.0, 3, 2),
        (0.0, 90.0, 4, 5),
        (0.0, -90.0, 5, 4),
    ];
    for (azimuth_deg, elevation_deg, source, opposite) in axes {
        let input = foa_encode(azimuth_deg.to_radians(), elevation_deg.to_radians());
        let mut output = [0.0_f32; 6];
        matrix.decode_frame(&input, &mut output);
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(
            output[source] > output[opposite],
            "axis ({azimuth_deg}, {elevation_deg}): source gain {} <= opposite gain {}",
            output[source],
            output[opposite]
        );
    }
}

#[test]
fn conditioning_matches_spherical_design_theory() {
    // Octahedron order 1: singular values sqrt(6), sqrt(2)^3; cond = sqrt(3).
    let octahedron = octahedron();
    let order1 = DecodeMatrix::build_for_custom(1, &octahedron, false).unwrap();
    assert_eq!(order1.quality().rank, 4);
    assert!(order1.quality().condition_number < 4.0);
    assert!(order1.quality().condition_number > 1.0);

    // Octahedron order 2: 6 speakers cannot span 9 harmonics; rank loss
    // must be reported with a bounded matrix.
    let order2 = DecodeMatrix::build_for_custom(2, &octahedron, false).unwrap();
    assert_eq!(order2.quality().rank, 6);
    assert!(order2.quality().rank < order2.ambi_channels);
    assert!(order2.matrix.iter().all(|value| value.is_finite()));
    assert!(order2.quality().peak_coefficient <= 8.0);

    // Icosahedron order 2: spherical 5-design spans all 9 harmonics.
    let icosahedron = icosahedron();
    for max_re in [false, true] {
        let matrix = DecodeMatrix::build_for_custom(2, &icosahedron, max_re).unwrap();
        assert_eq!(matrix.quality().rank, 9, "max_re={max_re}");
        assert!(
            matrix.quality().condition_number < 4.0,
            "max_re={max_re}: condition {}",
            matrix.quality().condition_number
        );
        assert!(matrix.matrix.iter().all(|value| value.is_finite()));
    }
}
