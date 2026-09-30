// Rust guideline compliant 2026-02-21
// Independent numerical references for the bounded AUD133 order extension.

use super::*;
use sotf_host::speaker_config::get_speaker_config;
use std::f64::consts::PI;

const JACOBI_CORRELATION_TOLERANCE: f64 = 1.0e-12;
const JACOBI_SWEEP_LIMIT: usize = 128;
const JACOBI_NUMERICAL_NULL_RELATIVE: f64 = 1.0e-14;

struct JacobiSvd {
    rows: usize,
    columns: usize,
    singular_values: Vec<f64>,
    left: Vec<f64>,
    right: Vec<f64>,
    normalized_off_diagonal_correlation: f64,
}

#[derive(Clone, Copy)]
struct OracleDirection {
    vector: [f64; 3],
    azimuth: f64,
    elevation: f64,
}

fn oracle_acn_to_degree_index(acn: usize) -> (usize, isize) {
    for degree in 0..=8 {
        for azimuthal_index in -(degree as isize)..=(degree as isize) {
            if (degree * (degree + 1)) as isize + azimuthal_index == acn as isize {
                return (degree, azimuthal_index);
            }
        }
    }
    panic!("no degree/m pair for ACN {acn}");
}

fn oracle_factorial(value: usize) -> f64 {
    (1..=value).fold(1.0, |product, factor| product * factor as f64)
}

fn oracle_associated_legendre(degree: usize, abs_m: usize, x: f64) -> f64 {
    assert!(abs_m <= degree);
    // Rodrigues' explicit polynomial, differentiated term by term. The
    // project uses the no-Condon–Shortley real-SH convention.
    let mut derivative_sum = 0.0;
    for k in 0..=degree / 2 {
        let exponent = degree - 2 * k;
        if exponent < abs_m {
            continue;
        }
        let coefficient = if k % 2 == 0 { 1.0 } else { -1.0 }
            * oracle_factorial(2 * degree - 2 * k)
            / (2.0_f64.powi(degree as i32)
                * oracle_factorial(k)
                * oracle_factorial(degree - k)
                * oracle_factorial(exponent));
        let falling_factorial = oracle_factorial(exponent) / oracle_factorial(exponent - abs_m);
        derivative_sum += coefficient * falling_factorial * x.powi((exponent - abs_m) as i32);
    }
    derivative_sum * (1.0 - x * x).max(0.0).powf(abs_m as f64 / 2.0)
}

fn oracle_harmonics(order: usize, azimuth: f64, elevation: f64) -> Vec<f64> {
    let mut harmonics = Vec::with_capacity((order + 1) * (order + 1));
    for degree in 0..=order {
        for azimuthal_index in -(degree as isize)..=(degree as isize) {
            let abs_m = azimuthal_index.unsigned_abs();
            let multiplicity = if abs_m == 0 { 1.0 } else { 2.0 };
            let normalization = (multiplicity * oracle_factorial(degree - abs_m)
                / oracle_factorial(degree + abs_m))
            .sqrt();
            let legendre = oracle_associated_legendre(degree, abs_m, elevation.sin());
            let angular = if azimuthal_index > 0 {
                (azimuthal_index as f64 * azimuth).cos()
            } else if azimuthal_index < 0 {
                (azimuthal_index.unsigned_abs() as f64 * azimuth).sin()
            } else {
                1.0
            };
            harmonics.push(normalization * legendre * angular);
        }
    }
    harmonics
}

fn oracle_legendre(degree: usize, x: f64) -> f64 {
    match degree {
        0 => 1.0,
        1 => x,
        _ => {
            let mut previous = 1.0;
            let mut current = x;
            for n in 2..=degree {
                let next =
                    ((2 * n - 1) as f64 * x * current - (n - 1) as f64 * previous) / n as f64;
                previous = current;
                current = next;
            }
            current
        }
    }
}

fn largest_legendre_root(degree: usize) -> f64 {
    let mut root_bracket = None;
    let mut left = -1.0;
    let mut left_value = oracle_legendre(degree, left);
    const GRID_STEPS: usize = 65_536;
    for step in 1..=GRID_STEPS {
        let right = -1.0 + 2.0 * step as f64 / GRID_STEPS as f64;
        let right_value = oracle_legendre(degree, right);
        if left_value == 0.0 || left_value.is_sign_positive() != right_value.is_sign_positive() {
            root_bracket = Some((left, right));
        }
        left = right;
        left_value = right_value;
    }
    let (mut left, mut right) = root_bracket.expect("Legendre root bracket exists");
    let mut left_value = oracle_legendre(degree, left);
    for _ in 0..80 {
        let midpoint = 0.5 * (left + right);
        let midpoint_value = oracle_legendre(degree, midpoint);
        if midpoint_value == 0.0 {
            return midpoint;
        }
        if left_value.is_sign_positive() == midpoint_value.is_sign_positive() {
            left = midpoint;
            left_value = midpoint_value;
        } else {
            right = midpoint;
        }
    }
    0.5 * (left + right)
}

fn matrix_column_correlation(
    matrix: &[f64],
    rows: usize,
    columns: usize,
    numerical_null_energy: f64,
) -> f64 {
    let mut maximum: f64 = 0.0;
    for first in 0..columns {
        for second in (first + 1)..columns {
            let mut first_energy = 0.0;
            let mut second_energy = 0.0;
            let mut cross = 0.0;
            for row in 0..rows {
                let a = matrix[row * columns + first];
                let b = matrix[row * columns + second];
                first_energy += a * a;
                second_energy += b * b;
                cross += a * b;
            }
            let denominator = (first_energy * second_energy).sqrt();
            if first_energy > numerical_null_energy
                && second_energy > numerical_null_energy
                && denominator > f64::MIN_POSITIVE
            {
                maximum = maximum.max(cross.abs() / denominator);
            }
        }
    }
    maximum
}

fn jacobi_svd(matrix: &[f64], rows: usize, columns: usize) -> JacobiSvd {
    assert_eq!(matrix.len(), rows * columns);
    let mut orthogonal_columns = matrix.to_vec();
    let mut right = vec![0.0; columns * columns];
    for index in 0..columns {
        right[index * columns + index] = 1.0;
    }

    let largest_initial_energy = (0..columns)
        .map(|column| {
            (0..rows)
                .map(|row| orthogonal_columns[row * columns + column].powi(2))
                .sum::<f64>()
        })
        .fold(0.0_f64, f64::max);
    let numerical_null_energy = largest_initial_energy * JACOBI_NUMERICAL_NULL_RELATIVE.powi(2);

    let mut converged = false;
    for _ in 0..JACOBI_SWEEP_LIMIT {
        let mut changed = false;
        for first in 0..columns {
            for second in (first + 1)..columns {
                let mut first_energy = 0.0;
                let mut second_energy = 0.0;
                let mut cross = 0.0;
                for row in 0..rows {
                    let a = orthogonal_columns[row * columns + first];
                    let b = orthogonal_columns[row * columns + second];
                    first_energy += a * a;
                    second_energy += b * b;
                    cross += a * b;
                }
                let denominator = (first_energy * second_energy).sqrt();
                if first_energy <= numerical_null_energy
                    || second_energy <= numerical_null_energy
                    || denominator <= f64::MIN_POSITIVE
                    || cross.abs() <= JACOBI_CORRELATION_TOLERANCE * denominator
                {
                    continue;
                }

                let tau = (second_energy - first_energy) / (2.0 * cross);
                let sign = if tau >= 0.0 { 1.0 } else { -1.0 };
                let tangent = sign / (tau.abs() + (1.0 + tau * tau).sqrt());
                let cosine = 1.0 / (1.0 + tangent * tangent).sqrt();
                let sine = cosine * tangent;
                for row in 0..rows {
                    let index_first = row * columns + first;
                    let index_second = row * columns + second;
                    let a = orthogonal_columns[index_first];
                    let b = orthogonal_columns[index_second];
                    orthogonal_columns[index_first] = cosine * a - sine * b;
                    orthogonal_columns[index_second] = sine * a + cosine * b;
                }
                for row in 0..columns {
                    let index_first = row * columns + first;
                    let index_second = row * columns + second;
                    let a = right[index_first];
                    let b = right[index_second];
                    right[index_first] = cosine * a - sine * b;
                    right[index_second] = sine * a + cosine * b;
                }
                changed = true;
            }
        }
        if !changed {
            converged = true;
            break;
        }
    }

    let normalized_off_diagonal_correlation =
        matrix_column_correlation(&orthogonal_columns, rows, columns, numerical_null_energy);
    assert!(
        converged && normalized_off_diagonal_correlation <= JACOBI_CORRELATION_TOLERANCE,
        "test Jacobi SVD did not converge: normalized off-diagonal correlation={normalized_off_diagonal_correlation:e}"
    );

    let singular_values: Vec<_> = (0..columns)
        .map(|column| {
            (0..rows)
                .map(|row| orthogonal_columns[row * columns + column].powi(2))
                .sum::<f64>()
                .sqrt()
        })
        .collect();
    let left = (0..rows * columns)
        .map(|index| {
            let column = index % columns;
            if singular_values[column] > 0.0 {
                orthogonal_columns[index] / singular_values[column]
            } else {
                0.0
            }
        })
        .collect();
    JacobiSvd {
        rows,
        columns,
        singular_values,
        left,
        right,
        normalized_off_diagonal_correlation,
    }
}

fn validate_jacobi_reference(matrix: &[f64], svd: &JacobiSvd) {
    let sigma_max = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let retained_for_reconstruction = sigma_max * 1.0e-14;
    let mut squared_error = 0.0;
    let mut squared_norm = 0.0;
    let retained: Vec<_> = svd
        .singular_values
        .iter()
        .enumerate()
        .filter_map(|(column, &sigma)| (sigma > retained_for_reconstruction).then_some(column))
        .collect();
    for row in 0..svd.rows {
        for column in 0..svd.columns {
            let reconstructed = retained
                .iter()
                .map(|&mode| {
                    svd.left[row * svd.columns + mode]
                        * svd.singular_values[mode]
                        * svd.right[column * svd.columns + mode]
                })
                .sum::<f64>();
            let original = matrix[row * svd.columns + column];
            squared_error += (reconstructed - original).powi(2);
            squared_norm += original * original;
        }
    }
    let relative_reconstruction_error = squared_error.sqrt() / squared_norm.sqrt().max(1.0);
    assert!(
        relative_reconstruction_error <= 1.0e-10,
        "Jacobi retained-triplet reconstruction error={relative_reconstruction_error:e}"
    );

    for (position, &first) in retained.iter().enumerate() {
        for &second in &retained[position + 1..] {
            let right_dot = (0..svd.columns)
                .map(|row| {
                    svd.right[row * svd.columns + first] * svd.right[row * svd.columns + second]
                })
                .sum::<f64>();
            assert!(
                right_dot.abs() <= 1.0e-10,
                "right orthogonality {right_dot:e}"
            );

            let left_dot = (0..svd.rows)
                .map(|row| {
                    svd.left[row * svd.columns + first] * svd.left[row * svd.columns + second]
                })
                .sum::<f64>();
            assert!(left_dot.abs() <= 1.0e-10, "left orthogonality {left_dot:e}");
        }
    }
    assert!(svd.normalized_off_diagonal_correlation <= 1.0e-12);
}

fn svd_rank_and_condition(svd: &JacobiSvd, relative_threshold: f64) -> (usize, f64) {
    let sigma_max = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let threshold = sigma_max * relative_threshold;
    let retained: Vec<_> = svd
        .singular_values
        .iter()
        .copied()
        .filter(|&sigma| sigma > threshold)
        .collect();
    let condition = retained
        .iter()
        .copied()
        .reduce(f64::min)
        .map(|smallest| sigma_max / smallest)
        .unwrap_or(f64::INFINITY);
    (retained.len(), condition)
}

fn decoder_from_jacobi(svd: &JacobiSvd, relative_cutoff: f64, relative_lambda: f64) -> Vec<f64> {
    let sigma_max = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let cutoff = relative_cutoff * sigma_max;
    let lambda = relative_lambda * sigma_max;
    let mut decoder = vec![0.0; svd.rows * svd.columns];
    for row in 0..svd.rows {
        for column in 0..svd.columns {
            for mode in 0..svd.columns {
                let sigma = svd.singular_values[mode];
                if sigma > cutoff {
                    let regularized_gain = sigma / (sigma * sigma + lambda * lambda);
                    decoder[row * svd.columns + column] += svd.left[row * svd.columns + mode]
                        * regularized_gain
                        * svd.right[column * svd.columns + mode];
                }
            }
        }
    }
    decoder
}

fn invert_well_conditioned(matrix: &[f64], size: usize) -> Vec<f64> {
    let mut augmented = vec![0.0; size * size * 2];
    for row in 0..size {
        for column in 0..size {
            augmented[row * (2 * size) + column] = matrix[row * size + column];
        }
        augmented[row * (2 * size) + size + row] = 1.0;
    }

    for pivot_column in 0..size {
        let pivot_row = (pivot_column..size)
            .max_by(|&first, &second| {
                augmented[first * 2 * size + pivot_column]
                    .abs()
                    .total_cmp(&augmented[second * 2 * size + pivot_column].abs())
            })
            .unwrap();
        assert!(augmented[pivot_row * 2 * size + pivot_column].abs() > 1.0e-12);
        for column in 0..(2 * size) {
            augmented.swap(
                pivot_column * 2 * size + column,
                pivot_row * 2 * size + column,
            );
        }
        let pivot = augmented[pivot_column * 2 * size + pivot_column];
        for column in 0..(2 * size) {
            augmented[pivot_column * 2 * size + column] /= pivot;
        }
        for row in 0..size {
            if row == pivot_column {
                continue;
            }
            let factor = augmented[row * 2 * size + pivot_column];
            for column in 0..(2 * size) {
                augmented[row * 2 * size + column] -=
                    factor * augmented[pivot_column * 2 * size + column];
            }
        }
    }

    let mut inverse = Vec::with_capacity(size * size);
    for row in 0..size {
        for column in 0..size {
            inverse.push(augmented[row * 2 * size + size + column]);
        }
    }
    inverse
}

fn oracle_normal_equation_decoder(y: &[f64], rows: usize, columns: usize) -> (Vec<f64>, f64) {
    let mut gram = vec![0.0; columns * columns];
    for first in 0..columns {
        for second in 0..columns {
            gram[first * columns + second] = (0..rows)
                .map(|row| y[row * columns + first] * y[row * columns + second])
                .sum();
        }
    }
    let svd = jacobi_svd(y, rows, columns);
    let sigma_max = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let lambda = sigma_max * 1.0e-6;
    for diagonal in 0..columns {
        gram[diagonal * columns + diagonal] += lambda * lambda;
    }
    let inverse = invert_well_conditioned(&gram, columns);
    let mut decoder = vec![0.0; rows * columns];
    for row in 0..rows {
        for column in 0..columns {
            decoder[row * columns + column] = (0..columns)
                .map(|mode| y[row * columns + mode] * inverse[mode * columns + column])
                .sum();
        }
    }

    let mut residual_squared = 0.0;
    for first in 0..columns {
        for second in 0..columns {
            let reconstructed = (0..rows)
                .map(|row| y[row * columns + first] * decoder[row * columns + second])
                .sum::<f64>();
            let expected = if first == second { 1.0 } else { 0.0 };
            residual_squared += (reconstructed - expected).powi(2);
        }
    }
    (decoder, residual_squared.sqrt() / (columns as f64).sqrt())
}

fn oracle_directions(count: usize) -> Vec<OracleDirection> {
    let golden_angle = PI * (3.0 - 5.0_f64.sqrt());
    (0..count)
        .map(|index| {
            let vertical = 1.0 - 2.0 * (index as f64 + 0.5) / count as f64;
            let radius = (1.0 - vertical * vertical).sqrt();
            let angle = (index as f64 * golden_angle).rem_euclid(2.0 * PI);
            let vector = [radius * angle.sin(), radius * angle.cos(), vertical];
            OracleDirection {
                vector,
                azimuth: vector[0].atan2(vector[1]),
                elevation: vector[2].asin(),
            }
        })
        .collect()
}

fn physical_harmonic_design(config: &SpeakerConfig, order: usize) -> (Vec<f64>, Vec<usize>) {
    let speakers: Vec<_> = config
        .speakers
        .iter()
        .filter(|speaker| !speaker.is_lfe)
        .collect();
    let columns = (order + 1) * (order + 1);
    let mut design = Vec::with_capacity(speakers.len() * columns);
    let mut channels = Vec::with_capacity(speakers.len());
    for speaker in speakers {
        design.extend(oracle_harmonics(
            order,
            speaker.azimuth as f64 * PI / 180.0,
            speaker.elevation as f64 * PI / 180.0,
        ));
        channels.push(speaker.channel);
    }
    (design, channels)
}

fn apply_max_re_reference(matrix: &mut [f64], rows: usize, columns: usize, order: usize) {
    let root = largest_legendre_root(order + 1);
    let weights: Vec<_> = (0..=order)
        .map(|degree| oracle_legendre(degree, root))
        .collect();
    for row in 0..rows {
        for column in 0..columns {
            let (degree, _) = oracle_acn_to_degree_index(column);
            matrix[row * columns + column] *= weights[degree];
        }
    }
}

fn active_output_svd(matrix: &[f32], config: &SpeakerConfig, columns: usize) -> JacobiSvd {
    let active: Vec<_> = config
        .speakers
        .iter()
        .filter(|speaker| !speaker.is_lfe)
        .collect();
    let mut rows = Vec::with_capacity(active.len() * columns);
    for speaker in active {
        rows.extend(
            matrix[speaker.channel * columns..(speaker.channel + 1) * columns]
                .iter()
                .map(|value| *value as f64),
        );
    }
    jacobi_svd(&rows, rows.len() / columns, columns)
}

fn matrix_max_abs_difference_f64(first: &[f32], second: &[f64]) -> f64 {
    assert_eq!(first.len(), second.len());
    first
        .iter()
        .zip(second)
        .map(|(actual, expected)| (*actual as f64 - *expected).abs())
        .fold(0.0_f64, f64::max)
}

#[test]
fn independent_acn_sn3d_and_max_re_references_cover_orders_four_through_seven() {
    for order in 1..=7 {
        let channels = (order + 1) * (order + 1);
        for degree in 0..=order {
            for azimuthal_index in -(degree as isize)..=(degree as isize) {
                let acn = (degree * (degree + 1)) as isize + azimuthal_index;
                let actual = spherical_harmonics::acn_to_degree_index(acn as usize);
                assert_eq!(actual, (degree as i32, azimuthal_index as i32));
            }
        }
        for &(azimuth, elevation) in &[
            (0.17, -0.43),
            (1.21, 0.31),
            (-2.4, 0.77),
            (0.0, 0.0),
            (PI / 2.0, 0.0),
            (-PI / 3.0, PI / 2.0),
            (0.0, -PI / 2.0),
        ] {
            let expected = oracle_harmonics(order, azimuth, elevation);
            let mut actual = vec![0.0; channels];
            spherical_harmonics_vector(order, azimuth, elevation, &mut actual);
            for (channel, (expected, actual)) in expected.iter().zip(actual).enumerate() {
                assert!(
                    (expected - actual).abs() <= 3.0e-14,
                    "order={order}, acn={channel}: expected={expected:.17e}, actual={actual:.17e}"
                );
            }
        }

        let weights = compute_max_re_weights(order);
        let root = largest_legendre_root(order + 1);
        for (acn, weight) in weights.iter().enumerate() {
            let (degree, _) = oracle_acn_to_degree_index(acn);
            let expected = oracle_legendre(degree, root);
            assert!((weight - expected).abs() <= 3.0e-14);
        }
        assert!(weights.iter().all(|weight| weight.is_finite()));
    }

    let expected_legacy_degrees: [&[f32]; 3] = [
        &[1.0, 0.577_350_26],
        &[1.0, 0.774_596_7, 0.4],
        &[1.0, 0.861_136_3, 0.612_333_6, 0.304_747],
    ];
    for (order_index, degrees) in expected_legacy_degrees.iter().enumerate() {
        let order = order_index + 1;
        let weights = compute_max_re_weights(order);
        for (acn, &weight) in weights.iter().enumerate() {
            let (degree, _) = oracle_acn_to_degree_index(acn);
            assert_eq!(weight as f32, degrees[degree]);
            assert_eq!((weight as f32).to_bits(), degrees[degree].to_bits());
        }
    }
}

#[test]
fn test_jacobi_reference_converges_and_applies_strict_cutoff_and_null_modes() {
    let left = rotation_matrix(4, 1, 2, 0.37);
    let right = rotation_matrix(4, 1, 2, -0.61);
    let matrix = rotated_diagonal_matrix(4, 4, &[1.0, 1.01e-7, 0.99e-7, 0.0], &left, &right);
    let svd = jacobi_svd(&matrix, 4, 4);
    validate_jacobi_reference(&matrix, &svd);
    let sigma_max = svd.singular_values.iter().copied().fold(0.0, f64::max);
    let threshold = sigma_max * 1.0e-7;
    let retained: Vec<_> = svd
        .singular_values
        .iter()
        .copied()
        .filter(|&sigma| sigma > threshold)
        .collect();
    assert_eq!(retained.len(), 2);
    assert!(retained
        .iter()
        .any(|sigma| (*sigma - 1.01e-7).abs() < 1.0e-15));
    assert!(svd
        .singular_values
        .iter()
        .any(|sigma| (*sigma - 0.99e-7).abs() < 1.0e-15));
    assert!(svd.singular_values.contains(&0.0));
    let (rank, _) = svd_rank_and_condition(&svd, 1.0e-7);
    assert_eq!(rank, 2);

    let rectangular_left = rotation_matrix(3, 0, 1, 0.23);
    let rectangular_right = rotation_matrix(4, 1, 3, -0.49);
    let rectangular = rotated_diagonal_matrix(
        3,
        4,
        &[1.0, 0.2, 0.0],
        &rectangular_left,
        &rectangular_right,
    );
    let rectangular_svd = jacobi_svd(&rectangular, 3, 4);
    validate_jacobi_reference(&rectangular, &rectangular_svd);
    let (rectangular_rank, _) = svd_rank_and_condition(&rectangular_svd, 1.0e-7);
    assert_eq!(rectangular_rank, 2);
    assert_eq!(
        rectangular_svd
            .singular_values
            .iter()
            .filter(|&&sigma| sigma > 1.0e-10)
            .count(),
        2
    );
}

fn rotation_matrix(size: usize, first: usize, second: usize, angle: f64) -> Vec<f64> {
    let mut rotation = vec![0.0; size * size];
    for diagonal in 0..size {
        rotation[diagonal * size + diagonal] = 1.0;
    }
    let cosine = angle.cos();
    let sine = angle.sin();
    rotation[first * size + first] = cosine;
    rotation[first * size + second] = -sine;
    rotation[second * size + first] = sine;
    rotation[second * size + second] = cosine;
    rotation
}

fn rotated_diagonal_matrix(
    rows: usize,
    columns: usize,
    singular_values: &[f64],
    left: &[f64],
    right: &[f64],
) -> Vec<f64> {
    assert_eq!(left.len(), rows * rows);
    assert_eq!(right.len(), columns * columns);
    assert_eq!(singular_values.len(), rows.min(columns));
    (0..rows * columns)
        .map(|index| {
            let row = index / columns;
            let column = index % columns;
            singular_values
                .iter()
                .enumerate()
                .map(|(mode, &sigma)| {
                    left[row * rows + mode] * sigma * right[column * columns + mode]
                })
                .sum()
        })
        .collect()
}

#[test]
fn allrad_grid_quadrature_and_virtual_solve_meet_order_specific_limits() {
    let expected_counts = [0, 64, 96, 128, 256, 384, 512, 512];
    assert_eq!(ALLRAD_VIRTUAL_SPEAKERS, expected_counts);
    for (order, &count) in expected_counts.iter().enumerate().skip(1) {
        let columns = (order + 1) * (order + 1);
        let directions = oracle_directions(count);
        let mut y = Vec::with_capacity(count * columns);
        for direction in &directions {
            y.extend(oracle_harmonics(
                order,
                direction.azimuth,
                direction.elevation,
            ));
        }

        let mut maximum_normalized_gram_error = 0.0_f64;
        for first in 0..columns {
            let (first_degree, _) = oracle_acn_to_degree_index(first);
            let first_norm = 4.0 * PI / (2 * first_degree + 1) as f64;
            for second in 0..columns {
                let (second_degree, _) = oracle_acn_to_degree_index(second);
                let second_norm = 4.0 * PI / (2 * second_degree + 1) as f64;
                let integral = 4.0 * PI / count as f64
                    * (0..count)
                        .map(|row| y[row * columns + first] * y[row * columns + second])
                        .sum::<f64>();
                let normalized = integral / (first_norm * second_norm).sqrt();
                let expected = if first == second { 1.0 } else { 0.0 };
                maximum_normalized_gram_error =
                    maximum_normalized_gram_error.max((normalized - expected).abs());
            }
        }
        let svd = jacobi_svd(&y, count, columns);
        validate_jacobi_reference(&y, &svd);
        let (rank, condition) = svd_rank_and_condition(&svd, 1.0e-12);
        let (_, residual) = oracle_normal_equation_decoder(&y, count, columns);
        println!(
            "AUD133_GRID order={order} channels={columns} count={count} rank={rank} condition={condition:.9} normalized_gram_error={maximum_normalized_gram_error:.9e} virtual_residual={residual:.9e}"
        );
        assert_eq!(rank, columns, "AllRAD grid order {order}");
        assert!(condition <= 4.0, "order={order}, condition={condition}");
        assert!(
            maximum_normalized_gram_error <= 0.01,
            "order={order}, gram error={maximum_normalized_gram_error}"
        );
        assert!(residual <= 1.0e-8, "order={order}, residual={residual}");
    }
}

#[test]
fn mode_matching_and_allrad_match_independent_references_on_named_layouts() {
    for (order, &virtual_count) in ALLRAD_VIRTUAL_SPEAKERS.iter().enumerate().skip(1) {
        let columns = (order + 1) * (order + 1);
        let virtual_directions = oracle_directions(virtual_count);
        let mut virtual_y = Vec::with_capacity(virtual_count * columns);
        for direction in &virtual_directions {
            virtual_y.extend(oracle_harmonics(
                order,
                direction.azimuth,
                direction.elevation,
            ));
        }
        let (plain_virtual_decoder, virtual_residual) =
            oracle_normal_equation_decoder(&virtual_y, virtual_count, columns);
        assert!(virtual_residual <= 1.0e-8);
        let virtual_svd = jacobi_svd(&virtual_y, virtual_count, columns);
        validate_jacobi_reference(&virtual_y, &virtual_svd);
        let (virtual_rank, virtual_condition) = svd_rank_and_condition(&virtual_svd, 1.0e-7);

        for layout in crate::params::TARGET_LAYOUTS {
            let config = get_speaker_config(layout).unwrap();
            let (physical_y, physical_channels) = physical_harmonic_design(config, order);
            let physical_svd = jacobi_svd(&physical_y, physical_channels.len(), columns);
            validate_jacobi_reference(&physical_y, &physical_svd);
            let physical_reference = decoder_from_jacobi(&physical_svd, 1.0e-7, 1.0e-6);
            let (expected_quality_rank, expected_quality_condition) =
                svd_rank_and_condition(&physical_svd, 1.0e-7);

            let mut virtual_from_physical = vec![0.0; config.total_channels * virtual_count];
            let physical: Vec<_> = config
                .speakers
                .iter()
                .filter(|speaker| !speaker.is_lfe)
                .collect();
            let physical_vectors: Vec<_> = physical
                .iter()
                .map(|speaker| (speaker.channel, speaker.to_cartesian()))
                .collect();
            let has_height = physical.iter().any(|speaker| speaker.elevation.abs() > 1.0);
            for (virtual_index, direction) in virtual_directions.iter().enumerate() {
                let mut gains = vec![0.0; config.total_channels];
                if has_height && direction.elevation.abs() > 1.0e-8 {
                    super::vbap_3d(&physical_vectors, direction.vector, &mut gains);
                } else {
                    super::vbap_2d(&physical_vectors, direction.azimuth, &mut gains);
                }
                for (channel, gain) in gains.into_iter().enumerate() {
                    virtual_from_physical[channel * virtual_count + virtual_index] = gain;
                }
            }
            let mut allrad_reference = vec![0.0; config.total_channels * columns];
            for channel in 0..config.total_channels {
                for acn in 0..columns {
                    allrad_reference[channel * columns + acn] = (0..virtual_count)
                        .map(|virtual_index| {
                            virtual_from_physical[channel * virtual_count + virtual_index]
                                * plain_virtual_decoder[virtual_index * columns + acn]
                        })
                        .sum();
                }
            }

            for apply_max_re in [false, true] {
                let mut expected_mode = physical_reference.clone();
                let mut expected_allrad = allrad_reference.clone();
                if apply_max_re {
                    apply_max_re_reference(
                        &mut expected_mode,
                        physical_channels.len(),
                        columns,
                        order,
                    );
                    apply_max_re_reference(
                        &mut expected_allrad,
                        config.total_channels,
                        columns,
                        order,
                    );
                }

                let mode =
                    DecodeMatrix::build(order, config, apply_max_re).unwrap_or_else(|error| {
                        panic!("mode matching {layout} order {order}: {error}")
                    });
                let mut mode_expected_full = vec![0.0; config.total_channels * columns];
                for (physical_row, &channel) in physical_channels.iter().enumerate() {
                    mode_expected_full[channel * columns..(channel + 1) * columns].copy_from_slice(
                        &expected_mode[physical_row * columns..(physical_row + 1) * columns],
                    );
                }
                let mode_error = matrix_max_abs_difference_f64(&mode.matrix, &mode_expected_full);
                assert!(
                    mode_error <= 2.0e-6,
                    "mode matrix {layout} order {order} max_re={apply_max_re}: error={mode_error:e}"
                );
                assert_eq!(mode.quality().rank, expected_quality_rank);
                assert!(
                    relative_difference(mode.quality().condition_number, expected_quality_condition)
                        <= 2.0e-6,
                    "mode quality {layout} order {order}: actual={}, expected={expected_quality_condition}",
                    mode.quality().condition_number
                );

                let allrad = DecodeMatrix::build_allrad(order, config, apply_max_re)
                    .unwrap_or_else(|error| panic!("AllRAD {layout} order {order}: {error}"));
                let allrad_error = matrix_max_abs_difference_f64(&allrad.matrix, &expected_allrad);
                assert!(
                    allrad_error <= 2.0e-6,
                    "AllRAD matrix {layout} order {order} max_re={apply_max_re}: error={allrad_error:e}"
                );
                assert_eq!(allrad.quality().rank, virtual_rank);
                assert!(
                    relative_difference(allrad.quality().condition_number, virtual_condition)
                        <= 2.0e-6,
                    "AllRAD virtual quality {layout} order {order}: actual={}, expected={virtual_condition}",
                    allrad.quality().condition_number
                );

                for (algorithm, matrix) in [("mode", &mode), ("allrad", &allrad)] {
                    let output_svd = active_output_svd(&matrix.matrix, config, columns);
                    assert!(output_svd.normalized_off_diagonal_correlation <= 1.0e-12);
                    let rank_threshold = f32::EPSILON as f64 * output_svd.rows.max(columns) as f64;
                    let (physical_rank, physical_condition) =
                        svd_rank_and_condition(&output_svd, rank_threshold);
                    assert!(physical_rank <= output_svd.rows);
                    assert!(physical_rank <= columns);
                    assert!(physical_condition.is_finite());
                    if output_svd.rows < columns {
                        assert!(physical_rank < columns);
                    }
                    println!(
                        "AUD133_PHYSICAL algorithm={algorithm} order={order} layout={layout} max_re={} active_rows={} input_channels={columns} rank={physical_rank} condition={physical_condition:.9}",
                        u8::from(apply_max_re),
                        output_svd.rows
                    );
                }
            }
        }
    }
}

fn relative_difference(actual: f64, expected: f64) -> f64 {
    (actual - expected).abs() / expected.abs().max(1.0)
}
