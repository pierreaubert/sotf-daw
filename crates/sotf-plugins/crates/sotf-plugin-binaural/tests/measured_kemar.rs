//! Offline measured KEMAR ingestion and interpolation checks.
//!
//! Gardner and Martin, MIT Media Laboratory (1994). Attribution and the original
//! unmodified SOFA file are in fixtures/kemar; this is mannequin measurement data.

// Rust guideline compliant 2026-02-21
use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use sotf_host::sofa::{SofaFile, SourcePosition, load_sofa};
use sotf_plugin_binaural::hrtf::{
    find_hrtf_neighbors, interpolate_hrtf_prepared, prepare_hrtf_spectra,
};

fn measured_sofa() -> SofaFile {
    load_sofa(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/kemar/mit_kemar_normal_pinna.sofa"),
    )
    .unwrap()
    .data
}

fn dft(ir: &[f32], bin: usize, size: usize) -> Complex<f64> {
    ir.iter()
        .enumerate()
        .map(|(i, &sample)| {
            let angle = -std::f64::consts::TAU * bin as f64 * i as f64 / size as f64;
            Complex::new(angle.cos(), angle.sin()) * f64::from(sample)
        })
        .sum()
}

#[test]
fn unmodified_measured_kemar_dataset_retains_documented_geometry_and_samples() {
    let sofa = measured_sofa();
    assert_eq!(sofa.num_measurements, 710);
    assert_eq!(sofa.ir_length, 512);
    assert_eq!(sofa.sample_rate, 44_100.0);
    assert_eq!(sofa.data_sample_rate, Some(44_100.0));
    assert_eq!(sofa.impulse_responses.len(), 710 * 512 * 2);
    assert!(sofa.impulse_responses.iter().all(|v| v.is_finite()));
    let min = sofa
        .positions
        .iter()
        .map(|p| p.elevation)
        .fold(f32::INFINITY, f32::min);
    let max = sofa
        .positions
        .iter()
        .map(|p| p.elevation)
        .fold(f32::NEG_INFINITY, f32::max);
    assert_eq!(min, -40.0);
    assert_eq!(max, 90.0);
    for index in 0..710 {
        let (_, left, right) = sofa.get_hrtf_slices(index).unwrap();
        assert!(left.iter().any(|v| v.abs() > 1e-5));
        assert!(right.iter().any(|v| v.abs() > 1e-5));
    }
    println!(
        "MEASURED_KEMAR measurements=710 ir_samples=512 rate=44100 minimum_elevation={min} maximum_elevation={max}"
    );
}

#[test]
fn measured_grid_nodes_match_independent_hrir_dft() {
    report_grid_nodes((0..710).step_by(17).chain(std::iter::once(709)));
}

#[test]
#[ignore = "manual complete measured-dataset node diagnostic"]
fn complete_dataset_nodes_match_independent_hrir_dft() {
    report_grid_nodes(0..710);
}

fn report_grid_nodes(indices: impl Iterator<Item = usize>) {
    let sofa = measured_sofa();
    let mut planner = RealFftPlanner::new();
    let fft = planner.plan_fft_forward(1024);
    let prepared = prepare_hrtf_spectra(&sofa, 1024, sofa.sample_rate, &fft);
    let mut maximum = 0.0_f64;
    let mut values = 0;
    for index in indices {
        let target = &sofa.positions[index];
        let (near, weights) = find_hrtf_neighbors(&sofa, target);
        let (left, right) = interpolate_hrtf_prepared(
            &near,
            &weights,
            &prepared,
            1024,
            sofa.sample_rate,
            0.0,
            target.azimuth,
            target.elevation,
        );
        let (_, reference_left, reference_right) = sofa.get_hrtf_slices(index).unwrap();
        for (actual, reference) in [(&left, reference_left), (&right, reference_right)] {
            for bin in [0, 5, 9, 17, 31, 63, 127, 185] {
                let expected = dft(reference, bin, 1024);
                let actual = Complex::new(f64::from(actual[bin].re), f64::from(actual[bin].im));
                maximum = maximum.max((actual - expected).norm());
                values += 1;
            }
        }
    }
    println!("MEASURED_KEMAR_GRID complex_values={values} maximum_dft_error={maximum:e}");
    // Preserve the existing complete-FIR numerical tolerance at measured nodes.
    assert!(
        maximum < 2e-5,
        "measured-node interpolation error {maximum}"
    );
}

// Frozen b481bd8 chord-plane interpolation for paired measurement diagnostics.
fn legacy_chord_weights(
    target: &SourcePosition,
    nearest: &[(usize, f32); 3],
    sofa: &SofaFile,
) -> [f32; 3] {
    let p = target.to_cartesian_unit_vector();

    let v0 = sofa.positions[nearest[0].0].to_cartesian_unit_vector();
    let v1 = sofa.positions[nearest[1].0].to_cartesian_unit_vector();
    let v2 = sofa.positions[nearest[2].0].to_cartesian_unit_vector();

    // Calculate barycentric coordinates using cross products
    // This is more stable than matrix inversion
    //
    // The barycentric coordinates (w0, w1, w2) satisfy:
    // p = w0*v0 + w1*v1 + w2*v2
    // w0 + w1 + w2 = 1
    //
    // Using the formula:
    // w0 = area(p,v1,v2) / area(v0,v1,v2)
    // w1 = area(v0,p,v2) / area(v0,v1,v2)
    // w2 = area(v0,v1,p) / area(v0,v1,v2)
    //
    // Where area is computed using cross product magnitude

    // Helper to compute cross product
    let cross = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };

    // Helper to compute dot product
    let dot = |a: [f32; 3], b: [f32; 3]| -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] };

    // Compute edge vectors
    let v01 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
    let v02 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
    let v0p = [p[0] - v0[0], p[1] - v0[1], p[2] - v0[2]];

    // Normal of triangle (v0, v1, v2)
    let n = cross(v01, v02);
    let n_dot_n = dot(n, n);

    // Check for degenerate triangle (collinear points)
    if n_dot_n < 1e-6 {
        log::warn!("[BinauralDecoder] Degenerate triangle detected, using nearest neighbor");
        return [1.0, 0.0, 0.0];
    }

    // Calculate barycentric coordinates
    // w1 corresponds to v1, w2 corresponds to v2
    let v02_cross_n = cross(v02, n);
    let n_cross_v01 = cross(n, v01);

    let w1 = dot(v02_cross_n, v0p) / n_dot_n;
    let w2 = dot(n_cross_v01, v0p) / n_dot_n;
    let w0 = 1.0 - w1 - w2;

    // Check if point is inside triangle (all weights non-negative)
    // If outside, clamp to valid range and warn
    let mut weights = [w0, w1, w2];
    if weights.iter().any(|&w| w < -0.01) {
        // Point is significantly outside triangle
        // This can happen with sparse HRTF measurements
        log::debug!(
            "[BinauralDecoder] Target outside triangle: weights=[{:.3}, {:.3}, {:.3}], clamping to boundary",
            w0,
            w1,
            w2
        );
        // Clamp negative weights to zero
        for w in &mut weights {
            if *w < 0.0 {
                *w = 0.0;
            }
        }

        // Renormalize so the clamped weights sum to 1.0.
        let sum: f32 = weights.iter().sum();
        if sum > 1e-6 {
            for w in &mut weights {
                *w /= sum;
            }
        } else {
            // All weights were negative, use nearest neighbor
            weights = [1.0, 0.0, 0.0];
        }
    }

    // These are interpolation weights for an HRTF field, not loudspeaker VBAP
    // gains. Preserve affine/constant fields by normalizing to unit sum.
    let sum: f32 = weights.iter().sum();
    if sum > 1e-6 {
        [weights[0] / sum, weights[1] / sum, weights[2] / sum]
    } else {
        [1.0, 0.0, 0.0]
    }
}

#[test]
fn held_out_measured_directions_report_spectral_cue_errors_for_both_weight_policies() {
    report_held_out_directions((0..710).step_by(17).chain(std::iter::once(709)), 43, false);
}

#[test]
#[ignore = "manual complete measured-dataset diagnostic"]
fn complete_dataset_leave_one_out_reports_raw_frequency_cues() {
    report_held_out_directions(0..710, 710, true);
}

fn report_held_out_directions(
    indices: impl Iterator<Item = usize>,
    expected_cases: usize,
    raw_frequency_cues: bool,
) {
    let sofa = measured_sofa();
    let mut planner = RealFftPlanner::new();
    let fft = planner.plan_fft_forward(1024);
    let inverse = planner.plan_fft_inverse(1024);
    let prepared = prepare_hrtf_spectra(&sofa, 1024, sofa.sample_rate, &fft);
    let mut maximum_ild = [0.0_f64; 2];
    let mut maximum_phase = [0.0_f64; 2];
    let mut cases = 0;
    for index in indices {
        let target = &sofa.positions[index];
        // Remove the reference measurement from neighbor selection. Mapping the
        // remaining indices back into the immutable cache never reads its HRIR.
        let mut view = sofa.clone();
        view.positions.remove(index);
        view.impulse_responses
            .drain(index * 2 * sofa.ir_length..(index + 1) * 2 * sofa.ir_length);
        view.num_measurements -= 1;
        let (near, directional) = find_hrtf_neighbors(&view, target);
        let legacy = legacy_chord_weights(target, &near, &view);
        let original_neighbors = near.map(|(i, distance)| (i + usize::from(i >= index), distance));
        assert!(original_neighbors.iter().all(|(i, _)| *i != index));
        let (_, left, right) = sofa.get_hrtf_slices(index).unwrap();
        let reference_onsets = [left, right].map(|samples| {
            let threshold = samples.iter().map(|v| v.abs()).fold(0.0_f32, f32::max) * 0.1;
            samples
                .iter()
                .position(|v| v.abs() >= threshold)
                .unwrap_or(0)
        });
        let reference: Vec<_> = (5..=185)
            .map(|bin| [dft(left, bin, 1024), dft(right, bin, 1024)])
            .collect();
        let reference_energy: [f64; 2] =
            std::array::from_fn(|ear| reference.iter().map(|h| h[ear].norm_sqr()).sum());
        let reference_ild = 10.0 * (reference_energy[0].log10() - reference_energy[1].log10());
        for (policy, weights) in [legacy, directional].into_iter().enumerate() {
            let (left, right) = interpolate_hrtf_prepared(
                &original_neighbors,
                &weights,
                &prepared,
                1024,
                sofa.sample_rate,
                0.0,
                target.azimuth,
                target.elevation,
            );
            let predicted = [&left, &right];
            let energy: [f64; 2] = std::array::from_fn(|ear| {
                (5..=185)
                    .map(|bin| f64::from(predicted[ear][bin].norm_sqr()))
                    .sum()
            });
            let ild = 10.0 * (energy[0].log10() - energy[1].log10());
            let mut phase_error = 0.0_f64;
            let mut phase_errors = [0.0_f64; 7];
            let mut reference_ear_magnitudes = [[0.0_f64; 2]; 7];
            for (probe, bin) in [5, 9, 17, 31, 63, 127, 185].into_iter().enumerate() {
                let actual = predicted.map(|spectrum| {
                    Complex::new(f64::from(spectrum[bin].re), f64::from(spectrum[bin].im))
                });
                let cross = actual[0] * actual[1].conj();
                let reference_cross = reference[bin - 5][0] * reference[bin - 5][1].conj();
                phase_errors[probe] = (cross * reference_cross.conj()).arg().abs();
                reference_ear_magnitudes[probe] = reference[bin - 5].map(|value| value.norm());
                phase_error = phase_error.max(phase_errors[probe]);
            }
            if raw_frequency_cues {
                let mut pre_onset_fraction = [0.0_f64; 2];
                let mut late_fraction = [0.0_f64; 2];
                for ear in 0..2 {
                    let mut spectrum = predicted[ear].to_vec();
                    // Match the production complete-real-design convention.
                    spectrum[0].im = 0.0;
                    spectrum[512].im = 0.0;
                    let mut impulse = vec![0.0_f32; 1024];
                    inverse.process(&mut spectrum, &mut impulse).unwrap();
                    let energy = |samples: &[f32]| {
                        samples.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>()
                    };
                    let total = energy(&impulse);
                    pre_onset_fraction[ear] = energy(&impulse[..reference_onsets[ear]]) / total;
                    late_fraction[ear] = energy(&impulse[512..]) / total;
                }
                println!(
                    "MEASURED_KEMAR_TIME index={index} policy={policy} pre_onset_energy_fraction={pre_onset_fraction:?} beyond_measured_support_energy_fraction={late_fraction:?} reference_onsets={reference_onsets:?}"
                );
                println!(
                    "MEASURED_KEMAR_FREQUENCY index={index} policy={policy} wrapped_ipd_errors_rad={phase_errors:?} reference_ear_magnitudes={reference_ear_magnitudes:?}"
                );
            }
            assert!(ild.is_finite() && reference_ild.is_finite() && phase_error.is_finite());
            maximum_ild[policy] = maximum_ild[policy].max((ild - reference_ild).abs());
            maximum_phase[policy] = maximum_phase[policy].max(phase_error);
            println!(
                "MEASURED_KEMAR_HELD_OUT index={index} azimuth={} elevation={} policy={policy} measured_ild_db={reference_ild} predicted_ild_db={ild} maximum_wrapped_interaural_phase_error_rad={phase_error} neighbors={original_neighbors:?} weights={weights:?}",
                target.azimuth, target.elevation
            );
        }
        cases += 1;
    }
    // These are retained quality observations, not perceptual acceptance gates.
    // Node accuracy, nonnegative weights and independent directional geometry
    // have separate fixed numerical gates; no acoustic threshold is widened here.
    println!(
        "MEASURED_KEMAR_HELD_OUT_SUMMARY directions={cases} maximum_band_ild_error_db={maximum_ild:?} maximum_wrapped_interaural_phase_error_rad={maximum_phase:?} band_bins=5..185 fft=1024 rate=44100"
    );
    assert_eq!(cases, expected_cases);
}
