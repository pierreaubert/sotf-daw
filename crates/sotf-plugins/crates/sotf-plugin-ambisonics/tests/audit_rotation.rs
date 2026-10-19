//! Decoder rotation covariance and independently measured spatial-vector norms.

// Rust guideline compliant 2026-02-21
use sotf_plugin_ambisonics::custom_layout::{CustomLayout, CustomSpeaker};
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;

fn unit(az: f64, el: f64) -> [f64; 3] {
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}
fn speaker(index: usize, d: [f64; 3]) -> CustomSpeaker {
    CustomSpeaker {
        label: format!("S{index}"),
        azimuth_deg: d[1].atan2(d[0]).to_degrees() as f32,
        elevation_deg: d[2].clamp(-1.0, 1.0).asin().to_degrees() as f32,
        is_lfe: false,
    }
}
fn layout() -> CustomLayout {
    // Octahedral full-rank FOA geometry in front/left/up Cartesian axes.
    CustomLayout {
        name: "audit-octahedron".into(),
        speakers: [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ]
        .into_iter()
        .enumerate()
        .map(|(i, d)| speaker(i, d))
        .collect(),
    }
}
fn rotate(v: [f64; 3]) -> [f64; 3] {
    // Independent Rodrigues rotation: axis (1,2,3)/sqrt(14), angle 0.73 rad.
    let k = [
        1.0 / 14.0_f64.sqrt(),
        2.0 / 14.0_f64.sqrt(),
        3.0 / 14.0_f64.sqrt(),
    ];
    let (s, c) = 0.73_f64.sin_cos();
    let dot = k.iter().zip(v).map(|(a, b)| a * b).sum::<f64>();
    let cross = [
        k[1] * v[2] - k[2] * v[1],
        k[2] * v[0] - k[0] * v[2],
        k[0] * v[1] - k[1] * v[0],
    ];
    std::array::from_fn(|i| v[i] * c + cross[i] * s + k[i] * dot * (1.0 - c))
}
fn spatial(gains: &[f32], layout: &CustomLayout) -> (f64, [f64; 3], [f64; 3]) {
    let mut energy = 0.0;
    let mut pressure = 0.0;
    let mut e = [0.0; 3];
    let mut v = [0.0; 3];
    for (&gain, speaker) in gains.iter().zip(&layout.speakers) {
        let g = f64::from(gain);
        let d = unit(
            f64::from(speaker.azimuth_deg).to_radians(),
            f64::from(speaker.elevation_deg).to_radians(),
        );
        energy += g * g;
        pressure += g;
        for i in 0..3 {
            e[i] += g * g * d[i];
            v[i] += g * d[i];
        }
    }
    (energy, e.map(|x| x / energy), v.map(|x| x / pressure))
}

#[test]
fn mode_matching_is_covariant_under_yaw_at_every_supported_order() {
    let mut worst = 0.0_f32;
    for order in 1..=7 {
        for max_re in [false, true] {
            let original = layout();
            let matrix = DecodeMatrix::build_for_custom(order, &original, max_re).unwrap();
            let input: Vec<f32> = (0..(order + 1) * (order + 1))
                .map(|i| (i as f32 * 0.37).cos() * 0.1)
                .collect();
            let mut expected = vec![0.0; 6];
            matrix.decode_frame(&input, &mut expected);
            for degrees in [17.0_f64, 93.0, -137.0] {
                let angle = degrees.to_radians();
                let mut turned = original.clone();
                for spk in &mut turned.speakers {
                    spk.azimuth_deg = ((f64::from(spk.azimuth_deg) + degrees + 180.0)
                        .rem_euclid(360.0)
                        - 180.0) as f32;
                }
                let decoder = DecodeMatrix::build_for_custom(order, &turned, max_re).unwrap();
                let mut rotated = input.clone();
                for degree in 1..=order {
                    for m in 1..=degree {
                        let center = degree * degree + degree;
                        let sine = input[center - m];
                        let cosine = input[center + m];
                        let (s, c) = (m as f64 * angle).sin_cos();
                        rotated[center - m] = (f64::from(sine) * c + f64::from(cosine) * s) as f32;
                        rotated[center + m] = (f64::from(cosine) * c - f64::from(sine) * s) as f32;
                    }
                }
                let mut actual = vec![0.0; 6];
                decoder.decode_frame(&rotated, &mut actual);
                let error = actual
                    .iter()
                    .zip(&expected)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0_f32, f32::max);
                worst = worst.max(error);
                assert!(
                    error < 2e-5,
                    "order={order} max_re={max_re} yaw={degrees}: covariance error {error}"
                );
            }
        }
    }
    eprintln!("mode-matching all-order yaw covariance maximum error: {worst}");
}

#[test]
fn full_3d_foa_rotation_preserves_mode_matching_feeds_and_spatial_vectors() {
    let original = layout();
    let mut turned = original.clone();
    for (i, spk) in original.speakers.iter().enumerate() {
        turned.speakers[i] = speaker(
            i,
            rotate(unit(
                f64::from(spk.azimuth_deg).to_radians(),
                f64::from(spk.elevation_deg).to_radians(),
            )),
        );
    }
    let mut max_error = 0.0_f64;
    for max_re in [false, true] {
        let a = DecodeMatrix::build_for_custom(1, &original, max_re).unwrap();
        let b = DecodeMatrix::build_for_custom(1, &turned, max_re).unwrap();
        for az in [-2.3_f64, -0.8, 0.0, 1.4, 2.9] {
            for el in [-1.2_f64, -0.3, 0.6, 1.3] {
                let d = unit(az, el);
                let r = rotate(d);
                let mut x = [0.0; 6];
                let mut y = [0.0; 6];
                a.decode_frame(&[1.0, d[1] as f32, d[2] as f32, d[0] as f32], &mut x);
                b.decode_frame(&[1.0, r[1] as f32, r[2] as f32, r[0] as f32], &mut y);
                for (a, b) in x.iter().zip(y) {
                    let error = f64::from((a - b).abs());
                    max_error = max_error.max(error);
                    assert!(error < 2e-5);
                }
                let (energy, e, v) = spatial(&x, &original);
                let (rot_energy, rot_e, rot_v) = spatial(&y, &turned);
                assert!((energy - rot_energy).abs() < 2e-5);
                for (actual, expected) in rot_e
                    .into_iter()
                    .zip(rotate(e))
                    .chain(rot_v.into_iter().zip(rotate(v)))
                {
                    assert!((actual - expected).abs() < 2e-5);
                }
                let weight = if max_re { 1.0 / 3.0_f64.sqrt() } else { 1.0 };
                // FOA octahedron feeds are 1/6 + (weight/2)*dot(d,u).
                // This gives independent Gerzon norms and output energy.
                assert!((energy - (1.0 / 6.0 + weight * weight / 2.0)).abs() < 2e-6);
                let expected_e = 2.0 * weight / (1.0 + 3.0 * weight * weight);
                let expected_v = weight;
                for i in 0..3 {
                    assert!((e[i] - expected_e * d[i]).abs() < 2e-6);
                    assert!((v[i] - expected_v * d[i]).abs() < 2e-6);
                }
            }
        }
    }
    eprintln!("mode-matching full 3D FOA covariance max error: {max_error}");
}

#[test]
fn allrad_has_bounded_full_3d_rotation_and_energy_error_from_its_finite_grid() {
    let original = layout();
    let mut turned = original.clone();
    for (i, spk) in original.speakers.iter().enumerate() {
        turned.speakers[i] = speaker(
            i,
            rotate(unit(
                f64::from(spk.azimuth_deg).to_radians(),
                f64::from(spk.elevation_deg).to_radians(),
            )),
        );
    }
    let (mut worst_feed, mut worst_energy) = (0.0_f64, 0.0_f64);
    for max_re in [false, true] {
        let a = DecodeMatrix::build_allrad_for_custom(1, &original, max_re).unwrap();
        let b = DecodeMatrix::build_allrad_for_custom(1, &turned, max_re).unwrap();
        for az in [-2.3_f64, -0.8, 0.0, 1.4, 2.9] {
            for el in [-1.2_f64, -0.3, 0.6, 1.3] {
                let d = unit(az, el);
                let r = rotate(d);
                let mut x = [0.0; 6];
                let mut y = [0.0; 6];
                a.decode_frame(&[1.0, d[1] as f32, d[2] as f32, d[0] as f32], &mut x);
                b.decode_frame(&[1.0, r[1] as f32, r[2] as f32, r[0] as f32], &mut y);
                let error = x
                    .iter()
                    .zip(y)
                    .map(|(a, b)| f64::from((a - b).abs()))
                    .fold(0.0_f64, f64::max);
                worst_feed = worst_feed.max(error);
                let (energy, e, v) = spatial(&x, &original);
                let (rot_energy, rot_e, rot_v) = spatial(&y, &turned);
                let relative = (energy - rot_energy).abs() / energy;
                worst_energy = worst_energy.max(relative);
                // AllRAD has a fixed 64-point world-space integration grid at FOA;
                // exact continuous rotation covariance is not its contract.
                assert!(
                    error < 0.05,
                    "AllRAD finite-grid rotated feed error {error}"
                );
                assert!(relative < 0.05, "AllRAD rotated energy error {relative}");
                for (actual, expected) in rot_e
                    .into_iter()
                    .zip(rotate(e))
                    .chain(rot_v.into_iter().zip(rotate(v)))
                {
                    assert!(
                        (actual - expected).abs() < 0.05,
                        "AllRAD rotated spatial vector error"
                    );
                }
            }
        }
    }
    eprintln!(
        "AllRAD finite-grid rotation max feed error {worst_feed}, relative energy error {worst_energy}"
    );
}
